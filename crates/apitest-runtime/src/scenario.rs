use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Instant,
};

use apitest_core::{
    ApiComponents, ApiResponse, ApiSchema, AssertionRule, EntityId, ExecutionEvent,
    ExecutionMetrics, ExecutionRequest, ExtractorRule, ProtocolExecutor, ProtocolKind,
    ScenarioNode, TestScenario, Variable, VariableScope, VariableSource,
};
use futures::{StreamExt, future::BoxFuture};
use regex::Regex;
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

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
pub struct ResponseSchemaContract {
    pub responses: Vec<ApiResponse>,
    pub components: ApiComponents,
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

struct CapturedResponse {
    status: Option<u16>,
    headers: BTreeMap<String, String>,
    body: String,
    metrics: Option<ExecutionMetrics>,
    error: Option<String>,
}

pub struct ScenarioRunner {
    executors: HashMap<ProtocolKind, Arc<dyn ProtocolExecutor>>,
    scripts: ScriptEngine,
}

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
        cases: HashMap<EntityId, ScenarioCase>,
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
                        let value = self
                            .evaluate_expression(expression, &mut state.variables, None)
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
                            self.evaluate_expression(source, &mut state.variables, None)
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
            match self.scripts.run(&case.pre_script, variables, None) {
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
                event = stream.next() => event,
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
                    body.extend_from_slice(&data);
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
                script_assertions.push(self.evaluate_assertion(
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
                    .run(&case.post_script, variables, Some(&script_response))
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

    fn evaluate_assertion(
        &self,
        rule: &AssertionRule,
        response: &CapturedResponse,
        variables: &mut BTreeMap<String, String>,
        script_response: &ScriptResponse,
        response_schema: Option<&ResponseSchemaContract>,
    ) -> ScriptAssertion {
        let (name, result) = match rule {
            AssertionRule::Status { expected } => (
                format!("Status = {expected}"),
                response
                    .status
                    .filter(|actual| actual == expected)
                    .map(|_| ())
                    .ok_or_else(|| {
                        format!(
                            "expected status {expected}, got {}",
                            response
                                .status
                                .map(|status| status.to_string())
                                .unwrap_or_else(|| "none".into())
                        )
                    }),
            ),
            AssertionRule::HeaderEquals { name, expected } => {
                let actual = header_value(&response.headers, name);
                (
                    format!("Header {name} = {expected}"),
                    actual
                        .filter(|actual| *actual == expected)
                        .map(|_| ())
                        .ok_or_else(|| {
                            format!(
                                "expected header {name} to equal {expected}, got {}",
                                actual.unwrap_or("none")
                            )
                        }),
                )
            }
            AssertionRule::JsonPathEquals { path, expected } => {
                let result = serde_json::from_str::<Value>(&response.body)
                    .map_err(|error| format!("response is not valid JSON: {error}"))
                    .and_then(|body| {
                        json_path(&body, path)
                            .and_then(|value| {
                                value.ok_or_else(|| format!("JSONPath {path} did not match"))
                            })
                            .map(json_value_text)
                    })
                    .and_then(|actual| {
                        (actual == *expected)
                            .then_some(())
                            .ok_or_else(|| format!("expected {expected}, got {actual}"))
                    });
                (format!("JSONPath {path} = {expected}"), result)
            }
            AssertionRule::Schema => (
                "Schema valid".into(),
                validate_response_schema(response_schema, response.status, &response.body),
            ),
            AssertionRule::DurationBelow { milliseconds } => {
                let actual = response.metrics.map(|metrics| metrics.elapsed_ms);
                (
                    format!("Duration < {milliseconds} ms"),
                    actual
                        .filter(|actual| actual < milliseconds)
                        .map(|_| ())
                        .ok_or_else(|| {
                            format!(
                                "expected duration below {milliseconds} ms, got {}",
                                actual
                                    .map(|value| format!("{value} ms"))
                                    .unwrap_or_else(|| "none".into())
                            )
                        }),
                )
            }
            AssertionRule::Script { expression } => (
                "Script expression".into(),
                self.evaluate_expression(expression, variables, Some(script_response))
                    .and_then(|value| {
                        value
                            .as_bool()
                            .filter(|value| *value)
                            .map(|_| ())
                            .ok_or_else(|| "expression did not evaluate to true".into())
                    }),
            ),
        };
        match result {
            Ok(()) => ScriptAssertion {
                name,
                passed: true,
                error: None,
            },
            Err(error) => ScriptAssertion {
                name,
                passed: false,
                error: Some(error),
            },
        }
    }

    fn evaluate_expression(
        &self,
        expression: &str,
        variables: &mut BTreeMap<String, String>,
        response: Option<&ScriptResponse>,
    ) -> Result<Value, String> {
        const RESULT_VARIABLE: &str = "__apitest_expression_result";
        let previous = variables.get(RESULT_VARIABLE).cloned();
        let script =
            format!("apitest.setVariable(\"{RESULT_VARIABLE}\", JSON.stringify(({expression})));");
        let mut result = self
            .scripts
            .run(&script, variables, response)
            .map_err(|error| error.to_string())?;
        let raw = result
            .variables
            .remove(RESULT_VARIABLE)
            .ok_or_else(|| "expression returned no value".to_owned())?;
        restore_variable(&mut result.variables, RESULT_VARIABLE, previous);
        *variables = result.variables;
        serde_json::from_str(&raw)
            .map_err(|error| format!("expression returned invalid JSON: {error}"))
    }
}

fn validate_response_schema(
    contract: Option<&ResponseSchemaContract>,
    status: Option<u16>,
    body: &str,
) -> Result<(), String> {
    let contract = contract.ok_or_else(|| "request has no response contract schema".to_owned())?;
    let status = status.ok_or_else(|| "response has no status for schema selection".to_owned())?;
    let response = contract
        .responses
        .iter()
        .find(|response| response.status == status.to_string())
        .or_else(|| {
            contract.responses.iter().find(|response| {
                let pattern = response.status.as_bytes();
                pattern.len() == 3
                    && pattern[0].is_ascii_digit()
                    && pattern[0] == status.to_string().as_bytes()[0]
                    && pattern[1..].eq_ignore_ascii_case(b"xx")
            })
        })
        .or_else(|| {
            contract
                .responses
                .iter()
                .find(|response| response.status.eq_ignore_ascii_case("default"))
        })
        .ok_or_else(|| format!("contract has no response schema for status {status}"))?;
    let content = response
        .content
        .iter()
        .find(|content| {
            let media_type = content.media_type.to_ascii_lowercase();
            media_type == "application/json" || media_type.ends_with("+json")
        })
        .or_else(|| response.content.first())
        .ok_or_else(|| format!("contract response {status} has no content schema"))?;
    let instance = serde_json::from_str::<Value>(body)
        .map_err(|error| format!("response is not valid JSON: {error}"))?;
    let schema = schema_document(&content.schema, &contract.components);
    let validator = jsonschema::validator_for(&schema)
        .map_err(|error| format!("contract response schema is invalid: {error}"))?;
    validator
        .validate(&instance)
        .map_err(|error| format!("response does not match contract schema: {error}"))
}

fn schema_document(schema: &ApiSchema, components: &ApiComponents) -> Value {
    let definitions = components
        .schemas
        .iter()
        .map(|(name, schema)| (name.clone(), schema_value(schema)))
        .collect::<serde_json::Map<_, _>>();
    let mut root = schema_value(schema);
    match &mut root {
        Value::Object(object) => {
            object.insert("$defs".into(), Value::Object(definitions));
        }
        _ => {
            root = serde_json::json!({
                "allOf": [root],
                "$defs": definitions,
            });
        }
    }
    normalize_component_references(&mut root);
    root
}

fn schema_value(schema: &ApiSchema) -> Value {
    match schema {
        ApiSchema::Inline(value) => value.clone(),
        ApiSchema::Component(name) => serde_json::json!({
            "$ref": format!("#/$defs/{}", json_pointer_token(name)),
        }),
    }
}

fn normalize_component_references(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref")
                && let Some(name) = reference.strip_prefix("#/components/schemas/")
            {
                *reference = format!("#/$defs/{name}");
            }
            for value in object.values_mut() {
                normalize_component_references(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_component_references(value);
            }
        }
        _ => {}
    }
}

fn json_pointer_token(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn restore_variable(
    variables: &mut BTreeMap<String, String>,
    name: &str,
    previous: Option<String>,
) {
    if let Some(value) = previous {
        variables.insert(name.to_owned(), value);
    } else {
        variables.remove(name);
    }
}

fn header_value<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn extract_value(rule: &ExtractorRule, response: &CapturedResponse) -> Result<String, String> {
    match &rule.source {
        VariableSource::JsonPath(path) => {
            let body = serde_json::from_str::<Value>(&response.body)
                .map_err(|error| format!("response is not valid JSON: {error}"))?;
            json_path(&body, path)?
                .map(json_value_text)
                .ok_or_else(|| format!("JSONPath {path} did not match"))
        }
        VariableSource::Header(name) => header_value(&response.headers, name)
            .map(str::to_owned)
            .ok_or_else(|| format!("response header {name} was not found")),
        VariableSource::Cookie(name) => {
            let set_cookie = header_value(&response.headers, "set-cookie")
                .ok_or_else(|| "response has no set-cookie header".to_owned())?;
            let pattern = Regex::new(&format!(r"(?:^|[,;]\s*){}=([^;,]*)", regex::escape(name)))
                .map_err(|error| error.to_string())?;
            pattern
                .captures(set_cookie)
                .and_then(|captures| captures.get(1))
                .map(|value| value.as_str().to_owned())
                .ok_or_else(|| format!("response cookie {name} was not found"))
        }
        VariableSource::Regex(pattern) => {
            let pattern = Regex::new(pattern).map_err(|error| error.to_string())?;
            let captures = pattern
                .captures(&response.body)
                .ok_or_else(|| "regular expression did not match the response body".to_owned())?;
            captures
                .get(1)
                .or_else(|| captures.get(0))
                .map(|value| value.as_str().to_owned())
                .ok_or_else(|| "regular expression produced no capture".to_owned())
        }
    }
}

fn json_path<'a>(value: &'a Value, path: &str) -> Result<Option<&'a Value>, String> {
    let Some(mut rest) = path.strip_prefix('$') else {
        return Err("JSONPath must start with $".into());
    };
    let mut current = Some(value);
    while !rest.is_empty() {
        if let Some(after_dot) = rest.strip_prefix('.') {
            let end = after_dot.find(['.', '[']).unwrap_or(after_dot.len());
            let name = &after_dot[..end];
            if name.is_empty() {
                return Err("JSONPath property name cannot be empty".into());
            }
            current = current.and_then(|value| value.get(name));
            rest = &after_dot[end..];
            continue;
        }
        if let Some(after_bracket) = rest.strip_prefix('[') {
            let end = after_bracket
                .find(']')
                .ok_or_else(|| "JSONPath bracket is not closed".to_owned())?;
            let token = after_bracket[..end].trim();
            if let Ok(index) = token.parse::<usize>() {
                current = current.and_then(|value| value.get(index));
            } else {
                let name = token
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .or_else(|| {
                        token
                            .strip_prefix('\'')
                            .and_then(|value| value.strip_suffix('\''))
                    })
                    .ok_or_else(|| {
                        "JSONPath brackets must contain an array index or quoted property"
                            .to_owned()
                    })?;
                current = current.and_then(|value| value.get(name));
            }
            rest = &after_bracket[end + 1..];
            continue;
        }
        return Err(format!("unsupported JSONPath segment {rest}"));
    }
    Ok(current)
}

fn json_value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(value).expect("JSON value should serialize"))
}

impl Default for ScenarioRunner {
    fn default() -> Self {
        Self::new()
    }
}
