use eframe::egui::{self, RichText, Stroke};

use crate::theme::UiExt;

pub(crate) fn tab_button(ui: &mut egui::Ui, selected: bool, label: &str) -> egui::Response {
    let palette = ui.palette();
    let response = ui.add(
        egui::Button::new(RichText::new(label).color(if selected {
            palette.accent_text
        } else {
            palette.muted
        }))
        .frame(false),
    );
    if selected {
        ui.painter().line_segment(
            [response.rect.left_bottom(), response.rect.right_bottom()],
            Stroke::new(2.0, palette.primary),
        );
    }
    response
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.2} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

pub(crate) fn empty_state(ui: &mut egui::Ui, title: &str, description: &str) {
    let palette = ui.palette();
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.label(RichText::new(title).strong().size(14.0));
        if !description.is_empty() {
            ui.label(RichText::new(description).color(palette.muted));
        }
    });
}
