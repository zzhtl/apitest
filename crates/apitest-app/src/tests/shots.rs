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
}
