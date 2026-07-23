use std::time::Duration;

use apitest_core::{
    ApiDefinition, EntityId, HttpMethod, HttpSpec, MockProfile, MockResponse, MockRule, Project,
    ProjectNode, ProjectNodeKind, ProtocolSpec, RequestCase, RunRecord, RunState, TestScenario,
};
use apitest_storage::{Database, PageRequest};
use chrono::{TimeDelta, Utc};

fn project_with_definition(database: &Database) -> (Project, ApiDefinition) {
    let project = Project::new("Workspace v2");
    let definition = ApiDefinition::new(
        "List users",
        ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, "https://example.test/users")),
    );
    database
        .save_project(&project)
        .expect("project should save");
    database
        .save_definition(project.id, &definition)
        .expect("definition should save");
    (project, definition)
}

#[test]
fn schema_v2_persists_cases_and_pages_project_nodes() {
    let database = Database::open_in_memory().expect("database should open");
    let (project, definition) = project_with_definition(&database);
    let request_case = RequestCase::for_definition(&definition, "happy path");
    database
        .save_request_case(project.id, &request_case)
        .expect("case should save");
    for (name, order) in [("Users", 20), ("Admin", 10), ("Billing", 30)] {
        database
            .save_project_node(&ProjectNode {
                id: EntityId::new(),
                project_id: project.id,
                parent_id: None,
                entity_id: None,
                kind: ProjectNodeKind::Folder,
                name: name.into(),
                sort_order: order,
            })
            .expect("node should save");
    }

    let cases = database
        .list_request_cases(project.id, definition.id, PageRequest::new(0, 20))
        .expect("cases should load");
    let nodes = database
        .list_project_nodes(project.id, None, PageRequest::new(0, 2))
        .expect("nodes should load");

    assert_eq!(database.schema_version().expect("version should load"), 2);
    assert_eq!(cases.total, 1);
    assert_eq!(cases.items, vec![request_case]);
    assert_eq!(nodes.total, 3);
    assert_eq!(
        nodes
            .items
            .iter()
            .map(|node| node.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Admin", "Users"]
    );
}

#[test]
fn atomic_request_save_keeps_its_resource_node_in_sync() {
    let database = Database::open_in_memory().expect("database should open");
    let (project, mut definition) = project_with_definition(&database);
    let request_case = RequestCase::for_definition(&definition, "Default");

    database
        .save_definition_and_case(project.id, &definition, &request_case)
        .expect("request should save");
    definition.name = "Renamed users".into();
    database
        .save_definition_and_case(project.id, &definition, &request_case)
        .expect("renamed request should save");

    let nodes = database
        .list_project_nodes(project.id, None, PageRequest::new(0, 20))
        .expect("nodes should load");
    assert_eq!(nodes.total, 1);
    assert_eq!(nodes.items[0].entity_id, Some(definition.id));
    assert_eq!(nodes.items[0].name, "Renamed users");

    assert!(
        database
            .delete_definition(project.id, definition.id)
            .expect("definition should delete")
    );
    assert_eq!(
        database
            .list_project_nodes(project.id, None, PageRequest::new(0, 20))
            .expect("nodes should load")
            .total,
        0
    );
}

#[test]
fn scenarios_and_mock_profiles_round_trip_as_project_documents() {
    let database = Database::open_in_memory().expect("database should open");
    let project = Project::new("Automation");
    database
        .save_project(&project)
        .expect("project should save");
    let scenario = TestScenario {
        name: "Smoke".into(),
        ..TestScenario::default()
    };
    let profile = MockProfile {
        name: "Local mock".into(),
        port: 0,
        rules: vec![MockRule {
            name: "Health".into(),
            method: HttpMethod::Get,
            path: "/health".into(),
            response: MockResponse {
                status: 200,
                body: "ok".into(),
                ..MockResponse::default()
            },
            ..MockRule::default()
        }],
        ..MockProfile::default()
    };
    database
        .save_scenario(project.id, &scenario)
        .expect("scenario should save");
    database
        .save_mock_profile(project.id, &profile)
        .expect("profile should save");

    assert_eq!(
        database
            .list_scenarios(project.id)
            .expect("scenarios should load"),
        vec![scenario]
    );
    assert_eq!(
        database
            .list_mock_profiles(project.id)
            .expect("profiles should load"),
        vec![profile]
    );
}

#[test]
fn run_history_retention_prunes_oldest_records() {
    let database = Database::open_in_memory().expect("database should open");
    let project = Project::new("History");
    database
        .save_project(&project)
        .expect("project should save");
    let now = Utc::now();
    for index in 0..4 {
        database
            .save_run_record(
                project.id,
                &RunRecord {
                    id: EntityId::new(),
                    request_case_id: None,
                    state: RunState::Passed,
                    status_code: Some(200),
                    elapsed_ms: Duration::from_millis(index).as_millis() as u64,
                    response_bytes: 2,
                    body_path: Some(format!("/tmp/apitest-history-body-{index}")),
                    error: None,
                    started_at: now + TimeDelta::seconds(index as i64),
                    finished_at: Some(now + TimeDelta::seconds(index as i64)),
                },
            )
            .expect("record should save");
    }

    let (deleted, mut body_paths) = database
        .prune_run_records_with_body_paths(project.id, 2, 30, now + TimeDelta::days(1))
        .expect("history should prune");
    let records = database
        .list_run_records(project.id, PageRequest::new(0, 10))
        .expect("history should load");

    assert_eq!(deleted, 2);
    body_paths.sort();
    assert_eq!(
        body_paths,
        [
            std::path::PathBuf::from("/tmp/apitest-history-body-0"),
            std::path::PathBuf::from("/tmp/apitest-history-body-1"),
        ]
    );
    assert_eq!(records.total, 2);
    assert_eq!(records.items[0].elapsed_ms, 3);
    assert_eq!(records.items[1].elapsed_ms, 2);
}
