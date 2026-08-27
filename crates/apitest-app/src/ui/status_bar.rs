use eframe::egui::{self, RichText};

use crate::app::ApiTestApp;
use crate::state::response::RunState;
use crate::theme::tokens::icon as icon_size;
use crate::theme::{self, UiExt};
use crate::ui::widgets::{Tone, format_bytes};

impl ApiTestApp {
    /// A one-line summary of what the workspace is doing right now.
    ///
    /// The project, environment, save state and last-run result used to be
    /// scattered across three panels or absent entirely.
    pub(crate) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let dirty = self.workspace_dirty();
        let environment = self
            .environments
            .iter()
            .find(|environment| environment.id() == self.active_environment)
            .map(|environment| environment.name.clone())
            .unwrap_or_else(|| "-".to_owned());
        let mock = self.mock_server.as_ref().map(|server| server.base_url());
        let response = &self.session().response;
        let run = match response.state {
            RunState::Idle => None,
            RunState::Running => Some((self.tr("请求中…", "Running…").to_owned(), Tone::Info)),
            RunState::Cancelling => {
                Some((self.tr("取消中…", "Cancelling…").to_owned(), Tone::Warning))
            }
            RunState::Completed => Some((
                format!(
                    "{} · {}",
                    response
                        .status
                        .map(|status| status.to_string())
                        .unwrap_or_else(|| "—".into()),
                    response
                        .metrics
                        .map(|metrics| format!("{} ms", metrics.elapsed_ms))
                        .unwrap_or_default()
                ),
                Tone::Success,
            )),
            RunState::Failed => Some((self.tr("失败", "Failed").to_owned(), Tone::Danger)),
            RunState::Cancelled => Some((self.tr("已取消", "Cancelled").to_owned(), Tone::Warning)),
        };
        let received = response
            .metrics
            .map(|metrics| format_bytes(metrics.received_bytes));

        egui::Frame::new()
            .fill(palette.rail_bg)
            .inner_margin(egui::Margin::symmetric(12, 3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::icon("folder-open", icon_size::SM).color(palette.muted));
                    ui.label(RichText::new(&self.project.name).small());
                    ui.separator();
                    ui.label(theme::icon("globe", icon_size::SM).color(palette.muted));
                    ui.label(RichText::new(environment).small());
                    if let Some(mock) = mock {
                        ui.separator();
                        ui.label(
                            RichText::new(format!("Mock {mock}"))
                                .small()
                                .color(palette.success),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.storage_worker.is_none() {
                            // Without a database nothing can be saved; a persistent
                            // notice beats the toast the failed save would spam.
                            ui.label(
                                RichText::new(
                                    self.tr("本地存储不可用", "Local storage unavailable"),
                                )
                                .small()
                                .color(palette.danger),
                            );
                        } else {
                            ui.label(
                                RichText::new(if dirty {
                                    self.tr("有未保存的更改", "Unsaved changes")
                                } else {
                                    self.tr("已保存", "Saved")
                                })
                                .small()
                                .color(if dirty {
                                    palette.warning
                                } else {
                                    palette.muted
                                }),
                            );
                        }
                        if let Some((label, tone)) = run {
                            ui.separator();
                            ui.label(RichText::new(label).small().color(tone.foreground(palette)));
                        }
                        if let Some(received) = received {
                            ui.separator();
                            ui.label(
                                RichText::new(format!("↓ {received}"))
                                    .small()
                                    .color(palette.muted),
                            );
                        }
                    });
                });
            });
    }
}
