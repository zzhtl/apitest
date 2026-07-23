use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};

use apitest_core::{
    ApiKeyLocation, AuthSpec, BodySpec, Environment, ExecutionError, ExecutionEvent,
    ExecutionMetrics, ExecutionRequest, ExecutionStream, FormField, GraphQlSpec, HttpSpec,
    MultipartPart, OAuth2Grant, ProtocolExecutor, ProtocolKind, ProtocolSpec, ResponseHead,
    Variable,
};
use apitest_storage::SecretStore;
use async_stream::try_stream;
use bytes::Bytes;
use chrono::Utc;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use indexmap::IndexMap;
use md5::Md5;
use reqwest::{
    Client, Method, Proxy,
    cookie::{CookieStore, Jar},
    header::{AUTHORIZATION, COOKIE, WWW_AUTHENTICATE},
    redirect::Policy,
};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone)]
pub struct HttpExecutor {
    secrets: Arc<dyn SecretStore>,
    cookies: Arc<Jar>,
}

impl HttpExecutor {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            secrets,
            cookies: Arc::new(Jar::default()),
        }
    }

    fn materialize_variables(
        &self,
        environment: &Environment,
        local_variables: &[Variable],
    ) -> Result<(Environment, Vec<Variable>), ExecutionError> {
        let materialize = |variable: &Variable| -> Result<Variable, ExecutionError> {
            let Some(reference) = &variable.secret_ref else {
                return Ok(variable.clone());
            };
            let value = self
                .secrets
                .get(reference)
                .map_err(|error| ExecutionError::Authentication(error.to_string()))?
                .ok_or_else(|| {
                    ExecutionError::Authentication(format!(
                        "secret `{}` is unavailable",
                        reference.as_str()
                    ))
                })?;
            Ok(variable.with_materialized_secret(value))
        };

        let mut resolved_environment = environment.clone();
        resolved_environment.variables = environment
            .variables
            .iter()
            .map(materialize)
            .collect::<Result<Vec<_>, _>>()?;
        let resolved_locals = local_variables
            .iter()
            .map(materialize)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((resolved_environment, resolved_locals))
    }
}

impl ProtocolExecutor for HttpExecutor {
    fn kind(&self) -> ProtocolKind {
        ProtocolKind::Http
    }

    fn execute(
        &self,
        request: ExecutionRequest,
        cancellation: CancellationToken,
    ) -> ExecutionStream {
        let executor = self.clone();
        Box::pin(try_stream! {
            if cancellation.is_cancelled() {
                Err(ExecutionError::Cancelled)?;
            }

            let started = Instant::now();
            yield ExecutionEvent::Started { id: request.id, at: Utc::now() };

            let (environment, local_variables) = executor.materialize_variables(
                &request.environment,
                &request.local_variables,
            )?;
            let (spec, is_sse) = normalize_protocol(request.protocol)?;
            let prepared = PreparedRequest::new(
                &spec,
                &environment,
                &local_variables,
                &*executor.secrets,
                Arc::clone(&executor.cookies),
            ).await?;
            let sent_bytes = prepared.sent_bytes;
            let response = tokio::select! {
                _ = cancellation.cancelled() => Err(ExecutionError::Cancelled),
                result = prepared.send() => result,
            }?;

            let head = response_head(&response);
            yield ExecutionEvent::ResponseHead(head);

            let mut received_bytes = 0_u64;
            if is_sse {
                let mut events = response.bytes_stream().eventsource();
                loop {
                    let next = tokio::select! {
                        _ = cancellation.cancelled() => Err(ExecutionError::Cancelled),
                        item = events.next() => Ok(item),
                    }?;
                    let Some(event) = next else { break };
                    let event = event.map_err(|error| ExecutionError::Protocol(error.to_string()))?;
                    received_bytes = received_bytes.saturating_add(event.data.len() as u64);
                    yield ExecutionEvent::Message {
                        outgoing: false,
                        media_type: Some("text/event-stream".to_owned()),
                        data: Bytes::from(event.data),
                        at: Utc::now(),
                    };
                }
            } else {
                let mut body = response.bytes_stream();
                loop {
                    let next = tokio::select! {
                        _ = cancellation.cancelled() => Err(ExecutionError::Cancelled),
                        item = body.next() => Ok(item),
                    }?;
                    let Some(chunk) = next else { break };
                    let chunk = chunk.map_err(|error| map_reqwest_error(error, spec.timeout_ms.max(1)))?;
                    received_bytes = received_bytes.saturating_add(chunk.len() as u64);
                    yield ExecutionEvent::Data(chunk);
                }
            }

            let metrics = ExecutionMetrics {
                elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                received_bytes,
                sent_bytes,
            };
            yield ExecutionEvent::Metrics(metrics);
            yield ExecutionEvent::Completed(metrics);
        })
    }
}

