use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Instant,
};

use apitest_core::{
    ExecutionEvent, ExecutionMetrics, ExecutionRequest, ProtocolExecutor, ProtocolKind, Variable,
    VariableScope,
};
use futures::StreamExt;
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
    #[error("scenario was cancelled")]
    Cancelled,
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
}

impl Default for ScenarioRunner {
    fn default() -> Self {
        Self::new()
    }
}
