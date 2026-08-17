use apitest_core::{AssertionRule, ExtractorRule, VariableSource};
use eframe::egui::{self, RichText};

use crate::i18n::Language;
use crate::theme::{self, Palette};

pub(crate) fn assertion_rules_editor(
    ui: &mut egui::Ui,
    rules: &mut Vec<AssertionRule>,
    language: Language,
    palette: Palette,
) {
    let mut remove = None;
    for (index, rule) in rules.iter_mut().enumerate() {
        ui.push_id(("assertion", index), |ui| {
            let current_kind = match rule {
                AssertionRule::Status { .. } => 0,
                AssertionRule::HeaderEquals { .. } => 1,
                AssertionRule::JsonPathEquals { .. } => 2,
                AssertionRule::Schema => 3,
                AssertionRule::DurationBelow { .. } => 4,
                AssertionRule::Script { .. } => 5,
            };
            let mut selected_kind = current_kind;
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("kind")
                    .selected_text(assertion_kind_label(current_kind, language))
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for kind in 0..6 {
                            ui.selectable_value(
                                &mut selected_kind,
                                kind,
                                assertion_kind_label(kind, language),
                            );
                        }
                    });
                if selected_kind == current_kind {
                    match rule {
                        AssertionRule::Status { expected } => {
                            ui.label(match language {
                                Language::Chinese => "期望状态码",
                                Language::English => "Expected status",
                            });
                            ui.add(egui::DragValue::new(expected).range(100..=599));
                        }
                        AssertionRule::HeaderEquals { name, expected } => {
                            ui.add(
                                egui::TextEdit::singleline(name)
                                    .hint_text("Content-Type")
                                    .desired_width(150.0),
                            );
                            ui.add(
                                egui::TextEdit::singleline(expected)
                                    .hint_text(match language {
                                        Language::Chinese => "期望值",
                                        Language::English => "Expected value",
                                    })
                                    .desired_width(220.0),
                            );
                        }
                        AssertionRule::JsonPathEquals { path, expected } => {
                            ui.add(
                                egui::TextEdit::singleline(path)
                                    .hint_text("$.data.id")
                                    .code_editor()
                                    .desired_width(180.0),
                            );
                            ui.add(
                                egui::TextEdit::singleline(expected)
                                    .hint_text(match language {
                                        Language::Chinese => "期望值",
                                        Language::English => "Expected value",
                                    })
                                    .desired_width(220.0),
                            );
                        }
                        AssertionRule::Schema => {
                            ui.label(
                                RichText::new(match language {
                                    Language::Chinese => "校验响应是否符合契约 Schema",
                                    Language::English => "Validate the response against its schema",
                                })
                                .color(palette.muted),
                            );
                        }
                        AssertionRule::DurationBelow { milliseconds } => {
                            ui.label("<");
                            ui.add(egui::DragValue::new(milliseconds).range(0..=3_600_000));
                            ui.label("ms");
                        }
                        AssertionRule::Script { expression } => {
                            ui.add(
                                egui::TextEdit::singleline(expression)
                                    .hint_text("response.status === 200")
                                    .code_editor()
                                    .desired_width(360.0),
                            );
                        }
                    }
                }
                if ui
                    .add_sized(
                        [26.0, 26.0],
                        egui::Button::new(theme::icon("x", 12.0)).frame(false),
                    )
                    .on_hover_text(match language {
                        Language::Chinese => "删除断言",
                        Language::English => "Delete assertion",
                    })
                    .clicked()
                {
                    remove = Some(index);
                }
            });
            if selected_kind != current_kind {
                *rule = new_assertion_rule(selected_kind);
            }
        });
    }
    if let Some(index) = remove {
        rules.remove(index);
    }
    ui.menu_button(
        match language {
            Language::Chinese => "+ 添加断言",
            Language::English => "+ Add assertion",
        },
        |ui| {
            for kind in 0..6 {
                if ui.button(assertion_kind_label(kind, language)).clicked() {
                    rules.push(new_assertion_rule(kind));
                    ui.close();
                }
            }
        },
    );
}

pub(crate) fn assertion_kind_label(kind: usize, language: Language) -> &'static str {
    match (kind, language) {
        (0, Language::Chinese) => "状态码",
        (0, Language::English) => "Status",
        (1, Language::Chinese) => "响应头等于",
        (1, Language::English) => "Header equals",
        (2, Language::Chinese) => "JSONPath 等于",
        (2, Language::English) => "JSONPath equals",
        (3, Language::Chinese) => "Schema 校验",
        (3, Language::English) => "Schema",
        (4, Language::Chinese) => "响应时间",
        (4, Language::English) => "Duration below",
        (5, Language::Chinese) => "脚本表达式",
        (5, Language::English) => "Script expression",
        _ => "Unknown",
    }
}