fn normalize_protocol(protocol: ProtocolSpec) -> Result<(HttpSpec, bool), ExecutionError> {
    match protocol {
        ProtocolSpec::Http(spec) => Ok((spec, false)),
        ProtocolSpec::Sse(spec) => Ok((spec.request, true)),
        ProtocolSpec::GraphQl(spec) => graph_ql_to_http(spec).map(|http| (http, false)),
        other => Err(ExecutionError::InvalidRequest(format!(
            "HTTP executor cannot run {:?}",
            other.kind()
        ))),
    }
}

fn graph_ql_to_http(spec: GraphQlSpec) -> Result<HttpSpec, ExecutionError> {
    let variables = if spec.variables.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(&spec.variables).map_err(|error| {
            ExecutionError::InvalidRequest(format!("invalid GraphQL variables: {error}"))
        })?
    };
    let body = serde_json::json!({
        "query": spec.query,
        "variables": variables,
        "operationName": spec.operation_name,
    });
    let mut http = HttpSpec::new(apitest_core::HttpMethod::Post, spec.endpoint);
    http.headers = spec.headers;
    http.auth = spec.auth;
    http.timeout_ms = spec.timeout_ms;
    http.body = BodySpec::Json(body.to_string());
    Ok(http)
}

struct PreparedRequest {
    builder: reqwest::RequestBuilder,
    sent_bytes: u64,
    timeout_ms: u64,
    digest: Option<DigestCredentials>,
}

