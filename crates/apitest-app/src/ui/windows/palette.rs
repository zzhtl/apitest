use apitest_core::{EntityId, ProtocolKind};
use eframe::egui::{self, RichText};

use crate::app::ApiTestApp;
use crate::state::action::{InteropAction, PendingAction};
use crate::state::workspace::Navigation;
use crate::theme;
use crate::theme::tokens::{icon as icon_size, radius};
use crate::ui::widgets::{Tone, badge};

pub(crate) const PALETTE_FIELD_ID: &str = "command_palette_query";

/// One row of the palette: either somewhere to go or something to do.
enum Command {
    Navigate(Navigation),
    OpenRequest(EntityId),
    Action(PendingAction),
    Interop(InteropAction),
    Send,
    Save,
    ToggleTheme,
    Snippet,
}

struct Entry {
    icon: &'static str,
    label: String,
    hint: String,
    command: Command,
}

impl ApiTestApp {
    fn palette_entries(&mut self, query: &str) -> Vec<Entry> {
        let goto = self.tr("跳转", "Go to");
        let action = self.tr("动作", "Action");
        let mut entries = vec![
            Entry {
                icon: "send",
                label: self.tr("发送请求", "Send request").to_owned(),
                hint: "Ctrl Enter".to_owned(),
                command: Command::Send,
            },
            Entry {
                icon: "save",
                label: self.tr("保存", "Save").to_owned(),
                hint: "Ctrl S".to_owned(),
                command: Command::Save,
            },
            Entry {
                icon: "plus",
                label: self.tr("新建请求", "New request").to_owned(),
                hint: "Ctrl N".to_owned(),
                command: Command::Action(PendingAction::NewRequest(ProtocolKind::Http)),
            },
            Entry {
                icon: "code",
                label: self.tr("生成代码片段", "Generate code snippet").to_owned(),
                hint: action.to_owned(),
                command: Command::Snippet,
            },
            Entry {
                icon: "sun-moon",
                label: self.tr("切换主题", "Toggle theme").to_owned(),
                hint: action.to_owned(),
                command: Command::ToggleTheme,
            },
            Entry {
                icon: "plus",
                label: self.tr("新建场景", "New scenario").to_owned(),
                hint: action.to_owned(),
                command: Command::Action(PendingAction::NewScenario),
            },
            Entry {
                icon: "plus",
                label: self.tr("新建 Mock", "New mock").to_owned(),
                hint: action.to_owned(),
                command: Command::Action(PendingAction::NewMock),
            },
            Entry {
                icon: "plus",
                label: self.tr("新建环境", "New environment").to_owned(),
                hint: action.to_owned(),
                command: Command::Action(PendingAction::NewEnvironment),
            },
        ];
        for (navigation, icon, chinese, english) in [
            (Navigation::Api, "braces", "接口", "APIs"),
            (Navigation::Scenario, "workflow", "测试场景", "Scenarios"),
            (Navigation::Mock, "server", "Mock 服务", "Mock servers"),
            (Navigation::History, "history", "运行历史", "Run history"),
            (Navigation::Environment, "database", "环境", "Environments"),
        ] {
            entries.push(Entry {
                icon,
                label: self.tr(chinese, english).to_owned(),
                hint: goto.to_owned(),
                command: Command::Navigate(navigation),
            });
        }
        for (interop, chinese, english) in [
            (
                InteropAction::ImportOpenApi,
                "导入 OpenAPI",
                "Import OpenAPI",
            ),
            (InteropAction::ImportCurl, "粘贴 cURL", "Paste cURL"),
            (
                InteropAction::PreviewOpenApi,
                "OpenAPI 设计与预览",
                "OpenAPI design & preview",
            ),
            (
                InteropAction::ExportOpenApi,
                "导出 OpenAPI",
                "Export OpenAPI",
            ),
        ] {
            entries.push(Entry {
                icon: "arrow-right-left",
                label: self.tr(chinese, english).to_owned(),
                hint: action.to_owned(),
                command: Command::Interop(interop),
            });
        }
        for hit in self.cached_search_hits(query).0 {
            entries.push(Entry {
                icon: "braces",
                label: hit.name,
                hint: goto.to_owned(),
                command: Command::OpenRequest(hit.id),
            });
        }

        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return entries;
        }
        entries.retain(|entry| entry.label.to_lowercase().contains(&needle));
        entries
    }

    pub(crate) fn command_palette(&mut self, context: &egui::Context) {
        if !self.show_palette {
            return;
        }
        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.show_palette = false;
            return;
        }
        let palette = theme::palette(context);
        let hint = self
            .tr("搜索接口或命令", "Search APIs and commands")
            .to_owned();
        let entries = self.palette_entries(&self.palette_query.clone());
        let mut chosen = None;
        let mut close = false;
        egui::Window::new(self.tr("命令面板", "Command palette"))
            .title_bar(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 90.0])
            .resizable(false)
            .collapsible(false)
            .fixed_size([560.0, 420.0])
            .frame(
                egui::Frame::popup(&context.global_style())
                    .fill(palette.panel)
                    .corner_radius(radius::MD),
            )
            .show(context, |ui| {
                let field = ui.add_sized(
                    [ui.available_width(), 34.0],
                    egui::TextEdit::singleline(&mut self.palette_query)
                        .id_source(PALETTE_FIELD_ID)
                        .hint_text(hint),
                );
                field.request_focus();
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .max_height(340.0)
                    .show(ui, |ui| {
                        for (index, entry) in entries.iter().enumerate() {
                            let row = ui.add_sized(
                                [ui.available_width(), 30.0],
                                egui::Button::new(theme::icon_label(
                                    entry.icon,
                                    &entry.label,
                                    icon_size::SM,
                                    palette.text,
                                ))
                                .frame(false)
                                .right_text(
                                    RichText::new(&entry.hint).small().color(palette.muted),
                                ),
                            );
                            let activate = row.clicked()
                                || (index == 0
                                    && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                            if activate {
                                chosen = Some(index);
                            }
                        }
                        if entries.is_empty() {
                            ui.add_space(12.0);
                            ui.vertical_centered(|ui| {
                                badge(ui, self.tr("没有匹配项", "No matches"), Tone::Warning);
                            });
                        }
                    });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(self.tr(
                        "Enter 执行第一项 · Esc 关闭",
                        "Enter runs the first entry · Esc closes",
                    ))
                    .small()
                    .color(palette.muted),
                );
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        if let Some(index) = chosen {
            let command = entries.into_iter().nth(index).map(|entry| entry.command);
            self.run_palette_command(command, context);
            close = true;
        }
        if close {
            self.show_palette = false;
            self.palette_query.clear();
        }
    }

    fn run_palette_command(&mut self, command: Option<Command>, context: &egui::Context) {
        let Some(command) = command else {
            return;
        };
        match command {
            Command::Navigate(navigation) => self.queue_action(PendingAction::Navigate(navigation)),
            Command::OpenRequest(id) => self.queue_action(PendingAction::SelectRequest(id)),
            Command::Action(action) => self.queue_action(action),
            Command::Interop(action) => self.handle_interop_action(action),
            Command::Send => self.send_current(context),
            Command::Save => {
                self.save_current();
            }
            Command::ToggleTheme => self.toggle_theme(context),
            Command::Snippet => self.show_snippet = true,
        }
    }
}
