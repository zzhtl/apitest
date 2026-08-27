use std::collections::BTreeMap;

use apitest_core::{ApiContract, AssertionRule, ExtractorRule, Variable, VariableScope};
use apitest_runtime::{
    CapturedResponse, ResponseSchemaContract, ScriptEngine, ScriptResponse, evaluate_assertion,
    extract_value,
};
use eframe::egui;

use crate::app::ApiTestApp;
use crate::i18n::Language;
use crate::state::action::{RuntimeMessage, ToastKind};
use crate::state::response::RunState;
use crate::state::verification::VerificationOutcome;
use crate::ui::code::KnownVariables;
use crate::workbench::DocumentId;

/// The inputs a run needs before it can be judged, gathered on the UI thread so
/// the evaluation itself can move to the background.
struct VerificationJob {
    language: Language,
    scripts: ScriptEngine,
    post_script: String,
    assertions: Vec<AssertionRule>,
    extractors: Vec<ExtractorRule>,
    schema: Option<ResponseSchemaContract>,
    response: CapturedResponse,
    variables: BTreeMap<String, String>,
}

impl ApiTestApp {
    /// Variables visible to scripts: the active environment plus the request's
    /// own locals, secrets excluded so a script cannot echo one back.
    pub(crate) fn script_variables(&self, request_index: usize) -> BTreeMap<String, String> {
        let mut variables = BTreeMap::new();
        if let Some(environment) = self
            .environments
            .iter()
            .find(|environment| environment.id() == self.active_environment)
        {
            for variable in &environment.to_environment().variables {
                if variable.enabled && !variable.is_secret() {
                    variables.insert(variable.name.clone(), variable.value.clone());
                }
            }
        }
        if let Some(request) = self.requests.get(request_index) {
            for variable in &request.request_case.local_variables {
                if variable.enabled && !variable.is_secret() {
                    variables.insert(variable.name.clone(), variable.value.clone());
                }
            }
        }
        variables
    }

    /// Names the active environment and the selected request can resolve.
    pub(crate) fn known_variables(&self) -> KnownVariables {
        let mut names = self
            .script_variables(self.selected)
            .into_keys()
            .collect::<Vec<_>>();
        // Secrets resolve at send time even though scripts never see them.
        if let Some(environment) = self
            .environments
            .iter()
            .find(|environment| environment.id() == self.active_environment)
        {
            names.extend(
                environment
                    .variables
                    .iter()
                    .filter(|variable| variable.enabled)
                    .map(|variable| variable.name.clone()),
            );
        }
        if let Some(request) = self.requests.get(self.selected) {
            names.extend(
                request
                    .request_case
                    .local_variables
                    .iter()
                    .map(|variable| variable.name.clone()),
            );
        }
        KnownVariables::from_names(names)
    }

    /// The URL as it will actually be sent, plus any names that did not resolve.
    ///
    /// Secret values are never substituted here — the preview shows the
    /// reference so a token cannot end up on screen.
    pub(crate) fn resolved_url(&self, template: &str) -> (String, Vec<String>) {
        if !template.contains("{{") {
            return (template.to_owned(), Vec::new());
        }
        let Some(environment) = self
            .environments
            .iter()
            .find(|environment| environment.id() == self.active_environment)
        else {
            return (template.to_owned(), Vec::new());
        };
        let mut resolvable = environment.to_environment();
        resolvable.variables = resolvable
            .variables
            .iter()
            .map(|variable| {
                if variable.is_secret() {
                    variable.with_materialized_secret("••••••")
                } else {
                    variable.clone()
                }
            })
            .collect();
        let overlays = self
            .requests
            .get(self.selected)
            .map(|request| request.request_case.local_variables.clone())
            .unwrap_or_default();
        match resolvable.resolve(template, &overlays) {
            Ok(resolved) => (resolved.value().to_owned(), resolved.missing().to_vec()),
            Err(error) => (error.to_string(), Vec::new()),
        }
    }

    /// Run the pre-request script and return the variables it produced.
    ///
    /// Deliberately synchronous: the user just pressed Send, the engine caps
    /// itself at two seconds, and `send_current` already blocks on the system
    /// keyring right before this.
    pub(crate) fn run_pre_request_script(
        &mut self,
        request_index: usize,
    ) -> Result<Vec<Variable>, String> {
        let Some(request) = self.requests.get(request_index) else {
            return Ok(Vec::new());
        };
        let script = request.request_case.pre_request_script.clone();
        if script.trim().is_empty() {
            return Ok(Vec::new());
        }
        let variables = self.script_variables(request_index);
        let result = self
            .scripts
            .run(&script, &variables, None)
            .map_err(|error| match self.language {
                Language::Chinese => format!("前置脚本：{error}"),
                Language::English => format!("pre-request script: {error}"),
            })?;
        if let Some(failed) = result.assertions.iter().find(|assertion| !assertion.passed) {
            let detail = failed.error.clone().unwrap_or_else(|| failed.name.clone());
            return Err(match self.language {
                Language::Chinese => format!("前置断言未通过：{detail}"),
                Language::English => format!("pre-request assertion failed: {detail}"),
            });
        }
        Ok(result
            .variables
            .into_iter()
            .filter(|(name, value)| variables.get(name) != Some(value))
            .map(|(name, value)| Variable::scoped(name, value, VariableScope::Local))
            .collect())
    }

