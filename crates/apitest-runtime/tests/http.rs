use std::sync::Arc;

use apitest_core::{
    ApiKeyLocation, AuthSpec, BodySpec, Environment, ExecutionError, ExecutionEvent,
    ExecutionRequest, FormField, GraphQlSpec, HttpMethod, HttpProxy, HttpSpec, KeyValue,
    MultipartPart, OAuth2Grant, ProtocolExecutor, ProtocolSpec, SecretRef, SseSpec, Variable,
};
use apitest_runtime::HttpExecutor;
use apitest_storage::{MemorySecretStore, SecretStore};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::Query,
    http::{StatusCode, header},
    response::IntoResponse,
    response::sse::{Event, Sse},
    routing::{any, get, post},
};
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use tokio::{net::TcpListener, time::Duration};
use tokio_util::sync::CancellationToken;

async fn spawn_server() -> String {
    async fn echo(Query(query): Query<std::collections::HashMap<String, String>>) -> Json<Value> {
        Json(json!({"name": query.get("name")}))
    }

    async fn graphql(Json(body): Json<Value>) -> Json<Value> {
        Json(json!({"received": body}))
    }

    async fn events() -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
        Sse::new(stream::iter([
            Ok(Event::default().event("ready").data("one")),
            Ok(Event::default().event("message").data("two")),
        ]))
    }

    async fn slow() -> &'static str {
        tokio::time::sleep(Duration::from_secs(5)).await;
        "late"
    }

    async fn inspect(request: axum::extract::Request) -> Json<Value> {
        let authorization = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let api_key = request
            .headers()
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let content_type = request
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let cookie = request
            .headers()
            .get("cookie")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let amz_date = request
            .headers()
            .get("x-amz-date")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let amz_target = request
            .headers()
            .get("x-amz-target")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let uri = request.uri().to_string();
        let body = to_bytes(request.into_body(), 2 * 1024 * 1024)
            .await
            .expect("request body should read");
        Json(json!({
            "authorization": authorization,
            "api_key": api_key,
            "content_type": content_type,
            "cookie": cookie,
            "amz_date": amz_date,
            "amz_target": amz_target,
            "uri": uri,
            "body": String::from_utf8_lossy(&body),
        }))
    }

    async fn oauth_token() -> Json<Value> {
        Json(json!({"access_token":"oauth-access-token","token_type":"Bearer"}))
    }

    async fn digest(request: axum::extract::Request) -> impl IntoResponse {
        let authorization = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        if authorization.starts_with("Digest ") {
            return Json(json!({"authorization": authorization})).into_response();
        }
        (
            StatusCode::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                r#"Digest realm="apitest", nonce="abcdef", qop="auth", algorithm=SHA-256, opaque="fixture""#,
            )],
        )
            .into_response()
    }

    async fn set_cookie() -> impl IntoResponse {
        (
            [(
                header::SET_COOKIE,
                "session=stored-cookie; Path=/; HttpOnly",
            )],
            Json(json!({"stored":true})),
        )
    }

    let app = Router::new()
        .route("/echo", get(echo))
        .route("/graphql", post(graphql))
        .route("/events", get(events))
        .route("/slow", get(slow))
        .route("/oauth/token", post(oauth_token))
        .route("/digest", get(digest))
        .route("/set-cookie", get(set_cookie))
        .route("/inspect", any(inspect));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener should bind");
    let address = listener.local_addr().expect("listener should have address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("test server should run");
    });
    format!("http://{address}")
}