impl PreparedRequest {
    async fn new(
        spec: &HttpSpec,
        environment: &Environment,
        local_variables: &[Variable],
        secrets: &dyn SecretStore,
        cookies: Arc<Jar>,
    ) -> Result<Self, ExecutionError> {
        let url = resolve_required(environment, local_variables, &spec.url, "URL")?;
        let mut url = Url::parse(&url)
            .map_err(|error| ExecutionError::InvalidRequest(format!("invalid URL: {error}")))?;
        {
            let mut query = url.query_pairs_mut();
            for pair in spec.query.iter().filter(|pair| pair.enabled) {
                let name =
                    resolve_required(environment, local_variables, &pair.name, "query name")?;
                let value =
                    resolve_required(environment, local_variables, &pair.value, "query value")?;
                query.append_pair(&name, &value);
            }
        }
        let client = build_client(
            spec,
            environment,
            local_variables,
            secrets,
            Arc::clone(&cookies),
        )
        .await?;

        let method = Method::from_bytes(spec.method.to_string().as_bytes())
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
        let mut builder = client
            .request(method, url.clone())
            .timeout(Duration::from_millis(spec.timeout_ms.max(1)));

        let mut aws_headers = Vec::new();
        for header in spec.headers.iter().filter(|header| header.enabled) {
            let name = resolve_required(environment, local_variables, &header.name, "header name")?;
            let value =
                resolve_required(environment, local_variables, &header.value, "header value")?;
            let normalized_name = name.to_ascii_lowercase();
            let sigv4_managed = matches!(&spec.auth, AuthSpec::AwsSigV4 { .. })
                && (matches!(
                    normalized_name.as_str(),
                    "authorization" | "x-amz-content-sha256" | "x-amz-date"
                ) || (normalized_name == "x-amz-security-token"
                    && matches!(
                        &spec.auth,
                        AuthSpec::AwsSigV4 {
                            session_token: Some(_),
                            ..
                        }
                    )));
            if sigv4_managed {
                continue;
            }
            if normalized_name.starts_with("x-amz-")
                && !matches!(
                    normalized_name.as_str(),
                    "x-amz-content-sha256" | "x-amz-date"
                )
            {
                aws_headers.push((normalized_name, value.clone()));
            }
            builder = builder.header(name, value);
        }

        let mut cookie_header = spec
            .cookies
            .iter()
            .filter(|cookie| cookie.enabled)
            .map(|cookie| {
                Ok(format!(
                    "{}={}",
                    resolve_required(environment, local_variables, &cookie.name, "cookie name")?,
                    resolve_required(environment, local_variables, &cookie.value, "cookie value",)?
                ))
            })
            .collect::<Result<Vec<_>, ExecutionError>>()?;
        if let Some(stored) = cookies.cookies(&url).and_then(|value| {
            value
                .to_str()
                .ok()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        }) {
            cookie_header.insert(0, stored);
        }
        if !cookie_header.is_empty() {
            builder = builder.header(COOKIE, cookie_header.join("; "));
        }

        let mut sent_bytes = 0_u64;
        let mut payload_hash = Some(sha256_hex(b""));
        match &spec.body {
            BodySpec::None => {}
            BodySpec::Json(value) => {
                let value = resolve_required(environment, local_variables, value, "JSON body")?;
                sent_bytes = value.len() as u64;
                payload_hash = Some(sha256_hex(value.as_bytes()));
                builder = builder
                    .header("content-type", "application/json")
                    .body(value);
            }
            BodySpec::Text(value) => {
                let value = resolve_required(environment, local_variables, value, "text body")?;
                sent_bytes = value.len() as u64;
                payload_hash = Some(sha256_hex(value.as_bytes()));
                builder = builder
                    .header("content-type", "text/plain; charset=utf-8")
                    .body(value);
            }
            BodySpec::Xml(value) => {
                let value = resolve_required(environment, local_variables, value, "XML body")?;
                sent_bytes = value.len() as u64;
                payload_hash = Some(sha256_hex(value.as_bytes()));
                builder = builder
                    .header("content-type", "application/xml")
                    .body(value);
            }
            BodySpec::FormUrlEncoded(fields) => {
                let fields = resolve_form(fields, environment, local_variables)?;
                let encoded = url::form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(fields.iter().map(|(name, value)| (name, value)))
                    .finish();
                sent_bytes = encoded.len() as u64;
                payload_hash = Some(sha256_hex(encoded.as_bytes()));
                builder = builder
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(encoded);
            }
            BodySpec::Multipart(parts) => {
                payload_hash = None;
                let mut form = reqwest::multipart::Form::new();
                for part in parts {
                    match part {
                        MultipartPart::Text(field) if field.enabled => {
                            let name = resolve_required(
                                environment,
                                local_variables,
                                &field.name,
                                "part name",
                            )?;
                            let value = resolve_required(
                                environment,
                                local_variables,
                                &field.value,
                                "part value",
                            )?;
                            sent_bytes = sent_bytes.saturating_add(value.len() as u64);
                            form = form.text(name, value);
                        }
                        MultipartPart::File {
                            name,
                            path,
                            content_type,
                            enabled,
                        } if *enabled => {
                            let mut file =
                                reqwest::multipart::Part::file(path)
                                    .await
                                    .map_err(|error| {
                                        ExecutionError::InvalidRequest(error.to_string())
                                    })?;
                            if let Some(content_type) = content_type {
                                file = file.mime_str(content_type).map_err(|error| {
                                    ExecutionError::InvalidRequest(error.to_string())
                                })?;
                            }
                            if let Ok(metadata) = tokio::fs::metadata(path).await {
                                sent_bytes = sent_bytes.saturating_add(metadata.len());
                            }
                            form = form.part(name.clone(), file);
                        }
                        _ => {}
                    }
                }
                builder = builder.multipart(form);
            }
            BodySpec::BinaryFile(path) => {
                let body = tokio::fs::read(path)
                    .await
                    .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
                sent_bytes = body.len() as u64;
                payload_hash = Some(sha256_hex(&body));
                builder = builder.body(body);
            }
        }

        let (builder, digest) = apply_auth(
            builder,
            &spec.auth,
            secrets,
            &client,
            environment,
            local_variables,
            spec.method,
            &url,
            payload_hash.as_deref(),
            &aws_headers,
        )
        .await?;
        Ok(Self {
            builder,
            sent_bytes,
            timeout_ms: spec.timeout_ms.max(1),
            digest,
        })
    }

