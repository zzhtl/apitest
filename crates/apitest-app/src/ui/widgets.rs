use eframe::egui::{self, Color32, RichText, Stroke, WidgetText};

use crate::theme::tokens::{icon as icon_size, radius, size};
use crate::theme::{self, Palette, UiExt};

/// Semantic colour role shared by badges and status chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Success,
    Warning,
    Danger,
    Info,
}

impl Tone {
    pub(crate) fn foreground(self, palette: Palette) -> Color32 {
        match self {
            Self::Success => palette.success,
            Self::Warning => palette.warning,
            Self::Danger => palette.danger,
            Self::Info => palette.info,
        }
    }
}

/// A small pill carrying a short status word or count.
pub(crate) fn badge(ui: &mut egui::Ui, text: impl Into<String>, tone: Tone) {
    let palette = ui.palette();
    let color = tone.foreground(palette);
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(radius::SM)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text.into()).small().strong().color(color));
        });
}

/// The unsaved-changes marker, so every workspace draws the same one.
pub(crate) fn dirty_marker(ui: &mut egui::Ui) {
    let palette = ui.palette();
    ui.label(RichText::new("●").color(palette.warning).size(8.0));
}

/// A vertical icon-over-label button for the activity rail.
///
/// The rail used a horizontal icon + label inside 48 points, which wrapped
/// `MOCK` onto two lines. Stacking them keeps every label on one line and gives
/// the selected entry a left indicator bar.
pub(crate) fn rail_button(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    tooltip: &str,
    selected: bool,
) -> egui::Response {
    let palette = ui.palette();
    let color = if selected {
        palette.accent_text
    } else {
        palette.muted
    };
    let response = ui
        .add_sized(
            size::RAIL_BUTTON,
            egui::Button::new(theme::stacked_icon_label(icon, label, color))
                .selected(selected)
                .fill(if selected {
                    palette.primary_soft
                } else {
                    Color32::TRANSPARENT
                })
                .stroke(Stroke::NONE)
                .corner_radius(radius::MD)
                .wrap_mode(egui::TextWrapMode::Extend),
        )
        .on_hover_text(tooltip);
    if selected {
        let rect = response.rect;
        let bar = egui::Rect::from_min_size(
            rect.left_top() + egui::vec2(-5.0, 8.0),
            egui::vec2(3.0, rect.height() - 16.0),
        );
        ui.painter().rect_filled(bar, radius::SM, palette.primary);
    }
    response
}

/// A full-width selectable row, used by every sidebar list so they match.
pub(crate) fn sidebar_row(
    ui: &mut egui::Ui,
    selected: bool,
    text: impl Into<WidgetText>,
) -> egui::Response {
    let palette = ui.palette();
    ui.add_sized(
        [ui.available_width(), size::ROW],
        egui::Button::new(text)
            .selected(selected)
            .fill(if selected {
                palette.primary_soft
            } else {
                Color32::TRANSPARENT
            })
            .stroke(Stroke::NONE)
            .corner_radius(radius::SM),
    )
}

/// Sidebar title with right-aligned actions, so all five sidebars line up.
pub(crate) fn sidebar_header(ui: &mut egui::Ui, title: &str, actions: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).strong().size(14.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), actions);
    });
}

/// The square icon button used for sidebar and toolbar actions.
pub(crate) fn icon_button(ui: &mut egui::Ui, icon: &str, tooltip: &str) -> egui::Response {
    ui.add_sized(
        [size::ICON_BUTTON, size::ICON_BUTTON],
        egui::Button::new(theme::icon(icon, icon_size::MD)),
    )
    .on_hover_text(tooltip)
}

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
    let underline = ui.ctx().animate_bool(response.id, selected);
    if underline > 0.0 {
        let width = response.rect.width() * underline;
        let left = response.rect.left_bottom();
        ui.painter().line_segment(
            [left, left + egui::vec2(width, 0.0)],
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

/// Centred placeholder for an empty panel.
pub(crate) fn empty_state(ui: &mut egui::Ui, title: &str, description: &str) {
    empty_state_inner(ui, title, description, None);
}

/// Centred placeholder with a primary action. Returns whether it was clicked.
///
/// Callers used to draw `empty_state` and then a bare `ui.button(..)`, which
/// left the button left-aligned under a centred block.
pub(crate) fn empty_state_action(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    action: &str,
) -> bool {
    empty_state_inner(ui, title, description, Some(action))
}

fn empty_state_inner(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    action: Option<&str>,
) -> bool {
    let palette = ui.palette();
    let mut clicked = false;
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.label(RichText::new(title).strong().size(14.0));
        if !description.is_empty() {
            ui.label(RichText::new(description).color(palette.muted));
        }
        if let Some(action) = action {
            ui.add_space(12.0);
            clicked = ui
                .add(
                    egui::Button::new(theme::icon_label(
                        "plus",
                        action,
                        icon_size::SM,
                        Color32::WHITE,
                    ))
                    .fill(palette.primary)
                    .stroke(Stroke::NONE)
                    .corner_radius(radius::SM)
                    .min_size(egui::vec2(0.0, size::FIELD)),
                )
                .clicked();
        }
    });
    clicked
}
