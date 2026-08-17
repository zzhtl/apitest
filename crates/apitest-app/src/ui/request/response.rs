use apitest_core::ProtocolKind;
use eframe::egui::{self, RichText, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::app::ApiTestApp;
use crate::i18n::{Language, tr};
use crate::state::response::{ResponseBodyMode, ResponseTab, TimelineEntry, TimelinePhase};
use crate::state::verification::VerificationOutcome;
use crate::theme::tokens::radius;
use crate::theme::{self, UiExt};
use crate::ui::widgets::{Tone, badge, empty_state, icon_button, tab_button};

impl ApiTestApp {
    pub(crate) fn response_panel(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let websocket_selected = self
            .requests
            .get(self.selected)
            .is_some_and(|request| request.protocol_kind() == ProtocolKind::WebSocket);
        let mut selected_tab = self.session().response_tab;
        ui.horizontal(|ui| {
            let checks = self
                .session()
                .verification
                .as_ref()
                .map(|outcome| outcome.assertions.len())
                .unwrap_or_default();
            for (tab, chinese, english) in [
                (ResponseTab::Body, "响应体", "Body"),
                (ResponseTab::Headers, "响应头", "Headers"),
                (ResponseTab::Timeline, "时间线", "Timeline"),
                (ResponseTab::Tests, "测试结果", "Tests"),
            ] {
                let label = self.tr(chinese, english);
                let label = if tab == ResponseTab::Tests && checks > 0 {
                    format!("{label} {checks}")
                } else {
                    label.to_owned()
                };
                if tab_button(ui, selected_tab == tab, &label).clicked() {
                    selected_tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let response = &self.session().response;
                if response.is_active() {
                    ui.spinner();
                }
                if let Some(metrics) = response.metrics {
                    ui.label(
                        RichText::new(format!(
                            "{} ms  ·  ↓ {} B  ·  ↑ {} B",
                            metrics.elapsed_ms, metrics.received_bytes, metrics.sent_bytes
                        ))
                        .color(palette.muted),
                    );
                }
                if let Some(version) = &response.version {
                    ui.label(RichText::new(version).color(palette.muted));
                }
                if let Some(status) = response.status {
                    badge(ui, status.to_string(), status_tone(status));
                }
            });
        });
        self.session_mut().response_tab = selected_tab;
        if websocket_selected {
            self.websocket_message_bar(ui);
        }
        ui.separator();
        if let Some(error) = self.session().response.error.clone() {
            egui::Frame::new()
                .fill(palette.primary_soft)
                .stroke(Stroke::new(1.0, palette.danger))
                .corner_radius(radius::SM)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.label(RichText::new(error).color(palette.danger));
                });
            ui.add_space(6.0);
        }
        match selected_tab {
            ResponseTab::Body => self.response_body(ui),
            ResponseTab::Headers => {
                let headers = self.session().response.headers.clone();
                response_headers(ui, &headers, self.language);
            }
            ResponseTab::Timeline => {
                let timeline = self.session().response.timeline.clone();
                response_timeline(ui, &timeline, self.language);
            }
            ResponseTab::Tests => {
                let outcome = self.session().verification.clone();
                verification_view(ui, outcome.as_ref(), self.language);
            }
        }
    }

    pub(crate) fn websocket_message_bar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let connected = self.session().execution_commands.is_some();
        let can_send = connected && !self.session().websocket_message.trim().is_empty();
        let message_hint = self
            .tr("输入 WebSocket 文本消息", "Enter a WebSocket text message")
            .to_owned();
        let mut send = false;
        let mut close = false;
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if connected {
                    self.tr("已连接", "Connected")
                } else {
                    self.tr("未连接", "Disconnected")
                })
                .small()
                .color(if connected {
                    palette.success
                } else {
                    palette.muted
                }),
            );
            let width = (ui.available_width() - 220.0).max(160.0);
            let mut message = std::mem::take(&mut self.session_mut().websocket_message);
            let response = ui.add_sized(
                [width, 30.0],
                egui::TextEdit::singleline(&mut message).hint_text(message_hint),
            );
            self.session_mut().websocket_message = message;
            if response.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                && can_send
            {
                send = true;
            }
            if ui
                .add_enabled(
                    can_send,
                    egui::Button::new(theme::icon_label(
                        "send",
                        self.tr("发送消息", "Send message"),
                        12.0,
                        palette.text,
                    )),
                )
                .clicked()
            {
                send = true;
            }
            if ui
                .add_enabled(
                    connected,
                    egui::Button::new(self.tr("关闭连接", "Close connection")),
                )
                .clicked()
            {
                close = true;
            }
        });
        if send {
            self.send_websocket_message();
        }
        if close {
            self.close_websocket_input();
        }
    }

    pub(crate) fn response_body(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        if self.session().response.body.is_empty() && !self.session().response.is_active() {
            empty_state(
                ui,
                self.tr("暂无响应", "No response yet"),
                self.tr(
                    "发送请求后将在此显示结果",
                    "Send a request to inspect the result",
                ),
            );
            return;
        }
        let mut mode = self.session().response_body_mode;
        let body = {
            let response = &self.session().response;
            response
                .pretty_body
                .as_ref()
                .filter(|_| mode == ResponseBodyMode::Pretty)
                .unwrap_or(&response.body)
                .clone()
        };
        ui.horizontal(|ui| {
            ui.selectable_value(&mut mode, ResponseBodyMode::Pretty, "Pretty");
            ui.selectable_value(&mut mode, ResponseBodyMode::Raw, "Raw");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, "copy", self.tr("复制响应", "Copy response")).clicked() {
                    ui.ctx().copy_text(body.clone());
                }
            });
        });
        self.session_mut().response_body_mode = mode;
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(
                egui::Label::new(RichText::new(&body).monospace().color(palette.text))
                    .selectable(true)
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
        });
        if self.session().response.truncated {
            ui.colored_label(
                palette.warning,
                self.tr(
                    "响应超过 10 MiB，预览已截断",
                    "The response preview was truncated at 10 MiB",
                ),
            );
        }
    }
}