    async fn send(self) -> Result<reqwest::Response, ExecutionError> {
        let timeout_ms = self.timeout_ms;
        let Some(credentials) = self.digest else {
            return self
                .builder
                .send()
                .await
                .map_err(|error| map_reqwest_error(error, timeout_ms));
        };
        let retry = self.builder.try_clone().ok_or_else(|| {
            ExecutionError::Authentication(
                "Digest authentication cannot replay this request body".into(),
            )
        })?;
        let response = self
            .builder
            .send()
            .await
            .map_err(|error| map_reqwest_error(error, timeout_ms))?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let challenge = response
            .headers()
            .get_all(WWW_AUTHENTICATE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find(|value| value.to_ascii_lowercase().contains("digest "))
            .ok_or_else(|| {
                ExecutionError::Authentication(
                    "server did not return a Digest authentication challenge".into(),
                )
            })?;
        let request = retry
            .try_clone()
            .and_then(|builder| builder.build().ok())
            .ok_or_else(|| {
                ExecutionError::Authentication("Digest request could not be rebuilt".into())
            })?;
        let authorization = digest_authorization(challenge, &request, &credentials)?;
        retry
            .header(AUTHORIZATION, authorization)
            .send()
            .await
            .map_err(|error| map_reqwest_error(error, timeout_ms))
    }
}

async fn build_client(
    spec: &HttpSpec,
    environment: &Environment,
    local_variables: &[Variable],
    secrets: &dyn SecretStore,
    cookies: Arc<Jar>,
) -> Result<Client, ExecutionError> {
    let mut builder = Client::builder()
        .redirect(if spec.follow_redirects {
            Policy::limited(10)
        } else {
            Policy::none()
        })
        .danger_accept_invalid_certs(!spec.validate_tls)
        .cookie_provider(cookies);
    if let Some(proxy) = &spec.proxy {
        let url = resolve_required(environment, local_variables, &proxy.url, "proxy URL")?;
        let mut configured = Proxy::all(&url)
            .map_err(|error| ExecutionError::InvalidRequest(format!("invalid proxy: {error}")))?;
        if let Some(username) = &proxy.username {
            let username =
                resolve_required(environment, local_variables, username, "proxy username")?;
            let password = proxy
                .password
                .as_ref()
                .map(|reference| secret_value(reference, secrets))
                .transpose()?
                .unwrap_or_default();
            configured = configured.basic_auth(&username, &password);
        }
        builder = builder.proxy(configured);
    }
    if let Some(certificate) = &spec.client_certificate {
        let pem = tokio::fs::read(&certificate.pem_file)
            .await
            .map_err(|error| {
                ExecutionError::InvalidRequest(format!(
                    "failed to read client certificate `{}`: {error}",
                    certificate.pem_file.display()
                ))
            })?;
        let identity = reqwest::Identity::from_pem(&pem).map_err(|error| {
            ExecutionError::InvalidRequest(format!("invalid client certificate PEM: {error}"))
        })?;
        builder = builder.identity(identity);
    }
    builder
        .build()
        .map_err(|error| map_reqwest_error(error, spec.timeout_ms.max(1)))
}

fn resolve_form(
    fields: &[FormField],
    environment: &Environment,
    local_variables: &[Variable],
) -> Result<Vec<(String, String)>, ExecutionError> {
    fields
        .iter()
        .filter(|field| field.enabled)
        .map(|field| {
            Ok((
                resolve_required(environment, local_variables, &field.name, "form field name")?,
                resolve_required(
                    environment,
                    local_variables,
                    &field.value,
                    "form field value",
                )?,
            ))
        })
        .collect()
}

fn resolve_required(
    environment: &Environment,
    local_variables: &[Variable],
    value: &str,
    field: &str,
) -> Result<String, ExecutionError> {
    let resolved = environment
        .resolve(value, local_variables)
        .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
    if resolved.missing().is_empty() {
        Ok(resolved.into_value())
    } else {
        Err(ExecutionError::InvalidRequest(format!(
            "{field} contains missing variables: {}",
            resolved.missing().join(", ")
        )))
    }
}

fn secret_value(
    reference: &apitest_core::SecretRef,
    secrets: &dyn SecretStore,
) -> Result<String, ExecutionError> {
    secrets
        .get(reference)
        .map_err(|error| ExecutionError::Authentication(error.to_string()))?
        .ok_or_else(|| {
            ExecutionError::Authentication(format!(
                "secret `{}` is unavailable",
                reference.as_str()
            ))
        })
}

#[derive(Debug, Clone)]
struct DigestCredentials {
    username: String,
    password: String,
}

// Authentication needs the resolved request parts for Digest, OAuth2, and SigV4 signing.
#[allow(clippy::too_many_arguments)]
async fn apply_auth(
    mut builder: reqwest::RequestBuilder,
    auth: &AuthSpec,
    secrets: &dyn SecretStore,
    client: &Client,
    environment: &Environment,
    local_variables: &[Variable],
    method: apitest_core::HttpMethod,
    url: &Url,
    payload_hash: Option<&str>,
    aws_headers: &[(String, String)],
) -> Result<(reqwest::RequestBuilder, Option<DigestCredentials>), ExecutionError> {
    let mut digest = None;
    match auth {
        AuthSpec::None => {}
        AuthSpec::Basic { username, password } => {
            builder = builder.basic_auth(username, Some(secret_value(password, secrets)?));
        }
        AuthSpec::Bearer { token } => {
            builder = builder.bearer_auth(secret_value(token, secrets)?);
        }
        AuthSpec::ApiKey {
            name,
            value,
            location,
        } => {
            let value = secret_value(value, secrets)?;
            match location {
                ApiKeyLocation::Header => builder = builder.header(name, value),
                ApiKeyLocation::Query => {
                    builder = builder.query(&[(name, value)]);
                }
            }
        }
        AuthSpec::OAuth2 {
            grant,
            token_url,
            client_id,
            client_secret,
            scopes,
            username,
            password,
            access_token,
            ..
        } => {
            let token = if let Some(access_token) = access_token {
                secret_value(access_token, secrets)?
            } else {
                acquire_oauth_token(
                    client,
                    *grant,
                    token_url,
                    client_id,
                    client_secret.as_ref(),
                    scopes,
                    username.as_deref(),
                    password.as_ref(),
                    environment,
                    local_variables,
                    secrets,
                )
                .await?
            };
            builder = builder.bearer_auth(token);
        }
        AuthSpec::Digest { username, password } => {
            digest = Some(DigestCredentials {
                username: resolve_required(
                    environment,
                    local_variables,
                    username,
                    "Digest username",
                )?,
                password: secret_value(password, secrets)?,
            });
        }
        AuthSpec::AwsSigV4 {
            access_key,
            secret_key,
            session_token,
            region,
            service,
        } => {
            let payload_hash = payload_hash.ok_or_else(|| {
                ExecutionError::Authentication(
                    "AWS SigV4 does not support streaming multipart bodies".into(),
                )
            })?;
            builder = apply_aws_sigv4(
                builder,
                method,
                url,
                payload_hash,
                &secret_value(access_key, secrets)?,
                &secret_value(secret_key, secrets)?,
                session_token
                    .as_ref()
                    .map(|reference| secret_value(reference, secrets))
                    .transpose()?
                    .as_deref(),
                &resolve_required(environment, local_variables, region, "AWS region")?,
                &resolve_required(environment, local_variables, service, "AWS service")?,
                Utc::now(),
                aws_headers,
            )?;
        }
    }
    Ok((builder, digest))
}

#[allow(clippy::too_many_arguments)]
async fn acquire_oauth_token(
    client: &Client,
    grant: OAuth2Grant,
    token_url: &str,
    client_id: &str,
    client_secret: Option<&apitest_core::SecretRef>,
    scopes: &[String],
    username: Option<&str>,
    password: Option<&apitest_core::SecretRef>,
    environment: &Environment,
    local_variables: &[Variable],
    secrets: &dyn SecretStore,
) -> Result<String, ExecutionError> {
    if grant == OAuth2Grant::AuthorizationCodePkce {
        return Err(ExecutionError::Authentication(
            "OAuth 2.0 authorization code flow requires an interactive login; configure a cached access token"
                .into(),
        ));
    }
    let token_url = resolve_required(environment, local_variables, token_url, "OAuth token URL")?;
    let client_id = resolve_required(environment, local_variables, client_id, "OAuth client ID")?;
    let mut fields = vec![(
        "grant_type",
        match grant {
            OAuth2Grant::ClientCredentials => "client_credentials",
            OAuth2Grant::Password => "password",
            OAuth2Grant::AuthorizationCodePkce => unreachable!("handled above"),
        }
        .to_owned(),
    )];
    if !scopes.is_empty() {
        fields.push(("scope", scopes.join(" ")));
    }
    if grant == OAuth2Grant::Password {
        let username = username.ok_or_else(|| {
            ExecutionError::Authentication("OAuth password flow username is missing".into())
        })?;
        let password = password.ok_or_else(|| {
            ExecutionError::Authentication("OAuth password flow password is missing".into())
        })?;
        fields.push((
            "username",
            resolve_required(environment, local_variables, username, "OAuth username")?,
        ));
        fields.push(("password", secret_value(password, secrets)?));
    }
    let secret = client_secret
        .map(|reference| secret_value(reference, secrets))
        .transpose()?;
    if secret.is_none() {
        fields.push(("client_id", client_id.clone()));
    }
    let mut request = client
        .post(token_url)
        .timeout(Duration::from_secs(30))
        .form(&fields);
    if let Some(secret) = secret {
        request = request.basic_auth(client_id, Some(secret));
    }
    let response = request
        .send()
        .await
        .map_err(|error| map_reqwest_error(error, 30_000))?;
    let status = response.status();
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| {
            ExecutionError::Authentication(format!("invalid OAuth token response: {error}"))
        })?;
    if !status.is_success() {
        let message = value
            .get("error_description")
            .or_else(|| value.get("error"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("token endpoint rejected the request");
        return Err(ExecutionError::Authentication(format!(
            "OAuth token request failed with {status}: {message}"
        )));
    }
    value
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            ExecutionError::Authentication("OAuth token response has no access_token".into())
        })
}

