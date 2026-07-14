use std::{net::SocketAddr, sync::Arc, time::Duration};

use apitest_core::{HttpMethod, KeyValue};
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    response::Response,
};
use http::{Method, StatusCode};
use thiserror::Error;
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockRoute {
    pub method: HttpMethod,
    pub path: String,
    pub status: u16,
    pub headers: Vec<KeyValue>,
    pub body: String,
    pub delay_ms: u64,
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
    let route = routes.iter().find(|route| {
        request.method() == method(route.method) && request.uri().path() == route.path
    });
    let Some(route) = route else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Mock route not found"))
            .expect("static mock response is valid");
    };

    if route.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(route.delay_ms)).await;
    }
    let status = StatusCode::from_u16(route.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut builder = Response::builder().status(status);
    for header in route.headers.iter().filter(|header| header.enabled) {
        builder = builder.header(&header.name, &header.value);
    }
    builder
        .body(Body::from(route.body.clone()))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("Invalid mock response configuration"))
                .expect("static mock response is valid")
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
