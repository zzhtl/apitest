use apitest_core::EntityId;
use egui_kittest::Harness;

use super::support::test_app;
use crate::app::ACTIVE_PROJECT_SETTING;
use crate::services::loader::document_tabs_setting;
use crate::state::action::PendingAction;
use crate::workbench::{DocumentId, DocumentKind, DocumentTabs};

#[test]
fn project_switching_creates_an_isolated_workspace_and_remembers_it() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let original_project = harness.state().project.id;

    harness.state_mut().create_project();

    assert_eq!(harness.state().projects.len(), 2);
    assert_ne!(harness.state().project.id, original_project);
    assert_eq!(harness.state().requests.len(), 1);
    assert_eq!(harness.state().environments.len(), 1);
    let stored: EntityId = harness
        .state()
        .database
        .as_ref()
        .expect("database should exist")
        .get_setting(ACTIVE_PROJECT_SETTING)
        .expect("setting should load")
        .expect("active project should be stored");
    assert_eq!(stored, harness.state().project.id);

    harness.state_mut().switch_project(original_project);
    assert_eq!(harness.state().project.id, original_project);
    assert_eq!(harness.state().requests[0].name, "Health check");
}

#[test]
fn opened_documents_are_persisted_per_project() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let environment_id = harness.state().environments[0].id();

    harness
        .state_mut()
        .perform_action(PendingAction::SelectEnvironment(environment_id));

    let key = document_tabs_setting(harness.state().project.id);
    let stored: DocumentTabs = harness
        .state()
        .database
        .as_ref()
        .expect("database should exist")
        .get_setting(&key)
        .expect("setting should load")
        .expect("tabs should be stored");
    assert_eq!(stored.items().len(), 2);
    assert_eq!(
        stored.active(),
        Some(DocumentId {
            kind: DocumentKind::Environment,
            entity_id: environment_id
        })
    );
}
