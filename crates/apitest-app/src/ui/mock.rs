use apitest_core::{KeyValue, MockRule};
use apitest_runtime::MockServer;
use eframe::egui::{self, RichText};

use crate::app::ApiTestApp;
use crate::i18n::Language;
use crate::state::action::PendingAction;
use crate::theme::{self, Palette};
use crate::ui::request::method_combo;
use crate::ui::widgets::empty_state;

impl ApiTestApp {
    pub(crate) fn mock_workspace(&mut self, ui: &mut egui::Ui, palette: Palette) {
        if self.mock_profiles.get(self.selected_mock).is_none() {
            empty_state(
                ui,
                self.tr("暂无 Mock 服务", "No mock servers"),
                self.tr(
                    "新建 Mock 后可从 API 契约智能生成响应",
                    "Create a mock to generate responses from API contracts",
                ),
                palette,
            );
            if ui.button(self.tr("新建 Mock", "New mock")).clicked() {
                self.perform_action(PendingAction::NewMock);
            }
            return;
        }
        let dirty = self.current_dirty();
        let running = self.mock_server.is_some();
        let base_url = self.mock_server.as_ref().map(MockServer::base_url);
        let mut save = false;
        let mut generate = false;
        let mut start = false;
        let mut stop = false;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                let profile = &mut self.mock_profiles[self.selected_mock];
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut profile.name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(300.0),
                    );
                    if dirty {
                        ui.label(RichText::new("●").color(palette.warning).size(8.0));
                    }
                    if let Some(base_url) = &base_url {
                        ui.label(RichText::new(base_url).color(palette.success));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if running {
                            if ui
                                .button(match self.language {
                                    Language::Chinese => "停止服务",
                                    Language::English => "Stop server",
                                })
                                .clicked()
                            {
                                stop = true;
                            }
                        } else if ui
                            .button(match self.language {
                                Language::Chinese => "启动服务",
                                Language::English => "Start server",
                            })
                            .clicked()
                        {
                            start = true;
                        }
                        if ui
                            .button(match self.language {
                                Language::Chinese => "智能生成",
                                Language::English => "Smart generate",
                            })
                            .on_hover_text(match self.language {
                                Language::Chinese => "从 HTTP 契约新增尚不存在的规则",
                                Language::English => "Add missing rules from HTTP contracts",
                            })
                            .clicked()
                        {
                            generate = true;
                        }
                        if ui
                            .add_enabled(
                                dirty,
                                egui::Button::new(match self.language {
                                    Language::Chinese => "保存",
                                    Language::English => "Save",
                                }),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(match self.language {
                        Language::Chinese => "监听地址",
                        Language::English => "Bind address",
                    });
                    ui.add_enabled(
                        !running,
                        egui::TextEdit::singleline(&mut profile.bind_address).desired_width(150.0),
                    );
                    ui.label(match self.language {
                        Language::Chinese => "端口",
                        Language::English => "Port",
                    });
                    ui.add_enabled(
                        !running,
                        egui::DragValue::new(&mut profile.port).range(0..=u16::MAX),
                    );
                    ui.label(
                        RichText::new(format!("{} rules", profile.rules.len()))
                            .color(palette.muted),
                    );
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    mock_rules_editor(ui, &mut profile.rules, self.language, palette, running);
                });
            });
        if save {
            self.save_current_mock();
        }
        if generate {
            self.generate_current_mock_rules();
        }
        if start {
            self.start_current_mock(ui.ctx());
        }
        if stop {
            self.stop_current_mock();
        }
    }
}

