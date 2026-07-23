use std::{collections::BTreeMap, net::SocketAddr, sync::Arc, time::Duration};

use apitest_core::{HttpMethod, KeyValue, MockRule};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    response::Response,
};
use http::{Method, StatusCode};
use indexmap::IndexMap;
use serde::Deserialize;
use thiserror::Error;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

use crate::ScriptEngine;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockRoute {
    pub priority: i64,
    pub method: HttpMethod,
    pub path: String,
    pub query: Vec<KeyValue>,
    pub request_headers: Vec<KeyValue>,
    pub body_contains: Option<String>,
    pub path_variables: IndexMap<String, String>,
    pub status: u16,
    pub headers: Vec<KeyValue>,
    pub body: String,
    pub delay_ms: u64,
    pub script: String,
}

impl From<&MockRule> for MockRoute {
    fn from(rule: &MockRule) -> Self {
        Self {
            priority: rule.priority,
            method: rule.method,
            path: rule.path.clone(),
            query: rule.query.clone(),
            request_headers: rule.headers.clone(),
            body_contains: rule.body_contains.clone(),
            path_variables: rule.path_variables.clone(),
            status: rule.response.status,
            headers: rule.response.headers.clone(),
            body: rule.response.body.clone(),
            delay_ms: rule.response.delay_ms,
            script: rule.response.script.clone(),
        }
    }
}

#[derive(Debug, Error)]
pub enum MockServerError {
    #[error("failed to bind mock server: {0}")]
    Bind(#[from] std::io::Error),
}

pub struct MockServer {
    address: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl MockServer {
    pub async fn start(
        address: SocketAddr,
        routes: Vec<MockRoute>,
    ) -> Result<Self, MockServerError> {
        let listener = TcpListener::bind(address).await?;
        let address = listener.local_addr()?;
        let app = Router::new()
            .fallback(mock_handler)
            .with_state(Arc::new(routes));
        let (shutdown, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let result = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = receiver.await;
                })
                .await;
            if let Err(error) = result {
                tracing::error!(%error, "mock server stopped unexpectedly");
            }
        });
        Ok(Self {
            address,
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn mock_handler(
    State(routes): State<Arc<Vec<MockRoute>>>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, 1024 * 1024).await {
        Ok(body) => body,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(Body::from("Mock request body exceeds 1 MiB"))
                .expect("static mock response is valid");
        }
    };
    let query = url::form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    let route = routes
        .iter()
        .filter(|route| {
            parts.method == method(route.method)
                && path_matches(&route.path, parts.uri.path(), &route.path_variables)
                && route
                    .query
                    .iter()
                    .filter(|value| value.enabled)
                    .all(|expected| {
                        query
                            .iter()
                            .any(|(name, value)| name == &expected.name && value == &expected.value)
                    })
                && route
                    .request_headers
                    .iter()
                    .filter(|value| value.enabled)
                    .all(|expected| {
                        parts
                            .headers
                            .get(&expected.name)
                            .and_then(|value| value.to_str().ok())
                            .is_some_and(|value| value == expected.value)
                    })
                && route
                    .body_contains
                    .as_ref()
                    .is_none_or(|expected| String::from_utf8_lossy(&body).contains(expected))
        })
        .max_by_key(|route| route.priority);
    let Some(route) = route else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Mock route not found"))
            .expect("static mock response is valid");
    };

    if route.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(route.delay_ms)).await;
    }
    if route.script.trim().is_empty() {
        let status =
            StatusCode::from_u16(route.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut builder = Response::builder().status(status);
        for header in route.headers.iter().filter(|header| header.enabled) {
            builder = builder.header(&header.name, &header.value);
        }
        return builder
            .body(Body::from(route.body.clone()))
            .unwrap_or_else(|_| invalid_mock_response());
    }

    let request = serde_json::json!({
        "method": parts.method.as_str(),
        "path": parts.uri.path(),
        "query": query.iter().cloned().collect::<BTreeMap<_, _>>(),
        "headers": parts.headers.iter().filter_map(|(name, value)| {
            value.to_str().ok().map(|value| (name.as_str(), value))
        }).collect::<BTreeMap<_, _>>(),
        "body": String::from_utf8_lossy(&body),
        "pathVariables": path_variable_values(&route.path, parts.uri.path()),
    });
    let initial = serde_json::json!({
        "status": route.status,
        "headers": route.headers.iter().filter(|header| header.enabled).map(|header| {
            (header.name.clone(), header.value.clone())
        }).collect::<BTreeMap<_, _>>(),
        "body": route.body,
    });
    let script = route.script.clone();
    let response: DynamicMockResponse = match tokio::task::spawn_blocking(move || {
        ScriptEngine::default().run_mock(&script, &request, &initial)
    })
    .await
    {
        Ok(Ok(value)) => match serde_json::from_value(value) {
            Ok(response) => response,
            Err(error) => return script_error_response(error.to_string()),
        },
        Ok(Err(error)) => return script_error_response(error.to_string()),
        Err(error) => return script_error_response(error.to_string()),
    };
    let status = StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut builder = Response::builder().status(status);
    for (name, value) in response.headers {
        builder = builder.header(name, value);
    }
    builder
        .body(Body::from(response.body))
        .unwrap_or_else(|_| invalid_mock_response())
}

#[derive(Deserialize)]
struct DynamicMockResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: String,
}

fn script_error_response(error: String) -> Response<Body> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from(format!("Mock response script failed: {error}")))
        .expect("static mock script error response is valid")
}

fn invalid_mock_response() -> Response<Body> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(Body::from("Invalid mock response configuration"))
        .expect("static mock response is valid")
}

fn path_variable_values(pattern: &str, actual: &str) -> BTreeMap<String, String> {
    pattern
        .trim_matches('/')
        .split('/')
        .zip(actual.trim_matches('/').split('/'))
        .filter_map(|(expected, actual)| {
            expected
                .strip_prefix('{')
                .and_then(|value| value.strip_suffix('}'))
                .or_else(|| expected.strip_prefix(':'))
                .map(|name| (name.to_owned(), actual.to_owned()))
        })
        .collect()
}

fn path_matches(pattern: &str, actual: &str, variables: &IndexMap<String, String>) -> bool {
    let pattern = pattern.trim_matches('/').split('/').collect::<Vec<_>>();
    let actual = actual.trim_matches('/').split('/').collect::<Vec<_>>();
    pattern.len() == actual.len()
        && pattern.iter().zip(actual).all(|(expected, actual)| {
            let name = expected
                .strip_prefix('{')
                .and_then(|value| value.strip_suffix('}'))
                .or_else(|| expected.strip_prefix(':'));
            name.map_or_else(
                || expected == &actual,
                |name| variables.get(name).is_none_or(|value| value == actual),
            )
        })
}

fn method(method: HttpMethod) -> Method {
    match method {
        HttpMethod::Get => Method::GET,
        HttpMethod::Post => Method::POST,
        HttpMethod::Put => Method::PUT,
        HttpMethod::Patch => Method::PATCH,
        HttpMethod::Delete => Method::DELETE,
        HttpMethod::Head => Method::HEAD,
        HttpMethod::Options => Method::OPTIONS,
        HttpMethod::Trace => Method::TRACE,
        HttpMethod::Connect => Method::CONNECT,
    }
}
