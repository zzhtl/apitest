use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Instant,
};

use apitest_core::{
    AssertionRule, EntityId, ExecutionEvent, ExecutionMetrics, ExecutionRequest, ExtractorRule,
    ProtocolExecutor, ProtocolKind, ScenarioNode, TestScenario, Variable, VariableScope,
};
use futures::{StreamExt, future::BoxFuture};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::verification::{
    CapturedResponse, ResponseSchemaContract, evaluate_assertion, evaluate_expression,
    extract_value, json_value_text, restore_variable,
};
use crate::{ScriptAssertion, ScriptEngine, ScriptResponse};

#[derive(Debug, Clone)]
pub struct Scenario {
    pub name: String,
    pub stop_on_failure: bool,
    pub steps: Vec<ScenarioStep>,
}

#[derive(Debug, Clone)]
pub struct ScenarioStep {
    pub name: String,
    pub request: ExecutionRequest,
    pub post_script: String,
}

#[derive(Debug, Clone)]
pub struct ScenarioCase {
    pub name: String,
    pub request: ExecutionRequest,
    pub pre_script: String,
    pub post_script: String,
    pub assertions: Vec<AssertionRule>,
    pub extractors: Vec<ExtractorRule>,
    pub response_schema: Option<ResponseSchemaContract>,
}

#[derive(Debug, Clone)]
pub struct ScenarioReport {
    pub name: String,
    pub passed: bool,
    pub elapsed_ms: u64,
    pub steps: Vec<StepReport>,
}

#[derive(Debug, Clone)]
pub struct StepReport {
    pub name: String,
    pub passed: bool,
    pub status: Option<u16>,
    pub metrics: Option<ExecutionMetrics>,
    pub assertions: Vec<ScriptAssertion>,
    pub error: Option<String>,
}

#[derive(Debug, Error)]
pub enum ScenarioError {
    #[error("no executor is registered for {0:?}")]
    MissingExecutor(ProtocolKind),
    #[error("scenario references missing request case {0}")]
    MissingCase(EntityId),
    #[error("invalid scenario expression: {0}")]
    InvalidExpression(String),
    #[error("invalid scenario loop: {0}")]
    InvalidLoop(String),
    #[error("scenario was cancelled")]
    Cancelled,
}

struct StructuredState {
    variables: BTreeMap<String, String>,
    reports: Vec<StepReport>,
}

pub struct ScenarioRunner {
    executors: HashMap<ProtocolKind, Arc<dyn ProtocolExecutor>>,
    scripts: ScriptEngine,
}

/// Cap on the response body a scenario step keeps for assertions, matching
/// the interactive preview limit.
const MAX_SCENARIO_BODY_BYTES: usize = 10 * 1024 * 1024;

impl ScenarioRunner {
    pub fn new() -> Self {
        Self {
            executors: HashMap::new(),
            scripts: ScriptEngine::default(),
        }
    }

    pub fn with_executor(mut self, executor: Arc<dyn ProtocolExecutor>) -> Self {
        self.executors.insert(executor.kind(), executor);
        self
    }

    pub fn register_executor(&mut self, executor: Arc<dyn ProtocolExecutor>) {
        self.executors.insert(executor.kind(), executor);
    }

    pub fn register_executor_for(
        &mut self,
        kind: ProtocolKind,
        executor: Arc<dyn ProtocolExecutor>,
    ) {
        self.executors.insert(kind, executor);
    }

