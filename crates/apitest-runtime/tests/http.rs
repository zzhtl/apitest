use std::sync::Arc;

use apitest_core::{
    Environment, ExecutionEvent, ExecutionRequest, GraphQlSpec, HttpMethod, HttpSpec, KeyValue,
    ProtocolExecutor, ProtocolSpec, SseSpec, Variable,
};
use apitest_runtime::HttpExecutor;
use apitest_storage::MemorySecretStore;
use axum::{
    Json, Router,
    extract::Query,
    response::sse::{Event, Sse},
    routing::{get, post},
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

    let app = Router::new()
        .route("/echo", get(echo))
        .route("/graphql", post(graphql))
        .route("/events", get(events))
        .route("/slow", get(slow));
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
