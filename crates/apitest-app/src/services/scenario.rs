use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Instant,
};

use apitest_core::{ApiContract, EntityId, ExecutionRequest, ProtocolKind, ScenarioNode};
use apitest_runtime::{ResponseSchemaContract, ScenarioCase, ScenarioReport, ScenarioRunner};
use eframe::egui::{self};
use tokio_util::sync::CancellationToken;

use crate::app::ApiTestApp;
use crate::i18n::{Language, tr};
use crate::services::document::document_snapshot;
use crate::state::action::{RuntimeMessage, ToastKind};
use crate::state::workspace::WorkspaceRequest;
use crate::workbench::{DocumentId, DocumentKind};

impl ApiTestApp {
    pub(crate) fn save_current_scenario(&mut self) -> bool {
        let Some(scenario) = self.scenarios.get(self.selected_scenario) else {
            return false;
        };
        if scenario.name.trim().is_empty() {
            self.toast(
                ToastKind::Error,
                self.tr("场景名称不能为空", "Scenario name cannot be empty"),
            );
            return false;
        }
        if let Err(error) = validate_scenario_nodes(&scenario.nodes, &self.requests, self.language)
        {
            self.toast(ToastKind::Error, error);
            return false;
        }
        let Some(database) = &self.database else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return false;
        };
        if let Err(error) = database.save_scenario(self.project.id, scenario) {
            self.toast(ToastKind::Error, error.to_string());
            return false;
        }
        let id = scenario.id;
        let name = scenario.name.clone();
        self.scenario_snapshots
            .insert(id, document_snapshot(scenario));
        self.document_tabs.rename(
            DocumentId {
                kind: DocumentKind::Scenario,
                entity_id: id,
            },
            name,
        );
        self.persist_document_tabs();
        self.toast(ToastKind::Success, self.tr("场景已保存", "Scenario saved"));
        true
    }

    pub(crate) fn run_current_scenario(&mut self, context: &egui::Context) {
        let Some(scenario) = self.scenarios.get(self.selected_scenario).cloned() else {
            return;
        };
        if scenario.name.trim().is_empty() {
            self.toast(
                ToastKind::Error,
                self.tr("场景名称不能为空", "Scenario name cannot be empty"),
            );
            return;
        }
        if let Err(error) = validate_scenario_nodes(&scenario.nodes, &self.requests, self.language)
        {
            self.toast(ToastKind::Error, error);
            return;
        }
        let case_ids = scenario_case_ids(&scenario.nodes);
        let request_indices = self
            .requests
            .iter()
            .enumerate()
            .filter_map(|(index, request)| {
                case_ids.contains(&request.request_case.id).then_some(index)
            })
            .collect::<Vec<_>>();
        for index in request_indices {
            let validation = self
                .validate_auth(&self.requests[index].draft.auth)
                .and_then(|_| self.validate_proxy(self.requests[index].draft.proxy.as_ref()));
            if let Err(error) = validation {
                self.toast(ToastKind::Error, error);
                return;
            }
            if let Err(error) = self.persist_request_secret(index) {
                self.toast(ToastKind::Error, error);
                return;
            }
        }
        let active_index = self
            .environments
            .iter()
            .position(|environment| environment.id() == self.active_environment)
            .unwrap_or_default();
        if let Err(error) = self.persist_environment_secrets(active_index) {
            self.toast(ToastKind::Error, error);
            return;
        }
        let datasets = match load_scenario_datasets(scenario.dataset_path.as_deref(), self.language)
        {
            Ok(datasets) => datasets,
            Err(error) => {
                self.toast(ToastKind::Error, error);
                return;
            }
        };
        let environment = self.environments[active_index].to_environment();
        let initial_variables = environment
            .variables
            .iter()
            .filter(|variable| variable.enabled && !variable.is_secret())
            .map(|variable| (variable.name.clone(), variable.value.clone()))
            .collect::<BTreeMap<_, _>>();
        let cases = self
            .requests
            .iter()
            .map(|request| {
                let mut execution =
                    ExecutionRequest::new(request.edited_protocol(), environment.clone());
                execution.case_id = Some(request.request_case.id);
                execution.local_variables = request.request_case.local_variables.clone();
                (
                    request.request_case.id,
                    ScenarioCase {
                        name: request.name.clone(),
                        request: execution,
                        pre_script: request.request_case.pre_request_script.clone(),
                        post_script: request.request_case.post_response_script.clone(),
                        assertions: request.request_case.assertions.clone(),
                        extractors: request.request_case.extractors.clone(),
                        response_schema: match &request.definition.contract {
                            ApiContract::Http(contract) => Some(ResponseSchemaContract {
                                responses: contract.responses.clone(),
                                components: self.project.components.clone(),
                            }),
                            _ => None,
                        },
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        // Shared read-only: data-driven runs iterate without re-cloning every
        // case (each embeds a full request body).
        let cases = std::sync::Arc::new(cases);
        let mut runner = ScenarioRunner::new();
        for kind in [
            ProtocolKind::Http,
            ProtocolKind::GraphQl,
            ProtocolKind::Sse,
            ProtocolKind::WebSocket,
            ProtocolKind::Grpc,
        ] {
            if let Some(executor) = self.executors.executor(kind) {
                runner.register_executor_for(kind, executor);
            }
        }
        if let Some(cancellation) = self.scenario_cancellation.take() {
            cancellation.cancel();
        }
        self.scenario_run_id = self.scenario_run_id.wrapping_add(1);
        let run_id = self.scenario_run_id;
        let cancellation = CancellationToken::new();
        self.scenario_cancellation = Some(cancellation.clone());
        self.scenario_report = None;
        let sender = self.sender.clone();
        let context = context.clone();
        self.runtime.spawn(async move {
            let started = Instant::now();
            let mut passed = true;
            let mut steps = Vec::new();
            let iterations = datasets.len();
            let mut result = Ok(());
            for (index, dataset) in datasets.into_iter().enumerate() {
                let mut variables = initial_variables.clone();
                variables.extend(dataset);
                match runner
                    .run_test_scenario(
                        scenario.clone(),
                        std::sync::Arc::clone(&cases),
                        variables,
                        cancellation.child_token(),
                    )
                    .await
                {
                    Ok(mut report) => {
                        if iterations > 1 {
                            for step in &mut report.steps {
                                step.name = format!("[{}] {}", index + 1, step.name);
                            }
                        }
                        passed &= report.passed;
                        steps.append(&mut report.steps);
                        if !report.passed && scenario.stop_on_failure {
                            break;
                        }
                    }
                    Err(error) => {
                        result = Err(error.to_string());
                        break;
                    }
                }
            }
            let report = result.map(|()| ScenarioReport {
                name: scenario.name,
                passed,
                elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                steps,
            });
            let _ = sender.send(RuntimeMessage::ScenarioFinished(run_id, report));
            context.request_repaint();
        });
    }

    pub(crate) fn delete_scenario(&mut self, id: EntityId) {
        let Some(database) = self.database.clone() else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return;
        };
        if let Err(error) = database.delete_scenario(self.project.id, id) {
            self.toast(ToastKind::Error, error.to_string());
            return;
        }
        self.scenario_snapshots.remove(&id);
        if let Some(index) = self.scenarios.iter().position(|scenario| scenario.id == id) {
            self.scenarios.remove(index);
        }
        self.selected_scenario = self
            .selected_scenario
            .min(self.scenarios.len().saturating_sub(1));
        self.close_document(DocumentId {
            kind: DocumentKind::Scenario,
            entity_id: id,
        });
        self.toast(
            ToastKind::Success,
            self.tr("场景已删除", "Scenario deleted"),
        );
    }

    pub(crate) fn stop_current_scenario(&mut self) {
        if let Some(cancellation) = self.scenario_cancellation.take() {
            cancellation.cancel();
            self.scenario_run_id = self.scenario_run_id.wrapping_add(1);
            self.toast(
                ToastKind::Info,
                self.tr("场景执行已停止", "Scenario stopped"),
            );
        }
    }
}

pub(crate) fn validate_scenario_nodes(
    nodes: &[ScenarioNode],
    requests: &[WorkspaceRequest],
    language: Language,
) -> Result<(), String> {
    for node in nodes {
        match node {
            ScenarioNode::Request { case_id, .. } => {
                if !requests
                    .iter()
                    .any(|request| request.request_case.id == *case_id)
                {
                    return Err(match language {
                        Language::Chinese => {
                            format!("场景引用了不存在的请求用例 {case_id}")
                        }
                        Language::English => {
                            format!("scenario references missing request case {case_id}")
                        }
                    });
                }
            }
            ScenarioNode::Group { name, nodes } => {
                if name.trim().is_empty() {
                    return Err(tr(
                        language,
                        "场景分组名称不能为空",
                        "Scenario group name cannot be empty",
                    )
                    .into());
                }
                validate_scenario_nodes(nodes, requests, language)?;
            }
            ScenarioNode::If {
                expression,
                then_nodes,
                else_nodes,
            } => {
                if expression.trim().is_empty() {
                    return Err(tr(
                        language,
                        "场景条件表达式不能为空",
                        "Scenario condition cannot be empty",
                    )
                    .into());
                }
                validate_scenario_nodes(then_nodes, requests, language)?;
                validate_scenario_nodes(else_nodes, requests, language)?;
            }
            ScenarioNode::Loop {
                source,
                item_name,
                max_iterations,
                nodes,
            } => {
                if source.trim().is_empty() || item_name.trim().is_empty() {
                    return Err(tr(
                        language,
                        "场景循环的来源与项名称不能为空",
                        "Scenario loop source and item name cannot be empty",
                    )
                    .into());
                }
                if *max_iterations == 0 {
                    return Err(tr(
                        language,
                        "场景循环次数上限必须大于 0",
                        "Scenario loop iteration limit must be positive",
                    )
                    .into());
                }
                validate_scenario_nodes(nodes, requests, language)?;
            }
            ScenarioNode::Delay { .. } => {}
        }
    }
    Ok(())
}

pub(crate) fn scenario_case_ids(nodes: &[ScenarioNode]) -> HashSet<EntityId> {
    fn collect(nodes: &[ScenarioNode], ids: &mut HashSet<EntityId>) {
        for node in nodes {
            match node {
                ScenarioNode::Request { case_id, .. } => {
                    ids.insert(*case_id);
                }
                ScenarioNode::Group { nodes, .. } | ScenarioNode::Loop { nodes, .. } => {
                    collect(nodes, ids);
                }
                ScenarioNode::If {
                    then_nodes,
                    else_nodes,
                    ..
                } => {
                    collect(then_nodes, ids);
                    collect(else_nodes, ids);
                }
                ScenarioNode::Delay { .. } => {}
            }
        }
    }

    let mut ids = HashSet::new();
    collect(nodes, &mut ids);
    ids
}

pub(crate) fn load_scenario_datasets(
    path: Option<&str>,
    language: Language,
) -> Result<Vec<BTreeMap<String, String>>, String> {
    let Some(path) = path else {
        return Ok(vec![BTreeMap::new()]);
    };
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let datasets = match extension.as_str() {
        "json" => {
            let source = std::fs::read_to_string(path).map_err(|error| match language {
                Language::Chinese => format!("读取场景数据集失败：{error}"),
                Language::English => format!("failed to read scenario dataset: {error}"),
            })?;
            let value =
                serde_json::from_str::<serde_json::Value>(&source).map_err(
                    |error| match language {
                        Language::Chinese => format!("场景 JSON 数据集无效：{error}"),
                        Language::English => format!("invalid JSON scenario dataset: {error}"),
                    },
                )?;
            match value {
                serde_json::Value::Array(rows) => rows
                    .into_iter()
                    .map(|row| json_dataset_row(row, language))
                    .collect::<Result<Vec<_>, _>>()?,
                serde_json::Value::Object(row) => {
                    vec![json_dataset_row(serde_json::Value::Object(row), language)?]
                }
                _ => {
                    return Err(tr(
                        language,
                        "场景 JSON 数据集必须是对象或对象数组",
                        "JSON scenario dataset must be an object or an array of objects",
                    )
                    .into());
                }
            }
        }
        "csv" => {
            let mut reader = csv::Reader::from_path(path).map_err(|error| match language {
                Language::Chinese => format!("读取场景 CSV 数据集失败：{error}"),
                Language::English => format!("failed to read CSV scenario dataset: {error}"),
            })?;
            let headers = reader
                .headers()
                .map_err(|error| match language {
                    Language::Chinese => format!("场景 CSV 数据集无效：{error}"),
                    Language::English => format!("invalid CSV scenario dataset: {error}"),
                })?
                .clone();
            let mut rows = Vec::new();
            for record in reader.records() {
                let record = record.map_err(|error| match language {
                    Language::Chinese => format!("场景 CSV 数据集无效：{error}"),
                    Language::English => format!("invalid CSV scenario dataset: {error}"),
                })?;
                rows.push(
                    headers
                        .iter()
                        .zip(record.iter())
                        .map(|(name, value)| (name.to_owned(), value.to_owned()))
                        .collect(),
                );
            }
            rows
        }
        _ => {
            return Err(tr(
                language,
                "场景数据集必须是 JSON 或 CSV 文件",
                "scenario dataset must be a JSON or CSV file",
            )
            .into());
        }
    };
    if datasets.is_empty() {
        return Err(tr(
            language,
            "场景数据集没有数据行",
            "scenario dataset contains no rows",
        )
        .into());
    }
    Ok(datasets)
}

pub(crate) fn json_dataset_row(
    value: serde_json::Value,
    language: Language,
) -> Result<BTreeMap<String, String>, String> {
    let serde_json::Value::Object(row) = value else {
        return Err(tr(
            language,
            "场景 JSON 数据集的每一行必须是对象",
            "every JSON scenario dataset row must be an object",
        )
        .into());
    };
    Ok(row
        .into_iter()
        .map(|(name, value)| {
            let value = value.as_str().map(str::to_owned).unwrap_or_else(|| {
                serde_json::to_string(&value).expect("JSON dataset value should serialize")
            });
            (name, value)
        })
        .collect())
}
