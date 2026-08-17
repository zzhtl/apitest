use apitest_core::{ProjectNodeKind, ProtocolKind, ProtocolSpec};
use eframe::egui::{self, Color32, RichText, Stroke, TextFormat, text::LayoutJob};

use crate::app::ApiTestApp;
use crate::i18n::Language;
use crate::services::document::document_snapshot;
use crate::services::history::{HISTORY_MAX_AGE_DAYS, HISTORY_MAX_RECORDS};
use crate::state::action::PendingAction;
use crate::state::workspace::{ResourceRow, WorkspaceRequest};
use crate::theme::{self, Palette};
use crate::ui::editors::protocol::{protocol_color, protocol_label};
use crate::ui::history::history_state_label;
use crate::ui::request::method_color;

impl ApiTestApp {
    pub(crate) fn api_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut new_protocol = None;
        let mut selection = None;
        let mut toggle_resource = None;
        let mut load_more = None;
        let resource_rows = self.visible_resource_rows();
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(10, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.tr("接口管理", "APIs"))
                            .strong()
                            .size(14.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button(theme::icon("plus", 14.0), |ui| {
                            protocol_creation_menu(ui, self.language, &mut new_protocol)
                        })
                        .response
                        .on_hover_text(self.tr("新建请求", "New request"));
                    });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(theme::icon("folder-open", 14.0).color(palette.muted));
                    ui.label(RichText::new(&self.project.name).strong());
                });
                ui.add_space(6.0);
                let search = self.search.to_lowercase();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if search.is_empty() {
                        for row in &resource_rows {
                            match row {
                                ResourceRow::Node { node, depth } => {
                                    ui.horizontal(|ui| {
                                        ui.add_space(*depth as f32 * 14.0);
                                        if node.kind == ProjectNodeKind::Folder {
                                            let marker =
                                                if self.expanded_resources.contains(&node.id) {
                                                    "▾"
                                                } else {
                                                    "▸"
                                                };
                                            if ui
                                                .add_sized(
                                                    [ui.available_width(), 32.0],
                                                    egui::Button::new(format!(
                                                        "{marker}  {}",
                                                        node.name
                                                    ))
                                                    .fill(Color32::TRANSPARENT)
                                                    .stroke(Stroke::NONE),
                                                )
                                                .clicked()
                                            {
                                                toggle_resource = Some(node.id);
                                            }
                                            return;
                                        }
                                        let request = node.entity_id.and_then(|id| {
                                            self.requests.iter().find(|request| request.id() == id)
                                        });
                                        let selected = request.is_some_and(|request| {
                                            self.requests
                                                .get(self.selected)
                                                .is_some_and(|current| current.id() == request.id())
                                        });
                                        let response = if let Some(request) = request {
                                            ui.add_sized(
                                                [ui.available_width(), 34.0],
                                                egui::Button::new(request_row_text(
                                                    request, palette,
                                                ))
                                                .selected(selected)
                                                .fill(if selected {
                                                    palette.primary_soft
                                                } else {
                                                    Color32::TRANSPARENT
                                                })
                                                .stroke(Stroke::NONE),
                                            )
                                        } else {
                                            ui.add_sized(
                                                [ui.available_width(), 34.0],
                                                egui::Button::new(&node.name)
                                                    .fill(Color32::TRANSPARENT)
                                                    .stroke(Stroke::NONE),
                                            )
                                        };
                                        if response.clicked()
                                            && !selected
                                            && let Some(id) = node.entity_id
                                            && node.kind == ProjectNodeKind::ApiDefinition
                                        {
                                            selection = Some(id);
                                        }
                                    });
                                }
                                ResourceRow::More { parent_id, depth } => {
                                    ui.horizontal(|ui| {
                                        ui.add_space(*depth as f32 * 14.0);
                                        if ui.button(self.tr("加载更多…", "Load more…")).clicked()
                                        {
                                            load_more = Some(*parent_id);
                                        }
                                    });
                                }
                            }
                        }
                    } else {
                        for request in &self.requests {
                            if !request.name.to_lowercase().contains(&search)
                                && !request.endpoint().to_lowercase().contains(&search)
                                && !protocol_label(request.protocol_kind())
                                    .to_lowercase()
                                    .contains(&search)
                            {
                                continue;
                            }
                            let selected = self
                                .requests
                                .get(self.selected)
                                .is_some_and(|current| current.id() == request.id());
                            let response = ui.add_sized(
                                [ui.available_width(), 34.0],
                                egui::Button::new(request_row_text(request, palette))
                                    .selected(selected)
                                    .fill(if selected {
                                        palette.primary_soft
                                    } else {
                                        Color32::TRANSPARENT
                                    })
                                    .stroke(Stroke::NONE),
                            );
                            if response.clicked() && !selected {
                                selection = Some(request.id());
                            }
                        }
                    }
                });
            });
        if let Some(kind) = new_protocol {
            self.queue_action(PendingAction::NewRequest(kind));
        }
        if let Some(id) = selection {
            self.queue_action(PendingAction::SelectRequest(id));
        }
        if let Some(id) = toggle_resource {
            self.toggle_resource(id);
        }
        if let Some(parent_id) = load_more {
            self.load_more_resources(parent_id);
        }
    }

    pub(crate) fn scenario_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut select = None;
        let mut create = false;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.tr("测试场景", "Scenarios")).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([28.0, 28.0], egui::Button::new(theme::icon("plus", 14.0)))
                            .on_hover_text(self.tr("新建场景", "New scenario"))
                            .clicked()
                        {
                            create = true;
                        }
                    });
                });
            });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, scenario) in self.scenarios.iter().enumerate() {
                let dirty = self
                    .scenario_snapshots
                    .get(&scenario.id)
                    .is_none_or(|saved| saved != &document_snapshot(scenario));
                let label = if dirty {
                    format!("{}  •", scenario.name)
                } else {
                    scenario.name.clone()
                };
                if ui
                    .selectable_label(index == self.selected_scenario, label)
                    .clicked()
                {
                    select = Some(scenario.id);
                }
            }
        });
        if create {
            self.queue_action(PendingAction::NewScenario);
        }
        if let Some(id) = select {
            self.queue_action(PendingAction::SelectScenario(id));
        }
    }

    pub(crate) fn mock_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut select = None;
        let mut create = false;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.tr("Mock 服务", "Mock servers")).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([28.0, 28.0], egui::Button::new(theme::icon("plus", 14.0)))
                            .on_hover_text(self.tr("新建 Mock", "New mock"))
                            .clicked()
                        {
                            create = true;
                        }
                    });
                });
            });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, profile) in self.mock_profiles.iter().enumerate() {
                let dirty = self
                    .mock_snapshots
                    .get(&profile.id)
                    .is_none_or(|saved| saved != &document_snapshot(profile));
                let label = if dirty {
                    format!("{}  •", profile.name)
                } else {
                    profile.name.clone()
                };
                if ui
                    .selectable_label(index == self.selected_mock, label)
                    .clicked()
                {
                    select = Some(profile.id);
                }
            }
        });
        if create {
            self.queue_action(PendingAction::NewMock);
        }
        if let Some(id) = select {
            self.queue_action(PendingAction::SelectMock(id));
        }
    }

    pub(crate) fn history_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut selection = None;
        let mut refresh = false;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.tr("运行历史", "Run history")).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [28.0, 28.0],
                                egui::Button::new(theme::icon("refresh-cw", 13.0)),
                            )
                            .on_hover_text(self.tr("刷新历史", "Refresh history"))
                            .clicked()
                        {
                            refresh = true;
                        }
                    });
                });
                ui.label(
                    RichText::new(match self.language {
                        Language::Chinese => {
                            format!("保留最近 {HISTORY_MAX_RECORDS} 条 / {HISTORY_MAX_AGE_DAYS} 天")
                        }
                        Language::English => format!(
                            "Keeps {HISTORY_MAX_RECORDS} records / {HISTORY_MAX_AGE_DAYS} days"
                        ),
                    })
                    .small()
                    .color(palette.muted),
                );
            });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, record) in self.run_records.iter().enumerate() {
                let request_name = record
                    .request_case_id
                    .and_then(|case_id| {
                        self.requests
                            .iter()
                            .find(|request| request.request_case.id == case_id)
                    })
                    .map(|request| request.name.as_str())
                    .unwrap_or(match self.language {
                        Language::Chinese => "已删除的请求",
                        Language::English => "Deleted request",
                    });
                let state = history_state_label(record.state, self.language);
                let label = format!(
                    "{}  {}\n{}  ·  {} ms",
                    state,
                    request_name,
                    record.started_at.format("%m-%d %H:%M:%S"),
                    record.elapsed_ms
                );
                if ui
                    .selectable_label(index == self.selected_history, label)
                    .clicked()
                {
                    selection = Some(index);
                }
            }
        });
        if refresh {
            self.reload_run_history();
        }
        if let Some(index) = selection {
            self.select_run_history(index);
        }
    }

    pub(crate) fn environment_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut new_environment = false;
        let mut selection = None;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(10, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.tr("环境管理", "Environments"))
                            .strong()
                            .size(14.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([28.0, 28.0], egui::Button::new(theme::icon("plus", 14.0)))
                            .on_hover_text(self.tr("新建环境", "New environment"))
                            .clicked()
                        {
                            new_environment = true;
                        }
                    });
                });
                ui.add_space(10.0);
                for (index, environment) in self.environments.iter().enumerate() {
                    let selected = index == self.selected_environment;
                    let marker = if environment.id() == self.active_environment {
                        "  ●"
                    } else {
                        ""
                    };
                    let dirty = if environment.is_dirty() { "  •" } else { "" };
                    let response = ui.add_sized(
                        [ui.available_width(), 34.0],
                        egui::Button::new(RichText::new(format!(
                            "{}{}{}",
                            environment.name, marker, dirty
                        )))
                        .selected(selected)
                        .fill(if selected {
                            palette.primary_soft
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE),
                    );
                    if response.clicked() && !selected {
                        selection = Some(environment.id());
                    }
                }
            });
        if new_environment {
            self.queue_action(PendingAction::NewEnvironment);
        }
        if let Some(id) = selection {
            self.queue_action(PendingAction::SelectEnvironment(id));
        }
    }
}