#[allow(clippy::too_many_arguments)]
fn apply_aws_sigv4(
    builder: reqwest::RequestBuilder,
    method: apitest_core::HttpMethod,
    url: &Url,
    payload_hash: &str,
    access_key: &str,
    secret_key: &str,
    session_token: Option<&str>,
    region: &str,
    service: &str,
    now: chrono::DateTime<Utc>,
    extra_headers: &[(String, String)],
) -> Result<reqwest::RequestBuilder, ExecutionError> {
    if access_key.is_empty() || secret_key.is_empty() || region.is_empty() || service.is_empty() {
        return Err(ExecutionError::Authentication(
            "AWS SigV4 credentials, region and service must not be empty".into(),
        ));
    }
    let host = url.host_str().ok_or_else(|| {
        ExecutionError::InvalidRequest("AWS SigV4 URL does not contain a host".into())
    })?;
    let default_port = match url.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    let host = match url.port() {
        Some(port) if Some(port) != default_port => format!("{host}:{port}"),
        _ => host.to_owned(),
    };
    let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
    let date = now.format("%Y%m%d").to_string();
    let canonical_uri = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    let canonical_query = canonical_aws_query(url);
    let mut headers = BTreeMap::<String, Vec<String>>::from([
        ("host".into(), vec![host.trim().to_owned()]),
        ("x-amz-content-sha256".into(), vec![payload_hash.to_owned()]),
        ("x-amz-date".into(), vec![amz_date.clone()]),
    ]);
    for (name, value) in extra_headers {
        headers
            .entry(name.to_ascii_lowercase())
            .or_default()
            .push(normalize_aws_header_value(value));
    }
    if let Some(token) = session_token {
        headers.insert(
            "x-amz-security-token".into(),
            vec![normalize_aws_header_value(token)],
        );
    }
    let canonical_headers = headers
        .iter()
        .map(|(name, values)| format!("{name}:{}\n", values.join(",")))
        .collect::<String>();
    let signed_headers = headers.keys().cloned().collect::<Vec<_>>().join(";");
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method, canonical_uri, canonical_query, canonical_headers, signed_headers, payload_hash
    );
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let date_key = hmac_sha256(format!("AWS4{secret_key}").as_bytes(), date.as_bytes())?;
    let region_key = hmac_sha256(&date_key, region.as_bytes())?;
    let service_key = hmac_sha256(&region_key, service.as_bytes())?;
    let signing_key = hmac_sha256(&service_key, b"aws4_request")?;
    let signature = hex_lower(&hmac_sha256(&signing_key, string_to_sign.as_bytes())?);
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );
    let mut builder = builder
        .header("host", host)
        .header("x-amz-date", amz_date)
        .header("x-amz-content-sha256", payload_hash)
        .header(AUTHORIZATION, authorization);
    if let Some(token) = session_token {
        builder = builder.header("x-amz-security-token", token);
    }
    Ok(builder)
}

