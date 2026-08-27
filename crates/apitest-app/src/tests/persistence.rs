use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use apitest_core::{GrpcCallKind, KeyValue, ProtocolKind, ProtocolSpec, TestScenario};
use egui_kittest::Harness;

use super::support::test_app;
use crate::i18n::Language;
use crate::persistence::StorageEvent;
use crate::state::action::PendingAction;

#[test]
fn deleting_a_new_request_cannot_be_undone_by_a_queued_save() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    harness
        .state_mut()
        .perform_action(PendingAction::NewRequest(ProtocolKind::Http));
    let index = harness.state().selected;
    harness.state_mut().requests[index].draft.url = "https://example.test/new".into();
    let request_id = harness.state().requests[index].id();
    let database = Arc::clone(
        harness
            .state()
            .database
            .as_ref()
            .expect("database should exist"),
    );

    assert!(harness.state_mut().queue_request_save(index, true, false));
    harness.state_mut().delete_request(request_id);

    assert!(
        database
            .get_definition(request_id)
            .expect("definition lookup should succeed")
            .is_none()
    );
}

#[test]
fn discarding_an_edit_restores_the_database_after_a_queued_save() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let request_id = harness.state().requests[0].id();
    harness.state_mut().requests[0].name = "Queued edit".into();

    assert!(harness.state_mut().queue_request_save(0, true, false));
    harness.state_mut().discard_current();

    let stored = harness
        .state()
        .database
        .as_ref()
        .expect("database should exist")
        .get_definition(request_id)
        .expect("definition lookup should succeed")
        .expect("definition should exist");
    assert_eq!(stored.name, "Health check");
}

#[test]
fn leaving_the_workspace_checks_dirty_background_documents() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    harness.state_mut().scenarios.push(TestScenario {
        name: "Unsaved background scenario".into(),
        ..TestScenario::default()
    });

    harness.state_mut().queue_action(PendingAction::Close);

    assert!(matches!(
        harness.state().confirmation,
        Some(crate::state::action::Confirmation::Unsaved(
            PendingAction::Close
        ))
    ));
    assert!(!harness.state().allow_close);
}

#[test]
fn deleting_a_new_environment_cannot_be_undone_by_a_queued_save() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    harness
        .state_mut()
        .perform_action(PendingAction::NewEnvironment);
    let index = harness.state().selected_environment;
    let environment_id = harness.state().environments[index].id();
    let database = Arc::clone(
        harness
            .state()
            .database
            .as_ref()
            .expect("database should exist"),
    );

    assert!(
        harness
            .state_mut()
            .queue_environment_save(index, true, false)
    );
    harness.state_mut().delete_environment(environment_id);

    assert!(
        database
            .list_environments(harness.state().project.id)
            .expect("environments should load")
            .iter()
            .all(|environment| environment.id != environment_id)
    );
}

#[test]
fn saving_before_leaving_waits_for_all_dirty_documents() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let request_id = harness.state().requests[0].id();
    harness.state_mut().requests[0].name = "Persisted before leave".into();
    let scenario = TestScenario {
        name: "Background scenario".into(),
        ..TestScenario::default()
    };
    let scenario_id = scenario.id;
    harness.state_mut().scenarios.push(scenario);

    assert!(harness.state_mut().save_workspace_and_wait());

    let database = harness
        .state()
        .database
        .as_ref()
        .expect("database should exist");
    assert_eq!(
        database
            .get_definition(request_id)
            .expect("definition should load")
            .expect("definition should exist")
            .name,
        "Persisted before leave"
    );
    assert!(
        database
            .list_scenarios(harness.state().project.id)
            .expect("scenarios should load")
            .iter()
            .any(|scenario| scenario.id == scenario_id)
    );
    assert!(!harness.state().workspace_dirty());
}

#[test]
fn saving_an_edit_preserves_definition_metadata() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let request_id = harness.state().requests[0].id();
    harness.state_mut().requests[0]
        .definition
        .description_markdown = "kept".into();
    harness.state_mut().requests[0].definition.tags = vec!["critical".into()];
    harness.state_mut().requests[0].name = "Updated health check".into();

    assert!(harness.state_mut().save_current_request());
    harness
        .state()
        .storage_worker
        .as_ref()
        .expect("storage worker should exist")
        .recv_timeout(Duration::from_secs(1))
        .expect("save should complete");
    let stored = harness
        .state()
        .database
        .as_ref()
        .expect("database should exist")
        .get_definition(request_id)
        .expect("definition should load")
        .expect("definition should exist");

    assert_eq!(stored.name, "Updated health check");
    assert_eq!(stored.description_markdown, "kept");
    assert_eq!(stored.tags, ["critical"]);
}

