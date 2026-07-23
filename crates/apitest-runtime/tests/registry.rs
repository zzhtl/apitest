use std::sync::Arc;

use apitest_core::{
    Environment, ExecutionError, ExecutionEvent, ExecutionRequest, ExecutionStream, GraphQlSpec,
    GrpcCallKind, GrpcSpec, HttpMethod, HttpSpec, ProtocolExecutor, ProtocolKind, ProtocolSpec,
    SseSpec, WebSocketSpec,
};
use apitest_runtime::ExecutorRegistry;
use futures::{StreamExt, stream};
use tokio_util::sync::CancellationToken;

struct HttpFamilyExecutor;

impl ProtocolExecutor for HttpFamilyExecutor {
    fn kind(&self) -> ProtocolKind {
        ProtocolKind::Http
    }

    fn execute(
        &self,
        request: ExecutionRequest,
        _cancellation: CancellationToken,
    ) -> ExecutionStream {
        Box::pin(stream::iter(vec![Ok(ExecutionEvent::Started {
            id: request.id,
            at: chrono::Utc::now(),
        })]))
    }
}

struct TaggedExecutor {
    kind: ProtocolKind,
    tag: &'static str,
}

impl ProtocolExecutor for TaggedExecutor {
    fn kind(&self) -> ProtocolKind {
        self.kind
    }

    fn execute(
        &self,
        _request: ExecutionRequest,
        _cancellation: CancellationToken,
    ) -> ExecutionStream {
        Box::pin(stream::iter([Ok(ExecutionEvent::Data(
            self.tag.to_owned().into(),
        ))]))
    }
}

fn protocol_samples() -> Vec<(ProtocolSpec, &'static str)> {
    vec![
        (
            ProtocolSpec::Http(HttpSpec::new(
                HttpMethod::Get,
                "https://example.test/health",
            )),
            "http",
        ),
        (
            ProtocolSpec::GraphQl(GraphQlSpec {
                endpoint: "https://example.test/graphql".into(),
                query: "{ ping }".into(),
                variables: "{}".into(),
                operation_name: None,
                headers: Vec::new(),
                auth: apitest_core::AuthSpec::None,
                timeout_ms: 1_000,
            }),
            "graphql",
        ),
        (
            ProtocolSpec::Sse(SseSpec {
                request: HttpSpec::new(HttpMethod::Get, "https://example.test/events"),
                reconnect: true,
            }),
            "sse",
        ),
        (
            ProtocolSpec::WebSocket(WebSocketSpec {
                url: "wss://example.test/socket".into(),
                query: Vec::new(),
                headers: Vec::new(),
                subprotocols: Vec::new(),
                validate_tls: true,
                connect_timeout_ms: 1_000,
            }),
            "websocket",
        ),
        (
            ProtocolSpec::Grpc(GrpcSpec {
                endpoint: "https://example.test:443".into(),
                service: "demo.Greeter".into(),
                method: "SayHello".into(),
                call_kind: GrpcCallKind::Unary,
                descriptor_set: None,
                proto_files: Vec::new(),
                import_paths: Vec::new(),
                use_reflection: true,
                metadata: Vec::new(),
                message_json: "{}".into(),
                validate_tls: true,
                timeout_ms: 1_000,
            }),
            "grpc",
        ),
    ]
}

#[tokio::test]
async fn registry_can_route_multiple_protocol_kinds_to_one_executor() {
    let executor: Arc<dyn ProtocolExecutor> = Arc::new(HttpFamilyExecutor);
    let mut registry = ExecutorRegistry::new();
    registry.register(ProtocolKind::Http, Arc::clone(&executor));
    registry.register(ProtocolKind::GraphQl, executor);
    let request = ExecutionRequest::new(
        ProtocolSpec::GraphQl(GraphQlSpec {
            endpoint: "https://example.test/graphql".into(),
            query: "{ ping }".into(),
            variables: "{}".into(),
            operation_name: None,
            headers: Vec::new(),
            auth: apitest_core::AuthSpec::None,
            timeout_ms: 1_000,
        }),
        Environment::new("Local"),
    );

    let mut handle = registry.start(request).expect("GraphQL should be routed");
    let event = handle.events.next().await.expect("event should be emitted");

    assert!(matches!(event, Ok(ExecutionEvent::Started { .. })));
}

#[tokio::test]
async fn registry_dispatches_every_protocol_to_its_registered_executor() {
    let samples = protocol_samples();
    let mut registry = ExecutorRegistry::new();
    for (protocol, tag) in &samples {
        let kind = protocol.kind();
        registry.register(kind, Arc::new(TaggedExecutor { kind, tag }));
    }

    for (protocol, expected_tag) in samples {
        let request = ExecutionRequest::new(protocol, Environment::new("Local"));
        let mut handle = registry.start(request).expect("protocol should be routed");
        let event = handle
            .events
            .next()
            .await
            .expect("registered executor should emit an event")
            .expect("registered executor event should succeed");

        assert!(matches!(
            event,
            ExecutionEvent::Data(data) if data.as_ref() == expected_tag.as_bytes()
        ));
    }
}

#[test]
fn registry_reports_an_unsupported_protocol() {
    let registry = ExecutorRegistry::new();
    let request = ExecutionRequest::new(
        ProtocolSpec::Grpc(apitest_core::GrpcSpec {
            endpoint: "http://127.0.0.1:50051".into(),
            service: "demo.Greeter".into(),
            method: "SayHello".into(),
            call_kind: apitest_core::GrpcCallKind::Unary,
            descriptor_set: None,
            proto_files: Vec::new(),
            import_paths: Vec::new(),
            use_reflection: true,
            metadata: Vec::new(),
            message_json: "{}".into(),
            validate_tls: true,
            timeout_ms: 1_000,
        }),
        Environment::new("Local"),
    );

    let error = match registry.start(request) {
        Ok(_) => panic!("missing executor should be reported"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        ExecutionError::UnsupportedProtocol(ProtocolKind::Grpc)
    ));
}
