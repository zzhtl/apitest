use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use apitest_core::{
    Environment, ExecutionError, ExecutionEvent, ExecutionMetrics, ExecutionRequest,
    ExecutionStream, ProtocolExecutor, ProtocolKind, ProtocolSpec, ResponseHead, Variable,
    WebSocketSpec,
};
use apitest_storage::SecretStore;
use async_stream::try_stream;
use bytes::Bytes;
use chrono::Utc;
use futures::{SinkExt, StreamExt};
use indexmap::IndexMap;
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderName, HeaderValue},
    },
};
use tokio_util::sync::CancellationToken;
use url::Url;

const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct WebSocketExecutor {
    secrets: Arc<dyn SecretStore>,
}

impl WebSocketExecutor {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self { secrets }
    }

    pub async fn open(
        &self,
        request: ExecutionRequest,
        cancellation: CancellationToken,
    ) -> Result<WebSocketSession, ExecutionError> {
        let ProtocolSpec::WebSocket(spec) = request.protocol else {
            return Err(ExecutionError::InvalidRequest(
                "WebSocket executor requires a WebSocket specification".to_owned(),
            ));
        };
        let (environment, locals) = materialize_variables(
            &request.environment,
            &request.local_variables,
            &*self.secrets,
        )?;
        let websocket_request = prepare_request(&spec, &environment, &locals)?;
        if websocket_request.uri().scheme_str() == Some("wss") && !spec.validate_tls {
            return Err(ExecutionError::Tls(
                "disabling WebSocket TLS validation is intentionally unsupported".to_owned(),
            ));
        }

        let connect = connect_async(websocket_request);
        let (socket, response) = tokio::select! {
            _ = cancellation.cancelled() => return Err(ExecutionError::Cancelled),
            result = tokio::time::timeout(Duration::from_millis(spec.connect_timeout_ms.max(1)), connect) => {
                match result {
                    Ok(Ok(value)) => value,
                    Ok(Err(error)) => return Err(ExecutionError::Network(error.to_string())),
                    Err(_) => return Err(ExecutionError::Timeout { timeout_ms: spec.connect_timeout_ms }),
                }
            }
        };

        let (command_tx, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(EVENT_CAPACITY);
        let head = websocket_response_head(&response);
        event_tx
            .send(Ok(ExecutionEvent::Started {
                id: request.id,
                at: Utc::now(),
            }))
            .await
            .map_err(|_| ExecutionError::Cancelled)?;
        event_tx
            .send(Ok(ExecutionEvent::ResponseHead(head)))
            .await
            .map_err(|_| ExecutionError::Cancelled)?;

        let task_cancellation = cancellation.clone();
        tokio::spawn(run_socket(socket, command_rx, event_tx, task_cancellation));
        Ok(WebSocketSession {
            commands: command_tx,
            events: event_rx,
            cancellation,
        })
    }
}

impl ProtocolExecutor for WebSocketExecutor {
    fn kind(&self) -> ProtocolKind {
        ProtocolKind::WebSocket
    }

    fn execute(
        &self,
        request: ExecutionRequest,
        cancellation: CancellationToken,
    ) -> ExecutionStream {
        let executor = self.clone();
        Box::pin(try_stream! {
            let mut session = executor.open(request, cancellation).await?;
            while let Some(event) = session.recv().await {
                yield event?;
            }
        })
    }
}

pub struct WebSocketSession {
    commands: mpsc::Sender<WebSocketCommand>,
    events: mpsc::Receiver<Result<ExecutionEvent, ExecutionError>>,
    cancellation: CancellationToken,
}

impl WebSocketSession {
    pub async fn send_text(&self, value: impl Into<String>) -> Result<(), ExecutionError> {
        self.commands
            .send(WebSocketCommand::Text(value.into()))
            .await
            .map_err(|_| ExecutionError::Cancelled)
    }

    pub async fn send_binary(&self, value: Bytes) -> Result<(), ExecutionError> {
        self.commands
            .send(WebSocketCommand::Binary(value))
            .await
            .map_err(|_| ExecutionError::Cancelled)
    }

    pub async fn close(&self) -> Result<(), ExecutionError> {
        self.commands
            .send(WebSocketCommand::Close)
            .await
            .map_err(|_| ExecutionError::Cancelled)
    }

    pub async fn recv(&mut self) -> Option<Result<ExecutionEvent, ExecutionError>> {
        self.events.recv().await
    }
}

