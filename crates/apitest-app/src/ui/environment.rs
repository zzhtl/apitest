use std::sync::Arc;

use apitest_storage::SecretStore;
use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};

use crate::app::ApiTestApp;
use crate::environment::EnvironmentDraft;
use crate::i18n::Language;
use crate::state::action::{Confirmation, ToastKind};
use crate::theme::{self, Palette};

impl ApiTestApp {
    pub(crate) fn environment_workspace(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let Some(environment) = self.environments.get(self.selected_environment) else {
            return;
        };
        let environment_id = environment.id();
        let dirty = environment.is_dirty();
        let save_label = self.tr("保存", "Save");
        let delete_tip = self.tr("删除环境", "Delete environment");
        let mut save = false;
        let mut delete = false;
        let mut variable_error = None;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(
                            &mut self.environments[self.selected_environment].name,
                        )
                        .font(egui::TextStyle::Heading)
                        .desired_width(300.0),
                    );
                    if dirty {
                        ui.label(RichText::new("●").color(palette.warning).size(8.0));
                    }
                    if environment_id == self.active_environment {
                        ui.label(
                            RichText::new(self.tr("当前环境", "Active")).color(palette.success),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [32.0, 30.0],
                                egui::Button::new(theme::icon("trash-2", 14.0)),
                            )
                            .on_hover_text(delete_tip)
                            .clicked()
                        {
                            delete = true;
                        }
                        if ui
                            .add_enabled(
                                dirty,
                                egui::Button::new(theme::icon_label(
                                    "save",
                                    save_label,
                                    12.0,
                                    palette.text,
                                )),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                    });
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(self.tr("环境变量", "Environment variables"))
                        .strong()
                        .size(14.0),
                );
                ui.add_space(4.0);
                variable_error = variable_table(
                    ui,
                    &mut self.environments[self.selected_environment],
                    Arc::clone(&self.secrets),
                    self.language,
                    palette,
                );
            });
        if let Some(error) = variable_error {
            self.toast(ToastKind::Error, error);
        }
        if save {
            self.save_current_environment();
        }
        if delete {
            self.confirmation = Some(Confirmation::DeleteEnvironment(environment_id));
        }
    }
}

pub(crate) fn variable_table(
    ui: &mut egui::Ui,
    environment: &mut EnvironmentDraft,
    secrets: Arc<dyn SecretStore>,
    language: Language,
    palette: Palette,
) -> Option<String> {
    let labels = match language {
        Language::Chinese => ("名称", "值", "类型", "普通", "密钥"),
        Language::English => ("Name", "Value", "Type", "Plain", "Secret"),
    };
    let environment_id = environment.id();
    let mut remove = None;
    let mut error = None;
    TableBuilder::new(ui)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(28.0))
        .column(Column::initial(190.0).at_least(150.0).resizable(true))
        .column(Column::remainder().at_least(220.0))
        .column(Column::exact(104.0))
        .column(Column::exact(32.0))
        .header(28.0, |mut header| {
            header.col(|_| {});
            for label in [labels.0, labels.1, labels.2] {
                header.col(|ui| {
                    ui.label(RichText::new(label).color(palette.muted));
                });
            }
            header.col(|_| {});
        })
        .body(|mut body| {
            for (index, variable) in environment.variables.iter_mut().enumerate() {
                body.row(36.0, |mut row| {
                    row.col(|ui| {
                        ui.checkbox(&mut variable.enabled, "");
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut variable.name)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    row.col(|ui| {
                        let mut edit = egui::TextEdit::singleline(&mut variable.value)
                            .desired_width(f32::INFINITY);
                        if variable.secret {
                            edit = edit.password(true).hint_text(if variable.configured {
                                "••••••••"
                            } else {
                                ""
                            });
                        }
                        ui.add(edit);
                    });
                    row.col(|ui| {
                        let previous = variable.secret;
                        egui::ComboBox::from_id_salt(("variable_type", variable.id.to_string()))
                            .selected_text(if variable.secret { labels.4 } else { labels.3 })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut variable.secret, false, labels.3);
                                ui.selectable_value(&mut variable.secret, true, labels.4);
                            });
                        if variable.secret && !previous {
                            let reference = variable.ensure_reference(environment_id).clone();
                            match secrets.get(&reference) {
                                Ok(Some(_)) => variable.configured = true,
                                Ok(None) => variable.configured = false,
                                Err(value) => error = Some(value.to_string()),
                            }
                        }
                    });
                    row.col(|ui| {
                        if !variable.is_empty()
                            && ui
                                .add_sized(
                                    [28.0, 28.0],
                                    egui::Button::new(theme::icon("x", 13.0)).frame(false),
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
        environment.variables.remove(index);
    }
    environment.ensure_empty_row();
    error
}