pub(crate) fn mock_rules_editor(
    ui: &mut egui::Ui,
    rules: &mut Vec<MockRule>,
    language: Language,
    palette: Palette,
    running: bool,
) {
    if running {
        ui.label(
            RichText::new(match language {
                Language::Chinese => "服务运行期间配置已锁定，停止服务后可编辑",
                Language::English => "Configuration is locked while the server is running",
            })
            .color(palette.warning),
        );
    }
    ui.add_enabled_ui(!running, |ui| {
        let mut remove = None;
        for (index, rule) in rules.iter_mut().enumerate() {
            ui.push_id(("mock_rule", rule.id), |ui| {
                let title = format!("{}  {}  {}", rule.method, rule.path, rule.name);
                egui::CollapsingHeader::new(title)
                    .default_open(index == 0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(
                                &mut rule.enabled,
                                match language {
                                    Language::Chinese => "启用",
                                    Language::English => "Enabled",
                                },
                            );
                            ui.label(match language {
                                Language::Chinese => "名称",
                                Language::English => "Name",
                            });
                            ui.add(egui::TextEdit::singleline(&mut rule.name).desired_width(220.0));
                            ui.label(match language {
                                Language::Chinese => "优先级",
                                Language::English => "Priority",
                            });
                            ui.add(egui::DragValue::new(&mut rule.priority));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [26.0, 26.0],
                                            egui::Button::new(theme::icon("x", 12.0)).frame(false),
                                        )
                                        .on_hover_text(match language {
                                            Language::Chinese => "删除规则",
                                            Language::English => "Delete rule",
                                        })
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            method_combo(ui, &mut rule.method, palette);
                            ui.add(
                                egui::TextEdit::singleline(&mut rule.path)
                                    .hint_text("/users/{id}")
                                    .desired_width(ui.available_width()),
                            );
                        });
                        ui.collapsing(
                            match language {
                                Language::Chinese => "请求匹配条件",
                                Language::English => "Request matching",
                            },
                            |ui| {
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "查询参数",
                                        Language::English => "Query parameters",
                                    })
                                    .strong(),
                                );
                                mock_key_value_editor(ui, &mut rule.query, language, palette);
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "请求头",
                                        Language::English => "Request headers",
                                    })
                                    .strong(),
                                );
                                mock_key_value_editor(ui, &mut rule.headers, language, palette);
                                let mut path_variables = rule
                                    .path_variables
                                    .iter()
                                    .map(|(name, value)| (name.clone(), value.clone()))
                                    .collect::<Vec<_>>();
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "路径变量",
                                        Language::English => "Path variables",
                                    })
                                    .strong(),
                                );
                                string_pair_editor(ui, &mut path_variables, language, palette);
                                rule.path_variables = path_variables.into_iter().collect();
                                let mut body_contains =
                                    rule.body_contains.clone().unwrap_or_default();
                                ui.horizontal(|ui| {
                                    ui.label(match language {
                                        Language::Chinese => "正文包含",
                                        Language::English => "Body contains",
                                    });
                                    ui.add(
                                        egui::TextEdit::singleline(&mut body_contains)
                                            .desired_width(ui.available_width()),
                                    );
                                });
                                rule.body_contains =
                                    (!body_contains.is_empty()).then_some(body_contains);
                            },
                        );
                        ui.collapsing(
                            match language {
                                Language::Chinese => "响应配置",
                                Language::English => "Response",
                            },
                            |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(match language {
                                        Language::Chinese => "状态码",
                                        Language::English => "Status",
                                    });
                                    ui.add(
                                        egui::DragValue::new(&mut rule.response.status)
                                            .range(100..=599),
                                    );
                                    ui.label(match language {
                                        Language::Chinese => "延迟",
                                        Language::English => "Delay",
                                    });
                                    ui.add(
                                        egui::DragValue::new(&mut rule.response.delay_ms)
                                            .range(0..=3_600_000),
                                    );
                                    ui.label("ms");
                                });
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "响应头",
                                        Language::English => "Response headers",
                                    })
                                    .strong(),
                                );
                                mock_key_value_editor(
                                    ui,
                                    &mut rule.response.headers,
                                    language,
                                    palette,
                                );
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "响应正文",
                                        Language::English => "Response body",
                                    })
                                    .strong(),
                                );
                                ui.add(
                                    egui::TextEdit::multiline(&mut rule.response.body)
                                        .code_editor()
                                        .desired_rows(8)
                                        .desired_width(f32::INFINITY),
                                );
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "动态脚本",
                                        Language::English => "Dynamic script",
                                    })
                                    .strong(),
                                );
                                ui.add(
                                    egui::TextEdit::multiline(&mut rule.response.script)
                                        .code_editor()
                                        .desired_rows(5)
                                        .hint_text(match language {
                                            Language::Chinese => "可选响应脚本",
                                            Language::English => "Optional response script",
                                        })
                                        .desired_width(f32::INFINITY),
                                );
                            },
                        );
                    });
            });
            ui.separator();
        }
        if let Some(index) = remove {
            rules.remove(index);
        }
        if ui
            .button(theme::icon_label(
                "plus",
                match language {
                    Language::Chinese => "添加 Mock 规则",
                    Language::English => "Add mock rule",
                },
                12.0,
                palette.text,
            ))
            .clicked()
        {
            let rule = MockRule {
                name: match language {
                    Language::Chinese => format!("Mock 规则 {}", rules.len() + 1),
                    Language::English => format!("Mock rule {}", rules.len() + 1),
                },
                ..MockRule::default()
            };
            rules.push(rule);
        }
    });
}

pub(crate) fn mock_key_value_editor(
    ui: &mut egui::Ui,
    values: &mut Vec<KeyValue>,
    language: Language,
    palette: Palette,
) {
    let mut remove = None;
    for (index, value) in values.iter_mut().enumerate() {
        ui.push_id(("mock_pair", index), |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut value.enabled, "");
                ui.add(
                    egui::TextEdit::singleline(&mut value.name)
                        .hint_text(match language {
                            Language::Chinese => "名称",
                            Language::English => "Name",
                        })
                        .desired_width(180.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut value.value)
                        .hint_text(match language {
                            Language::Chinese => "值",
                            Language::English => "Value",
                        })
                        .desired_width(260.0),
                );
                if ui
                    .add_sized(
                        [26.0, 26.0],
                        egui::Button::new(theme::icon("x", 12.0)).frame(false),
                    )
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        values.remove(index);
    }
    if ui
        .small_button(match language {
            Language::Chinese => "+ 添加",
            Language::English => "+ Add",
        })
        .clicked()
    {
        values.push(KeyValue::enabled("", ""));
    }
    if values.is_empty() {
        ui.label(RichText::new("—").color(palette.muted));
    }
}

pub(crate) fn string_pair_editor(
    ui: &mut egui::Ui,
    values: &mut Vec<(String, String)>,
    language: Language,
    palette: Palette,
) {
    let mut remove = None;
    for (index, (name, value)) in values.iter_mut().enumerate() {
        ui.push_id(("string_pair", index), |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text(match language {
                            Language::Chinese => "变量名",
                            Language::English => "Variable",
                        })
                        .desired_width(180.0),
                );
                ui.add(
                    egui::TextEdit::singleline(value)
                        .hint_text(match language {
                            Language::Chinese => "匹配值",
                            Language::English => "Expected value",
                        })
                        .desired_width(260.0),
                );
                if ui
                    .add_sized(
                        [26.0, 26.0],
                        egui::Button::new(theme::icon("x", 12.0)).frame(false),
                    )
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        values.remove(index);
    }
    if ui
        .small_button(match language {
            Language::Chinese => "+ 添加",
            Language::English => "+ Add",
        })
        .clicked()
    {
        values.push((String::new(), String::new()));
    }
    if values.is_empty() {
        ui.label(RichText::new("—").color(palette.muted));
    }
}
