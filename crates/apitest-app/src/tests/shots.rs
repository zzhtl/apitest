//! Renders the workbench off-screen so layout can be reviewed without a display
//! server. Ignored by default; run with
//! `cargo test -p apitest-app --lib tests::shots -- --ignored --nocapture`.
use std::path::PathBuf;

use egui_kittest::Harness;

use crate::app::ApiTestApp;
use crate::state::workspace::Navigation;
use crate::theme::ThemeMode;

fn out_dir() -> PathBuf {
    let dir = PathBuf::from(
        std::env::var("APITEST_SHOT_DIR").unwrap_or_else(|_| "/tmp/apitest-shots".to_owned()),
    );
    std::fs::create_dir_all(&dir).expect("shot directory");
    dir
}

fn capture(name: &str, theme: ThemeMode, prepare: impl FnOnce(&mut ApiTestApp)) {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .wgpu()
        .build_eframe(|context| ApiTestApp::new(context));
    harness.state_mut().theme = theme;
    crate::theme::apply(&harness.ctx, theme);
    prepare(harness.state_mut());
    harness.run_steps(4);
    println!(
        "{name}: ctx_theme={:?} app_theme={:?}",
        harness.ctx.theme(),
        harness.state().theme
    );
    let image = harness.render().expect("render");
    let path = out_dir().join(format!("{name}.png"));
    image.save(&path).expect("save");
    println!("wrote {}", path.display());
}

#[test]
#[ignore = "visual review tool"]
fn render_every_workspace() {
    capture("api-dark", ThemeMode::Dark, |_| {});
    capture("api-light", ThemeMode::Light, |_| {});
    capture("scenario", ThemeMode::Dark, |app| {
        app.perform_action(crate::state::action::PendingAction::NewScenario);
    });
    capture("mock", ThemeMode::Dark, |app| {
        app.perform_action(crate::state::action::PendingAction::NewMock);
    });
    capture("history", ThemeMode::Dark, |app| {
        app.navigation = Navigation::History;
    });
    capture("environment", ThemeMode::Dark, |app| {
        app.navigation = Navigation::Environment;
    });
    capture("palette", ThemeMode::Dark, |app| {
        app.show_palette = true;
    });
    capture("snippet", ThemeMode::Dark, |app| {
        app.requests[0].draft.url = "https://api.example.com/v1/users".into();
        app.show_snippet = true;
    });
    capture("body-editor", ThemeMode::Dark, |app| {
        app.session_mut().editor_tab = crate::state::workspace::EditorTab::Body;
        app.requests[0].draft.url = "{{base_url}}/users/{{user_id}}".into();
        app.requests[0].draft.body_mode = crate::draft::BodyMode::Json;
        app.requests[0].draft.body = "{\n  \"name\": \"{{base_url}}\",\n  \"missing\": \"{{nope}}\",\n  \"count\": 42,\n  \"ok\": true,\n  \"tags\": [\"a\", \"b\"]\n}".into();
        app.environments[0].set_plain_value("base_url", "https://api.example.com");
    });
    capture("response-body", ThemeMode::Dark, |app| {
        let session = app.session_mut();
        session.response.status = Some(200);
        session.response.body = "{\"data\":{\"id\":42,\"name\":\"order\"},\"ok\":true}".into();
        session.response.finish_body();
        session.body_search = "order".into();
    });
    capture("response-tree", ThemeMode::Dark, |app| {
        let session = app.session_mut();
        session.response.status = Some(200);
        session.response.body = "{\"data\":{\"id\":42,\"name\":\"order\",\"tags\":[\"a\",\"b\"]},\"ok\":true,\"total\":3}".into();
        session.response.finish_body();
        session.response_body_mode = crate::state::response::ResponseBodyMode::Tree;
    });
    capture("docs-tab", ThemeMode::Dark, |app| {
        app.session_mut().editor_tab = crate::state::workspace::EditorTab::Docs;
        app.requests[0].definition.status = apitest_core::ApiStatus::Testing;
        app.requests[0].definition.tags = vec!["orders".into(), "public".into()];
        app.requests[0].definition.description_markdown =
            "## 查询订单\n\n返回当前用户的订单列表。".into();
    });
    capture("request-tests", ThemeMode::Dark, |app| {
        app.session_mut().editor_tab = crate::state::workspace::EditorTab::Tests;
        app.requests[0].request_case.assertions = vec![
            apitest_core::AssertionRule::Status { expected: 200 },
            apitest_core::AssertionRule::JsonPathEquals {
                path: "$.data.id".into(),
                expected: "42".into(),
            },
        ];
        app.requests[0].request_case.extractors = vec![apitest_core::ExtractorRule {
            name: "token".into(),
            source: apitest_core::VariableSource::JsonPath("$.token".into()),
        }];
    });
}
