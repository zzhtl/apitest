use std::{
    io::Write,
    time::{Duration, Instant},
};

use apitest_core::{
    AssertionRule, ExtractorRule, HttpMethod, KeyValue, MockRule, ScenarioNode, VariableSource,
};
use egui_kittest::Harness;
use indexmap::IndexMap;

use super::support::{mock_get, test_app};
use crate::services::loader::{load_automation, load_document_tabs};
use crate::services::scenario::load_scenario_datasets;
use crate::state::action::PendingAction;
use crate::workbench::{DocumentId, DocumentKind};

#[test]
fn nested_scenario_saves_reloads_and_restores_its_document_tab() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let request_id = harness.state().requests[0].request_case.id;

    harness
        .state_mut()
        .perform_action(PendingAction::NewScenario);
    let scenario = &mut harness.state_mut().scenarios[0];
    scenario.name = "Release smoke".into();
    scenario.nodes = vec![ScenarioNode::Group {
        name: "Authentication".into(),
        nodes: vec![ScenarioNode::If {
            expression: "env.enabled".into(),
            then_nodes: vec![ScenarioNode::Request {
                case_id: request_id,
                assertions: vec![
                    AssertionRule::Status { expected: 200 },
                    AssertionRule::HeaderEquals {
                        name: "content-type".into(),
                        expected: "application/json".into(),
                    },
                ],
                extractors: vec![ExtractorRule {
                    name: "request_id".into(),
                    source: VariableSource::Header("x-request-id".into()),
                }],
            }],
            else_nodes: vec![ScenarioNode::Delay { millis: 50 }],
        }],
    }];
    let expected = scenario.clone();

    assert!(harness.state().current_dirty());
    assert!(harness.state_mut().save_current_scenario());
    assert!(!harness.state().current_dirty());

    let state = harness.state();
    let database = state.database.as_deref().expect("database should exist");
    let (scenarios, _, errors) = load_automation(Some(database), state.project.id);
    assert!(
        errors.is_empty(),
        "reload should not report errors: {errors:?}"
    );
    assert_eq!(scenarios, vec![expected.clone()]);
    let restored_tabs = load_document_tabs(
        Some(database),
        state.project.id,
        &state.requests,
        &state.environments,
        &scenarios,
        &state.mock_profiles,
    );
    assert_eq!(
        restored_tabs.active(),
        Some(DocumentId {
            kind: DocumentKind::Scenario,
            entity_id: expected.id
        })
    );
    assert_eq!(
        restored_tabs
            .items()
            .iter()
            .find(|tab| tab.id.entity_id == expected.id)
            .map(|tab| tab.title.as_str()),
        Some("Release smoke")
    );
}

#[test]
fn scenario_runs_from_the_workbench_and_exposes_a_step_report() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let request_id = harness.state().requests[0].request_case.id;
    harness
        .state_mut()
        .perform_action(PendingAction::NewScenario);
    harness.state_mut().scenarios[0].nodes = vec![ScenarioNode::Request {
        case_id: request_id,
        assertions: vec![AssertionRule::Status { expected: 200 }],
        extractors: Vec::new(),
    }];

    let context = harness.ctx.clone();
    harness.state_mut().run_current_scenario(&context);
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        harness.state_mut().drain_runtime();
        if harness.state().scenario_cancellation.is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "scenario should finish within one second"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    let report = harness
        .state()
        .scenario_report
        .as_ref()
        .expect("scenario report should be retained");
    assert!(report.passed);
    assert_eq!(report.steps.len(), 1);
    assert_eq!(report.steps[0].name, "Health check");
    assert_eq!(report.steps[0].status, Some(200));
    assert_eq!(report.steps[0].assertions[0].name, "Status = 200");
}