#[test]
fn non_http_requests_autosave_and_reload_without_protocol_loss() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let mut expected = Vec::new();

    for (kind, name) in [
        (ProtocolKind::GraphQl, "GraphQL users"),
        (ProtocolKind::Sse, "SSE events"),
        (ProtocolKind::WebSocket, "WebSocket chat"),
        (ProtocolKind::Grpc, "gRPC greeter"),
    ] {
        let state = harness.state_mut();
        state.perform_action(PendingAction::NewRequest(kind));
        let index = state.selected;
        let mut protocol = crate::state::workspace::default_protocol(kind);
        match &mut protocol {
            ProtocolSpec::GraphQl(spec) => {
                spec.endpoint = "https://api.example.test/graphql".into();
                spec.query = "query Users { users { id name } }".into();
                spec.variables = r#"{"limit":25}"#.into();
                spec.operation_name = Some("Users".into());
                spec.headers.push(KeyValue::enabled("x-tenant", "dev"));
                spec.timeout_ms = 12_345;
            }
            ProtocolSpec::Sse(spec) => {
                spec.request.url = "https://api.example.test/events".into();
                spec.request
                    .headers
                    .push(KeyValue::enabled("last-event-id", "42"));
                spec.request.timeout_ms = 45_000;
                spec.reconnect = false;
            }
            ProtocolSpec::WebSocket(spec) => {
                spec.url = "wss://api.example.test/chat".into();
                spec.query.push(KeyValue::enabled("room", "rust"));
                spec.headers.push(KeyValue::enabled("x-client", "desktop"));
                spec.subprotocols = vec!["chat.v2".into(), "json".into()];
                spec.validate_tls = false;
                spec.connect_timeout_ms = 9_876;
            }
            ProtocolSpec::Grpc(spec) => {
                spec.endpoint = "https://grpc.example.test:443".into();
                spec.service = "demo.v1.Greeter".into();
                spec.method = "WatchHello".into();
                spec.call_kind = GrpcCallKind::ServerStreaming;
                spec.proto_files.push("proto/greeter.proto".into());
                spec.import_paths.push("proto".into());
                spec.use_reflection = false;
                spec.metadata.push(KeyValue::enabled("x-tenant", "dev"));
                spec.message_json = r#"{"name":"ApiTest"}"#.into();
                spec.validate_tls = false;
                spec.timeout_ms = 54_321;
            }
            ProtocolSpec::Http(_) => panic!("test only configures non-HTTP protocols"),
        }
        state.requests[index].name = name.into();
        state.requests[index].alternate_protocol = Some(protocol.clone());
        state.requests[index].sync_edit_revision(Instant::now());
        state.requests[index]
            .autosave
            .mark_changed(Instant::now() - Duration::from_secs(1));
        expected.push((name, protocol));
    }

    let context = harness.ctx.clone();
    harness.state_mut().schedule_request_autosaves(&context);
    for _ in 0..expected.len() {
        let event = harness
            .state()
            .storage_worker
            .as_ref()
            .expect("storage worker should exist")
            .recv_timeout(Duration::from_secs(1))
            .expect("autosave should complete");
        assert!(matches!(
            event,
            StorageEvent::RequestSaved { notify: false, .. }
        ));
    }

    let database = Arc::clone(
        harness
            .state()
            .database
            .as_ref()
            .expect("database should exist"),
    );
    let project = harness.state().project.clone();
    let (reloaded, _, _, errors) = crate::services::loader::load_project_content(
        Some(database.as_ref()),
        &project,
        Language::Chinese,
    );
    assert!(
        errors.is_empty(),
        "reload should not report errors: {errors:?}"
    );
    for (name, protocol) in expected {
        let request = reloaded
            .iter()
            .find(|request| request.name == name)
            .expect("saved protocol request should reload");
        assert_eq!(request.edited_protocol(), protocol);
        let definition = database
            .get_definition(request.id())
            .expect("definition should load")
            .expect("definition should exist");
        assert_eq!(definition.contract.example_protocol(), protocol);
    }
}

#[test]
fn environment_save_is_acknowledged_by_the_background_worker() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let environment_id = harness.state().environments[0].id();
    harness.state_mut().environments[0].name = "Staging".into();

    assert!(harness.state_mut().save_current_environment());
    let event = harness
        .state()
        .storage_worker
        .as_ref()
        .expect("storage worker should exist")
        .recv_timeout(Duration::from_secs(1))
        .expect("save should complete");
    assert!(matches!(
        event,
        StorageEvent::EnvironmentSaved {
            environment,
            notify: true,
            ..
        } if environment.id == environment_id && environment.name == "Staging"
    ));
    assert_eq!(
        harness
            .state()
            .database
            .as_ref()
            .expect("database should exist")
            .list_environments(harness.state().project.id)
            .expect("environments should load")[0]
            .name,
        "Staging"
    );
}

#[test]
fn strict_dirty_checks_see_edits_the_sweep_has_not_observed_yet() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);

    // A programmatic edit that no input event or sweep has synced yet.
    harness.state_mut().requests[0].draft.url = "https://example.test/stale-snapshot".into();

    // The cheap per-frame check still trusts the stale snapshot…
    assert!(!harness.state().workspace_dirty());
    // …while every decision point re-syncs and must see the edit.
    assert!(harness.state_mut().workspace_dirty_strict());
    assert!(harness.state().workspace_dirty());

    // The sweep marked the change, so the debounced autosave becomes due and
    // queues without any further input.
    harness.state_mut().requests[0]
        .autosave
        .mark_changed(Instant::now() - Duration::from_secs(1));
    let context = harness.ctx.clone();
    harness.state_mut().schedule_request_autosaves(&context);
    let event = harness
        .state()
        .storage_worker
        .as_ref()
        .expect("storage worker should exist")
        .recv_timeout(Duration::from_secs(1))
        .expect("autosave should complete");
    assert!(matches!(event, StorageEvent::RequestSaved { .. }));
}
