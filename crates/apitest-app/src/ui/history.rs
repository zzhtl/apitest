use apitest_core::RunState as HistoryRunState;
use eframe::egui::{self, Color32, RichText};

use crate::app::ApiTestApp;
use crate::i18n::Language;
use crate::theme::Palette;
use crate::ui::widgets::{empty_state, format_bytes};

impl ApiTestApp {
    pub(crate) fn history_workspace(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let Some(record) = self.run_records.get(self.selected_history).cloned() else {
            empty_state(
                ui,
                self.tr("暂无运行历史", "No run history"),
                self.tr(
                    "发送请求后会在本地保存脱敏的执行记录",
                    "Run a request to save a redacted local record",
                ),
                palette,
            );
            return;
        };
        let request_name = record
            .request_case_id
            .and_then(|case_id| {
                self.requests
                    .iter()
                    .find(|request| request.request_case.id == case_id)
            })
            .map(|request| request.name.as_str())
            .unwrap_or(self.tr("已删除的请求", "Deleted request"));
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(request_name).heading());
                    ui.colored_label(
                        history_state_color(record.state, palette),
                        history_state_label(record.state, self.language),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    history_metric(
                        ui,
                        self.tr("状态码", "Status"),
                        record
                            .status_code
                            .map(|status| status.to_string())
                            .unwrap_or_else(|| "—".into()),
                        palette,
                    );
                    history_metric(
                        ui,
                        self.tr("耗时", "Elapsed"),
                        format!("{} ms", record.elapsed_ms),
                        palette,
                    );
                    history_metric(
                        ui,
                        self.tr("响应大小", "Response size"),
                        format_bytes(record.response_bytes),
                        palette,
                    );
                    history_metric(
                        ui,
                        self.tr("开始时间", "Started"),
                        record.started_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                        palette,
                    );
                });
                if let Some(error) = &record.error {
                    ui.colored_label(palette.danger, error);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.tr("响应正文", "Response body")).strong());
                    ui.label(
                        RichText::new(self.tr("已脱敏并外置存储", "Redacted external storage"))
                            .small()
                            .color(palette.muted),
                    );
                });
                if record.body_path.is_none() {
                    empty_state(ui, self.tr("没有响应正文", "No response body"), "", palette);
                } else {
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.history_body_preview)
                                    .monospace()
                                    .color(palette.text),
                            )
                            .selectable(true)
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
                    if self.history_body_truncated {
                        ui.colored_label(
                            palette.warning,
                            self.tr(
                                "历史正文超过 10 MiB，预览已截断",
                                "History body preview was truncated at 10 MiB",
                            ),
                        );
                    }
                }
            });
    }
}

pub(crate) fn history_state_label(state: HistoryRunState, language: Language) -> &'static str {
    match (state, language) {
        (HistoryRunState::Running, Language::Chinese) => "运行中",
        (HistoryRunState::Running, Language::English) => "Running",
        (HistoryRunState::Passed, Language::Chinese) => "通过",
        (HistoryRunState::Passed, Language::English) => "Passed",
        (HistoryRunState::Failed, Language::Chinese) => "失败",
        (HistoryRunState::Failed, Language::English) => "Failed",
        (HistoryRunState::Cancelled, Language::Chinese) => "已取消",
        (HistoryRunState::Cancelled, Language::English) => "Cancelled",
    }
}

pub(crate) fn history_state_color(state: HistoryRunState, palette: Palette) -> Color32 {
    match state {
        HistoryRunState::Running => palette.info,
        HistoryRunState::Passed => palette.success,
        HistoryRunState::Failed => palette.danger,
        HistoryRunState::Cancelled => palette.warning,
    }
}

pub(crate) fn history_metric(ui: &mut egui::Ui, label: &str, value: String, palette: Palette) {
    ui.group(|ui| {
        ui.label(RichText::new(label).small().color(palette.muted));
        ui.label(RichText::new(value).strong());
    });
}
