use std::sync::Arc;

use apitest_core::{Environment, ExecutionEvent, ExecutionRequest, ProtocolSpec, WebSocketSpec};
use apitest_runtime::WebSocketExecutor;
use apitest_storage::MemorySecretStore;
use axum::{
    Router,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::Response,
    routing::get,
};
use futures::StreamExt;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

async fn echo(socket: WebSocketUpgrade) -> Response {
    socket.on_upgrade(handle_socket)
}

async fn handle_socket(mut socket: WebSocket) {
    while let Some(Ok(message)) = socket.next().await {
        match message {
            Message::Text(text) => {
                if socket.send(Message::Text(text)).await.is_err() {
                    break;
                }
            }
            Message::Binary(bytes) => {
                if socket.send(Message::Binary(bytes)).await.is_err() {
                    break;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

async fn spawn_server() -> String {
    let app = Router::new().route("/echo", get(echo));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener should bind");
    let address = listener.local_addr().expect("listener should have address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("test server should run");
    });
    format!("ws://{address}/echo")
}

#[tokio::test]
async fn opens_interactive_session_and_echoes_text() {
    let endpoint = spawn_server().await;
    let executor = WebSocketExecutor::new(Arc::new(MemorySecretStore::default()));
    let spec = WebSocketSpec {
        url: endpoint,
        query: Vec::new(),
        headers: Vec::new(),
        subprotocols: Vec::new(),
        validate_tls: true,
        connect_timeout_ms: 1_000,
    };
    let request = ExecutionRequest::new(ProtocolSpec::WebSocket(spec), Environment::new("test"));
    let mut session = executor
        .open(request, CancellationToken::new())
        .await
        .expect("session should connect");

    session
        .send_text("hello")
        .await
        .expect("message should send");

    let mut outgoing = false;
    let mut incoming = false;
    for _ in 0..6 {
        let event = session
            .recv()
            .await
            .expect("session should emit event")
            .expect("session event should succeed");
        if let ExecutionEvent::Message {
            outgoing: direction,
            data,
            ..
        } = event
            && data.as_ref() == b"hello"
        {
            outgoing |= direction;
            incoming |= !direction;
        }
        if outgoing && incoming {
            break;
        }
    }

    assert!(outgoing, "sent message should appear in timeline");
    assert!(incoming, "echoed message should appear in timeline");
    session.close().await.expect("session should close");
}
