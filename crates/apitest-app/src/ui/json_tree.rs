use eframe::egui::{self, RichText};
use serde_json::Value;

use crate::theme::{Palette, UiExt};

/// Nodes above this depth start collapsed, so a deep payload does not fill the
/// panel the moment it arrives.
const AUTO_EXPAND_DEPTH: usize = 2;

/// A collapsible view of a JSON document.
///
/// Complements the pretty text view: text is better for copying and searching,
/// a tree is better for finding one field in a large nested payload.
pub(crate) fn json_tree(ui: &mut egui::Ui, value: &Value, filter: &str) {
    let palette = ui.palette();
    let filter = filter.trim().to_lowercase();
    egui::ScrollArea::both().show(ui, |ui| {
        node(ui, palette, "", value, 0, &filter);
    });
}

fn node(ui: &mut egui::Ui, palette: Palette, key: &str, value: &Value, depth: usize, filter: &str) {
    match value {
        Value::Object(map) => {
            let summary = format!("{{{}}}", map.len());
            container(ui, palette, key, &summary, depth, filter, map.len(), |ui| {
                for (name, child) in map {
                    node(ui, palette, name, child, depth + 1, filter);
                }
            });
        }
        Value::Array(items) => {
            let summary = format!("[{}]", items.len());
            container(
                ui,
                palette,
                key,
                &summary,
                depth,
                filter,
                items.len(),
                |ui| {
                    for (index, child) in items.iter().enumerate() {
                        node(ui, palette, &index.to_string(), child, depth + 1, filter);
                    }
                },
            );
        }
        leaf => {
            let (text, color) = leaf_text(leaf, palette);
            if !matches(filter, key, &text) {
                return;
            }
            ui.horizontal(|ui| {
                ui.add_space(depth as f32 * 14.0);
                if !key.is_empty() {
                    ui.label(RichText::new(key).monospace().color(palette.accent_text));
                    ui.label(RichText::new(":").monospace().color(palette.muted));
                }
                ui.label(RichText::new(text).monospace().color(color));
            });
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "one recursive drawing helper")]
fn container(
    ui: &mut egui::Ui,
    palette: Palette,
    key: &str,
    summary: &str,
    depth: usize,
    filter: &str,
    len: usize,
    children: impl FnOnce(&mut egui::Ui),
) {
    let label = if key.is_empty() {
        summary.to_owned()
    } else {
        format!("{key}  {summary}")
    };
    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 14.0);
        egui::CollapsingHeader::new(RichText::new(label).monospace().color(palette.text))
            .id_salt((depth, key.to_owned(), len))
            // A filter is only useful if the matches are already visible.
            .default_open(depth < AUTO_EXPAND_DEPTH || !filter.is_empty())
            .show(ui, children);
    });
}

fn leaf_text(value: &Value, palette: Palette) -> (String, egui::Color32) {
    match value {
        Value::Null => ("null".to_owned(), palette.muted),
        Value::Bool(flag) => (flag.to_string(), palette.warning),
        Value::Number(number) => (number.to_string(), palette.info),
        Value::String(text) => (format!("\"{text}\""), palette.success),
        _ => (value.to_string(), palette.text),
    }
}

fn matches(filter: &str, key: &str, text: &str) -> bool {
    filter.is_empty() || key.to_lowercase().contains(filter) || text.to_lowercase().contains(filter)
}