async fn execute_json(executor: &HttpExecutor, spec: HttpSpec) -> Value {
    let events = executor
        .execute(
            ExecutionRequest::new(ProtocolSpec::Http(spec), Environment::new("test")),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;
    let mut body = Vec::new();
    for event in events {
        if let ExecutionEvent::Data(chunk) = event.expect("request should succeed") {
            body.extend_from_slice(&chunk);
        }
    }
    serde_json::from_slice(&body).expect("response should be JSON")
}

#[tokio::test]
async fn executes_http_with_environment_and_query_variables() {
    let base_url = spawn_server().await;
    let executor = HttpExecutor::new(Arc::new(MemorySecretStore::default()));
    let mut spec = HttpSpec::new(HttpMethod::Get, "{{base_url}}/echo");
    spec.query.push(KeyValue::enabled("name", "{{user}}"));
    let environment = Environment::new("test")
        .with_variable(Variable::plain("base_url", base_url))
        .with_variable(Variable::plain("user", "Ada"));

    let events = executor
        .execute(
            ExecutionRequest::new(ProtocolSpec::Http(spec), environment),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    let mut body = Vec::new();
    let mut status = None;
    for event in events {
        match event.expect("request should succeed") {
            ExecutionEvent::ResponseHead(head) => status = head.status,
            ExecutionEvent::Data(chunk) => body.extend_from_slice(&chunk),
            _ => {}
        }
    }
    assert_eq!(status, Some(200));
    assert_eq!(
        serde_json::from_slice::<Value>(&body).expect("body should be JSON"),
        json!({"name":"Ada"})
    );
}

#[tokio::test]
async fn sends_graphql_as_a_structured_post_request() {
    let base_url = spawn_server().await;
    let executor = HttpExecutor::new(Arc::new(MemorySecretStore::default()));
    let spec = GraphQlSpec {
        endpoint: format!("{base_url}/graphql"),
        query: "query User($id: ID!) { user(id: $id) { name } }".to_owned(),
        variables: r#"{"id":"42"}"#.to_owned(),
        operation_name: Some("User".to_owned()),
        headers: Vec::new(),
        auth: Default::default(),
        timeout_ms: 1_000,
    };

    let events = executor
        .execute(
            ExecutionRequest::new(ProtocolSpec::GraphQl(spec), Environment::new("test")),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    assert!(events.iter().all(Result::is_ok));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Ok(ExecutionEvent::Data(_))))
    );
}

