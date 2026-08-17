use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};

use apitest_core::ProtocolKind;

use super::support::test_app;
use crate::i18n::Language;
use crate::state::action::PendingAction;
use crate::state::workspace::{EditorTab, Navigation};
use crate::theme::ThemeMode;
use crate::{
    draft::{AuthDraft, AuthMode},
    theme,
};

#[test]
fn minimum_window_keeps_request_inputs_visible() {
    let harness = Harness::builder()
        .with_size(egui::vec2(980.0, 640.0))
        .build_eframe(test_app);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(980.0, 640.0));
    let inputs = harness
        .query_all_by_role(Role::TextInput)
        .collect::<Vec<_>>();

    assert!(
        inputs.len() >= 6,
        "expected the search, name, URL and pair inputs"
    );
    assert!(
        inputs
            .iter()
            .all(|input| screen.contains_rect(input.rect()))
    );
    let wide_inputs = inputs
        .iter()
        .filter(|input| input.rect().width() >= 120.0)
        .count();
    assert!(
        wide_inputs >= 5,
        "request fields should not collapse at minimum size"
    );
}

#[test]
fn light_english_layout_keeps_primary_actions_visible() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(980.0, 640.0))
        .build_eframe(test_app);
    harness.state_mut().theme = ThemeMode::Light;
    harness.state_mut().language = Language::English;
    theme::apply(&harness.ctx, ThemeMode::Light);
    harness.step();

    assert!(harness.query_by_label("Params").is_some());
    assert!(
        harness
            .query_all_by_label_contains("Send")
            .any(|node| node.accesskit_node().role() == Role::Button)
    );
    assert!(
        harness
            .query_all_by_role(Role::TextInput)
            .all(|input| input.rect().max.x <= 980.0 && input.rect().max.y <= 640.0)
    );
}

#[test]
fn advanced_authentication_modes_expose_editable_fields() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let request_id = harness.state().requests[0].id();
    harness.state_mut().session_mut().editor_tab = EditorTab::Auth;

    for (mode, field) in [
        (AuthMode::OAuth2, "Token 地址"),
        (AuthMode::Digest, "用户名"),
        (AuthMode::AwsSigV4, "Access Key"),
    ] {
        harness.state_mut().requests[0].draft.auth = AuthDraft::for_mode(mode, request_id);
        harness.step();
        assert!(
            harness.query_by_label_contains(field).is_some(),
            "{mode:?} should expose {field}"
        );
    }
}

#[test]
fn environment_navigation_exposes_real_variable_editor() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);

    assert!(harness.query_by_label_contains("RUN").is_some());
    assert!(harness.query_by_label_contains("MOCK").is_some());
    assert!(harness.query_by_label_contains("HIST").is_some());
    harness.get_by_label_contains("ENV").click();
    harness.run();

    assert!(harness.query_by_label("环境变量").is_some());
    assert_eq!(harness.state().navigation, Navigation::Environment);
}

/// The shortcuts that move between tabs and close them, which previously had no
/// bindings at all.
#[test]
fn tab_shortcuts_cycle_and_close_documents() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    harness
        .state_mut()
        .perform_action(PendingAction::NewRequest(ProtocolKind::Http));
    harness
        .state_mut()
        .perform_action(PendingAction::NewRequest(ProtocolKind::Http));
    harness.run_steps(2);
    assert_eq!(harness.state().document_tabs.items().len(), 3);

    let before = harness
        .state()
        .document_tabs
        .active()
        .expect("an active tab");
    press(&mut harness, egui::Key::Tab, egui::Modifiers::COMMAND);
    let after = harness
        .state()
        .document_tabs
        .active()
        .expect("an active tab");
    assert_ne!(before, after, "Ctrl Tab moves to the next document");

    press(&mut harness, egui::Key::W, egui::Modifiers::COMMAND);
    harness.run_steps(2);
    assert_eq!(
        harness.state().document_tabs.items().len(),
        2,
        "Ctrl W closes the active document",
    );
}

fn press(
    harness: &mut Harness<'_, crate::app::ApiTestApp>,
    key: egui::Key,
    modifiers: egui::Modifiers,
) {
    // `InputState::modifiers` mirrors `RawInput::modifiers`, not the per-event
    // copy, so both have to be set for a chord to register.
    harness.input_mut().modifiers = modifiers;
    harness.input_mut().events.push(egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    });
    harness.step();
    harness.input_mut().modifiers = egui::Modifiers::NONE;
}
