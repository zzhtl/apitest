use apitest_core::{EntityId, ProjectNode, ProjectNodeKind, ProtocolKind, ProtocolSpec};
use eframe::egui::{self, RichText, TextFormat, text::LayoutJob};

use crate::app::ApiTestApp;
use crate::i18n::{Language, tr};
use crate::services::document::document_snapshot;
use crate::services::history::{HISTORY_MAX_AGE_DAYS, HISTORY_MAX_RECORDS};
use crate::services::tree::TreeAction;
use crate::state::action::{Confirmation, PendingAction};
use crate::state::workspace::{ResourceRow, WorkspaceRequest};
use crate::theme::tokens::icon as icon_size;
use crate::theme::tokens::pad;
use crate::theme::{self, Palette, UiExt};
use crate::ui::editors::protocol::{protocol_color, protocol_label};
use crate::ui::history::history_state_label;
use crate::ui::request::method_color;
use crate::ui::widgets::{empty_state, icon_button, sidebar_header, sidebar_row};

impl ApiTestApp {
    pub(crate) fn api_sidebar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let mut new_protocol = None;
        let mut selection = None;
        let mut toggle_resource = None;
        let mut load_more = None;
        let mut tree_action = None;
        let mut drop_target: Option<(EntityId, Option<EntityId>)> = None;
        let resource_rows = self.visible_resource_rows();
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(pad::CHROME)
            .show(ui, |ui| {
                sidebar_header(ui, self.tr("接口管理", "APIs"), |ui| {
                    ui.menu_button(theme::icon("plus", icon_size::MD), |ui| {
                        protocol_creation_menu(ui, self.language, &mut new_protocol)
                    })
                    .response
                    .on_hover_text(self.tr("新建请求", "New request"));
                    if icon_button(ui, "folder-plus", self.tr("新建文件夹", "New folder")).clicked()
                    {
                        tree_action = Some(TreeAction::NewFolder { parent: None });
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(theme::icon("folder-open", icon_size::MD).color(palette.muted));
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
                                        let is_folder = node.kind == ProjectNodeKind::Folder;
                                        let expanded = self.expanded_resources.contains(&node.id);
                                        let request = node.entity_id.and_then(|id| {
                                            self.requests.iter().find(|request| request.id() == id)
                                        });
                                        let selected = request.is_some_and(|request| {
                                            self.requests
                                                .get(self.selected)
                                                .is_some_and(|current| current.id() == request.id())
                                        });
                                        // Every row is a drag source so it can be
                                        // moved between folders.
                                        let payload = node.id;
                                        let response = ui
                                            .dnd_drag_source(
                                                egui::Id::new(("resource", node.id)),
                                                payload,
                                                |ui| {
                                                    if is_folder {
                                                        let marker =
                                                            if expanded { "▾" } else { "▸" };
                                                        sidebar_row(
                                                            ui,
                                                            false,
                                                            format!("{marker}  {}", node.name),
                                                        );
                                                    } else if let Some(request) = request {
                                                        sidebar_row(
                                                            ui,
                                                            selected,
                                                            request_row_text(request, palette),
                                                        );
                                                    } else {
                                                        sidebar_row(ui, false, node.name.as_str());
                                                    }
                                                },
                                            )
                                            .response;
                                        if is_folder
                                            && let Some(dragged) =
                                                dropped_payload(ui, &response, node.id)
                                        {
                                            drop_target = Some((dragged, Some(node.id)));
                                        }
                                        if response.clicked() {
                                            if is_folder {
                                                toggle_resource = Some(node.id);
                                            } else if !selected
                                                && let Some(id) = node.entity_id
                                                && node.kind == ProjectNodeKind::ApiDefinition
                                            {
                                                selection = Some(id);
                                            }
                                        }
                                        self.resource_context_menu(
                                            &response,
                                            node,
                                            &mut tree_action,
                                        );
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
                        let hits = self.search_hits(&search);
                        if hits.is_empty() {
                            empty_state(
                                ui,
                                self.tr("没有匹配的接口", "No matching APIs"),
                                self.tr("换个关键词试试", "Try a different keyword"),
                            );
                        }
                        for hit in &hits {
                            let selected = self
                                .requests
                                .get(self.selected)
                                .is_some_and(|current| current.id() == hit.id);
                            let response =
                                match self.requests.iter().find(|request| request.id() == hit.id) {
                                    Some(request) => sidebar_row(
                                        ui,
                                        selected,
                                        request_row_text(request, palette),
                                    ),
                                    None => sidebar_row(ui, selected, hit.name.as_str()),
                                };
                            if response.clicked() && !selected {
                                selection = Some(hit.id);
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
        if let Some((node, parent)) = drop_target {
            self.move_resource(node, parent);
        }
        if let Some(action) = tree_action {
            self.apply_tree_action(action);
        }
    }

    /// Right-click actions for one tree row.
    fn resource_context_menu(
        &self,
        response: &egui::Response,
        node: &ProjectNode,
        action: &mut Option<TreeAction>,
    ) {
        let is_folder = node.kind == ProjectNodeKind::Folder;
        egui::containers::Popup::context_menu(response).show(|ui| {
            ui.set_min_width(180.0);
            if is_folder
                && ui
                    .button(theme::icon_label(
                        "folder-plus",
                        self.tr("新建子文件夹", "New subfolder"),
                        icon_size::SM,
                        ui.palette().text,
                    ))
                    .clicked()
            {
                *action = Some(TreeAction::NewFolder {
                    parent: Some(node.id),
                });
                ui.close();
            }
            if ui
                .button(theme::icon_label(
                    "pencil",
                    self.tr("重命名", "Rename"),
                    icon_size::SM,
                    ui.palette().text,
                ))
                .clicked()
            {
                *action = Some(TreeAction::Rename {
                    node: node.id,
                    name: node.name.clone(),
                });
                ui.close();
            }
            if !is_folder
                && let Some(entity_id) = node.entity_id
                && ui
                    .button(theme::icon_label(
                        "copy",
                        self.tr("复制请求", "Duplicate request"),
                        icon_size::SM,
                        ui.palette().text,
                    ))
                    .clicked()
            {
                *action = Some(TreeAction::Duplicate { entity_id });
                ui.close();
            }
            ui.separator();
            if ui
                .button(RichText::new(self.tr("删除", "Delete")).color(ui.palette().danger))
                .clicked()
            {
                *action = Some(if is_folder {
                    TreeAction::DeleteFolder { node: node.id }
                } else {
                    match node.entity_id {
                        Some(entity_id) => TreeAction::DeleteRequest { entity_id },
                        None => TreeAction::DeleteFolder { node: node.id },
                    }
                });
                ui.close();
            }
        });
    }

    pub(crate) fn scenario_sidebar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let mut select = None;
        let mut create = false;
        let mut delete = None;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(pad::CHROME)
            .show(ui, |ui| {
                sidebar_header(ui, self.tr("测试场景", "Scenarios"), |ui| {
                    if icon_button(ui, "plus", self.tr("新建场景", "New scenario")).clicked() {
                        create = true;
                    }
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
                let row = sidebar_row(ui, index == self.selected_scenario, label);
                if row.clicked() {
                    select = Some(scenario.id);
                }
                let id = scenario.id;
                egui::containers::Popup::context_menu(&row).show(|ui| {
                    if ui
                        .button(RichText::new(self.tr("删除", "Delete")).color(ui.palette().danger))
                        .clicked()
                    {
                        delete = Some(id);
                        ui.close();
                    }
                });
            }
        });
        if create {
            self.queue_action(PendingAction::NewScenario);
        }
        if let Some(id) = select {
            self.queue_action(PendingAction::SelectScenario(id));
        }
        if let Some(id) = delete {
            self.confirmation = Some(Confirmation::DeleteScenario(id));
        }
    }

    pub(crate) fn mock_sidebar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let mut select = None;
        let mut create = false;
        let mut delete = None;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(pad::CHROME)
            .show(ui, |ui| {
                sidebar_header(ui, self.tr("Mock 服务", "Mock servers"), |ui| {
                    if icon_button(ui, "plus", self.tr("新建 Mock", "New mock")).clicked() {
                        create = true;
                    }
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
                let row = sidebar_row(ui, index == self.selected_mock, label);
                if row.clicked() {
                    select = Some(profile.id);
                }
                let id = profile.id;
                egui::containers::Popup::context_menu(&row).show(|ui| {
                    if ui
                        .button(RichText::new(self.tr("删除", "Delete")).color(ui.palette().danger))
                        .clicked()
                    {
                        delete = Some(id);
                        ui.close();
                    }
                });
            }
        });
        if create {
            self.queue_action(PendingAction::NewMock);
        }
        if let Some(id) = select {
            self.queue_action(PendingAction::SelectMock(id));
        }
        if let Some(id) = delete {
            self.confirmation = Some(Confirmation::DeleteMock(id));
        }
    }

    pub(crate) fn history_sidebar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let mut selection = None;
        let mut refresh = false;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(pad::CHROME)
            .show(ui, |ui| {
                sidebar_header(ui, self.tr("运行历史", "Run history"), |ui| {
                    if icon_button(ui, "refresh-cw", self.tr("刷新历史", "Refresh history"))
                        .clicked()
                    {
                        refresh = true;
                    }
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
                    .unwrap_or(self.tr("已删除的请求", "Deleted request"));
                let state = history_state_label(record.state, self.language);
                let label = format!(
                    "{}  {}\n{}  ·  {} ms",
                    state,
                    request_name,
                    record.started_at.format("%m-%d %H:%M:%S"),
                    record.elapsed_ms
                );
                if sidebar_row(ui, index == self.selected_history, label).clicked() {
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

    pub(crate) fn environment_sidebar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let mut new_environment = false;
        let mut selection = None;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(pad::CHROME)
            .show(ui, |ui| {
                sidebar_header(ui, self.tr("环境管理", "Environments"), |ui| {
                    if icon_button(ui, "plus", self.tr("新建环境", "New environment")).clicked()
                    {
                        new_environment = true;
                    }
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
                    let response = sidebar_row(
                        ui,
                        selected,
                        RichText::new(format!("{}{}{}", environment.name, marker, dirty)),
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
    ui.label(tr(language, "新建接口", "New API"));
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

/// The node a drag released over `response`, ignoring self-drops.
fn dropped_payload(ui: &egui::Ui, response: &egui::Response, target: EntityId) -> Option<EntityId> {
    if !response.contains_pointer() || !ui.input(|input| input.pointer.any_released()) {
        return None;
    }
    let payload = egui::DragAndDrop::take_payload::<EntityId>(ui.ctx())?;
    (*payload != target).then_some(*payload)
}
