pub(crate) mod cookies;
pub(crate) mod palette;
pub(crate) mod rename;
pub(crate) mod snippet;

use std::time::Duration;

use apitest_interop::{OpenApiIssueLevel, validate_openapi};
use eframe::egui::{self, RichText, Stroke};

use crate::app::{ApiTestApp, LANGUAGE_SETTING, THEME_SETTING};
use crate::i18n::Language;
use crate::services::history::{
    HISTORY_AGE_RANGE, HISTORY_MAX_AGE_DAYS_SETTING, HISTORY_MAX_RECORDS_SETTING,
    HISTORY_RECORDS_RANGE,
};
use crate::state::action::{Confirmation, InteropAction, OpenApiPreviewTab, ToastKind};
use crate::theme::tokens::icon as icon_size;
use crate::theme::tokens::radius;
use crate::theme::{self, ThemeMode, UiExt};

impl ApiTestApp {
    pub(crate) fn settings_window(&mut self, context: &egui::Context) {
        let mut open = self.show_settings;
        let mut theme_changed = false;
        let mut language_changed = false;
        let mut history_changed = false;
        let mut open_cookies = false;
        let dark_label = self.tr("深色", "Dark");
        let light_label = self.tr("浅色", "Light");
        egui::Window::new(self.tr("设置", "Settings"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(360.0)
            .show(context, |ui| {
                ui.label(RichText::new(self.tr("外观", "Appearance")).strong());
                ui.horizontal(|ui| {
                    theme_changed |= ui
                        .selectable_value(&mut self.theme, ThemeMode::Dark, dark_label)
                        .changed();
                    theme_changed |= ui
                        .selectable_value(&mut self.theme, ThemeMode::Light, light_label)
                        .changed();
                });
                ui.add_space(8.0);
                ui.label(RichText::new(self.tr("语言", "Language")).strong());
                ui.horizontal(|ui| {
                    language_changed |= ui
                        .selectable_value(&mut self.language, Language::Chinese, "简体中文")
                        .changed();
                    language_changed |= ui
                        .selectable_value(&mut self.language, Language::English, "English")
                        .changed();
                });
                ui.add_space(8.0);
                ui.label(RichText::new(self.tr("运行历史", "Run history")).strong());
                ui.horizontal(|ui| {
                    ui.label(self.tr("保留条数", "Records kept"));
                    history_changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.history_max_records)
                                .range(HISTORY_RECORDS_RANGE),
                        )
                        .changed();
                    ui.label(self.tr("保留天数", "Days kept"));
                    history_changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.history_max_age_days)
                                .range(HISTORY_AGE_RANGE),
                        )
                        .changed();
                });
                ui.add_space(8.0);
                ui.label(RichText::new("Cookie").strong());
                if ui
                    .button(self.tr("管理 Cookies", "Manage cookies"))
                    .clicked()
                {
                    open_cookies = true;
                }
                ui.add_space(12.0);
                ui.label(RichText::new(self.tr("快捷键", "Keyboard shortcuts")).strong());
                let muted = ui.palette().muted;
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (chord, chinese, english) in ApiTestApp::SHORTCUTS {
                            ui.horizontal(|ui| {
                                ui.add_sized(
                                    [120.0, 20.0],
                                    egui::Label::new(RichText::new(*chord).monospace().small()),
                                );
                                ui.label(
                                    RichText::new(match self.language {
                                        Language::Chinese => *chinese,
                                        Language::English => *english,
                                    })
                                    .small()
                                    .color(muted),
                                );
                            });
                        }
                    });
            });
        self.show_settings = open;
        if theme_changed {
            theme::apply(context, self.theme);
            let theme = self.theme;
            self.persist_setting(THEME_SETTING, &theme);
        }
        if language_changed {
            let language = self.language;
            self.persist_setting(LANGUAGE_SETTING, &language);
        }
        if open_cookies {
            self.show_cookies = true;
        }
        if history_changed {
            let records = self.history_max_records;
            let days = self.history_max_age_days;
            self.persist_setting(HISTORY_MAX_RECORDS_SETTING, &records);
            self.persist_setting(HISTORY_MAX_AGE_DAYS_SETTING, &days);
        }
    }

    pub(crate) fn curl_import_window(&mut self, context: &egui::Context) {
        let mut open = self.show_curl_import;
        let mut import = false;
        let mut cancel = false;
        let title = self.tr("粘贴 cURL", "Paste cURL");
        let hint = self
            .tr(
                "粘贴完整的 curl 命令；命令只会被解析，不会执行",
                "Paste a complete curl command; it is parsed and never executed",
            )
            .to_owned();
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .default_size([720.0, 420.0])
            .show(context, |ui| {
                ui.label(RichText::new(&hint).color(ui.palette().muted));
                ui.add_sized(
                    ui.available_size() - egui::vec2(0.0, 44.0),
                    egui::TextEdit::multiline(&mut self.curl_import_source)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .hint_text("curl --request GET 'https://api.example.com/users'"),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !self.curl_import_source.trim().is_empty(),
                            egui::Button::new(self.tr("导入", "Import")),
                        )
                        .clicked()
                    {
                        import = true;
                    }
                    if ui.button(self.tr("取消", "Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            open = false;
        }
        self.show_curl_import = open;
        if import {
            let source = self.curl_import_source.clone();
            match self.import_source(InteropAction::ImportCurl, &source) {
                Ok(count) => {
                    self.curl_import_source.clear();
                    self.show_curl_import = false;
                    self.toast(
                        ToastKind::Success,
                        match self.language {
                            Language::Chinese => format!("已导入 {count} 个请求"),
                            Language::English => format!("Imported {count} requests"),
                        },
                    );
                }
                Err(error) => self.toast(ToastKind::Error, error),
            }
        }
    }

    pub(crate) fn openapi_preview_window(&mut self, context: &egui::Context) {
        let mut open = self.show_openapi_preview;
        let mut refresh = false;
        let mut validate = false;
        let mut export_yaml = false;
        let mut export_html = false;
        let palette = theme::palette(context);
        egui::Window::new(self.tr("OpenAPI 设计与预览", "OpenAPI design & preview"))
            .open(&mut open)
            .collapsible(false)
            .default_size([900.0, 650.0])
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    for (tab, chinese, english) in [
                        (OpenApiPreviewTab::Yaml, "YAML", "YAML"),
                        (OpenApiPreviewTab::Validation, "校验结果", "Validation"),
                        (
                            OpenApiPreviewTab::Documentation,
                            "HTML 文档",
                            "HTML documentation",
                        ),
                    ] {
                        ui.selectable_value(
                            &mut self.openapi_preview_tab,
                            tab,
                            match self.language {
                                Language::Chinese => chinese,
                                Language::English => english,
                            },
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(self.tr("刷新", "Refresh")).clicked() {
                            refresh = true;
                        }
                        if ui.button(self.tr("校验", "Validate")).clicked() {
                            validate = true;
                        }
                        if ui.button(self.tr("导出 HTML", "Export HTML")).clicked() {
                            export_html = true;
                        }
                        if ui.button(self.tr("导出 YAML", "Export YAML")).clicked() {
                            export_yaml = true;
                        }
                    });
                });
                ui.separator();
                match self.openapi_preview_tab {
                    OpenApiPreviewTab::Yaml => {
                        ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::multiline(&mut self.openapi_source)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    }
                    OpenApiPreviewTab::Validation => {
                        if self.openapi_issues.is_empty() {
                            ui.colored_label(
                                palette.success,
                                self.tr("未发现问题", "No issues found"),
                            );
                        }
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for issue in &self.openapi_issues {
                                let (label, color) = match issue.level {
                                    OpenApiIssueLevel::Error => {
                                        (self.tr("错误", "Error"), palette.danger)
                                    }
                                    OpenApiIssueLevel::Warning => {
                                        (self.tr("警告", "Warning"), palette.warning)
                                    }
                                };
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(label).strong().color(color));
                                    ui.label(RichText::new(&issue.path).monospace());
                                    ui.label(&issue.message);
                                });
                                ui.separator();
                            }
                        });
                    }
                    OpenApiPreviewTab::Documentation => {
                        ui.label(
                            RichText::new(self.tr(
                                "以下为可直接发布的静态 HTML 源码预览",
                                "Preview of the publish-ready static HTML source",
                            ))
                            .color(palette.muted),
                        );
                        egui::ScrollArea::both().show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&self.openapi_html)
                                        .monospace()
                                        .color(palette.text),
                                )
                                .selectable(true)
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        });
                    }
                }
            });
        self.show_openapi_preview = open;
        if refresh && let Err(error) = self.refresh_openapi_preview() {
            self.toast(ToastKind::Error, error);
        }
        if validate {
            match validate_openapi(&self.openapi_source) {
                Ok(issues) => {
                    self.openapi_issues = issues;
                    self.openapi_preview_tab = OpenApiPreviewTab::Validation;
                }
                Err(error) => self.toast(ToastKind::Error, error.to_string()),
            }
        }
        if export_yaml {
            let source = self.openapi_source.clone();
            self.export_text("openapi.yaml", "OpenAPI", &source);
        }
        if export_html {
            let html = self.openapi_html.clone();
            self.export_text("api-documentation.html", "HTML", &html);
        }
    }

    pub(crate) fn confirmation_window(&mut self, context: &egui::Context) {
        let Some(confirmation) = self.confirmation else {
            return;
        };
        let title = match confirmation {
            Confirmation::Unsaved(_) => self.tr("未保存的更改", "Unsaved changes"),
            Confirmation::DeleteRequest(_) => self.tr("删除请求", "Delete request"),
            Confirmation::DeleteEnvironment(_) => self.tr("删除环境", "Delete environment"),
            Confirmation::DeleteFolder { .. } => self.tr("删除文件夹", "Delete folder"),
            Confirmation::DeleteScenario(_) => self.tr("删除场景", "Delete scenario"),
            Confirmation::DeleteMock(_) => self.tr("删除 Mock", "Delete mock"),
        };
        let message = match confirmation {
            Confirmation::Unsaved(_) => self
                .tr(
                    "当前内容尚未保存。",
                    "The current changes have not been saved.",
                )
                .to_owned(),
            Confirmation::DeleteRequest(_) => self
                .tr(
                    "该请求及其本地密钥将被删除。",
                    "This request and its local secrets will be deleted.",
                )
                .to_owned(),
            Confirmation::DeleteEnvironment(_) => self
                .tr(
                    "该环境及其本地密钥将被删除。",
                    "This environment and its local secrets will be deleted.",
                )
                .to_owned(),
            // Spell out the blast radius: a folder takes its requests with it.
            // The count was computed when the confirmation was raised, so the
            // dialog does not query the database on every frame it stays open.
            Confirmation::DeleteFolder { requests, .. } => match self.language {
                Language::Chinese => {
                    format!("该文件夹及其中的 {requests} 个请求（含本地密钥）将被删除。")
                }
                Language::English => format!(
                    "This folder and the {requests} requests inside it will be deleted, along with their local secrets."
                ),
            },
            Confirmation::DeleteScenario(_) => self
                .tr("该测试场景将被删除。", "This scenario will be deleted.")
                .to_owned(),
            Confirmation::DeleteMock(_) => self
                .tr(
                    "该 Mock 服务将被删除。",
                    "This mock server will be deleted.",
                )
                .to_owned(),
        };
        let mut save = false;
        let mut discard = false;
        let mut confirm_delete = false;
        let mut cancel = false;
        // A modal blocks the workspace behind it and closes on Esc or a click
        // on the backdrop — both mean "cancel", never the destructive choice.
        let modal = egui::Modal::new(egui::Id::new("confirmation_modal")).show(context, |ui| {
            ui.set_min_width(340.0);
            ui.label(RichText::new(title).strong().size(15.0));
            ui.add_space(6.0);
            ui.label(&message);
            ui.add_space(12.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match confirmation {
                    Confirmation::Unsaved(_) => {
                        if ui.button(self.tr("保存", "Save")).clicked() {
                            save = true;
                        }
                        if ui.button(self.tr("放弃", "Discard")).clicked() {
                            discard = true;
                        }
                    }
                    Confirmation::DeleteRequest(_)
                    | Confirmation::DeleteEnvironment(_)
                    | Confirmation::DeleteFolder { .. }
                    | Confirmation::DeleteScenario(_)
                    | Confirmation::DeleteMock(_) => {
                        if ui
                            .button(
                                RichText::new(self.tr("删除", "Delete")).color(ui.palette().danger),
                            )
                            .clicked()
                        {
                            confirm_delete = true;
                        }
                    }
                }
                if ui.button(self.tr("取消", "Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        if modal.should_close() {
            cancel = true;
        }
        if cancel {
            self.confirmation = None;
        } else if save {
            if let Confirmation::Unsaved(action) = confirmation {
                let saved = if action.leaves_workspace() {
                    self.save_workspace_and_wait()
                } else {
                    self.save_current_and_wait()
                };
                if !saved {
                    return;
                }
                self.confirmation = None;
                self.perform_action(action);
            }
        } else if discard {
            if let Confirmation::Unsaved(action) = confirmation {
                let discarded = if action.leaves_workspace() {
                    self.discard_workspace()
                } else {
                    self.discard_current()
                };
                if !discarded {
                    return;
                }
                self.confirmation = None;
                self.perform_action(action);
            }
        } else if confirm_delete {
            self.confirmation = None;
            match confirmation {
                Confirmation::DeleteRequest(id) => self.delete_request(id),
                Confirmation::DeleteEnvironment(id) => self.delete_environment(id),
                Confirmation::DeleteFolder { node, .. } => self.delete_folder(node),
                Confirmation::DeleteScenario(id) => self.delete_scenario(id),
                Confirmation::DeleteMock(id) => self.delete_mock(id),
                Confirmation::Unsaved(_) => {}
            }
        }
    }

    pub(crate) fn toast_area(&mut self, context: &egui::Context) {
        self.toasts.retain_live();
        if self.toasts.is_empty() {
            return;
        }
        context.request_repaint_after(Duration::from_millis(250));
        let palette = theme::palette(context);
        let copy_label = self.tr("复制", "Copy");
        let close_label = self.tr("关闭", "Close");
        let mut dismiss = None;
        let mut copy = None;
        egui::Area::new("toasts".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, [-18.0, -18.0])
            .order(egui::Order::Foreground)
            .show(context, |ui| {
                ui.with_layout(
                    egui::Layout::bottom_up(egui::Align::Max).with_cross_justify(false),
                    |ui| {
                        for (index, toast) in self.toasts.iter().enumerate() {
                            let (icon, color) = match toast.kind {
                                ToastKind::Success => ("circle-check", palette.success),
                                ToastKind::Info => ("info", palette.info),
                                ToastKind::Error => ("circle-alert", palette.danger),
                            };
                            egui::Frame::popup(ui.style())
                                .fill(palette.panel)
                                .stroke(Stroke::new(1.0, color))
                                .corner_radius(radius::MD)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(theme::icon(icon, icon_size::LG).color(color));
                                        ui.label(&toast.message);
                                        if toast.kind == ToastKind::Error
                                            && ui
                                                .add(
                                                    egui::Button::new(theme::icon(
                                                        "copy",
                                                        icon_size::SM,
                                                    ))
                                                    .frame(false),
                                                )
                                                .on_hover_text(copy_label)
                                                .clicked()
                                        {
                                            copy = Some(toast.message.clone());
                                        }
                                        if ui
                                            .add(
                                                egui::Button::new(theme::icon("x", icon_size::SM))
                                                    .frame(false),
                                            )
                                            .on_hover_text(close_label)
                                            .clicked()
                                        {
                                            dismiss = Some(index);
                                        }
                                    });
                                });
                        }
                    },
                );
            });
        if let Some(message) = copy {
            context.copy_text(message);
        }
        if let Some(index) = dismiss {
            self.toasts.remove(index);
        }
    }
}