#[test]
fn scenario_datasets_load_json_and_csv_rows_and_reject_invalid_shapes() {
    let mut json = tempfile::Builder::new()
        .suffix(".json")
        .tempfile()
        .expect("JSON dataset should create");
    json.write_all(br#"[{"tenant":"alpha","limit":2},{"tenant":"beta","limit":3}]"#)
        .expect("JSON dataset should write");
    let json_rows = load_scenario_datasets(Some(
        json.path()
            .to_str()
            .expect("temporary path should be UTF-8"),
    ))
    .expect("JSON dataset should load");
    assert_eq!(json_rows.len(), 2);
    assert_eq!(
        json_rows[0].get("tenant").map(String::as_str),
        Some("alpha")
    );
    assert_eq!(json_rows[1].get("limit").map(String::as_str), Some("3"));

    let mut csv = tempfile::Builder::new()
        .suffix(".csv")
        .tempfile()
        .expect("CSV dataset should create");
    csv.write_all(b"tenant,enabled\nalpha,true\nbeta,false\n")
        .expect("CSV dataset should write");
    let csv_rows = load_scenario_datasets(Some(
        csv.path().to_str().expect("temporary path should be UTF-8"),
    ))
    .expect("CSV dataset should load");
    assert_eq!(csv_rows.len(), 2);
    assert_eq!(csv_rows[1].get("tenant").map(String::as_str), Some("beta"));

    let mut invalid = tempfile::Builder::new()
        .suffix(".json")
        .tempfile()
        .expect("invalid dataset should create");
    invalid
        .write_all(br#"["not an object"]"#)
        .expect("invalid dataset should write");
    let error = load_scenario_datasets(Some(
        invalid
            .path()
            .to_str()
            .expect("temporary path should be UTF-8"),
    ))
    .expect_err("non-object rows should fail");
    assert!(error.contains("row must be an object"));
}

#[test]
fn smart_mock_generation_saves_reloads_and_restores_its_document_tab() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);

    harness.state_mut().perform_action(PendingAction::NewMock);
    harness.state_mut().mock_profiles[0].name = "Contract mock".into();
    harness.state_mut().generate_current_mock_rules();

    let profile = &mut harness.state_mut().mock_profiles[0];
    assert_eq!(profile.rules.len(), 1);
    assert_eq!(profile.rules[0].method, HttpMethod::Get);
    assert_eq!(profile.rules[0].path, "/health");
    profile.rules[0].response.status = 202;
    profile.rules[0].response.body = r#"{"accepted":true}"#.into();
    let expected = profile.clone();

    assert!(harness.state_mut().save_current_mock());
    assert!(!harness.state().current_dirty());

    let state = harness.state();
    let database = state.database.as_deref().expect("database should exist");
    let (_, profiles, errors) = load_automation(Some(database), state.project.id);
    assert!(
        errors.is_empty(),
        "reload should not report errors: {errors:?}"
    );
    assert_eq!(profiles, vec![expected.clone()]);
    let restored_tabs = load_document_tabs(
        Some(database),
        state.project.id,
        &state.requests,
        &state.environments,
        &state.scenarios,
        &profiles,
    );
    assert_eq!(
        restored_tabs.active(),
        Some(DocumentId {
            kind: DocumentKind::Mock,
            entity_id: expected.id
        })
    );
}

#[test]
fn desktop_mock_configuration_starts_a_server_and_matches_requests() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    harness.state_mut().perform_action(PendingAction::NewMock);
    let mut rule = MockRule {
        name: "Admin user".into(),
        method: HttpMethod::Get,
        path: "/users/{id}".into(),
        query: vec![KeyValue::enabled("role", "admin")],
        headers: vec![KeyValue::enabled("x-token", "desktop")],
        path_variables: IndexMap::from([("id".into(), "42".into())]),
        ..MockRule::default()
    };
    rule.response.status = 201;
    rule.response.headers = vec![KeyValue::enabled("content-type", "application/json")];
    rule.response.body = r#"{"id":42}"#.into();
    harness.state_mut().mock_profiles[0].name = "Desktop server".into();
    harness.state_mut().mock_profiles[0].rules = vec![rule];

    let context = harness.ctx.clone();
    harness.state_mut().start_current_mock(&context);
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        harness.state_mut().drain_runtime();
        if harness.state().mock_server.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "mock server should start within one second"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let address = harness
        .state()
        .mock_server
        .as_ref()
        .expect("mock server should be running")
        .address();

    let matched = mock_get(address, "/users/42?role=admin", &[("x-token", "desktop")]);
    assert!(
        matched.starts_with("HTTP/1.1 201"),
        "response was {matched:?}"
    );
    assert!(matched.contains("content-type: application/json"));
    assert!(matched.ends_with(r#"{"id":42}"#));

    let unmatched = mock_get(address, "/users/7?role=admin", &[("x-token", "desktop")]);
    assert!(
        unmatched.starts_with("HTTP/1.1 404"),
        "response was {unmatched:?}"
    );
    assert!(unmatched.ends_with("Mock route not found"));

    harness.state_mut().stop_current_mock();
    assert!(harness.state().mock_server.is_none());
}
