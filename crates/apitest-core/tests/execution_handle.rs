use std::sync::Arc;

use apitest_core::{
    Environment, ExecutionCommand, ExecutionControlError, ExecutionEvent, ExecutionHandle,
    ExecutionRequest, ExecutionStream, HttpMethod, HttpSpec, ProtocolExecutor, ProtocolKind,
    ProtocolSpec,
};
use futures::{StreamExt, stream};
use tokio_util::sync::CancellationToken;

struct ImmediateExecutor;

impl ProtocolExecutor for ImmediateExecutor {
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

#[tokio::test]
async fn default_execution_handle_streams_events_and_cancels() {
    let executor: Arc<dyn ProtocolExecutor> = Arc::new(ImmediateExecutor);
    let request = ExecutionRequest::new(
        ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, "https://example.test")),
        Environment::new("Local"),
    );
    let expected_id = request.id;
    let mut handle = executor.start(request);

    let event = handle
        .events
        .next()
        .await
        .expect("executor should emit an event")
        .expect("event should succeed");
    assert!(matches!(event, ExecutionEvent::Started { id, .. } if id == expected_id));

    handle.cancel();
    assert!(handle.is_cancelled());
}

#[test]
fn non_interactive_handle_rejects_protocol_commands() {
    let handle = ExecutionHandle::from_stream(
        apitest_core::ExecutionId::new(),
        Box::pin(stream::empty()),
        CancellationToken::new(),
    );

    let error = handle
        .try_command(ExecutionCommand::CompleteInput)
        .expect_err("HTTP-style handle should reject interactive commands");

    assert_eq!(error, ExecutionControlError::Unsupported);
}
