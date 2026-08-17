use apitest_core::{EntityId, ScenarioNode};
use eframe::egui::{self, CornerRadius, RichText, Stroke};

use crate::i18n::{Language, tr};
use crate::theme::{self, UiExt};
use crate::ui::scenario::rules::{assertion_rules_editor, extractor_rules_editor};

pub(crate) fn scenario_nodes_editor(
    ui: &mut egui::Ui,
    nodes: &mut Vec<ScenarioNode>,
    request_options: &[(EntityId, String)],
    language: Language,
    depth: usize,
) {
    let palette = ui.palette();
    let mut remove = None;
    for (index, node) in nodes.iter_mut().enumerate() {
        ui.push_id(("scenario_node", depth, index), |ui| {
            egui::Frame::new()
                .fill(palette.control)
                .stroke(Stroke::new(1.0, palette.divider))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let label = match node {
                            ScenarioNode::Request { .. } => tr(language, "请求", "Request"),
                            ScenarioNode::Group { .. } => tr(language, "分组", "Group"),
                            ScenarioNode::If { .. } => "If",
                            ScenarioNode::Loop { .. } => tr(language, "循环", "Loop"),
                            ScenarioNode::Delay { .. } => tr(language, "等待", "Delay"),
                        };
                        ui.label(RichText::new(label).strong().color(palette.accent_text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_sized(
                                    [26.0, 26.0],
                                    egui::Button::new(theme::icon("x", 12.0)).frame(false),
                                )
                                .on_hover_text(tr(language, "删除节点", "Delete node"))
                                .clicked()
                            {
                                remove = Some(index);
                            }
                        });
                    });
                    ui.separator();
                    match node {
                        ScenarioNode::Request {
                            case_id,
                            assertions,
                            extractors,
                        } => {
                            let selected = request_options
                                .iter()
                                .find(|(id, _)| id == case_id)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or(tr(language, "请求已不存在", "Missing request"));
                            ui.horizontal(|ui| {
                                ui.label(tr(language, "执行请求", "Run request"));
                                egui::ComboBox::from_id_salt("request_case")
                                    .selected_text(selected)
                                    .width(260.0)
                                    .show_ui(ui, |ui| {
                                        for (id, name) in request_options {
                                            ui.selectable_value(case_id, *id, name);
                                        }
                                    });
                            });
                            ui.collapsing(
                                format!(
                                    "{} ({})",
                                    tr(language, "断言", "Assertions"),
                                    assertions.len()
                                ),
                                |ui| assertion_rules_editor(ui, assertions, language),
                            );
                            ui.collapsing(
                                format!(
                                    "{} ({})",
                                    tr(language, "提取变量", "Extract variables"),
                                    extractors.len()
                                ),
                                |ui| extractor_rules_editor(ui, extractors, language),
                            );
                        }
                        ScenarioNode::Group { name, nodes } => {
                            ui.horizontal(|ui| {
                                ui.label(tr(language, "分组名称", "Group name"));
                                ui.add(
                                    egui::TextEdit::singleline(name)
                                        .desired_width(ui.available_width()),
                                );
                            });
                            ui.indent("group_nodes", |ui| {
                                scenario_nodes_editor(
                                    ui,
                                    nodes,
                                    request_options,
                                    language,
                                    depth + 1,
                                );
                            });
                        }
                        ScenarioNode::If {
                            expression,
                            then_nodes,
                            else_nodes,
                        } => {
                            ui.horizontal(|ui| {
                                ui.label(tr(language, "条件表达式", "Expression"));
                                ui.add(
                                    egui::TextEdit::singleline(expression)
                                        .code_editor()
                                        .desired_width(ui.available_width()),
                                );
                            });
                            ui.label(RichText::new("Then").strong().color(palette.success));
                            ui.indent("then_nodes", |ui| {
                                scenario_nodes_editor(
                                    ui,
                                    then_nodes,
                                    request_options,
                                    language,
                                    depth + 1,
                                );
                            });
                            ui.label(RichText::new("Else").strong().color(palette.warning));
                            ui.indent("else_nodes", |ui| {
                                scenario_nodes_editor(
                                    ui,
                                    else_nodes,
                                    request_options,
                                    language,
                                    depth + 1,
                                );
                            });
                        }
                        ScenarioNode::Loop {
                            source,
                            item_name,
                            max_iterations,
                            nodes,
                        } => {
                            ui.horizontal(|ui| {
                                ui.label(tr(language, "数据源", "Source"));
                                ui.add(
                                    egui::TextEdit::singleline(source)
                                        .code_editor()
                                        .desired_width(220.0),
                                );
                                ui.label(tr(language, "变量名", "Item"));
                                ui.add(egui::TextEdit::singleline(item_name).desired_width(120.0));
                                ui.label(tr(language, "上限", "Limit"));
                                ui.add(egui::DragValue::new(max_iterations).range(1..=100_000));
                            });
                            ui.indent("loop_nodes", |ui| {
                                scenario_nodes_editor(
                                    ui,
                                    nodes,
                                    request_options,
                                    language,
                                    depth + 1,
                                );
                            });
                        }
                        ScenarioNode::Delay { millis } => {
                            ui.horizontal(|ui| {
                                ui.label(tr(language, "等待时长", "Duration"));
                                ui.add(egui::DragValue::new(millis).range(0..=3_600_000));
                                ui.label("ms");
                            });
                        }
                    }
                });
        });
        ui.add_space(8.0);
    }
    if let Some(index) = remove {
        nodes.remove(index);
    }

    ui.menu_button(
        theme::icon_label(
            "plus",
            tr(language, "添加节点", "Add node"),
            12.0,
            palette.text,
        ),
        |ui| {
            let request = ui.add_enabled(
                !request_options.is_empty(),
                egui::Button::new(tr(language, "请求", "Request")),
            );
            if request.clicked() {
                nodes.push(ScenarioNode::Request {
                    case_id: request_options[0].0,
                    assertions: Vec::new(),
                    extractors: Vec::new(),
                });
                ui.close();
            }
            if ui.button(tr(language, "分组", "Group")).clicked() {
                nodes.push(ScenarioNode::Group {
                    name: tr(language, "新分组", "New group").into(),
                    nodes: Vec::new(),
                });
                ui.close();
            }
            if ui.button("If").clicked() {
                nodes.push(ScenarioNode::If {
                    expression: String::new(),
                    then_nodes: Vec::new(),
                    else_nodes: Vec::new(),
                });
                ui.close();
            }
            if ui.button(tr(language, "循环", "Loop")).clicked() {
                nodes.push(ScenarioNode::Loop {
                    source: String::new(),
                    item_name: "item".into(),
                    max_iterations: 100,
                    nodes: Vec::new(),
                });
                ui.close();
            }
            if ui.button(tr(language, "等待", "Delay")).clicked() {
                nodes.push(ScenarioNode::Delay { millis: 100 });
                ui.close();
            }
        },
    );
}