    /// Judge a finished run on the background runtime.
    pub(crate) fn spawn_verification(
        &mut self,
        document: DocumentId,
        run: u64,
        context: &egui::Context,
    ) {
        let Some(job) = self.verification_job(document) else {
            return;
        };
        let sender = self.sender.clone();
        let context = context.clone();
        self.runtime.spawn(async move {
            let outcome = evaluate(job);
            let _ = sender.send(RuntimeMessage::Verified(run, Box::new(outcome)));
            context.request_repaint();
        });
    }

    fn verification_job(&mut self, document: DocumentId) -> Option<VerificationJob> {
        let index = self
            .requests
            .iter()
            .position(|request| request.id() == document.entity_id)?;
        let request = &self.requests[index];
        let case = &request.request_case;
        if case.post_response_script.trim().is_empty()
            && case.assertions.is_empty()
            && case.extractors.is_empty()
        {
            return None;
        }
        let post_script = case.post_response_script.clone();
        let assertions = case.assertions.clone();
        let extractors = case.extractors.clone();
        let schema = match &request.definition.contract {
            ApiContract::Http(contract) => Some(ResponseSchemaContract {
                responses: contract.responses.clone(),
                components: self.project.components.clone(),
            }),
            _ => None,
        };
        let variables = self.script_variables(index);
        let session = self.sessions.entry(document);
        let response = CapturedResponse {
            status: session.response.status,
            headers: session.response.headers.iter().cloned().collect(),
            body: session.response.body.clone(),
            metrics: session.response.metrics,
            error: session.response.error.clone(),
        };
        Some(VerificationJob {
            language: self.language,
            scripts: self.scripts.clone(),
            post_script,
            assertions,
            extractors,
            schema,
            response,
            variables,
        })
    }

    pub(crate) fn apply_verification(&mut self, run: u64, outcome: VerificationOutcome) {
        let Some(document) = self.sessions.owner(run) else {
            return;
        };
        let extracted = outcome.extracted.clone();
        let failed = outcome.failed_count();
        let error = outcome.error.clone();
        let passed = outcome.passed();
        let session = self.sessions.entry(document);
        session.verification = Some(outcome);
        if session.response.state == RunState::Completed && !passed {
            // A completed transfer with a failed assertion is still a failed run.
            session.response.state = RunState::Failed;
        }
        self.store_extracted_variables(&extracted);
        if let Some(error) = error {
            self.toast(ToastKind::Error, error);
        } else if failed > 0 {
            self.toast(
                ToastKind::Error,
                match self.language {
                    crate::i18n::Language::Chinese => format!("{failed} 条断言未通过"),
                    crate::i18n::Language::English => format!("{failed} assertions failed"),
                },
            );
        }
    }

    /// Extractors feed the active environment, which is where the next request
    /// will look the variables up.
    fn store_extracted_variables(&mut self, extracted: &[(String, String)]) {
        if extracted.is_empty() {
            return;
        }
        let Some(index) = self
            .environments
            .iter()
            .position(|environment| environment.id() == self.active_environment)
        else {
            return;
        };
        for (name, value) in extracted {
            self.environments[index].set_plain_value(name, value);
        }
        self.toast(
            ToastKind::Info,
            match self.language {
                crate::i18n::Language::Chinese => {
                    format!("已提取 {} 个变量到当前环境", extracted.len())
                }
                crate::i18n::Language::English => {
                    format!(
                        "Extracted {} variables into the environment",
                        extracted.len()
                    )
                }
            },
        );
    }
}

fn evaluate(job: VerificationJob) -> VerificationOutcome {
    let mut outcome = VerificationOutcome::default();
    let mut variables = job.variables;
    let script_response = ScriptResponse {
        status: job.response.status.unwrap_or_default(),
        headers: job.response.headers.clone(),
        body: job.response.body.clone(),
    };
    for rule in &job.assertions {
        outcome.assertions.push(evaluate_assertion(
            &job.scripts,
            rule,
            &job.response,
            &mut variables,
            &script_response,
            job.schema.as_ref(),
        ));
    }
    for rule in &job.extractors {
        match extract_value(rule, &job.response) {
            Ok(value) => {
                variables.insert(rule.name.clone(), value.clone());
                outcome.extracted.push((rule.name.clone(), value));
            }
            Err(error) => {
                outcome.error = Some(match job.language {
                    Language::Chinese => format!("提取 {} 失败：{error}", rule.name),
                    Language::English => format!("failed to extract {}: {error}", rule.name),
                });
                return outcome;
            }
        }
    }
    if !job.post_script.trim().is_empty() {
        match job
            .scripts
            .run(&job.post_script, &variables, Some(&script_response))
        {
            Ok(result) => {
                outcome.assertions.extend(result.assertions);
                for (name, value) in result.variables {
                    if variables.get(&name) != Some(&value) {
                        outcome.extracted.push((name, value));
                    }
                }
            }
            Err(error) => {
                outcome.error = Some(match job.language {
                    Language::Chinese => format!("后置脚本：{error}"),
                    Language::English => format!("post-response script: {error}"),
                });
            }
        }
    }
    outcome
}
