pub(crate) mod auth;
pub(crate) mod body;
pub(crate) mod protocol;

use apitest_core::KeyValue;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

use crate::draft::EditablePair;
use crate::i18n::{Language, tr};
use crate::theme::tokens::icon as icon_size;
use crate::theme::{self, UiExt};

pub(crate) fn key_value_editor(ui: &mut egui::Ui, values: &mut Vec<KeyValue>, language: Language) {
    let palette = ui.palette();
    if values.last().is_none_or(|value| {
        !value.name.is_empty() || !value.value.is_empty() || !value.description.is_empty()
    }) {
        values.push(KeyValue {
            name: String::new(),
            value: String::new(),
            enabled: false,
            description: String::new(),
        });
    }
    let mut remove = None;
    TableBuilder::new(ui)
        .column(Column::exact(28.0))
        .column(Column::initial(180.0).resizable(true))
        .column(Column::remainder())
        .column(Column::exact(32.0))
        .header(26.0, |mut header| {
            header.col(|_| {});
            header.col(|ui| {
                ui.label(RichText::new(tr(language, "名称", "Name")).color(palette.muted));
            });
            header.col(|ui| {
                ui.label(RichText::new(tr(language, "值", "Value")).color(palette.muted));
            });
            header.col(|_| {});
        })
        .body(|mut body| {
            for (index, value) in values.iter_mut().enumerate() {
                body.row(34.0, |mut row| {
                    row.col(|ui| {
                        ui.checkbox(&mut value.enabled, "");
                    });
                    row.col(|ui| {
                        if ui.text_edit_singleline(&mut value.name).changed()
                            && !value.name.is_empty()
                        {
                            value.enabled = true;
                        }
                    });
                    row.col(|ui| {
                        if ui.text_edit_singleline(&mut value.value).changed()
                            && !value.value.is_empty()
                        {
                            value.enabled = true;
                        }
                    });
                    row.col(|ui| {
                        if (!value.name.is_empty() || !value.value.is_empty())
                            && ui
                                .add_sized(
                                    [26.0, 26.0],
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
        values.remove(index);
    }
}

pub(crate) fn editable_pairs(
    ui: &mut egui::Ui,
    pairs: &mut Vec<EditablePair>,
    language: Language,
    description: bool,
) {
    let palette = ui.palette();
    let labels = match language {
        Language::Chinese => ("名称", "值", "说明"),
        Language::English => ("Key", "Value", "Description"),
    };
    let mut remove = None;
    let mut table = TableBuilder::new(ui)
        .striped(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(28.0))
        .column(Column::initial(170.0).at_least(140.0).resizable(true))
        .column(Column::initial(230.0).at_least(180.0).resizable(true));
    if description {
        table = table.column(Column::remainder().at_least(120.0));
    }
    table
        .column(Column::exact(32.0))
        .header(28.0, |mut header| {
            header.col(|_| {});
            header.col(|ui| {
                ui.label(RichText::new(labels.0).color(palette.muted));
            });
            header.col(|ui| {
                ui.label(RichText::new(labels.1).color(palette.muted));
            });
            if description {
                header.col(|ui| {
                    ui.label(RichText::new(labels.2).color(palette.muted));
                });
            }
            header.col(|_| {});
        })
        .body(|mut body| {
            for (index, pair) in pairs.iter_mut().enumerate() {
                body.row(36.0, |mut row| {
                    row.col(|ui| {
                        ui.checkbox(&mut pair.enabled, "");
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut pair.name).desired_width(f32::INFINITY),
                        );
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut pair.value)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    if description {
                        row.col(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut pair.description)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    }
                    row.col(|ui| {
                        if !pair.is_empty()
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
        pairs.remove(index);
    }
}
