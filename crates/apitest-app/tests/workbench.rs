use std::time::{Duration, Instant};

use apitest_app::{AutoSaveState, DocumentId, DocumentKind, DocumentTabs};
use apitest_core::EntityId;

fn document(kind: DocumentKind) -> DocumentId {
    DocumentId {
        kind,
        entity_id: EntityId::new(),
    }
}

#[test]
fn opening_an_existing_document_activates_without_duplicating_it() {
    let api = document(DocumentKind::Api);
    let scenario = document(DocumentKind::Scenario);
    let mut tabs = DocumentTabs::default();

    tabs.open(api, "List users");
    tabs.open(scenario, "Smoke");
    tabs.open(api, "List users");

    assert_eq!(tabs.items().len(), 2);
    assert_eq!(tabs.active(), Some(api));
    assert!(tabs.rename(api, "Users"));
    assert_eq!(tabs.items()[0].title, "Users");
    assert_eq!(tabs.close(api), Some(scenario));
    assert_eq!(tabs.active(), Some(scenario));
}

#[test]
fn tabs_round_trip_and_drop_documents_that_no_longer_exist() {
    let api = document(DocumentKind::Api);
    let removed = document(DocumentKind::Environment);
    let mut tabs = DocumentTabs::default();
    tabs.open(api, "List users");
    tabs.open(removed, "Removed environment");
    tabs.activate(api);

    let document = serde_json::to_string(&tabs).expect("tabs should serialize");
    let mut restored: DocumentTabs =
        serde_json::from_str(&document).expect("tabs should deserialize");
    restored.retain(|id| id != removed);

    assert_eq!(restored.items().len(), 1);
    assert_eq!(restored.items()[0].id, api);
    assert_eq!(restored.active(), Some(api));
}

#[test]
fn autosave_acknowledgements_never_hide_newer_edits() {
    let start = Instant::now();
    let mut state = AutoSaveState::new(Duration::from_millis(500));
    let first = state.mark_changed(start);

    assert!(!state.is_due(start + Duration::from_millis(499)));
    assert!(state.is_due(start + Duration::from_millis(500)));
    state.mark_queued(first);
    let second = state.mark_changed(start + Duration::from_millis(600));
    state.mark_saved(first);

    assert!(state.is_dirty());
    assert_eq!(state.current_revision(), second);

    state.mark_queued(second);
    state.mark_failed(second, start + Duration::from_millis(700));
    assert!(!state.is_due(start + Duration::from_millis(1_199)));
    assert!(state.is_due(start + Duration::from_millis(1_200)));
    state.mark_queued(second);
    state.mark_saved(second);
    assert!(!state.is_dirty());
}
