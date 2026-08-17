use apitest_core::ProtocolKind;
use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke};

use crate::app::{ApiTestApp, THEME_SETTING};
use crate::i18n::Language;
use crate::services::loader::active_environment_setting;
use crate::state::action::{InteropAction, PendingAction};
use crate::state::workspace::Navigation;
use crate::theme::{self, ThemeMode, UiExt};
use crate::ui::sidebar::protocol_creation_menu;

pub(crate) const SEARCH_FIELD_ID: &str = "global_api_search";

impl ApiTestApp {
    pub(crate) fn keyboard_shortcuts(&mut self, context: &egui::Context) {
        if self.confirmation.is_some() {
            return;
        }
        let (send, save, new_request, search) = context.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::Enter),
                input.modifiers.command && input.key_pressed(egui::Key::S),
                input.modifiers.command && input.key_pressed(egui::Key::N),
                input.modifiers.command && input.key_pressed(egui::Key::K),
            )
        });
        if search {
            self.navigation = Navigation::Api;
            context.memory_mut(|memory| memory.request_focus(egui::Id::new(SEARCH_FIELD_ID)));
        }
        if send && self.navigation == Navigation::Api {
            self.send_current(context);
        }
        if save {
            self.save_current();
        }
        if new_request {
            self.queue_action(PendingAction::NewRequest(ProtocolKind::Http));
        }
    }

    pub(crate) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let palette = ui.palette();
        let search_hint = self.tr("搜索接口  Ctrl K", "Search APIs  Ctrl K");
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
                        [220.0, 32.0],
                        egui::TextEdit::singleline(&mut self.search)
                            .id_source(SEARCH_FIELD_ID)
                            .hint_text(search_hint),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [32.0, 32.0],
                                egui::Button::new(theme::icon("settings", 15.0)),
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
            self.theme = match self.theme {
                ThemeMode::Dark => ThemeMode::Light,
                ThemeMode::Light => ThemeMode::Dark,
            };
            theme::apply(ui.ctx(), self.theme);
            let theme = self.theme;
            self.persist_setting(THEME_SETTING, &theme);
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
                        let color = if selected {
                            palette.accent_text
                        } else {
                            palette.muted
                        };
                        let button = egui::Button::new(theme::icon_label(icon, label, 11.0, color))
                            .fill(if selected {
                                palette.primary_soft
                            } else {
                                Color32::TRANSPARENT
                            })
                            .stroke(Stroke::NONE);
                        if ui
                            .add_sized([48.0, 42.0], button)
                            .on_hover_text(tooltip)
                            .clicked()
                        {
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
                                                if ui
                                                    .add(
                                                        egui::Button::new(title)
                                                            .frame(false)
                                                            .selected(selected),
                                                    )
                                                    .clicked()
                                                {
                                                    activate = Some(tab.id);
                                                }
                                                if ui
                                                    .add_sized(
                                                        [20.0, 20.0],
                                                        egui::Button::new(theme::icon("x", 11.0))
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
