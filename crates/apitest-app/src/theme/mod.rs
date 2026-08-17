mod fonts;
pub(crate) mod tokens;

use egui::{
    Color32, CornerRadius, FontFamily, FontId, RichText, Stroke, TextFormat, TextStyle, Vec2,
    WidgetText, text::LayoutJob,
};
use iconflow::{Pack, Size, Style, try_icon};
use serde::{Deserialize, Serialize};

pub use fonts::install_fonts;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub app_bg: Color32,
    pub rail_bg: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub control: Color32,
    pub hover: Color32,
    pub divider: Color32,
    pub control_border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub primary: Color32,
    pub primary_soft: Color32,
    pub accent_text: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
    pub info: Color32,
}

impl Palette {
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self {
                app_bg: rgb(0x0f1115),
                rail_bg: rgb(0x11141a),
                panel: rgb(0x171a21),
                surface: rgb(0x1b1f27),
                control: rgb(0x20252e),
                hover: rgb(0x282e38),
                divider: rgb(0x313844),
                control_border: rgb(0x667080),
                text: rgb(0xf3f4f6),
                muted: rgb(0xa6afbd),
                primary: rgb(0x5865f2),
                primary_soft: rgb(0x262b52),
                accent_text: rgb(0x9ba3ff),
                success: rgb(0x32c48d),
                warning: rgb(0xf5b642),
                danger: rgb(0xff7373),
                info: rgb(0x67b3ff),
            },
            ThemeMode::Light => Self {
                app_bg: rgb(0xf4f6f8),
                rail_bg: rgb(0xf7f8fa),
                panel: Color32::WHITE,
                surface: rgb(0xfafbfc),
                control: Color32::WHITE,
                hover: rgb(0xeff2f5),
                divider: rgb(0xdce1e7),
                control_border: rgb(0x8792a2),
                text: rgb(0x20242c),
                muted: rgb(0x5f6977),
                primary: rgb(0x5865f2),
                primary_soft: rgb(0xeef0ff),
                accent_text: rgb(0x4f5bd5),
                success: rgb(0x0b7d57),
                warning: rgb(0x8a5a00),
                danger: rgb(0xc73535),
                info: rgb(0x2563a8),
            },
        }
    }
}

/// The palette that matches the visuals currently installed on the context.
///
/// Deriving it from `Visuals::dark_mode` keeps drawing code from having to
/// thread a `Palette` argument through every nested widget function, and cannot
/// drift out of sync with what `apply` installed.
pub(crate) fn palette_of(visuals: &egui::Visuals) -> Palette {
    Palette::for_mode(if visuals.dark_mode {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    })
}

pub(crate) fn palette(ctx: &egui::Context) -> Palette {
    Palette::for_mode(match ctx.theme() {
        egui::Theme::Light => ThemeMode::Light,
        egui::Theme::Dark => ThemeMode::Dark,
    })
}

pub(crate) trait UiExt {
    fn palette(&self) -> Palette;
}

impl UiExt for egui::Ui {
    fn palette(&self) -> Palette {
        palette_of(self.visuals())
    }
}

pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    let palette = Palette::for_mode(mode);
    let egui_theme = match mode {
        ThemeMode::Dark => egui::Theme::Dark,
        ThemeMode::Light => egui::Theme::Light,
    };
    ctx.set_theme(egui_theme);
    let mut style = (*ctx.style_of(egui_theme)).clone();
    let mut visuals = match mode {
        ThemeMode::Dark => egui::Visuals::dark(),
        ThemeMode::Light => egui::Visuals::light(),
    };
    visuals.panel_fill = palette.panel;
    visuals.window_fill = palette.panel;
    visuals.extreme_bg_color = palette.app_bg;
    visuals.text_edit_bg_color = Some(palette.control);
    visuals.faint_bg_color = palette.hover;
    visuals.code_bg_color = palette.surface;
    visuals.selection.bg_fill = palette.primary;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.hyperlink_color = palette.accent_text;
    visuals.override_text_color = Some(palette.text);
    visuals.widgets.noninteractive.bg_fill = palette.panel;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.divider);
    visuals.widgets.inactive.bg_fill = palette.control;
    visuals.widgets.inactive.weak_bg_fill = palette.control;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.control_border);
    visuals.widgets.hovered.bg_fill = palette.hover;
    visuals.widgets.hovered.weak_bg_fill = palette.hover;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.primary);
    visuals.widgets.active.bg_fill = palette.primary_soft;
    visuals.widgets.active.weak_bg_fill = palette.primary_soft;
    // A 2 pt ring: `active` is also the keyboard-focus state, and the 1 pt
    // border it used to draw was hard to spot against the control fill.
    visuals.widgets.active.bg_stroke = Stroke::new(2.0, palette.primary);
    visuals.widgets.open = visuals.widgets.hovered;
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(5);
    }
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.menu_corner_radius = CornerRadius::same(6);
    style.visuals = visuals;
    style.spacing.item_spacing = Vec2::new(8.0, 6.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.spacing.interact_size.y = 32.0;
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(11.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(13.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(16.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(13.0, FontFamily::Monospace),
    );
    ctx.set_style_of(egui_theme, style);
}

pub fn icon(name: &str, size: f32) -> RichText {
    let Some(icon) = try_icon(Pack::Lucide, name, Style::Regular, Size::Regular).ok() else {
        return RichText::new("?").size(size);
    };
    let glyph = char::from_u32(icon.codepoint).unwrap_or('?');
    RichText::new(glyph.to_string()).font(FontId::new(size, FontFamily::Name(icon.family.into())))
}

/// Icon stacked above a short label, centred — the activity-rail entry.
///
/// A horizontal icon + label needed more width than the rail has, which wrapped
/// `MOCK` onto a second line.
pub fn stacked_icon_label(name: &str, label: &str, color: Color32) -> WidgetText {
    let Some(icon) = try_icon(Pack::Lucide, name, Style::Regular, Size::Regular).ok() else {
        return WidgetText::from(label.to_owned());
    };
    let glyph = char::from_u32(icon.codepoint).unwrap_or('?');
    let mut job = LayoutJob {
        halign: egui::Align::Center,
        ..LayoutJob::default()
    };
    job.append(
        &format!("{glyph}\n"),
        0.0,
        TextFormat {
            font_id: FontId::new(14.0, FontFamily::Name(icon.family.into())),
            color,
            ..Default::default()
        },
    );
    job.append(
        label,
        0.0,
        TextFormat {
            font_id: FontId::new(10.0, FontFamily::Proportional),
            color,
            ..Default::default()
        },
    );
    WidgetText::from(job)
}

pub fn icon_label(name: &str, label: &str, size: f32, color: Color32) -> WidgetText {
    let Some(icon) = try_icon(Pack::Lucide, name, Style::Regular, Size::Regular).ok() else {
        return WidgetText::from(label.to_owned());
    };
    let glyph = char::from_u32(icon.codepoint).unwrap_or('?');
    let mut job = LayoutJob::default();
    job.append(
        &glyph.to_string(),
        0.0,
        TextFormat {
            font_id: FontId::new(size, FontFamily::Name(icon.family.into())),
            color,
            ..Default::default()
        },
    );
    job.append(
        &format!("  {label}"),
        0.0,
        TextFormat {
            font_id: FontId::new(size, FontFamily::Proportional),
            color,
            ..Default::default()
        },
    );
    WidgetText::from(job)
}

const fn rgb(value: u32) -> Color32 {
    Color32::from_rgb(
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::{Palette, ThemeMode};
    use egui::Color32;

    #[test]
    fn text_and_controls_meet_contrast_targets() {
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let palette = Palette::for_mode(mode);
            assert!(contrast(palette.text, palette.surface) >= 4.5);
            assert!(contrast(palette.muted, palette.surface) >= 4.5);
            assert!(contrast(palette.control_border, palette.panel) >= 3.0);
            assert!(contrast(Color32::WHITE, palette.primary) >= 4.5);
        }
    }

    fn contrast(left: Color32, right: Color32) -> f32 {
        let left = luminance(left);
        let right = luminance(right);
        (left.max(right) + 0.05) / (left.min(right) + 0.05)
    }

    fn luminance(color: Color32) -> f32 {
        let channel = |value: u8| {
            let value = f32::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }
}
