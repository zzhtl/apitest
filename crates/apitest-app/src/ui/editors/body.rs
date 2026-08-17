use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

use crate::draft::{BodyMode, EditableMultipartPart, MultipartValueMode, RequestDraft};
use crate::i18n::{Language, tr};
use crate::theme::tokens::icon as icon_size;
use crate::theme::{self, UiExt};
use crate::ui::editors::editable_pairs;
use crate::ui::widgets::empty_state;

pub(crate) fn body_editor(
    ui: &mut egui::Ui,
    draft: &mut RequestDraft,
    language: Language,
) -> Option<String> {
    let palette = ui.palette();
    let mut error = None;
    ui.horizontal_wrapped(|ui| {
        for (mode, label) in [
            (BodyMode::None, "none"),
            (BodyMode::Json, "JSON"),
            (BodyMode::Text, "Text"),
            (BodyMode::Xml, "XML"),
            (BodyMode::FormUrlEncoded, "x-www-form-urlencoded"),
            (BodyMode::Multipart, "multipart"),
            (BodyMode::Binary, "binary"),
        ] {
            ui.selectable_value(&mut draft.body_mode, mode, label);
        }
        if draft.body_mode == BodyMode::Json
            && ui
                .button(theme::icon_label(
                    "braces",
                    tr(language, "格式化", "Format"),
                    12.0,
                    palette.text,
                ))
                .clicked()
        {
            match serde_json::from_str::<serde_json::Value>(&draft.body)
                .and_then(|value| serde_json::to_string_pretty(&value))
            {
                Ok(pretty) => draft.body = pretty,
                Err(value) => {
                    error = Some(match language {
                        Language::Chinese => format!("JSON 格式错误：{value}"),
                        Language::English => format!("Invalid JSON: {value}"),
                    })
                }
            }
        }
    });
    ui.add_space(4.0);
    match draft.body_mode {
        BodyMode::None => empty_state(ui, tr(language, "无请求体", "No body"), ""),
        BodyMode::Json | BodyMode::Text | BodyMode::Xml => {
            egui::ScrollArea::both().show(ui, |ui| {
                ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::multiline(&mut draft.body)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
        }
        BodyMode::FormUrlEncoded => editable_pairs(ui, &mut draft.form_fields, language, false),
        BodyMode::Multipart => multipart_table(ui, &mut draft.multipart_parts, language),
        BodyMode::Binary => {
            ui.horizontal(|ui| {
                let path = draft
                    .binary_file
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| match language {
                        Language::Chinese => "未选择文件".into(),
                        Language::English => "No file selected".into(),
                    });
                ui.add_sized(
                    [ui.available_width() - 106.0, 32.0],
                    egui::Label::new(RichText::new(path).monospace().color(palette.muted))
                        .truncate(),
                );
                if ui
                    .button(theme::icon_label(
                        "folder-open",
                        tr(language, "选择", "Choose"),
                        12.0,
                        palette.text,
                    ))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    draft.binary_file = Some(path);
                }
            });
        }
    }
    error
}

pub(crate) fn multipart_table(
    ui: &mut egui::Ui,
    parts: &mut Vec<EditableMultipartPart>,
    language: Language,
) {
    let palette = ui.palette();
    let labels = match language {
        Language::Chinese => ("名称", "类型", "值 / 文件", "Content-Type", "选择"),
        Language::English => ("Key", "Type", "Value / File", "Content-Type", "Choose"),
    };
    let mut remove = None;
    TableBuilder::new(ui)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(28.0))
        .column(Column::initial(150.0).at_least(120.0).resizable(true))
        .column(Column::exact(82.0))
        .column(Column::remainder().at_least(170.0))
        .column(Column::initial(130.0).at_least(110.0).resizable(true))
        .column(Column::exact(32.0))
        .header(28.0, |mut header| {
            header.col(|_| {});
            for label in [labels.0, labels.1, labels.2, labels.3] {
                header.col(|ui| {
                    ui.label(RichText::new(label).color(palette.muted));
                });
            }
            header.col(|_| {});
        })
        .body(|mut body| {
            for (index, part) in parts.iter_mut().enumerate() {
                body.row(36.0, |mut row| {
                    row.col(|ui| {
                        ui.checkbox(&mut part.enabled, "");
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut part.name).desired_width(f32::INFINITY),
                        );
                    });
                    row.col(|ui| {
                        egui::ComboBox::from_id_salt(("multipart_mode", index))
                            .selected_text(match part.mode {
                                MultipartValueMode::Text => "Text",
                                MultipartValueMode::File => "File",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut part.mode,
                                    MultipartValueMode::Text,
                                    "Text",
                                );
                                ui.selectable_value(
                                    &mut part.mode,
                                    MultipartValueMode::File,
                                    "File",
                                );
                            });
                    });
                    row.col(|ui| match part.mode {
                        MultipartValueMode::Text => {
                            ui.add(
                                egui::TextEdit::singleline(&mut part.value)
                                    .desired_width(f32::INFINITY),
                            );
                        }
                        MultipartValueMode::File => {
                            let text = part
                                .file
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| labels.4.into());
                            if ui.button(text).clicked()
                                && let Some(path) = rfd::FileDialog::new().pick_file()
                            {
                                part.file = Some(path);
                            }
                        }
                    });
                    row.col(|ui| {
                        ui.add_enabled(
                            part.mode == MultipartValueMode::File,
                            egui::TextEdit::singleline(&mut part.content_type)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    row.col(|ui| {
                        if !part.is_empty()
                            && ui
                                .add_sized(
                                    [28.0, 28.0],
                                    egui::Button::new(theme::icon("x", icon_size::SM)).frame(false),
                                )
                                .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                });
            }
        });
    if let Some(index) = remove {
        parts.remove(index);
    }
}