fn normalize_aws_header_value(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn canonical_aws_query(url: &Url) -> String {
    let mut parameters = url
        .query_pairs()
        .map(|(name, value)| (aws_encode(&name), aws_encode(&value)))
        .collect::<Vec<_>>();
    parameters.sort();
    parameters
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn aws_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

fn hmac_sha256(key: &[u8], value: &[u8]) -> Result<Vec<u8>, ExecutionError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|error| {
        ExecutionError::Authentication(format!("failed to initialize AWS signer: {error}"))
    })?;
    mac.update(value);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn sha256_hex(value: &[u8]) -> String {
    hex_lower(&Sha256::digest(value))
}

fn hex_lower(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value {
        encoded.push(char::from(HEX[(byte >> 4) as usize]));
        encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    encoded
}

#[derive(Debug, Clone, Copy)]
enum DigestAlgorithm {
    Md5 { session: bool },
    Sha256 { session: bool },
}

fn digest_authorization(
    challenge: &str,
    request: &reqwest::Request,
    credentials: &DigestCredentials,
) -> Result<String, ExecutionError> {
    let digest_start = challenge
        .to_ascii_lowercase()
        .find("digest ")
        .ok_or_else(|| ExecutionError::Authentication("invalid Digest challenge".into()))?;
    let parameters = parse_digest_parameters(&challenge[digest_start + "digest ".len()..]);
    let realm = parameters.get("realm").ok_or_else(|| {
        ExecutionError::Authentication("Digest challenge is missing realm".into())
    })?;
    let nonce = parameters.get("nonce").ok_or_else(|| {
        ExecutionError::Authentication("Digest challenge is missing nonce".into())
    })?;
    let algorithm_name = parameters
        .get("algorithm")
        .map(String::as_str)
        .unwrap_or("MD5");
    let algorithm = match algorithm_name.to_ascii_uppercase().as_str() {
        "MD5" => DigestAlgorithm::Md5 { session: false },
        "MD5-SESS" => DigestAlgorithm::Md5 { session: true },
        "SHA-256" => DigestAlgorithm::Sha256 { session: false },
        "SHA-256-SESS" => DigestAlgorithm::Sha256 { session: true },
        other => {
            return Err(ExecutionError::Authentication(format!(
                "unsupported Digest algorithm `{other}`"
            )));
        }
    };
    let qop = parameters.get("qop").map(|value| {
        value
            .split(',')
            .map(str::trim)
            .find(|value| value.eq_ignore_ascii_case("auth"))
            .map(str::to_owned)
    });
    if qop == Some(None) {
        return Err(ExecutionError::Authentication(
            "Digest challenge only supports auth-int; request replay is unavailable".into(),
        ));
    }
    let qop = qop.flatten();
    let uri = match request.url().query() {
        Some(query) => format!("{}?{query}", request.url().path()),
        None => request.url().path().to_owned(),
    };
    let cnonce = sha256_hex(
        format!(
            "{}:{}:{}",
            credentials.username,
            nonce,
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        )
        .as_bytes(),
    )
    .chars()
    .take(16)
    .collect::<String>();
    let nc = "00000001";
    let mut ha1 = digest_hex(
        algorithm,
        &format!("{}:{realm}:{}", credentials.username, credentials.password),
    );
    if matches!(
        algorithm,
        DigestAlgorithm::Md5 { session: true } | DigestAlgorithm::Sha256 { session: true }
    ) {
        ha1 = digest_hex(algorithm, &format!("{ha1}:{nonce}:{cnonce}"));
    }
    let ha2 = digest_hex(algorithm, &format!("{}:{uri}", request.method().as_str()));
    let response = if let Some(qop) = &qop {
        digest_hex(
            algorithm,
            &format!("{ha1}:{nonce}:{nc}:{cnonce}:{qop}:{ha2}"),
        )
    } else {
        digest_hex(algorithm, &format!("{ha1}:{nonce}:{ha2}"))
    };
    let mut authorization = format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", response=\"{}\", algorithm={}",
        escape_digest(&credentials.username),
        escape_digest(realm),
        escape_digest(nonce),
        escape_digest(&uri),
        response,
        algorithm_name
    );
    if let Some(opaque) = parameters.get("opaque") {
        authorization.push_str(&format!(", opaque=\"{}\"", escape_digest(opaque)));
    }
    if let Some(qop) = qop {
        authorization.push_str(&format!(
            ", qop={qop}, nc={nc}, cnonce=\"{}\"",
            escape_digest(&cnonce)
        ));
    }
    Ok(authorization)
}

fn parse_digest_parameters(value: &str) -> HashMap<String, String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' && quoted {
            escaped = true;
            continue;
        }
        if character == '"' {
            quoted = !quoted;
            current.push(character);
        } else if character == ',' && !quoted {
            fields.push(std::mem::take(&mut current));
        } else {
            current.push(character);
        }
    }
    if !current.trim().is_empty() {
        fields.push(current);
    }
    fields
        .into_iter()
        .filter_map(|field| {
            let (name, value) = field.split_once('=')?;
            Some((
                name.trim().to_ascii_lowercase(),
                value.trim().trim_matches('"').to_owned(),
            ))
        })
        .collect()
}

fn digest_hex(algorithm: DigestAlgorithm, value: &str) -> String {
    match algorithm {
        DigestAlgorithm::Md5 { .. } => hex_lower(&Md5::digest(value.as_bytes())),
        DigestAlgorithm::Sha256 { .. } => hex_lower(&Sha256::digest(value.as_bytes())),
    }
}

fn escape_digest(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn response_head(response: &reqwest::Response) -> ResponseHead {
    let mut headers = IndexMap::new();
    for name in response.headers().keys() {
        let values = response
            .headers()
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok().map(str::to_owned))
            .collect::<Vec<_>>();
        headers.insert(name.to_string(), values);
    }
    ResponseHead {
        status: Some(response.status().as_u16()),
        version: Some(format!("{:?}", response.version())),
        headers,
    }
}

fn map_reqwest_error(error: reqwest::Error, timeout_ms: u64) -> ExecutionError {
    if error.is_timeout() {
        ExecutionError::Timeout { timeout_ms }
    } else if error.is_builder() {
        ExecutionError::InvalidRequest(error.to_string())
    } else {
        ExecutionError::Network(error.to_string())
    }
}
