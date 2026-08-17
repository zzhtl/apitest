pub(crate) mod nodes;
pub(crate) mod rules;

use apitest_runtime::ScenarioReport;
use eframe::egui::{self, RichText};

use crate::app::ApiTestApp;
use crate::i18n::{Language, tr};
use crate::state::action::PendingAction;
use crate::theme::tokens::pad;
use crate::theme::{self, UiExt};
use crate::ui::scenario::nodes::scenario_nodes_editor;
use crate::ui::widgets::{dirty_marker, empty_state_action};

impl ApiTestApp {
    pub(crate) fn scenario_workspace(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        if self.scenarios.get(self.selected_scenario).is_none() {
            if empty_state_action(
                ui,
                tr(self.language, "暂无测试场景", "No scenarios"),
                tr(
                    self.language,
                    "新建场景后可视化编排请求和控制节点",
                    "Create a scenario to arrange requests and control nodes",
                ),
                tr(self.language, "新建场景", "New scenario"),
            ) {
                self.perform_action(PendingAction::NewScenario);
            }
            return;
        }
        let dirty = self.current_dirty();
        let request_options = self
            .requests
            .iter()
            .map(|request| (request.request_case.id, request.name.clone()))
            .collect::<Vec<_>>();
        let running = self.scenario_cancellation.is_some();
        let report = self.scenario_report.as_ref();
        let mut save = false;
        let mut run = false;
        let mut stop = false;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(pad::WORKSPACE)
            .show(ui, |ui| {
                let scenario = &mut self.scenarios[self.selected_scenario];
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut scenario.name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(320.0),
                    );
                    if dirty {
                        dirty_marker(ui);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if running {
                            if ui.button(tr(self.language, "停止", "Stop")).clicked() {
                                stop = true;
                            }
                            ui.spinner();
                        } else if ui
                            .button(theme::icon_label(
                                "play",
                                tr(self.language, "运行场景", "Run scenario"),
                                12.0,
                                palette.text,
                            ))
                            .clicked()
                        {
                            run = true;
                        }
                        if ui
                            .add_enabled(
                                dirty,
                                egui::Button::new(theme::icon_label(
                                    "save",
                                    tr(self.language, "保存", "Save"),
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
                ui.horizontal(|ui| {
                    ui.checkbox(
                        &mut scenario.stop_on_failure,
                        tr(self.language, "失败时停止", "Stop on failure"),
                    );
                    let dataset = scenario.dataset_path.as_deref().unwrap_or(tr(
                        self.language,
                        "未选择数据集",
                        "No dataset",
                    ));
                    ui.label(RichText::new(dataset).small().color(palette.muted));
                    if ui
                        .button(tr(self.language, "选择 CSV/JSON", "Choose CSV/JSON"))
                        .clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("Dataset", &["csv", "json"])
                            .pick_file()
                    {
                        scenario.dataset_path = Some(path.display().to_string());
                    }
                    if scenario.dataset_path.is_some()
                        && ui.button(tr(self.language, "清除", "Clear")).clicked()
                    {
                        scenario.dataset_path = None;
                    }
                });
                if let Some(report) = report {
                    ui.separator();
                    scenario_report_view(ui, report, self.language);
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    scenario_nodes_editor(
                        ui,
                        &mut scenario.nodes,
                        &request_options,
                        self.language,
                        0,
                    );
                });
            });
        if save {
            self.save_current_scenario();
        }
        if run {
            self.run_current_scenario(ui.ctx());
        }
        if stop {
            self.stop_current_scenario();
        }
    }
}

pub(crate) fn scenario_report_view(ui: &mut egui::Ui, report: &ScenarioReport, language: Language) {
    let palette = ui.palette();
    let result_color = if report.passed {
        palette.success
    } else {
        palette.danger
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(match (language, report.passed) {
                (Language::Chinese, true) => "执行通过",
                (Language::Chinese, false) => "执行失败",
                (Language::English, true) => "Passed",
                (Language::English, false) => "Failed",
            })
            .strong()
            .color(result_color),
        );
        ui.label(
            RichText::new(format!("{} ms", report.elapsed_ms))
                .small()
                .color(palette.muted),
        );
        ui.label(
            RichText::new(format!(
                "{} {}",
                report.steps.len(),
                tr(language, "个步骤", "steps")
            ))
            .small()
            .color(palette.muted),
        );
    });
    for (index, step) in report.steps.iter().enumerate() {
        ui.push_id(("scenario_report_step", index), |ui| {
            let status = step
                .status
                .map(|status| status.to_string())
                .unwrap_or_else(|| "—".into());
            let elapsed = step
                .metrics
                .map(|metrics| format!("{} ms", metrics.elapsed_ms))
                .unwrap_or_else(|| "—".into());
            let title = format!(
                "{}  {}  ·  {}  ·  {}",
                if step.passed { "✓" } else { "✕" },
                step.name,
                status,
                elapsed
            );
            egui::CollapsingHeader::new(RichText::new(title).color(if step.passed {
                palette.success
            } else {
                palette.danger
            }))
            .show(ui, |ui| {
                if let Some(error) = &step.error {
                    ui.colored_label(palette.danger, error);
                }
                for assertion in &step.assertions {
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            if assertion.passed {
                                palette.success
                            } else {
                                palette.danger
                            },
                            if assertion.passed { "✓" } else { "✕" },
                        );
                        ui.label(&assertion.name);
                        if let Some(error) = &assertion.error {
                            ui.label(RichText::new(error).small().color(palette.muted));
                        }
                    });
                }
            });
        });
    }
}
