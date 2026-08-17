use apitest_core::{EntityId, ProjectNodeKind};
use egui_kittest::Harness;

use super::support::test_app;
use crate::app::ACTIVE_PROJECT_SETTING;
use crate::services::loader::document_tabs_setting;
use crate::services::tree::TreeAction;
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

/// Folders can be created, renamed, moved and deleted from the tree — none of
/// which the sidebar could do before.
#[test]
fn resource_tree_supports_folders_and_moves() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);

    harness
        .state_mut()
        .apply_tree_action(TreeAction::NewFolder { parent: None });
    let (folder, _) = harness
        .state()
        .rename_target
        .clone()
        .expect("a new folder is offered for renaming");
    harness.state_mut().rename_target = None;
    harness.state_mut().rename_resource(folder, "订单");

    harness
        .state_mut()
        .apply_tree_action(TreeAction::NewFolder {
            parent: Some(folder),
        });
    let (nested, _) = harness
        .state()
        .rename_target
        .clone()
        .expect("a nested folder is offered for renaming");
    harness.state_mut().rename_target = None;

    let root = harness.state().resource_pages[&None]
        .items
        .iter()
        .filter(|node| node.kind == ProjectNodeKind::Folder)
        .count();
    assert_eq!(root, 1, "only the parent folder sits at the root");

    // Moving the nested folder back to the root re-parents it.
    harness.state_mut().move_resource(nested, None);
    harness.state_mut().reload_resource_page(None);
    let root = harness.state().resource_pages[&None]
        .items
        .iter()
        .filter(|node| node.kind == ProjectNodeKind::Folder)
        .count();
    assert_eq!(root, 2, "the nested folder moved out");

    // A folder may not become its own descendant.
    harness.state_mut().move_resource(nested, Some(nested));
    harness.state_mut().reload_resource_page(None);
    assert_eq!(
        harness.state().resource_pages[&None]
            .items
            .iter()
            .filter(|node| node.kind == ProjectNodeKind::Folder)
            .count(),
        2,
    );

    harness.state_mut().delete_folder(folder);
    harness.state_mut().reload_resource_page(None);
    assert_eq!(
        harness.state().resource_pages[&None]
            .items
            .iter()
            .filter(|node| node.kind == ProjectNodeKind::Folder)
            .count(),
        1,
        "deleting a folder removes it from the tree",
    );
}

/// Scenarios and mock profiles could be created but never removed.
#[test]
fn scenarios_and_mocks_can_be_deleted_from_the_sidebar() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    harness
        .state_mut()
        .perform_action(PendingAction::NewScenario);
    harness.state_mut().save_current_scenario();
    harness.state_mut().perform_action(PendingAction::NewMock);
    harness.state_mut().save_current_mock();
    let scenario = harness.state().scenarios[0].id;
    let mock = harness.state().mock_profiles[0].id;

    harness.state_mut().delete_scenario(scenario);
    harness.state_mut().delete_mock(mock);

    assert!(harness.state().scenarios.is_empty());
    assert!(harness.state().mock_profiles.is_empty());
    let database = harness.state().database.clone().expect("database");
    let project = harness.state().project.id;
    assert!(
        database
            .list_scenarios(project)
            .expect("scenarios")
            .is_empty()
    );
    assert!(
        database
            .list_mock_profiles(project)
            .expect("profiles")
            .is_empty()
    );
}
