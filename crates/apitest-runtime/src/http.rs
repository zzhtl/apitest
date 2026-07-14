use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use apitest_core::{
    ApiKeyLocation, AuthSpec, BodySpec, Environment, ExecutionError, ExecutionEvent,
    ExecutionMetrics, ExecutionRequest, ExecutionStream, FormField, GraphQlSpec, HttpSpec,
    MultipartPart, ProtocolExecutor, ProtocolKind, ProtocolSpec, ResponseHead, Variable,
};
use apitest_storage::SecretStore;
use async_stream::try_stream;
use bytes::Bytes;
use chrono::Utc;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use indexmap::IndexMap;
use reqwest::{Client, Method, redirect::Policy};
use tokio_util::sync::CancellationToken;
use url::Url;

#[derive(Clone)]
pub struct HttpExecutor {
    secrets: Arc<dyn SecretStore>,
}

impl HttpExecutor {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self { secrets }
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
            let prepared = PreparedRequest::new(&spec, &environment, &local_variables, &*executor.secrets).await?;
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
                    let chunk = chunk.map_err(map_reqwest_error)?;
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
}

impl PreparedRequest {
    async fn new(
        spec: &HttpSpec,
        environment: &Environment,
        local_variables: &[Variable],
        secrets: &dyn SecretStore,
    ) -> Result<Self, ExecutionError> {
        let client = build_client(spec)?;
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

        let method = Method::from_bytes(spec.method.to_string().as_bytes())
            .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
        let mut builder = client
            .request(method, url)
            .timeout(Duration::from_millis(spec.timeout_ms.max(1)));

        for header in spec.headers.iter().filter(|header| header.enabled) {
            let name = resolve_required(environment, local_variables, &header.name, "header name")?;
            let value =
                resolve_required(environment, local_variables, &header.value, "header value")?;
            builder = builder.header(name, value);
        }

        let mut sent_bytes = 0_u64;
        match &spec.body {
            BodySpec::None => {}
            BodySpec::Json(value) => {
                let value = resolve_required(environment, local_variables, value, "JSON body")?;
                sent_bytes = value.len() as u64;
                builder = builder
                    .header("content-type", "application/json")
                    .body(value);
            }
            BodySpec::Text(value) => {
                let value = resolve_required(environment, local_variables, value, "text body")?;
                sent_bytes = value.len() as u64;
                builder = builder
                    .header("content-type", "text/plain; charset=utf-8")
                    .body(value);
            }
            BodySpec::Xml(value) => {
                let value = resolve_required(environment, local_variables, value, "XML body")?;
                sent_bytes = value.len() as u64;
                builder = builder
                    .header("content-type", "application/xml")
                    .body(value);
            }
            BodySpec::FormUrlEncoded(fields) => {
                let fields = resolve_form(fields, environment, local_variables)?;
                sent_bytes = fields
                    .iter()
                    .map(|(name, value)| name.len() + value.len())
                    .sum::<usize>() as u64;
                builder = builder.form(&fields);
            }
            BodySpec::Multipart(parts) => {
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
                builder = builder.body(body);
            }
        }

        builder = apply_auth(builder, &spec.auth, secrets)?;
        Ok(Self {
            builder,
            sent_bytes,
        })
    }

    async fn send(self) -> Result<reqwest::Response, ExecutionError> {
        self.builder.send().await.map_err(map_reqwest_error)
    }
}

fn build_client(spec: &HttpSpec) -> Result<Client, ExecutionError> {
    Client::builder()
        .redirect(if spec.follow_redirects {
            Policy::limited(10)
        } else {
            Policy::none()
        })
        .danger_accept_invalid_certs(!spec.validate_tls)
        .cookie_store(true)
        .build()
        .map_err(map_reqwest_error)
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

fn apply_auth(
    mut builder: reqwest::RequestBuilder,
    auth: &AuthSpec,
    secrets: &dyn SecretStore,
) -> Result<reqwest::RequestBuilder, ExecutionError> {
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
        AuthSpec::OAuth2 { .. } => {
            return Err(ExecutionError::Authentication(
                "OAuth token acquisition requires an interactive authorization session".to_owned(),
            ));
        }
        AuthSpec::Digest { .. } => {
            return Err(ExecutionError::Authentication(
                "Digest authentication requires a server challenge".to_owned(),
            ));
        }
        AuthSpec::AwsSigV4 { .. } => {
            return Err(ExecutionError::Authentication(
                "AWS SigV4 signing is not configured".to_owned(),
            ));
        }
    }
    Ok(builder)
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

fn map_reqwest_error(error: reqwest::Error) -> ExecutionError {
    if error.is_timeout() {
        ExecutionError::Timeout { timeout_ms: 0 }
    } else if error.is_builder() {
        ExecutionError::InvalidRequest(error.to_string())
    } else {
        ExecutionError::Network(error.to_string())
    }
}
