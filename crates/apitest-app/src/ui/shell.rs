use apitest_core::ProtocolKind;
use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke};

use crate::app::{ApiTestApp, THEME_SETTING};
use crate::draft::BodyMode;
use crate::i18n::Language;
use crate::services::loader::active_environment_setting;
use crate::services::tree::TreeAction;
use crate::state::action::{InteropAction, PendingAction, ToastKind};
use crate::state::response::ResponseTab;
use crate::state::workspace::Navigation;
use crate::theme::tokens::icon as icon_size;
use crate::theme::{self, ThemeMode, UiExt};
use crate::ui::request::response::RESPONSE_FIND_FIELD_ID;
use crate::ui::sidebar::protocol_creation_menu;
use crate::ui::widgets::{icon_button, rail_button};

pub(crate) const SEARCH_FIELD_ID: &str = "global_api_search";

impl ApiTestApp {
    /// The shortcut table, also rendered in the settings window.
    pub(crate) const SHORTCUTS: &'static [(&'static str, &'static str, &'static str)] = &[
        ("Ctrl Enter", "发送请求", "Send request"),
        ("F5", "重新发送", "Resend"),
        ("Shift F5", "停止请求", "Stop the run"),
        ("Ctrl S", "保存", "Save"),
        ("Ctrl N", "新建请求", "New request"),
        ("Ctrl D", "复制当前请求", "Duplicate request"),
        ("Ctrl W", "关闭当前标签", "Close tab"),
        ("Ctrl Tab", "下一个标签", "Next tab"),
        ("Ctrl Shift Tab", "上一个标签", "Previous tab"),
        ("Ctrl 1..9", "切换到第 N 个标签", "Jump to tab N"),
        ("Ctrl F", "在响应中查找", "Find in response"),
        ("Ctrl L", "聚焦接口筛选", "Focus the API filter"),
        ("Ctrl Shift F", "格式化 JSON 请求体", "Format the JSON body"),
        ("Ctrl K", "命令面板", "Command palette"),
        ("Esc", "关闭弹层", "Dismiss overlays"),
    ];

    pub(crate) fn keyboard_shortcuts(&mut self, context: &egui::Context) {
        if self.confirmation.is_some() {
            return;
        }
        // The palette owns the keyboard while it is open.
        if self.show_palette || self.rename_target.is_some() {
            return;
        }
        // A focused text field owns plain keys: Esc must release focus instead
        // of dismissing overlays, and F5 must not resend mid-edit. Modified
        // chords (Ctrl …) stay live because they never insert text.
        let typing = context.egui_wants_keyboard_input();
        let keys = context.input(|input| {
            let command = input.modifiers.command;
            let shift = input.modifiers.shift;
            Keys {
                send: command && input.key_pressed(egui::Key::Enter),
                resend: !typing && !shift && input.key_pressed(egui::Key::F5),
                stop: shift && input.key_pressed(egui::Key::F5),
                save: command && input.key_pressed(egui::Key::S),
                new_request: command && !shift && input.key_pressed(egui::Key::N),
                duplicate: command && input.key_pressed(egui::Key::D),
                close_tab: command && input.key_pressed(egui::Key::W),
                next_tab: command && !shift && input.key_pressed(egui::Key::Tab),
                previous_tab: command && shift && input.key_pressed(egui::Key::Tab),
                find: command && !shift && input.key_pressed(egui::Key::F),
                focus_search: command && input.key_pressed(egui::Key::L),
                format: command && shift && input.key_pressed(egui::Key::F),
                palette: command && input.key_pressed(egui::Key::K),
                escape: !typing && input.key_pressed(egui::Key::Escape),
                tab_jump: [
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                    egui::Key::Num6,
                    egui::Key::Num7,
                    egui::Key::Num8,
                    egui::Key::Num9,
                ]
                .iter()
                .position(|key| command && input.key_pressed(*key)),
            }
        });
        if keys.palette {
            self.show_palette = true;
            self.palette_query.clear();
            return;
        }
        if keys.escape {
            self.show_settings = false;
            self.show_snippet = false;
            self.show_curl_import = false;
            self.show_openapi_preview = false;
        }
        if (keys.send || keys.resend) && self.navigation == Navigation::Api {
            self.send_current(context);
        }
        if keys.stop {
            self.stop();
        }
        if keys.save {
            self.save_current();
        }
        if keys.new_request {
            self.queue_action(PendingAction::NewRequest(ProtocolKind::Http));
        }
        if keys.duplicate
            && let Some(document) = self.active_api_document()
        {
            self.apply_tree_action(TreeAction::Duplicate {
                entity_id: document.entity_id,
            });
        }
        if keys.close_tab
            && let Some(active) = self.document_tabs.active()
        {
            self.queue_action(PendingAction::CloseDocument(active));
        }
        if keys.next_tab || keys.previous_tab {
            self.cycle_document(keys.next_tab);
        }
        if let Some(index) = keys.tab_jump
            && let Some(tab) = self.document_tabs.items().get(index)
        {
            let id = tab.id;
            self.activate_document(id);
        }
        if keys.find && self.navigation == Navigation::Api {
            self.session_mut().response_tab = ResponseTab::Body;
            context
                .memory_mut(|memory| memory.request_focus(egui::Id::new(RESPONSE_FIND_FIELD_ID)));
        }
        if keys.focus_search {
            context.memory_mut(|memory| memory.request_focus(egui::Id::new(SEARCH_FIELD_ID)));
        }
        if keys.format {
            self.format_current_body();
        }
    }

    /// Move to the neighbouring tab, wrapping at either end.
    fn cycle_document(&mut self, forward: bool) {
        let tabs = self.document_tabs.items().to_vec();
        if tabs.len() < 2 {
            return;
        }
        let current = self
            .document_tabs
            .active()
            .and_then(|active| tabs.iter().position(|tab| tab.id == active))
            .unwrap_or_default();
        let next = if forward {
            (current + 1) % tabs.len()
        } else {
            (current + tabs.len() - 1) % tabs.len()
        };
        self.activate_document(tabs[next].id);
    }

    fn format_current_body(&mut self) {
        let Some(request) = self.requests.get_mut(self.selected) else {
            return;
        };
        if request.draft.body_mode != BodyMode::Json {
            return;
        }
        match serde_json::from_str::<serde_json::Value>(&request.draft.body)
            .and_then(|value| serde_json::to_string_pretty(&value))
        {
            Ok(pretty) => request.draft.body = pretty,
            Err(error) => self.toast(
                ToastKind::Error,
                match self.language {
                    Language::Chinese => format!("JSON 格式错误：{error}"),
                    Language::English => format!("Invalid JSON: {error}"),
                },
            ),
        }
    }

    pub(crate) fn toggle_theme(&mut self, context: &egui::Context) {
        self.theme = match self.theme {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        };
        theme::apply(context, self.theme);
        let theme = self.theme;
        self.persist_setting(THEME_SETTING, &theme);
    }

    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let search_hint = self.tr("筛选接口", "Filter APIs");
        let palette_tip = self.tr("命令面板  Ctrl K", "Command palette  Ctrl K");
        let new_label = self.tr("新建", "New");
        let settings_tip = self.tr("设置", "Settings");
        let theme_tip = self.tr("切换主题", "Toggle theme");
        let mut new_protocol = None;
        let mut toggle_theme = false;
        let mut show_settings = false;
        let mut selected_environment = None;
        let mut selected_project = None;
        let mut new_project = false;
        let mut interop_action = None;
        let mut open_palette = false;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let logo = egui::Button::new(RichText::new("A").strong().color(Color32::WHITE))
                        .fill(palette.primary)
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(4));
                    ui.add_sized([28.0, 28.0], logo);
                    ui.label(RichText::new("ApiTest").size(16.0).strong());
                    ui.separator();
                    egui::ComboBox::from_id_salt("active_project")
                        .selected_text(&self.project.name)
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            for project in &self.projects {
                                if ui
                                    .selectable_label(project.id == self.project.id, &project.name)
                                    .clicked()
                                {
                                    selected_project = Some(project.id);
                                }
                            }
                            ui.separator();
                            if ui
                                .button(theme::icon_label(
                                    "plus",
                                    self.tr("新建项目", "New project"),
                                    12.0,
                                    palette.text,
                                ))
                                .clicked()
                            {
                                new_project = true;
                            }
                        });
                    ui.add_space(8.0);
                    ui.add_sized(
                        [200.0, 32.0],
                        egui::TextEdit::singleline(&mut self.search)
                            // A fixed id so Ctrl+L can focus the field from anywhere.
                            .id(egui::Id::new(SEARCH_FIELD_ID))
                            .hint_text(search_hint),
                    );
                    if icon_button(ui, "search", palette_tip).clicked() {
                        open_palette = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [32.0, 32.0],
                                egui::Button::new(theme::icon("settings", icon_size::LG)),
                            )
                            .on_hover_text(settings_tip)
                            .clicked()
                        {
                            show_settings = true;
                        }
                        let theme_icon = match self.theme {
                            ThemeMode::Dark => "sun",
                            ThemeMode::Light => "moon",
                        };
                        if ui
                            .add_sized(
                                [32.0, 32.0],
                                egui::Button::new(theme::icon(theme_icon, 15.0)),
                            )
                            .on_hover_text(theme_tip)
                            .clicked()
                        {
                            toggle_theme = true;
                        }
                        ui.menu_button(
                            theme::icon_label("plus", new_label, 13.0, Color32::WHITE),
                            |ui| protocol_creation_menu(ui, self.language, &mut new_protocol),
                        );
                        ui.menu_button(self.tr("导入 / 导出", "Import / Export"), |ui| {
                            ui.label(RichText::new(self.tr("导入", "Import")).strong());
                            for (action, chinese, english) in [
                                (InteropAction::ImportOpenApi, "OpenAPI 文件", "OpenAPI file"),
                                (
                                    InteropAction::ImportPostman,
                                    "Postman 集合",
                                    "Postman collection",
                                ),
                                (InteropAction::ImportHar, "HAR 文件", "HAR file"),
                                (InteropAction::ImportCurl, "粘贴 cURL", "Paste cURL"),
                            ] {
                                if ui
                                    .button(match self.language {
                                        Language::Chinese => chinese,
                                        Language::English => english,
                                    })
                                    .clicked()
                                {
                                    interop_action = Some(action);
                                    ui.close();
                                }
                            }
                            ui.separator();
                            ui.label(RichText::new(self.tr("导出", "Export")).strong());
                            for (action, chinese, english) in [
                                (
                                    InteropAction::PreviewOpenApi,
                                    "OpenAPI 设计与预览",
                                    "OpenAPI design & preview",
                                ),
                                (InteropAction::ExportOpenApi, "OpenAPI YAML", "OpenAPI YAML"),
                                (
                                    InteropAction::ExportPostman,
                                    "Postman 集合",
                                    "Postman collection",
                                ),
                                (InteropAction::ExportHar, "HAR 文件", "HAR file"),
                                (
                                    InteropAction::ExportCurl,
                                    "当前请求 cURL",
                                    "Current request cURL",
                                ),
                                (
                                    InteropAction::ExportDocumentation,
                                    "静态 HTML 文档",
                                    "Static HTML documentation",
                                ),
                            ] {
                                if ui
                                    .button(match self.language {
                                        Language::Chinese => chinese,
                                        Language::English => english,
                                    })
                                    .clicked()
                                {
                                    interop_action = Some(action);
                                    ui.close();
                                }
                            }
                        });
                        let active_name = self
                            .environments
                            .iter()
                            .find(|environment| environment.id() == self.active_environment)
                            .map(|environment| environment.name.as_str())
                            .unwrap_or("-");
                        egui::ComboBox::from_id_salt("active_environment")
                            .selected_text(theme::icon_label(
                                "globe",
                                active_name,
                                12.0,
                                palette.text,
                            ))
                            .width(126.0)
                            .show_ui(ui, |ui| {
                                for environment in &self.environments {
                                    if ui
                                        .selectable_label(
                                            environment.id() == self.active_environment,
                                            &environment.name,
                                        )
                                        .clicked()
                                    {
                                        selected_environment = Some(environment.id());
                                    }
                                }
                            });
                    });
                });
            });
        if let Some(kind) = new_protocol {
            self.queue_action(PendingAction::NewRequest(kind));
        }
        if let Some(id) = selected_project
            && id != self.project.id
        {
            self.queue_action(PendingAction::SelectProject(id));
        }
        if new_project {
            self.queue_action(PendingAction::NewProject);
        }
        if toggle_theme {
            self.toggle_theme(ui.ctx());
        }
        if show_settings {
            self.show_settings = true;
        }
        if let Some(id) = selected_environment {
            self.active_environment = id;
            let key = active_environment_setting(self.project.id);
            self.persist_setting(&key, &id);
        }
        if let Some(action) = interop_action {
            self.handle_interop_action(action);
        }
        if open_palette {
            self.show_palette = true;
            self.palette_query.clear();
        }
    }

    pub(crate) fn activity_bar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let mut navigate = None;
        egui::Frame::new()
            .fill(palette.rail_bg)
            .inner_margin(egui::Margin::symmetric(6, 10))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    for (target, icon, label, tooltip) in [
                        (Navigation::Api, "braces", "API", self.tr("接口", "APIs")),
                        (
                            Navigation::Scenario,
                            "workflow",
                            "RUN",
                            self.tr("测试场景", "Scenarios"),
                        ),
                        (
                            Navigation::Mock,
                            "server",
                            "MOCK",
                            self.tr("Mock 服务", "Mock servers"),
                        ),
                        (
                            Navigation::History,
                            "history",
                            "HIST",
                            self.tr("运行历史", "Run history"),
                        ),
                        (
                            Navigation::Environment,
                            "database",
                            "ENV",
                            self.tr("环境", "Environments"),
                        ),
                    ] {
                        let selected = self.navigation == target;
                        if rail_button(ui, icon, label, tooltip, selected).clicked() {
                            navigate = Some(target);
                        }
                    }
                });
            });
        if let Some(navigation) = navigate
            && navigation != self.navigation
        {
            self.queue_action(PendingAction::Navigate(navigation));
        }
    }

    pub(crate) fn document_tab_bar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let tabs = self.document_tabs.items().to_vec();
        let active = self.document_tabs.active();
        let mut activate = None;
        let mut close = None;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("document_tabs")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for tab in &tabs {
                                ui.push_id((tab.id.kind, tab.id.entity_id), |ui| {
                                    let selected = active == Some(tab.id);
                                    let dirty = self.document_dirty(tab.id);
                                    egui::Frame::new()
                                        .fill(if selected {
                                            palette.primary_soft
                                        } else {
                                            Color32::TRANSPARENT
                                        })
                                        .corner_radius(CornerRadius::same(4))
                                        .inner_margin(egui::Margin::symmetric(8, 3))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                let title = if dirty {
                                                    format!("{}  •", tab.title)
                                                } else {
                                                    tab.title.clone()
                                                };
                                                let title_button = ui.add(
                                                    egui::Button::new(title)
                                                        .frame(false)
                                                        .selected(selected)
                                                        .sense(
                                                            egui::Sense::click()
                                                                .union(egui::Sense::hover()),
                                                        ),
                                                );
                                                if title_button.clicked() {
                                                    activate = Some(tab.id);
                                                }
                                                // Middle-click closes the tab,
                                                // as in a browser.
                                                if title_button
                                                    .clicked_by(egui::PointerButton::Middle)
                                                {
                                                    close = Some(tab.id);
                                                }
                                                if ui
                                                    .add_sized(
                                                        [20.0, 20.0],
                                                        egui::Button::new(theme::icon(
                                                            "x",
                                                            icon_size::SM,
                                                        ))
                                                        .frame(false),
                                                    )
                                                    .on_hover_text(self.tr("关闭", "Close"))
                                                    .clicked()
                                                {
                                                    close = Some(tab.id);
                                                }
                                            });
                                        });
                                });
                            }
                        });
                    });
            });
        if let Some(id) = activate {
            self.activate_document(id);
        }
        if let Some(id) = close {
            self.activate_document(id);
            self.queue_action(PendingAction::CloseDocument(id));
        }
    }
}

/// The shortcut chords pressed this frame.
struct Keys {
    send: bool,
    resend: bool,
    stop: bool,
    save: bool,
    new_request: bool,
    duplicate: bool,
    close_tab: bool,
    next_tab: bool,
    previous_tab: bool,
    find: bool,
    focus_search: bool,
    format: bool,
    palette: bool,
    escape: bool,
    /// Zero-based tab index selected via Ctrl+1..9.
    tab_jump: Option<usize>,
}
