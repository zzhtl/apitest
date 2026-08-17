use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use apitest_core::{
    ProtocolExecutor, ProtocolKind, RunState as HistoryRunState, SecretRef, Variable,
};
use apitest_runtime::ExecutorRegistry;
use apitest_storage::{BodyStore, PageRequest};
use egui_kittest::{Harness, kittest::Queryable as _};

use super::support::{HistoryExecutor, test_app};
use crate::draft::EditablePair;
use crate::state::action::PendingAction;
use crate::state::workspace::Navigation;

#[test]
fn execution_history_externalizes_and_redacts_streamed_response_bodies() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let temp = tempfile::tempdir().expect("history directory should create");
    harness.state_mut().body_store = Some(
        BodyStore::new(temp.path().join("bodies")).expect("history body store should initialize"),
    );
    harness.state_mut().requests[0]
        .draft
        .headers
        .push(EditablePair::new("Authorization", "Bearer token-secret"));
    let mut executors = ExecutorRegistry::new();
    let executor: Arc<dyn ProtocolExecutor> = Arc::new(HistoryExecutor);
    executors.register(ProtocolKind::Http, executor);
    harness.state_mut().executors = Arc::new(executors);

    let context = harness.ctx.clone();
    harness.state_mut().send_current(&context);
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        harness.state_mut().drain_runtime();
        if harness.state().history_record.is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "history should finish within one second"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    let history = harness
        .state()
        .database
        .as_ref()
        .expect("database should exist")
        .list_run_records(harness.state().project.id, PageRequest::new(0, 10))
        .expect("history should load");
    assert_eq!(history.total, 1);
    assert_eq!(history.items[0].state, HistoryRunState::Passed);
    assert_eq!(history.items[0].status_code, Some(200));
    assert_eq!(history.items[0].response_bytes, 18);
    let body_path = history.items[0]
        .body_path
        .as_ref()
        .expect("history body should be externalized");
    let body = std::fs::read(body_path).expect("history body should read");
    assert_eq!(body, b"token=[REDACTED]");
    assert!(!String::from_utf8_lossy(&body).contains("token-secret"));

    harness
        .state_mut()
        .perform_action(PendingAction::Navigate(Navigation::History));
    harness.step();
    assert!(harness.query_by_label_contains("运行历史").is_some());
    assert!(
        harness
            .query_by_label_contains("token=[REDACTED]")
            .is_some()
    );
    assert!(harness.query_by_label_contains("token-secret").is_none());
}

#[test]
fn history_redaction_includes_request_local_secret_references() {
    let harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let reference = SecretRef::new("keyring://request-local-api-key");
    harness
        .state()
        .secrets
        .set(&reference, "local-secret")
        .expect("local secret should save");
    let mut harness = harness;
    harness.state_mut().requests[0]
        .request_case
        .local_variables
        .push(Variable::secret("api_key", reference.0));

    let values = harness.state().history_redaction_values(0, 0);

    assert!(values.iter().any(|value| value == "local-secret"));
}
