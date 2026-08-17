pub(crate) mod response;

use std::sync::Arc;

use apitest_core::{HttpMethod, ProtocolKind};
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};

use crate::app::ApiTestApp;
use crate::draft::{ProxyDraft, RequestDraft};
use crate::i18n::Language;
use crate::state::action::{Confirmation, PendingAction, ToastKind};
use crate::state::response::RunState;
use crate::state::workspace::EditorTab;
use crate::theme::tokens::icon as icon_size;
use crate::theme::tokens::pad;
use crate::theme::{self, Palette, UiExt};
use crate::ui::editors::auth::auth_editor;
use crate::ui::editors::body::body_editor;
use crate::ui::editors::editable_pairs;
use crate::ui::editors::protocol::{protocol_color, protocol_editor, protocol_label};
use crate::ui::widgets::{dirty_marker, empty_state_action};

impl ApiTestApp {
    pub(crate) fn request_workspace(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        if self.requests.get(self.selected).is_none() {
            if empty_state_action(
                ui,
                self.tr("暂无请求", "No requests"),
                self.tr(
                    "新建请求后可编辑参数、请求体与认证",
                    "Create a request to edit parameters, body and authentication",
                ),
                self.tr("新建请求", "Create request"),
            ) {
                self.queue_action(PendingAction::NewRequest(ProtocolKind::Http));
            }
            return;
        }
        let maximum = (ui.available_height() - 220.0).max(220.0);
        egui::Panel::top("request_composer")
            .resizable(true)
            .default_size(350.0)
            .size_range(220.0..=maximum)
            .frame(
                egui::Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0, palette.divider)),
            )
            .show(ui, |ui| self.request_composer(ui));
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(pad::COMPOSER)
            .show(ui, |ui| self.response_panel(ui));
    }

    pub(crate) fn request_composer(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        ui.set_min_size(ui.available_size());
        let index = self.selected;
        if self.requests[index].alternate_protocol.is_some() {
            self.protocol_request_composer(ui);
            return;
        }
        let request_id = self.requests[index].id();
        let dirty = self.requests[index].is_dirty();
        let save_label = self.tr("保存", "Save");
        let send_label = self.tr("发送", "Send");
        let stop_label = self.tr("停止", "Stop");
        let delete_tip = self.tr("删除请求", "Delete request");
        let settings_tip = self.tr("请求设置", "Request settings");
        let follow_redirects_label = self.tr("跟随重定向", "Follow redirects");
        let validate_tls_label = self.tr("校验 TLS 证书", "Validate TLS certificates");
        let mut save = false;
        let mut delete = false;
        let mut send = false;
        let mut stop = false;
        let mut editor_error = None;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(pad::COMPOSER)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.requests[index].name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(280.0),
                    );
                    if dirty {
                        dirty_marker(ui);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [32.0, 30.0],
                                egui::Button::new(theme::icon("trash-2", icon_size::MD))
                                    .stroke(Stroke::NONE),
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
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    method_combo(ui, &mut self.requests[index].draft.method);
                    let reserved = 44.0 + 96.0 + 16.0;
                    let url_width = (ui.available_width() - reserved).max(180.0);
                    ui.add_sized(
                        [url_width, 36.0],
                        egui::TextEdit::singleline(&mut self.requests[index].draft.url)
                            .hint_text("https://api.example.com/v1/users/{{id}}"),
                    );
                    ui.menu_button(theme::icon("settings", icon_size::LG), |ui| {
                        ui.set_min_width(230.0);
                        ui.label(RichText::new(settings_tip).strong());
                        ui.horizontal(|ui| {
                            ui.label(self.tr("超时", "Timeout"));
                            ui.add(
                                egui::DragValue::new(&mut self.requests[index].draft.timeout_ms)
                                    .range(1..=600_000)
                                    .suffix(" ms"),
                            );
                        });
                        ui.checkbox(
                            &mut self.requests[index].draft.follow_redirects,
                            follow_redirects_label,
                        );
                        ui.checkbox(
                            &mut self.requests[index].draft.validate_tls,
                            validate_tls_label,
                        );
                        ui.separator();
                        let mut proxy_enabled = self.requests[index].draft.proxy.is_some();
                        if ui
                            .checkbox(&mut proxy_enabled, self.tr("使用代理", "Use proxy"))
                            .changed()
                        {
                            self.requests[index].draft.proxy = if proxy_enabled {
                                let mut proxy = ProxyDraft::new(request_id);
                                match self.secrets.get(&proxy.password.reference) {
                                    Ok(Some(_)) => proxy.password.configured = true,
                                    Ok(None) => {}
                                    Err(error) => editor_error = Some(error.to_string()),
                                }
                                Some(proxy)
                            } else {
                                None
                            };
                        }
                        let proxy_username_hint = self
                            .tr("代理用户名（可选）", "Proxy username (optional)")
                            .to_owned();
                        let proxy_password_hint = self
                            .tr("代理密码（可选）", "Proxy password (optional)")
                            .to_owned();
                        if let Some(proxy) = self.requests[index].draft.proxy.as_mut() {
                            ui.add(
                                egui::TextEdit::singleline(&mut proxy.url)
                                    .hint_text("http://127.0.0.1:8080 / socks5://..."),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut proxy.username)
                                    .hint_text(proxy_username_hint),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut proxy.password.replacement)
                                    .password(true)
                                    .hint_text(if proxy.password.configured {
                                        "••••••••"
                                    } else {
                                        &proxy_password_hint
                                    }),
                            );
                        }
                        ui.horizontal(|ui| {
                            let certificate = self.requests[index]
                                .draft
                                .client_certificate
                                .as_ref()
                                .map(|certificate| certificate.pem_file.display().to_string())
                                .unwrap_or_else(|| {
                                    self.tr("未配置客户端证书", "No client certificate").into()
                                });
                            ui.label(RichText::new(certificate).small().color(palette.muted));
                            if ui.button(self.tr("选择 PEM", "Choose PEM")).clicked()
                                && let Some(pem_file) = rfd::FileDialog::new()
                                    .add_filter("PEM", &["pem", "crt", "key"])
                                    .pick_file()
                            {
                                self.requests[index].draft.client_certificate =
                                    Some(apitest_core::ClientCertificate { pem_file });
                            }
                            if self.requests[index].draft.client_certificate.is_some()
                                && ui.button(self.tr("清除", "Clear")).clicked()
                            {
                                self.requests[index].draft.client_certificate = None;
                            }
                        });
                    })
                    .response
                    .on_hover_text(settings_tip);
                    if self.response.is_active() {
                        let stopping = self.response.state == RunState::Cancelling;
                        if ui
                            .add_enabled(
                                !stopping,
                                egui::Button::new(theme::icon_label(
                                    "circle-stop",
                                    stop_label,
                                    13.0,
                                    palette.danger,
                                ))
                                .fill(palette.primary_soft)
                                .stroke(Stroke::new(1.0, palette.danger))
                                .min_size(Vec2::new(92.0, 36.0)),
                            )
                            .clicked()
                        {
                            stop = true;
                        }
                    } else if ui
                        .add_sized(
                            [92.0, 36.0],
                            egui::Button::new(theme::icon_label(
                                "send",
                                send_label,
                                13.0,
                                Color32::WHITE,
                            ))
                            .fill(palette.primary)
                            .stroke(Stroke::NONE),
                        )
                        .clicked()
                    {
                        send = true;
                    }
                });
                ui.add_space(6.0);
                editor_tabs(
                    ui,
                    &mut self.editor_tab,
                    self.language,
                    &self.requests[index].draft,
                );
                ui.separator();
                let language = self.language;
                match self.editor_tab {
                    EditorTab::Params => {
                        editable_pairs(ui, &mut self.requests[index].draft.query, language, true)
                    }
                    EditorTab::Headers => {
                        editable_pairs(ui, &mut self.requests[index].draft.headers, language, true)
                    }
                    EditorTab::Cookies => {
                        editable_pairs(ui, &mut self.requests[index].draft.cookies, language, true)
                    }
                    EditorTab::Body => {
                        editor_error = body_editor(ui, &mut self.requests[index].draft, language)
                    }
                    EditorTab::Auth => {
                        editor_error = auth_editor(
                            ui,
                            &mut self.requests[index].draft.auth,
                            request_id,
                            Arc::clone(&self.secrets),
                            language,
                        )
                    }
                }
                self.requests[index].draft.ensure_empty_rows();
            });
        if let Some(error) = editor_error {
            self.toast(ToastKind::Error, error);
        }
        if save {
            self.save_current_request();
        }
        if delete {
            self.confirmation = Some(Confirmation::DeleteRequest(request_id));
        }
        if send {
            self.send_current(ui.ctx());
        }
        if stop {
            self.stop();
        }
    }

    pub(crate) fn protocol_request_composer(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let index = self.selected;
        let request_id = self.requests[index].id();
        let kind = self.requests[index].protocol_kind();
        let dirty = self.requests[index].is_dirty();
        let mut save = false;
        let mut delete = false;
        let mut send = false;
        let mut stop = false;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(pad::COMPOSER)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(protocol_label(kind))
                            .strong()
                            .color(protocol_color(kind, palette)),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.requests[index].name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(280.0),
                    );
                    if dirty {
                        dirty_marker(ui);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [32.0, 30.0],
                                egui::Button::new(theme::icon("trash-2", icon_size::MD))
                                    .stroke(Stroke::NONE),
                            )
                            .on_hover_text(self.tr("删除请求", "Delete request"))
                            .clicked()
                        {
                            delete = true;
                        }
                        if ui
                            .add_enabled(
                                dirty,
                                egui::Button::new(theme::icon_label(
                                    "save",
                                    self.tr("保存", "Save"),
                                    12.0,
                                    palette.text,
                                )),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                        if self.response.is_active() {
                            if ui
                                .button(theme::icon_label(
                                    "circle-stop",
                                    self.tr("停止", "Stop"),
                                    13.0,
                                    palette.danger,
                                ))
                                .clicked()
                            {
                                stop = true;
                            }
                        } else if ui
                            .add(
                                egui::Button::new(theme::icon_label(
                                    "send",
                                    self.tr("连接 / 发送", "Connect / Send"),
                                    13.0,
                                    Color32::WHITE,
                                ))
                                .fill(palette.primary)
                                .stroke(Stroke::NONE),
                            )
                            .clicked()
                        {
                            send = true;
                        }
                    });
                });
                ui.separator();
                if let Some(protocol) = self.requests[index].alternate_protocol.as_mut() {
                    protocol_editor(ui, protocol, self.language);
                }
            });
        if save {
            self.save_current_request();
        }
        if delete {
            self.confirmation = Some(Confirmation::DeleteRequest(request_id));
        }
        if send {
            self.send_current(ui.ctx());
        }
        if stop {
            self.stop();
        }
    }
}

