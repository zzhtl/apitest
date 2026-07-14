use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::EntityId;

const REDACTED_VALUE: &str = "••••••••";
const MAX_RESOLUTION_DEPTH: usize = 64;

/// A stable pointer to a secret stored outside project data.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(pub String);

impl SecretRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Variable precedence increases from global to local.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariableScope {
    Global,
    Environment,
    Local,
}

/// A non-secret value or an external secret reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    pub id: EntityId,
    pub name: String,
    pub value: String,
    pub scope: VariableScope,
    pub enabled: bool,
    pub secret_ref: Option<SecretRef>,
}

impl Variable {
    pub fn plain(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self::scoped(name, value, VariableScope::Environment)
    }

    pub fn scoped(name: impl Into<String>, value: impl Into<String>, scope: VariableScope) -> Self {
        Self {
            id: EntityId::new(),
            name: name.into(),
            value: value.into(),
            scope,
            enabled: true,
            secret_ref: None,
        }
    }

    pub fn secret(name: impl Into<String>, reference: impl Into<String>) -> Self {
        Self {
            id: EntityId::new(),
            name: name.into(),
            value: String::new(),
            scope: VariableScope::Environment,
            enabled: true,
            secret_ref: Some(SecretRef::new(reference)),
        }
    }

    pub fn is_secret(&self) -> bool {
        self.secret_ref.is_some()
    }

    pub fn with_materialized_secret(&self, value: impl Into<String>) -> Self {
        let mut materialized = self.clone();
        materialized.value = value.into();
        materialized
    }

    fn redacted(&self) -> Self {
        let mut redacted = self.clone();
        if redacted.is_secret() {
            redacted.value = REDACTED_VALUE.to_owned();
        }
        redacted
    }
}

/// A named variable set selected for request execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub id: EntityId,
    pub name: String,
    pub variables: Vec<Variable>,
}

impl Environment {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: EntityId::new(),
            name: name.into(),
            variables: Vec::new(),
        }
    }

    pub fn with_variable(mut self, variable: Variable) -> Self {
        self.variables.push(variable);
        self
    }

    pub fn resolve(
        &self,
        template: &str,
        overlays: &[Variable],
    ) -> Result<ResolvedText, EnvironmentError> {
        let mut candidates: Vec<&Variable> = self
            .variables
            .iter()
            .chain(overlays.iter())
            .filter(|variable| variable.enabled)
            .collect();
        candidates.sort_by_key(|variable| variable.scope);

        let mut values = HashMap::with_capacity(candidates.len());
        for variable in candidates {
            values.insert(variable.name.as_str(), variable.value.as_str());
        }

        let mut missing = Vec::new();
        let mut missing_seen = HashSet::new();
        let value = resolve_text(
            template,
            &values,
            &mut Vec::new(),
            &mut missing,
            &mut missing_seen,
            0,
        )?;

        Ok(ResolvedText { value, missing })
    }

    pub fn redacted_variables(&self) -> Vec<Variable> {
        self.variables.iter().map(Variable::redacted).collect()
    }
}

/// The rendered text and names that could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedText {
    value: String,
    missing: Vec<String>,
}

impl ResolvedText {
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn missing(&self) -> &[String] {
        &self.missing
    }

    pub fn into_value(self) -> String {
        self.value
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EnvironmentError {
    #[error("variable cycle detected: {path}")]
    Cycle { path: String },
    #[error("variable nesting exceeds {MAX_RESOLUTION_DEPTH} levels")]
    TooDeep,
}

fn resolve_text(
    template: &str,
    values: &HashMap<&str, &str>,
    stack: &mut Vec<String>,
    missing: &mut Vec<String>,
    missing_seen: &mut HashSet<String>,
    depth: usize,
) -> Result<String, EnvironmentError> {
    if depth > MAX_RESOLUTION_DEPTH {
        return Err(EnvironmentError::TooDeep);
    }

    let mut output = String::with_capacity(template.len());
    let mut cursor = 0;

    while let Some(relative_start) = template[cursor..].find("{{") {
        let start = cursor + relative_start;
        output.push_str(&template[cursor..start]);
        let name_start = start + 2;
        let Some(relative_end) = template[name_start..].find("}}") else {
            output.push_str(&template[start..]);
            return Ok(output);
        };
        let end = name_start + relative_end;
        let name = template[name_start..end].trim();

        if let Some(value) = values.get(name) {
            if let Some(cycle_start) = stack.iter().position(|item| item == name) {
                let mut path = stack[cycle_start..].to_vec();
                path.push(name.to_owned());
                return Err(EnvironmentError::Cycle {
                    path: path.join(" -> "),
                });
            }

            stack.push(name.to_owned());
            output.push_str(&resolve_text(
                value,
                values,
                stack,
                missing,
                missing_seen,
                depth + 1,
            )?);
            stack.pop();
        } else {
            let token_end = end + 2;
            output.push_str(&template[start..token_end]);
            if missing_seen.insert(name.to_owned()) {
                missing.push(name.to_owned());
            }
        }

        cursor = end + 2;
    }

    output.push_str(&template[cursor..]);
    Ok(output)
}