    pub async fn run(
        &self,
        scenario: Scenario,
        cancellation: CancellationToken,
    ) -> Result<ScenarioReport, ScenarioError> {
        let started = Instant::now();
        let expected_steps = scenario.steps.len();
        let mut reports = Vec::with_capacity(expected_steps);
        let mut variables = BTreeMap::<String, String>::new();

        for step in scenario.steps {
            if cancellation.is_cancelled() {
                return Err(ScenarioError::Cancelled);
            }
            let kind = step.request.protocol.kind();
            let executor = self
                .executors
                .get(&kind)
                .ok_or(ScenarioError::MissingExecutor(kind))?;
            let mut request = step.request;
            request
                .local_variables
                .extend(variables.iter().map(|(name, value)| {
                    Variable::scoped(name.clone(), value.clone(), VariableScope::Local)
                }));
            let mut stream = executor.execute(request, cancellation.child_token());
            let mut status = None;
            let mut headers = BTreeMap::new();
            let mut body = Vec::new();
            let mut metrics = None;
            let mut error = None;

            while let Some(event) = stream.next().await {
                match event {
                    Ok(ExecutionEvent::ResponseHead(head)) => {
                        status = head.status;
                        for (name, values) in head.headers {
                            headers.insert(name, values.join(", "));
                        }
                    }
                    Ok(ExecutionEvent::Data(data)) | Ok(ExecutionEvent::Message { data, .. }) => {
                        body.extend_from_slice(&data);
                    }
                    Ok(ExecutionEvent::Metrics(value)) | Ok(ExecutionEvent::Completed(value)) => {
                        metrics = Some(value)
                    }
                    Ok(ExecutionEvent::Started { .. }) => {}
                    Err(execution_error) => {
                        error = Some(execution_error.to_string());
                        break;
                    }
                }
            }

            let mut assertions = Vec::new();
            if error.is_none() && !step.post_script.trim().is_empty() {
                let response = ScriptResponse {
                    status: status.unwrap_or_default(),
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                };
                match self
                    .scripts
                    .run(&step.post_script, &variables, Some(&response))
                {
                    Ok(result) => {
                        variables = result.variables;
                        assertions = result.assertions;
                    }
                    Err(script_error) => error = Some(script_error.to_string()),
                }
            }
            let passed = error.is_none() && assertions.iter().all(|assertion| assertion.passed);
            reports.push(StepReport {
                name: step.name,
                passed,
                status,
                metrics,
                assertions,
                error,
            });
            if !passed && scenario.stop_on_failure {
                break;
            }
        }

        let passed = reports.len() == expected_steps && reports.iter().all(|step| step.passed);
        Ok(ScenarioReport {
            name: scenario.name,
            passed,
            elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            steps: reports,
        })
    }

    pub async fn run_test_scenario(
        &self,
        scenario: TestScenario,
        cases: Arc<HashMap<EntityId, ScenarioCase>>,
        initial_variables: BTreeMap<String, String>,
        cancellation: CancellationToken,
    ) -> Result<ScenarioReport, ScenarioError> {
        let started = Instant::now();
        let mut state = StructuredState {
            variables: initial_variables,
            reports: Vec::new(),
        };
        let completed = self
            .run_nodes(
                &scenario.nodes,
                &cases,
                scenario.stop_on_failure,
                &mut state,
                &cancellation,
            )
            .await?;
        let passed = completed && state.reports.iter().all(|step| step.passed);
        Ok(ScenarioReport {
            name: scenario.name,
            passed,
            elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            steps: state.reports,
        })
    }

