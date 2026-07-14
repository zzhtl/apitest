use std::sync::{Arc, Mutex};

use apitest_core::{
    Environment, ExecutionEvent, ExecutionMetrics, ExecutionRequest, ExecutionStream, HttpMethod,
    HttpSpec, ProtocolExecutor, ProtocolKind, ProtocolSpec, ResponseHead,
};
use apitest_runtime::{Scenario, ScenarioRunner, ScenarioStep};
use bytes::Bytes;
use futures::stream;
use indexmap::IndexMap;
use tokio_util::sync::CancellationToken;

struct FakeExecutor {
    seen_tokens: Arc<Mutex<Vec<Option<String>>>>,
}

impl ProtocolExecutor for FakeExecutor {
    fn kind(&self) -> ProtocolKind {
        ProtocolKind::Http
    }

    fn execute(
        &self,
        request: ExecutionRequest,
        _cancellation: CancellationToken,
    ) -> ExecutionStream {
        let token = request
            .local_variables
            .iter()
            .find(|variable| variable.name == "token")
            .map(|variable| variable.value.clone());
        self.seen_tokens.lock().expect("record lock").push(token);
        Box::pin(stream::iter(vec![
            Ok(ExecutionEvent::ResponseHead(ResponseHead {
                status: Some(201),
                version: Some("HTTP/1.1".into()),
                headers: IndexMap::new(),
            })),
            Ok(ExecutionEvent::Data(Bytes::from_static(br#"{"ok":true}"#))),
            Ok(ExecutionEvent::Completed(ExecutionMetrics {
                elapsed_ms: 4,
                received_bytes: 11,
                sent_bytes: 0,
            })),
        ]))
    }
}

#[tokio::test]
async fn runs_steps_sequentially_and_carries_script_variables() {
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let runner = ScenarioRunner::new().with_executor(Arc::new(FakeExecutor {
        seen_tokens: seen_tokens.clone(),
    }));
    let request = || {
        ExecutionRequest::new(
            ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, "https://example.com")),
            Environment::new("test"),
        )
    };
    let scenario = Scenario {
        name: "smoke".into(),
        stop_on_failure: true,
        steps: vec![
            ScenarioStep {
                name: "login".into(),
                request: request(),
                post_script: r#"
                    apitest.setVariable("token", "secret");
                    apitest.test("created", () => apitest.expect(response.status).toBe(201));
                "#
                .into(),
            },
            ScenarioStep {
                name: "profile".into(),
                request: request(),
                post_script: String::new(),
            },
        ],
    };

    let report = runner
        .run(scenario, CancellationToken::new())
        .await
        .expect("scenario should run");

    assert!(report.passed);
    assert_eq!(report.steps.len(), 2);
    assert_eq!(
        *seen_tokens.lock().expect("record lock"),
        vec![None, Some("secret".into())]
    );
}
