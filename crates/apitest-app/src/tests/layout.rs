use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};

use super::support::test_app;
use crate::i18n::Language;
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
    harness.state_mut().editor_tab = EditorTab::Auth;

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