#[tokio::test]
async fn exposes_sse_messages_incrementally() {
    let base_url = spawn_server().await;
    let executor = HttpExecutor::new(Arc::new(MemorySecretStore::default()));
    let spec = SseSpec {
        request: HttpSpec::new(HttpMethod::Get, format!("{base_url}/events")),
        reconnect: false,
    };

    let events = executor
        .execute(
            ExecutionRequest::new(ProtocolSpec::Sse(spec), Environment::new("test")),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;
    let messages = events
        .into_iter()
        .filter_map(|event| match event.expect("SSE request should succeed") {
            ExecutionEvent::Message { data, .. } => {
                Some(String::from_utf8_lossy(&data).into_owned())
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(messages, ["one", "two"]);
}

#[tokio::test]
async fn cancellation_stops_an_in_flight_request() {
    let base_url = spawn_server().await;
    let executor = HttpExecutor::new(Arc::new(MemorySecretStore::default()));
    let request = ExecutionRequest::new(
        ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, format!("{base_url}/slow"))),
        Environment::new("test"),
    );
    let cancellation = CancellationToken::new();
    let mut events = executor.execute(request, cancellation.clone());

    cancellation.cancel();
    let result = events
        .next()
        .await
        .expect("stream should terminate with an event");

    assert!(result.is_err());
}

#[tokio::test]
async fn applies_basic_bearer_and_api_key_authentication() {
    let base_url = spawn_server().await;
    let secrets = MemorySecretStore::default();
    let basic = SecretRef::new("keyring://basic");
    let bearer = SecretRef::new("keyring://bearer");
    let api_key = SecretRef::new("keyring://api-key");
    secrets
        .set(&basic, "secret")
        .expect("basic secret should save");
    secrets
        .set(&bearer, "token-value")
        .expect("bearer secret should save");
    secrets
        .set(&api_key, "query-value")
        .expect("api key should save");
    let executor = HttpExecutor::new(Arc::new(secrets));

    let mut spec = HttpSpec::new(HttpMethod::Get, format!("{base_url}/inspect"));
    spec.auth = AuthSpec::Basic {
        username: "ada".into(),
        password: basic,
    };
    let response = execute_json(&executor, spec).await;
    assert_eq!(response["authorization"], "Basic YWRhOnNlY3JldA==");

    let mut spec = HttpSpec::new(HttpMethod::Get, format!("{base_url}/inspect"));
    spec.auth = AuthSpec::Bearer { token: bearer };
    let response = execute_json(&executor, spec).await;
    assert_eq!(response["authorization"], "Bearer token-value");

    let mut spec = HttpSpec::new(HttpMethod::Get, format!("{base_url}/inspect"));
    spec.auth = AuthSpec::ApiKey {
        name: "access".into(),
        value: api_key,
        location: ApiKeyLocation::Query,
    };
    let response = execute_json(&executor, spec).await;
    assert!(
        response["uri"]
            .as_str()
            .is_some_and(|uri| uri.contains("access=query-value"))
    );

    let mut spec = HttpSpec::new(HttpMethod::Get, format!("{base_url}/inspect"));
    spec.auth = AuthSpec::ApiKey {
        name: "x-api-key".into(),
        value: SecretRef::new("keyring://api-key"),
        location: ApiKeyLocation::Header,
    };
    let response = execute_json(&executor, spec).await;
    assert_eq!(response["api_key"], "query-value");
}

#[tokio::test]
async fn acquires_oauth_tokens_and_answers_digest_challenges() {
    let base_url = spawn_server().await;
    let secrets = MemorySecretStore::default();
    let client_secret = SecretRef::new("keyring://oauth-client");
    let digest_password = SecretRef::new("keyring://digest-password");
    secrets
        .set(&client_secret, "client-secret")
        .expect("OAuth secret should save");
    secrets
        .set(&digest_password, "digest-secret")
        .expect("Digest secret should save");
    let executor = HttpExecutor::new(Arc::new(secrets));

    let mut oauth = HttpSpec::new(HttpMethod::Get, format!("{base_url}/inspect"));
    oauth.auth = AuthSpec::OAuth2 {
        grant: OAuth2Grant::ClientCredentials,
        authorization_url: None,
        token_url: format!("{base_url}/oauth/token"),
        client_id: "apitest-client".into(),
        client_secret: Some(client_secret),
        scopes: vec!["users:read".into()],
        username: None,
        password: None,
        access_token: None,
    };
    let response = execute_json(&executor, oauth).await;
    assert_eq!(response["authorization"], "Bearer oauth-access-token");

    let mut digest = HttpSpec::new(HttpMethod::Get, format!("{base_url}/digest?view=full"));
    digest.auth = AuthSpec::Digest {
        username: "ada".into(),
        password: digest_password,
    };
    let response = execute_json(&executor, digest).await;
    let authorization = response["authorization"]
        .as_str()
        .expect("Digest header should be returned");
    assert!(authorization.starts_with("Digest username=\"ada\""));
    assert!(authorization.contains("algorithm=SHA-256"));
    assert!(authorization.contains("uri=\"/digest?view=full\""));
    assert!(authorization.contains("qop=auth"));
}

#[tokio::test]
async fn signs_aws_requests_and_persists_cookies_between_runs() {
    let base_url = spawn_server().await;
    let secrets = MemorySecretStore::default();
    let access_key = SecretRef::new("keyring://aws-access");
    let secret_key = SecretRef::new("keyring://aws-secret");
    secrets
        .set(&access_key, "AKIDEXAMPLE")
        .expect("AWS access key should save");
    secrets
        .set(&secret_key, "aws-secret")
        .expect("AWS secret should save");
    let executor = HttpExecutor::new(Arc::new(secrets));

    let mut signed = HttpSpec::new(HttpMethod::Get, format!("{base_url}/inspect"));
    signed.auth = AuthSpec::AwsSigV4 {
        access_key,
        secret_key,
        session_token: None,
        region: "us-east-1".into(),
        service: "execute-api".into(),
    };
    signed.headers.push(KeyValue::enabled(
        "x-amz-target",
        "DynamoDB_20120810.ListTables",
    ));
    let response = execute_json(&executor, signed).await;
    assert!(
        response["authorization"]
            .as_str()
            .is_some_and(|value| value.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"))
    );
    assert!(
        response["amz_date"]
            .as_str()
            .is_some_and(|value| value.len() == 16 && value.ends_with('Z'))
    );
    assert_eq!(response["amz_target"], "DynamoDB_20120810.ListTables");
    assert!(
        response["authorization"]
            .as_str()
            .is_some_and(|value| value.contains("x-amz-target")),
        "the transmitted x-amz-target header must be signed"
    );

    execute_json(
        &executor,
        HttpSpec::new(HttpMethod::Get, format!("{base_url}/set-cookie")),
    )
    .await;
    let mut inspect = HttpSpec::new(HttpMethod::Get, format!("{base_url}/inspect"));
    inspect
        .cookies
        .push(KeyValue::enabled("explicit", "cookie-value"));
    let response = execute_json(&executor, inspect).await;
    let cookies = response["cookie"].as_str().expect("cookies should be sent");
    assert!(cookies.contains("session=stored-cookie"));
    assert!(cookies.contains("explicit=cookie-value"));
}

#[tokio::test]
async fn invalid_proxy_configuration_fails_before_network_io() {
    let executor = HttpExecutor::new(Arc::new(MemorySecretStore::default()));
    let mut spec = HttpSpec::new(HttpMethod::Get, "https://example.test");
    spec.proxy = Some(HttpProxy {
        url: "://invalid".into(),
        username: None,
        password: None,
    });

    let events = executor
        .execute(
            ExecutionRequest::new(ProtocolSpec::Http(spec), Environment::new("test")),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    assert!(events.iter().any(|event| {
        matches!(event, Err(ExecutionError::InvalidRequest(message)) if message.contains("proxy"))
    }));
}

#[tokio::test]
async fn sends_form_multipart_and_binary_request_bodies() {
    let base_url = spawn_server().await;
    let executor = HttpExecutor::new(Arc::new(MemorySecretStore::default()));

    let mut form = HttpSpec::new(HttpMethod::Post, format!("{base_url}/inspect"));
    form.body = BodySpec::FormUrlEncoded(vec![FormField {
        name: "name".into(),
        value: "Ada Lovelace".into(),
        enabled: true,
    }]);
    let response = execute_json(&executor, form).await;
    assert!(
        response["content_type"]
            .as_str()
            .is_some_and(|value| value.starts_with("application/x-www-form-urlencoded"))
    );
    assert_eq!(response["body"], "name=Ada+Lovelace");

    let file = tempfile::NamedTempFile::new().expect("fixture should create");
    std::fs::write(file.path(), "file-content").expect("fixture should write");
    let mut multipart = HttpSpec::new(HttpMethod::Post, format!("{base_url}/inspect"));
    multipart.body = BodySpec::Multipart(vec![
        MultipartPart::Text(FormField {
            name: "label".into(),
            value: "avatar".into(),
            enabled: true,
        }),
        MultipartPart::File {
            name: "file".into(),
            path: file.path().to_path_buf(),
            content_type: Some("text/plain".into()),
            enabled: true,
        },
    ]);
    let response = execute_json(&executor, multipart).await;
    assert!(
        response["content_type"]
            .as_str()
            .is_some_and(|value| value.starts_with("multipart/form-data"))
    );
    let body = response["body"].as_str().expect("body should be text");
    assert!(body.contains("avatar"));
    assert!(body.contains("file-content"));

    let mut binary = HttpSpec::new(HttpMethod::Post, format!("{base_url}/inspect"));
    binary.body = BodySpec::BinaryFile(file.path().to_path_buf());
    let response = execute_json(&executor, binary).await;
    assert_eq!(response["body"], "file-content");
}

#[tokio::test]
async fn timeout_error_reports_the_configured_duration() {
    let base_url = spawn_server().await;
    let executor = HttpExecutor::new(Arc::new(MemorySecretStore::default()));
    let mut spec = HttpSpec::new(HttpMethod::Get, format!("{base_url}/slow"));
    spec.timeout_ms = 20;

    let events = executor
        .execute(
            ExecutionRequest::new(ProtocolSpec::Http(spec), Environment::new("test")),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    assert!(
        events
            .iter()
            .any(|event| matches!(event, Err(ExecutionError::Timeout { timeout_ms: 20 })))
    );
}

#[tokio::test]
async fn sse_reconnects_after_the_server_closes_and_resumes_from_the_last_event_id() {
    // Each connection serves one event then ends; the executor must dial
    // again with `Last-Event-ID` when reconnect is enabled.
    let seen_ids = Arc::new(std::sync::Mutex::new(Vec::<Option<String>>::new()));
    let handler_ids = Arc::clone(&seen_ids);
    let app = Router::new().route(
        "/events",
        get(move |headers: axum::http::HeaderMap| {
            let seen_ids = Arc::clone(&handler_ids);
            async move {
                let last = headers
                    .get("last-event-id")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let attempt = {
                    let mut seen = seen_ids.lock().expect("id log should lock");
                    seen.push(last);
                    seen.len()
                };
                let payload = if attempt == 1 {
                    "id: 1\ndata: first\n\n"
                } else {
                    "id: 2\ndata: second\n\n"
                };
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    payload.to_owned(),
                )
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener should bind");
    let address = listener.local_addr().expect("listener should have address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("test server should run");
    });

    let executor = HttpExecutor::new(Arc::new(MemorySecretStore::default()));
    let spec = SseSpec {
        request: HttpSpec::new(HttpMethod::Get, format!("http://{address}/events")),
        reconnect: true,
    };
    let cancellation = CancellationToken::new();
    let mut stream = executor.execute(
        ExecutionRequest::new(ProtocolSpec::Sse(spec), Environment::new("test")),
        cancellation.clone(),
    );

    let mut messages = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let event = tokio::time::timeout_at(deadline, stream.next())
            .await
            .expect("reconnect should deliver the second event in time");
        match event {
            Some(Ok(ExecutionEvent::Message { data, .. })) => {
                messages.push(String::from_utf8_lossy(&data).into_owned());
                if messages.len() == 2 {
                    cancellation.cancel();
                }
            }
            Some(Ok(_)) => {}
            Some(Err(ExecutionError::Cancelled)) | None => break,
            Some(Err(error)) => panic!("unexpected SSE error: {error}"),
        }
    }

    assert_eq!(messages, ["first", "second"]);
    let seen = seen_ids.lock().expect("id log should lock");
    assert!(seen.len() >= 2, "server should have seen a reconnect");
    assert_eq!(seen[0], None);
    assert_eq!(seen[1].as_deref(), Some("1"));
}

#[tokio::test]
async fn oauth_tokens_are_cached_and_refreshed_instead_of_reacquired() {
    let grants = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let handler_grants = Arc::clone(&grants);
    async fn authorization(request: axum::extract::Request) -> Json<Value> {
        let authorization = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        Json(json!({"authorization": authorization}))
    }
    let app = Router::new()
        .route(
            "/token",
            post(
                move |axum::extract::Form(form): axum::extract::Form<
                    std::collections::HashMap<String, String>,
                >| {
                    let grants = Arc::clone(&handler_grants);
                    async move {
                        let grant = form.get("grant_type").cloned().unwrap_or_default();
                        grants
                            .lock()
                            .expect("grant log should lock")
                            .push(grant.clone());
                        let token = if grant == "refresh_token" {
                            "refreshed-token"
                        } else {
                            "initial-token"
                        };
                        // 60 s safety margin leaves a 1 s cached lifetime.
                        Json(json!({
                            "access_token": token,
                            "token_type": "Bearer",
                            "expires_in": 61,
                            "refresh_token": "refresh-1",
                        }))
                    }
                },
            ),
        )
        .route("/inspect", any(authorization));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener should bind");
    let address = listener.local_addr().expect("listener should have address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("test server should run");
    });

    let executor = HttpExecutor::new(Arc::new(MemorySecretStore::default()));
    let spec = || {
        let mut spec = HttpSpec::new(HttpMethod::Get, format!("http://{address}/inspect"));
        spec.auth = AuthSpec::OAuth2 {
            grant: OAuth2Grant::ClientCredentials,
            authorization_url: None,
            token_url: format!("http://{address}/token"),
            client_id: "apitest-client".into(),
            client_secret: None,
            scopes: vec!["users:read".into()],
            username: None,
            password: None,
            access_token: None,
        };
        spec
    };

    let first = execute_json(&executor, spec()).await;
    assert_eq!(first["authorization"], "Bearer initial-token");
    let second = execute_json(&executor, spec()).await;
    assert_eq!(
        second["authorization"], "Bearer initial-token",
        "the cached token must be reused without a new token request",
    );
    assert_eq!(
        grants.lock().expect("grant log should lock").as_slice(),
        ["client_credentials"],
    );

    // Let the cached token expire; renewal must go through the refresh grant.
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let third = execute_json(&executor, spec()).await;
    assert_eq!(third["authorization"], "Bearer refreshed-token");
    assert_eq!(
        grants.lock().expect("grant log should lock").as_slice(),
        ["client_credentials", "refresh_token"],
    );
}
