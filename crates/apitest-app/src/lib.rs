mod app;
mod draft;
mod theme;

pub use app::ApiTestApp;
pub use draft::{BodyMode, EditablePair, RequestDraft};

use eframe::egui;

pub fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ApiTest")
            .with_app_id("io.github.zzhtl.apitest")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([980.0, 640.0]),
        centered: true,
        ..Default::default()
    }
}