    fn run_nodes<'a>(
        &'a self,
        nodes: &'a [ScenarioNode],
        cases: &'a HashMap<EntityId, ScenarioCase>,
        stop_on_failure: bool,
        state: &'a mut StructuredState,
        cancellation: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<bool, ScenarioError>> {
        Box::pin(async move {
            for node in nodes {
                if cancellation.is_cancelled() {
                    return Err(ScenarioError::Cancelled);
                }
                match node {
                    ScenarioNode::Request {
                        case_id,
                        assertions,
                        extractors,
                    } => {
                        let case = cases
                            .get(case_id)
                            .ok_or(ScenarioError::MissingCase(*case_id))?;
                        let report = self
                            .run_case(
                                case,
                                assertions,
                                extractors,
                                &mut state.variables,
                                cancellation,
                            )
                            .await?;
                        let passed = report.passed;
                        state.reports.push(report);
                        if !passed && stop_on_failure {
                            return Ok(false);
                        }
                    }
                    ScenarioNode::Group { nodes, .. } => {
                        if !self
                            .run_nodes(nodes, cases, stop_on_failure, state, cancellation)
                            .await?
                        {
                            return Ok(false);
                        }
                    }
                    ScenarioNode::If {
                        expression,
                        then_nodes,
                        else_nodes,
                    } => {
                        let value = evaluate_expression(
                            &self.scripts,
                            expression,
                            &mut state.variables,
                            None,
                        )
                        .map_err(ScenarioError::InvalidExpression)?;
                        let condition = value.as_bool().ok_or_else(|| {
                            ScenarioError::InvalidExpression(
                                "condition must evaluate to a boolean".into(),
                            )
                        })?;
                        let branch = if condition { then_nodes } else { else_nodes };
                        if !self
                            .run_nodes(branch, cases, stop_on_failure, state, cancellation)
                            .await?
                        {
                            return Ok(false);
                        }
                    }
                    ScenarioNode::Loop {
                        source,
                        item_name,
                        max_iterations,
                        nodes,
                    } => {
                        if *max_iterations == 0 {
                            return Err(ScenarioError::InvalidLoop(
                                "iteration limit must be positive".into(),
                            ));
                        }
                        let value = if let Some(value) = state.variables.get(source) {
                            serde_json::from_str(value).map_err(|error| {
                                ScenarioError::InvalidLoop(format!(
                                    "variable {source} is not valid JSON: {error}"
                                ))
                            })?
                        } else {
                            evaluate_expression(&self.scripts, source, &mut state.variables, None)
                                .map_err(ScenarioError::InvalidLoop)?
                        };
                        let values = value.as_array().ok_or_else(|| {
                            ScenarioError::InvalidLoop(
                                "loop source must evaluate to an array".into(),
                            )
                        })?;
                        let index_name = format!("{item_name}_index");
                        let previous_item = state.variables.get(item_name).cloned();
                        let previous_index = state.variables.get(&index_name).cloned();
                        for (index, value) in
                            values.iter().take(*max_iterations as usize).enumerate()
                        {
                            state
                                .variables
                                .insert(item_name.clone(), json_value_text(value));
                            state
                                .variables
                                .insert(index_name.clone(), index.to_string());
                            if !self
                                .run_nodes(nodes, cases, stop_on_failure, state, cancellation)
                                .await?
                            {
                                restore_variable(&mut state.variables, item_name, previous_item);
                                restore_variable(&mut state.variables, &index_name, previous_index);
                                return Ok(false);
                            }
                        }
                        restore_variable(&mut state.variables, item_name, previous_item);
                        restore_variable(&mut state.variables, &index_name, previous_index);
                    }
                    ScenarioNode::Delay { millis } => {
                        tokio::select! {
                            _ = cancellation.cancelled() => {
                                return Err(ScenarioError::Cancelled);
                            }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(*millis)) => {}
                        }
                    }
                }
            }
            Ok(true)
        })
    }

    async fn run_case(
        &self,
        case: &ScenarioCase,
        node_assertions: &[AssertionRule],
        node_extractors: &[ExtractorRule],
        variables: &mut BTreeMap<String, String>,
        cancellation: &CancellationToken,
    ) -> Result<StepReport, ScenarioError> {
        let mut script_assertions = Vec::new();
        if !case.pre_script.trim().is_empty() {
            match self
                .scripts
                .run_async(&case.pre_script, variables, None)
                .await
            {
                Ok(result) => {
                    *variables = result.variables;
                    script_assertions = result.assertions;
                    if script_assertions.iter().any(|assertion| !assertion.passed) {
                        return Ok(StepReport {
                            name: case.name.clone(),
                            passed: false,
                            status: None,
                            metrics: None,
                            assertions: script_assertions,
                            error: None,
                        });
                    }
                }
                Err(error) => {
                    return Ok(StepReport {
                        name: case.name.clone(),
                        passed: false,
                        status: None,
                        metrics: None,
                        assertions: Vec::new(),
                        error: Some(error.to_string()),
                    });
                }
            }
        }

        let kind = case.request.protocol.kind();
        let executor = self
            .executors
            .get(&kind)
            .ok_or(ScenarioError::MissingExecutor(kind))?;
        let mut request = case.request.clone();
        request
            .local_variables
            .extend(variables.iter().map(|(name, value)| {
                Variable::scoped(name.clone(), value.clone(), VariableScope::Local)
            }));
        let mut stream = executor.execute(request, cancellation.child_token());
        let mut status = None;
        let mut headers = BTreeMap::new();
        let mut body = Vec::new();
        let mut metrics = None;
        let mut error = None;
        loop {
            let event = tokio::select! {
                _ = cancellation.cancelled() => return Err(ScenarioError::Cancelled),
                event = stream.next() => event
            };
            let Some(event) = event else {
                break;
            };
            match event {
                Ok(ExecutionEvent::ResponseHead(head)) => {
                    status = head.status;
                    for (name, values) in head.headers {
                        headers.insert(name, values.join(", "));
                    }
                }
                Ok(ExecutionEvent::Data(data)) | Ok(ExecutionEvent::Message { data, .. }) => {
                    // Assertions never need more than the UI preview keeps; an
                    // uncapped buffer let one huge endpoint exhaust memory.
                    let available = MAX_SCENARIO_BODY_BYTES.saturating_sub(body.len());
                    body.extend_from_slice(&data[..data.len().min(available)]);
                }
                Ok(ExecutionEvent::Metrics(value)) | Ok(ExecutionEvent::Completed(value)) => {
                    metrics = Some(value);
                }
                Ok(ExecutionEvent::Started { .. }) => {}
                Err(execution_error) => {
                    error = Some(execution_error.to_string());
                    break;
                }
            }
        }
        let mut response = CapturedResponse {
            status,
            headers,
            body: String::from_utf8_lossy(&body).into_owned(),
            metrics,
            error,
        };
        if response.error.is_none() {
            let script_response = ScriptResponse {
                status: response.status.unwrap_or_default(),
                headers: response.headers.clone(),
                body: response.body.clone(),
            };
            for assertion in case.assertions.iter().chain(node_assertions) {
                script_assertions.push(evaluate_assertion(
                    &self.scripts,
                    assertion,
                    &response,
                    variables,
                    &script_response,
                    case.response_schema.as_ref(),
                ));
            }
            for extractor in case.extractors.iter().chain(node_extractors) {
                match extract_value(extractor, &response) {
                    Ok(value) => {
                        variables.insert(extractor.name.clone(), value);
                    }
                    Err(extract_error) => {
                        response.error = Some(format!(
                            "failed to extract variable {}: {extract_error}",
                            extractor.name
                        ));
                        break;
                    }
                }
            }
            if response.error.is_none() && !case.post_script.trim().is_empty() {
                match self
                    .scripts
                    .run_async(&case.post_script, variables, Some(&script_response))
                    .await
                {
                    Ok(result) => {
                        *variables = result.variables;
                        script_assertions.extend(result.assertions);
                    }
                    Err(script_error) => response.error = Some(script_error.to_string()),
                }
            }
        }
        let passed =
            response.error.is_none() && script_assertions.iter().all(|assertion| assertion.passed);
        Ok(StepReport {
            name: case.name.clone(),
            passed,
            status: response.status,
            metrics: response.metrics,
            assertions: script_assertions,
            error: response.error,
        })
    }
}

impl Default for ScenarioRunner {
    fn default() -> Self {
        Self::new()
    }
}