impl Drop for WebSocketSession {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

enum WebSocketCommand {
    Text(String),
    Binary(Bytes),
    Close,
}

async fn run_socket<S>(
    socket: tokio_tungstenite::WebSocketStream<S>,
    mut commands: mpsc::Receiver<WebSocketCommand>,
    events: mpsc::Sender<Result<ExecutionEvent, ExecutionError>>,
    cancellation: CancellationToken,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let started = Instant::now();
    let (mut writer, mut reader) = socket.split();
    let mut sent_bytes = 0_u64;
    let mut received_bytes = 0_u64;

    loop {
        tokio::select! {
            _ = cancellation.cancelled() => {
                let _ = writer.send(Message::Close(None)).await;
                let _ = events.send(Err(ExecutionError::Cancelled)).await;
                return;
            }
            command = commands.recv() => {
                match command {
                    Some(WebSocketCommand::Text(value)) => {
                        let data = Bytes::copy_from_slice(value.as_bytes());
                        if let Err(error) = writer.send(Message::Text(value.into())).await {
                            let _ = events.send(Err(ExecutionError::Network(error.to_string()))).await;
                            return;
                        }
                        sent_bytes = sent_bytes.saturating_add(data.len() as u64);
                        if events.send(Ok(ExecutionEvent::Message {
                            outgoing: true,
                            media_type: Some("text/plain".to_owned()),
                            data,
                            at: Utc::now(),
                        })).await.is_err() {
                            return;
                        }
                    }
                    Some(WebSocketCommand::Binary(value)) => {
                        if let Err(error) = writer.send(Message::Binary(value.clone())).await {
                            let _ = events.send(Err(ExecutionError::Network(error.to_string()))).await;
                            return;
                        }
                        sent_bytes = sent_bytes.saturating_add(value.len() as u64);
                        if events.send(Ok(ExecutionEvent::Message {
                            outgoing: true,
                            media_type: Some("application/octet-stream".to_owned()),
                            data: value,
                            at: Utc::now(),
                        })).await.is_err() {
                            return;
                        }
                    }
                    Some(WebSocketCommand::Close) | None => {
                        let _ = writer.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
            message = reader.next() => {
                match message {
                    Some(Ok(Message::Text(value))) => {
                        let data = Bytes::copy_from_slice(value.as_bytes());
                        received_bytes = received_bytes.saturating_add(data.len() as u64);
                        if events.send(Ok(ExecutionEvent::Message {
                            outgoing: false,
                            media_type: Some("text/plain".to_owned()),
                            data,
                            at: Utc::now(),
                        })).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Binary(value))) => {
                        received_bytes = received_bytes.saturating_add(value.len() as u64);
                        if events.send(Ok(ExecutionEvent::Message {
                            outgoing: false,
                            media_type: Some("application/octet-stream".to_owned()),
                            data: value,
                            at: Utc::now(),
                        })).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Ping(value))) => {
                        if writer.send(Message::Pong(value)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(error)) => {
                        let _ = events.send(Err(ExecutionError::Network(error.to_string()))).await;
                        return;
                    }
                }
            }
        }
    }

    let metrics = ExecutionMetrics {
        elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        received_bytes,
        sent_bytes,
    };
    let _ = events.send(Ok(ExecutionEvent::Completed(metrics))).await;
}

fn prepare_request(
    spec: &WebSocketSpec,
    environment: &Environment,
    locals: &[Variable],
) -> Result<http::Request<()>, ExecutionError> {
    let url = resolve_required(environment, locals, &spec.url, "WebSocket URL")?;
    let mut url = Url::parse(&url).map_err(|error| {
        ExecutionError::InvalidRequest(format!("invalid WebSocket URL: {error}"))
    })?;
    {
        let mut query = url.query_pairs_mut();
        for pair in spec.query.iter().filter(|pair| pair.enabled) {
            query.append_pair(
                &resolve_required(environment, locals, &pair.name, "query name")?,
                &resolve_required(environment, locals, &pair.value, "query value")?,
            );
        }
    }
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?;
    for pair in spec.headers.iter().filter(|pair| pair.enabled) {
        let name = resolve_required(environment, locals, &pair.name, "header name")?;
        let value = resolve_required(environment, locals, &pair.value, "header value")?;
        request.headers_mut().append(
            HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?,
            HeaderValue::from_str(&value)
                .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?,
        );
    }
    if !spec.subprotocols.is_empty() {
        request.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_str(&spec.subprotocols.join(", "))
                .map_err(|error| ExecutionError::InvalidRequest(error.to_string()))?,
        );
    }
    Ok(request)
}

fn materialize_variables(
    environment: &Environment,
    locals: &[Variable],
    secrets: &dyn SecretStore,
) -> Result<(Environment, Vec<Variable>), ExecutionError> {
    let materialize = |variable: &Variable| -> Result<Variable, ExecutionError> {
        let Some(reference) = &variable.secret_ref else {
            return Ok(variable.clone());
        };
        let value = secrets
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

    let mut environment = environment.clone();
    environment.variables = environment
        .variables
        .iter()
        .map(materialize)
        .collect::<Result<Vec<_>, _>>()?;
    let locals = locals
        .iter()
        .map(materialize)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((environment, locals))
}

fn resolve_required(
    environment: &Environment,
    locals: &[Variable],
    value: &str,
    field: &str,
) -> Result<String, ExecutionError> {
    let resolved = environment
        .resolve(value, locals)
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

fn websocket_response_head<T>(response: &http::Response<T>) -> ResponseHead {
    let mut headers = IndexMap::new();
    for name in response.headers().keys() {
        headers.insert(
            name.to_string(),
            response
                .headers()
                .get_all(name)
                .iter()
                .filter_map(|value| value.to_str().ok().map(str::to_owned))
                .collect(),
        );
    }
    ResponseHead {
        status: Some(response.status().as_u16()),
        version: Some(format!("{:?}", response.version())),
        headers,
    }
}
