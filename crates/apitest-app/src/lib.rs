mod app;
mod draft;
mod environment;
mod i18n;
mod persistence;
mod services;
mod state;
mod theme;
mod ui;
mod workbench;

#[cfg(test)]
mod tests;

pub use app::ApiTestApp;
pub use draft::{
    AuthDraft, AuthMode, BodyMode, EditableMultipartPart, EditablePair, MultipartValueMode,
    ProxyDraft, RequestDraft, SecretDraft,
};
pub use workbench::{AutoSaveState, DocumentId, DocumentKind, DocumentTab, DocumentTabs};

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