pub(crate) fn response_headers(
    ui: &mut egui::Ui,
    headers: &[(String, String)],
    language: Language,
) {
    if headers.is_empty() {
        empty_state(ui, tr(language, "暂无响应头", "No response headers"), "");
        return;
    }
    TableBuilder::new(ui)
        .striped(true)
        .column(Column::initial(220.0).at_least(140.0).resizable(true))
        .column(Column::remainder().at_least(240.0))
        .body(|mut body| {
            for (name, value) in headers {
                body.row(30.0, |mut row| {
                    row.col(|ui| {
                        ui.label(RichText::new(name).strong());
                    });
                    row.col(|ui| {
                        ui.label(value);
                    });
                });
            }
        });
}

pub(crate) fn response_timeline(ui: &mut egui::Ui, timeline: &[TimelineEntry], language: Language) {
    let palette = ui.palette();
    if timeline.is_empty() {
        empty_state(ui, tr(language, "暂无时间线", "No timeline"), "");
        return;
    }
    for entry in timeline {
        let (label, color) = match entry.phase {
            TimelinePhase::Started => (("开始", "Started"), palette.info),
            TimelinePhase::ResponseHead => (("收到响应头", "Response head"), palette.info),
            TimelinePhase::FirstByte => (("收到首字节", "First byte"), palette.accent_text),
            TimelinePhase::Completed => (("完成", "Completed"), palette.success),
            TimelinePhase::Cancelled => (("已取消", "Cancelled"), palette.warning),
            TimelinePhase::Failed => (("失败", "Failed"), palette.danger),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("●").color(color).size(9.0));
            ui.add_sized(
                [150.0, 24.0],
                egui::Label::new(match language {
                    Language::Chinese => label.0,
                    Language::English => label.1,
                }),
            );
            ui.label(RichText::new(format!("{} ms", entry.elapsed_ms)).color(palette.muted));
        });
    }
}

/// 2xx succeeded, 3xx redirected, 4xx is the caller's fault, 5xx the server's.
pub(crate) fn status_tone(status: u16) -> Tone {
    match status {
        100..=299 => Tone::Success,
        300..=399 => Tone::Info,
        400..=499 => Tone::Warning,
        _ => Tone::Danger,
    }
}

/// Assertion results and extracted variables for the most recent run.
fn verification_view(ui: &mut egui::Ui, outcome: Option<&VerificationOutcome>, language: Language) {
    let palette = ui.palette();
    let Some(outcome) = outcome.filter(|outcome| !outcome.is_empty()) else {
        empty_state(
            ui,
            tr(language, "暂无测试结果", "No test results"),
            tr(
                language,
                "在“测试”标签配置断言或提取器后，发送请求即可看到结果",
                "Add assertions or extractors in the Tests tab, then send the request",
            ),
        );
        return;
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        if let Some(error) = &outcome.error {
            ui.colored_label(palette.danger, error);
            ui.add_space(6.0);
        }
        for assertion in &outcome.assertions {
            ui.horizontal(|ui| {
                badge(
                    ui,
                    tr(
                        language,
                        if assertion.passed { "通过" } else { "失败" },
                        if assertion.passed { "Pass" } else { "Fail" },
                    ),
                    if assertion.passed {
                        Tone::Success
                    } else {
                        Tone::Danger
                    },
                );
                ui.label(&assertion.name);
                if let Some(error) = &assertion.error {
                    ui.label(RichText::new(error).small().color(palette.danger));
                }
            });
        }
        if !outcome.extracted.is_empty() {
            ui.add_space(10.0);
            ui.label(
                RichText::new(tr(language, "提取的变量", "Extracted variables"))
                    .strong()
                    .size(14.0),
            );
            for (name, value) in &outcome.extracted {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).monospace().strong());
                    ui.label(RichText::new(value).monospace().color(palette.muted));
                });
            }
        }
    });
}
