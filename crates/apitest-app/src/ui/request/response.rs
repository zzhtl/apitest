use apitest_core::ProtocolKind;
use eframe::egui::{self, CornerRadius, RichText, Stroke};
use egui_extras::{Column, TableBuilder};

use crate::app::ApiTestApp;
use crate::i18n::Language;
use crate::state::response::{ResponseBodyMode, ResponseTab, TimelineEntry, TimelinePhase};
use crate::theme::{self, UiExt};
use crate::ui::widgets::{empty_state, tab_button};

impl ApiTestApp {
    pub(crate) fn response_panel(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let websocket_selected = self
            .requests
            .get(self.selected)
            .is_some_and(|request| request.protocol_kind() == ProtocolKind::WebSocket);
        ui.horizontal(|ui| {
            for (tab, chinese, english) in [
                (ResponseTab::Body, "响应体", "Body"),
                (ResponseTab::Headers, "响应头", "Headers"),
                (ResponseTab::Timeline, "时间线", "Timeline"),
            ] {
                if tab_button(ui, self.response_tab == tab, self.tr(chinese, english)).clicked() {
                    self.response_tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.response.is_active() {
                    ui.spinner();
                }
                if let Some(metrics) = self.response.metrics {
                    ui.label(
                        RichText::new(format!(
                            "{} ms  ·  ↓ {} B  ·  ↑ {} B",
                            metrics.elapsed_ms, metrics.received_bytes, metrics.sent_bytes
                        ))
                        .color(palette.muted),
                    );
                }
                if let Some(version) = &self.response.version {
                    ui.label(RichText::new(version).color(palette.muted));
                }
                if let Some(status) = self.response.status {
                    let color = if status < 400 {
                        palette.success
                    } else {
                        palette.danger
                    };
                    ui.label(RichText::new(status.to_string()).strong().color(color));
                }
            });
        });
        if websocket_selected {
            self.websocket_message_bar(ui);
        }
        ui.separator();
        if let Some(error) = &self.response.error {
            egui::Frame::new()
                .fill(palette.primary_soft)
                .stroke(Stroke::new(1.0, palette.danger))
                .corner_radius(CornerRadius::same(5))
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.label(RichText::new(error).color(palette.danger));
                });
            ui.add_space(6.0);
        }
        match self.response_tab {
            ResponseTab::Body => self.response_body(ui),
            ResponseTab::Headers => response_headers(ui, &self.response.headers, self.language),
            ResponseTab::Timeline => response_timeline(ui, &self.response.timeline, self.language),
        }
    }

    pub(crate) fn websocket_message_bar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let connected = self.execution_commands.is_some();
        let can_send = connected && !self.websocket_message.trim().is_empty();
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
            let response = ui.add_sized(
                [width, 30.0],
                egui::TextEdit::singleline(&mut self.websocket_message).hint_text(message_hint),
            );
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
        if self.response.body.is_empty() && !self.response.is_active() {
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
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.response_body_mode,
                ResponseBodyMode::Pretty,
                "Pretty",
            );
            ui.selectable_value(&mut self.response_body_mode, ResponseBodyMode::Raw, "Raw");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_sized([28.0, 28.0], egui::Button::new(theme::icon("copy", 14.0)))
                    .on_hover_text(self.tr("复制响应", "Copy response"))
                    .clicked()
                {
                    let text = self
                        .response
                        .pretty_body
                        .as_ref()
                        .filter(|_| self.response_body_mode == ResponseBodyMode::Pretty)
                        .unwrap_or(&self.response.body)
                        .clone();
                    ui.ctx().copy_text(text);
                }
            });
        });
        let body = self
            .response
            .pretty_body
            .as_ref()
            .filter(|_| self.response_body_mode == ResponseBodyMode::Pretty)
            .unwrap_or(&self.response.body);
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(
                egui::Label::new(RichText::new(body).monospace().color(palette.text))
                    .selectable(true)
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
        });
        if self.response.truncated {
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
        empty_state(
            ui,
            match language {
                Language::Chinese => "暂无响应头",
                Language::English => "No response headers",
            },
            "",
        );
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
        empty_state(
            ui,
            match language {
                Language::Chinese => "暂无时间线",
                Language::English => "No timeline",
            },
            "",
        );
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