pub(crate) fn method_combo(ui: &mut egui::Ui, method: &mut HttpMethod) {
    let palette = ui.palette();
    egui::ComboBox::from_id_salt("http_method")
        .selected_text(
            RichText::new(method.to_string())
                .strong()
                .color(method_color(*method, palette)),
        )
        .width(82.0)
        .show_ui(ui, |ui| {
            for value in [
                HttpMethod::Get,
                HttpMethod::Post,
                HttpMethod::Put,
                HttpMethod::Patch,
                HttpMethod::Delete,
                HttpMethod::Head,
                HttpMethod::Options,
                HttpMethod::Trace,
                HttpMethod::Connect,
            ] {
                ui.selectable_value(method, value, value.to_string());
            }
        });
}

pub(crate) fn method_color(method: HttpMethod, palette: Palette) -> Color32 {
    match method {
        HttpMethod::Get | HttpMethod::Head => palette.success,
        HttpMethod::Post => palette.accent_text,
        HttpMethod::Put | HttpMethod::Patch => palette.warning,
        HttpMethod::Delete => palette.danger,
        HttpMethod::Options | HttpMethod::Trace | HttpMethod::Connect => palette.info,
    }
}

pub(crate) fn editor_tabs(
    ui: &mut egui::Ui,
    selected: &mut EditorTab,
    language: Language,
    draft: &RequestDraft,
) {
    let palette = ui.palette();
    ui.horizontal(|ui| {
        let params = draft.query.iter().filter(|pair| !pair.is_empty()).count();
        let headers = draft.headers.iter().filter(|pair| !pair.is_empty()).count();
        let cookies = draft.cookies.iter().filter(|pair| !pair.is_empty()).count();
        for (tab, chinese, english, count) in [
            (EditorTab::Params, "参数", "Params", Some(params)),
            (EditorTab::Headers, "请求头", "Headers", Some(headers)),
            (EditorTab::Cookies, "Cookie", "Cookies", Some(cookies)),
            (EditorTab::Body, "请求体", "Body", None),
            (EditorTab::Auth, "认证", "Auth", None),
        ] {
            let label = match language {
                Language::Chinese => chinese,
                Language::English => english,
            };
            let label = count
                .filter(|count| *count > 0)
                .map(|count| format!("{label} {count}"))
                .unwrap_or_else(|| label.to_owned());
            let response = ui.add(
                egui::Button::new(RichText::new(label).color(if *selected == tab {
                    palette.accent_text
                } else {
                    palette.muted
                }))
                .frame(false),
            );
            if *selected == tab {
                ui.painter().line_segment(
                    [response.rect.left_bottom(), response.rect.right_bottom()],
                    Stroke::new(2.0, palette.primary),
                );
            }
            if response.clicked() {
                *selected = tab;
            }
        }
    });
}