pub(crate) fn new_assertion_rule(kind: usize) -> AssertionRule {
    match kind {
        0 => AssertionRule::Status { expected: 200 },
        1 => AssertionRule::HeaderEquals {
            name: String::new(),
            expected: String::new(),
        },
        2 => AssertionRule::JsonPathEquals {
            path: "$.".into(),
            expected: String::new(),
        },
        3 => AssertionRule::Schema,
        4 => AssertionRule::DurationBelow {
            milliseconds: 1_000,
        },
        5 => AssertionRule::Script {
            expression: String::new(),
        },
        _ => AssertionRule::Status { expected: 200 },
    }
}

pub(crate) fn extractor_rules_editor(
    ui: &mut egui::Ui,
    rules: &mut Vec<ExtractorRule>,
    language: Language,
    palette: Palette,
) {
    let mut remove = None;
    for (index, rule) in rules.iter_mut().enumerate() {
        ui.push_id(("extractor", index), |ui| {
            let current_kind = match &rule.source {
                VariableSource::JsonPath(_) => 0,
                VariableSource::Header(_) => 1,
                VariableSource::Cookie(_) => 2,
                VariableSource::Regex(_) => 3,
            };
            let mut selected_kind = current_kind;
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut rule.name)
                        .hint_text(match language {
                            Language::Chinese => "变量名",
                            Language::English => "Variable name",
                        })
                        .desired_width(150.0),
                );
                egui::ComboBox::from_id_salt("source_kind")
                    .selected_text(extractor_kind_label(current_kind, language))
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for kind in 0..4 {
                            ui.selectable_value(
                                &mut selected_kind,
                                kind,
                                extractor_kind_label(kind, language),
                            );
                        }
                    });
                if selected_kind == current_kind {
                    let (value, hint) = match &mut rule.source {
                        VariableSource::JsonPath(value) => (value, "$.data.token"),
                        VariableSource::Header(value) => (value, "X-Request-Id"),
                        VariableSource::Cookie(value) => (value, "session_id"),
                        VariableSource::Regex(value) => (value, "token=(.+)"),
                    };
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .hint_text(hint)
                            .code_editor()
                            .desired_width(260.0),
                    );
                }
                if ui
                    .add_sized(
                        [26.0, 26.0],
                        egui::Button::new(theme::icon("x", 12.0)).frame(false),
                    )
                    .on_hover_text(match language {
                        Language::Chinese => "删除提取器",
                        Language::English => "Delete extractor",
                    })
                    .clicked()
                {
                    remove = Some(index);
                }
            });
            if selected_kind != current_kind {
                rule.source = new_variable_source(selected_kind);
            }
        });
    }
    if let Some(index) = remove {
        rules.remove(index);
    }
    ui.menu_button(
        match language {
            Language::Chinese => "+ 添加提取器",
            Language::English => "+ Add extractor",
        },
        |ui| {
            for kind in 0..4 {
                if ui.button(extractor_kind_label(kind, language)).clicked() {
                    rules.push(ExtractorRule {
                        name: String::new(),
                        source: new_variable_source(kind),
                    });
                    ui.close();
                }
            }
        },
    );
    if rules.is_empty() {
        ui.label(
            RichText::new(match language {
                Language::Chinese => "可将响应字段写入后续步骤变量",
                Language::English => "Extract response values for later steps",
            })
            .small()
            .color(palette.muted),
        );
    }
}

pub(crate) fn extractor_kind_label(kind: usize, language: Language) -> &'static str {
    match (kind, language) {
        (0, _) => "JSONPath",
        (1, Language::Chinese) => "响应头",
        (1, Language::English) => "Header",
        (2, Language::Chinese) => "Cookie",
        (2, Language::English) => "Cookie",
        (3, Language::Chinese) => "正则表达式",
        (3, Language::English) => "Regex",
        _ => "Unknown",
    }
}

pub(crate) fn new_variable_source(kind: usize) -> VariableSource {
    match kind {
        0 => VariableSource::JsonPath("$.".into()),
        1 => VariableSource::Header(String::new()),
        2 => VariableSource::Cookie(String::new()),
        3 => VariableSource::Regex(String::new()),
        _ => VariableSource::JsonPath("$.".into()),
    }
}