pub(crate) fn protocol_creation_menu(
    ui: &mut egui::Ui,
    language: Language,
    selected: &mut Option<ProtocolKind>,
) {
    ui.set_min_width(180.0);
    ui.label(match language {
        Language::Chinese => "新建接口",
        Language::English => "New API",
    });
    ui.separator();
    for (kind, label) in [
        (ProtocolKind::Http, "HTTP"),
        (ProtocolKind::GraphQl, "GraphQL"),
        (ProtocolKind::Sse, "SSE"),
        (ProtocolKind::WebSocket, "WebSocket"),
        (ProtocolKind::Grpc, "gRPC"),
    ] {
        if ui.button(label).clicked() {
            *selected = Some(kind);
            ui.close();
        }
    }
}

pub(crate) fn request_row_text(request: &WorkspaceRequest, palette: Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let (prefix, color) = match request.alternate_protocol.as_ref() {
        None | Some(ProtocolSpec::Http(_)) => (
            request.draft.method.to_string(),
            method_color(request.draft.method, palette),
        ),
        Some(protocol) => (
            protocol_label(protocol.kind()).to_owned(),
            protocol_color(protocol.kind(), palette),
        ),
    };
    job.append(
        &prefix,
        0.0,
        TextFormat {
            font_id: egui::FontId::new(11.0, egui::FontFamily::Proportional),
            color,
            ..Default::default()
        },
    );
    job.append(
        &format!(
            "  {}{}",
            request.name,
            if request.is_dirty() { "  •" } else { "" }
        ),
        0.0,
        TextFormat {
            font_id: egui::FontId::new(13.0, egui::FontFamily::Proportional),
            color: palette.text,
            ..Default::default()
        },
    );
    job
}
