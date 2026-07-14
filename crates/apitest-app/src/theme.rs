use std::{path::Path, sync::Arc};

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub canvas: Color32,
    pub panel: Color32,
    pub raised: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub success: Color32,
    pub warning: Color32,
    pub danger: Color32,
}

impl Palette {
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self {
                canvas: Color32::from_rgb(16, 19, 26),
                panel: Color32::from_rgb(22, 26, 35),
                raised: Color32::from_rgb(29, 34, 45),
                border: Color32::from_rgb(45, 52, 66),
                text: Color32::from_rgb(232, 235, 242),
                muted: Color32::from_rgb(139, 148, 166),
                accent: Color32::from_rgb(99, 102, 241),
                accent_hover: Color32::from_rgb(119, 122, 255),
                success: Color32::from_rgb(52, 211, 153),
                warning: Color32::from_rgb(251, 191, 36),
                danger: Color32::from_rgb(248, 113, 113),
            },
            ThemeMode::Light => Self {
                canvas: Color32::from_rgb(246, 247, 251),
                panel: Color32::WHITE,
                raised: Color32::from_rgb(241, 243, 248),
                border: Color32::from_rgb(222, 226, 235),
                text: Color32::from_rgb(31, 36, 48),
                muted: Color32::from_rgb(105, 115, 135),
                accent: Color32::from_rgb(79, 70, 229),
                accent_hover: Color32::from_rgb(67, 56, 202),
                success: Color32::from_rgb(5, 150, 105),
                warning: Color32::from_rgb(217, 119, 6),
                danger: Color32::from_rgb(220, 38, 38),
            },
        }
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
    visuals.extreme_bg_color = palette.canvas;
    visuals.text_edit_bg_color = Some(palette.canvas);
    visuals.faint_bg_color = palette.raised;
    visuals.code_bg_color = palette.canvas;
    visuals.selection.bg_fill = palette.accent;
    visuals.hyperlink_color = palette.accent;
    visuals.override_text_color = Some(palette.text);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border);
    visuals.widgets.inactive.bg_fill = palette.raised;
    visuals.widgets.inactive.weak_bg_fill = palette.raised;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border);
    visuals.widgets.hovered.bg_fill = palette.raised;
    visuals.widgets.hovered.weak_bg_fill = palette.raised;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, palette.accent);
    visuals.widgets.active.bg_fill = palette.accent;
    visuals.widgets.active.weak_bg_fill = palette.accent;
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(6);
    }
    visuals.window_corner_radius = CornerRadius::same(10);
    style.visuals = visuals;
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 7.0);
    style.spacing.interact_size.y = 34.0;
    ctx.set_style_of(egui_theme, style);
}

pub fn install_cjk_font(ctx: &egui::Context) {
    let candidates = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "/System/Library/Fonts/PingFang.ttc",
    ];
    let Some(bytes) = candidates
        .iter()
        .find_map(|path| std::fs::read(Path::new(path)).ok())
    else {
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("apitest-cjk".into(), Arc::new(FontData::from_owned(bytes)));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        if let Some(fonts) = fonts.families.get_mut(&family) {
            fonts.push("apitest-cjk".into());
        }
    }
    ctx.set_fonts(fonts);
}
