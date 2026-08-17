use std::time::Duration;

use apitest_interop::{OpenApiIssueLevel, validate_openapi};
use eframe::egui::{self, CornerRadius, RichText, Stroke, Vec2};

use crate::app::{ApiTestApp, LANGUAGE_SETTING, THEME_SETTING};
use crate::i18n::Language;
use crate::state::action::{Confirmation, InteropAction, OpenApiPreviewTab, ToastKind};
use crate::theme::{self, Palette, ThemeMode};

impl ApiTestApp {
    pub(crate) fn settings_window(&mut self, context: &egui::Context) {
        let mut open = self.show_settings;
        let mut theme_changed = false;
        let mut language_changed = false;
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
                ui.label(RichText::new(&hint).color(Palette::for_mode(self.theme).muted));
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
        let palette = Palette::for_mode(self.theme);
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
        };
        let message = match confirmation {
            Confirmation::Unsaved(_) => self.tr(
                "当前内容尚未保存。",
                "The current changes have not been saved.",
            ),
            Confirmation::DeleteRequest(_) => self.tr(
                "该请求及其本地密钥将被删除。",
                "This request and its local secrets will be deleted.",
            ),
            Confirmation::DeleteEnvironment(_) => self.tr(
                "该环境及其本地密钥将被删除。",
                "This environment and its local secrets will be deleted.",
            ),
        };
        let mut save = false;
        let mut discard = false;
        let mut confirm_delete = false;
        let mut cancel = false;
        egui::Window::new(title)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .resizable(false)
            .collapsible(false)
            .show(context, |ui| {
                ui.set_min_width(340.0);
                ui.label(message);
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
                        Confirmation::DeleteRequest(_) | Confirmation::DeleteEnvironment(_) => {
                            if ui
                                .button(
                                    RichText::new(self.tr("删除", "Delete"))
                                        .color(Palette::for_mode(self.theme).danger),
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
                Confirmation::Unsaved(_) => {}
            }
        }
    }

    pub(crate) fn toast_area(&mut self, context: &egui::Context) {
        let Some(toast) = &self.toast else {
            return;
        };
        let lifetime = if toast.kind == ToastKind::Error {
            Duration::from_secs(8)
        } else {
            Duration::from_secs(4)
        };
        if toast.created_at.elapsed() >= lifetime {
            self.toast = None;
            return;
        }
        context.request_repaint_after(Duration::from_millis(250));
        let palette = Palette::for_mode(self.theme);
        let (icon, color) = match toast.kind {
            ToastKind::Success => ("circle-check", palette.success),
            ToastKind::Info => ("info", palette.info),
            ToastKind::Error => ("circle-alert", palette.danger),
        };
        let message = toast.message.clone();
        let mut close = false;
        egui::Area::new("toast".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, [-18.0, -18.0])
            .order(egui::Order::Foreground)
            .show(context, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, color))
                    .corner_radius(CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(theme::icon(icon, 15.0).color(color));
                            ui.label(message);
                            if ui
                                .add_sized([24.0, 24.0], egui::Button::new(theme::icon("x", 13.0)))
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    });
            });
        if close {
            self.toast = None;
        }
    }
}
