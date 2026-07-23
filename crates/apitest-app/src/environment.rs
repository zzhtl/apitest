use std::time::{Duration, Instant};

use apitest_core::{EntityId, Environment, SecretRef, Variable, VariableScope};

use crate::workbench::AutoSaveState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditableVariable {
    pub id: EntityId,
    pub enabled: bool,
    pub name: String,
    pub value: String,
    pub secret: bool,
    pub reference: Option<SecretRef>,
    pub configured: bool,
}

impl EditableVariable {
    pub fn empty() -> Self {
        Self {
            id: EntityId::new(),
            enabled: true,
            name: String::new(),
            value: String::new(),
            secret: false,
            reference: None,
            configured: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.value.is_empty() && self.reference.is_none()
    }

    pub fn is_ready(&self) -> bool {
        !self.secret || self.configured || !self.value.is_empty()
    }

    pub fn has_pending_secret(&self) -> bool {
        self.secret && !self.value.is_empty()
    }

    pub fn ensure_reference(&mut self, environment_id: EntityId) -> &SecretRef {
        self.reference.get_or_insert_with(|| {
            SecretRef::new(format!(
                "keyring://environment-{environment_id}-variable-{}",
                self.id
            ))
        })
    }

    pub fn mark_secret_saved(&mut self) {
        self.configured = true;
        self.value.clear();
    }

    fn to_variable(&self, environment_id: EntityId) -> Variable {
        Variable {
            id: self.id,
            name: self.name.clone(),
            value: if self.secret {
                String::new()
            } else {
                self.value.clone()
            },
            scope: VariableScope::Environment,
            enabled: self.enabled,
            secret_ref: self.secret.then(|| {
                self.reference.clone().unwrap_or_else(|| {
                    SecretRef::new(format!(
                        "keyring://environment-{environment_id}-variable-{}",
                        self.id
                    ))
                })
            }),
        }
    }
}

impl From<&Variable> for EditableVariable {
    fn from(value: &Variable) -> Self {
        Self {
            id: value.id,
            enabled: value.enabled,
            name: value.name.clone(),
            value: if value.is_secret() {
                String::new()
            } else {
                value.value.clone()
            },
            secret: value.is_secret(),
            reference: value.secret_ref.clone(),
            configured: value.is_secret(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnvironmentDraft {
    pub saved: Environment,
    pub name: String,
    pub variables: Vec<EditableVariable>,
    pub persisted: bool,
    pub autosave: AutoSaveState,
    observed_snapshot: Vec<u8>,
}

impl EnvironmentDraft {
    pub fn new(name: impl Into<String>) -> Self {
        let environment = Environment::new(name);
        let name = environment.name.clone();
        let variables = Vec::new();
        let observed_snapshot = environment_snapshot(&name, &variables);
        let mut autosave = AutoSaveState::new(Duration::from_millis(500));
        autosave.mark_changed(Instant::now());
        let mut draft = Self {
            name,
            saved: environment,
            variables,
            persisted: false,
            autosave,
            observed_snapshot,
        };
        draft.ensure_empty_row();
        draft
    }

    pub fn from_environment(environment: Environment) -> Self {
        let name = environment.name.clone();
        let variables = environment
            .variables
            .iter()
            .map(EditableVariable::from)
            .collect::<Vec<_>>();
        let observed_snapshot = environment_snapshot(&name, &variables);
        let mut draft = Self {
            name,
            variables,
            saved: environment,
            persisted: true,
            autosave: AutoSaveState::new(Duration::from_millis(500)),
            observed_snapshot,
        };
        draft.ensure_empty_row();
        draft
    }

    pub fn id(&self) -> EntityId {
        self.saved.id
    }

    pub fn to_environment(&self) -> Environment {
        let mut environment = self.saved.clone();
        environment.name = self.name.clone();
        environment.variables = self
            .variables
            .iter()
            .filter(|variable| !variable.is_empty())
            .map(|variable| variable.to_variable(environment.id))
            .collect();
        environment
    }

    pub fn is_dirty(&self) -> bool {
        !self.persisted
            || self.to_environment() != self.saved
            || self.autosave.is_dirty()
            || self
                .variables
                .iter()
                .any(EditableVariable::has_pending_secret)
    }

    pub fn ensure_empty_row(&mut self) {
        if self
            .variables
            .last()
            .is_none_or(|variable| !variable.is_empty())
        {
            self.variables.push(EditableVariable::empty());
        }
    }

    pub fn sync_edit_revision(&mut self, now: Instant) {
        let snapshot = environment_snapshot(&self.name, &self.variables);
        if snapshot != self.observed_snapshot {
            self.observed_snapshot = snapshot;
            self.autosave.mark_changed(now);
        }
    }

    pub fn mark_saved(&mut self, environment: Environment, revision: u64) {
        self.saved = environment;
        self.persisted = true;
        self.autosave.mark_saved(revision);
        self.ensure_empty_row();
    }

    pub fn discard(&mut self) {
        let saved = self.saved.clone();
        *self = Self::from_environment(saved);
    }
}

fn environment_snapshot(name: &str, variables: &[EditableVariable]) -> Vec<u8> {
    let values = variables
        .iter()
        .filter(|variable| !variable.is_empty())
        .map(|variable| {
            (
                variable.id,
                variable.enabled,
                variable.name.as_str(),
                variable.value.as_str(),
                variable.secret,
                variable.reference.as_ref(),
                variable.configured,
            )
        })
        .collect::<Vec<_>>();
    serde_json::to_vec(&(name, values)).expect("environment editor state should serialize")
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{EditableVariable, EnvironmentDraft};

    #[test]
    fn secret_values_are_replaced_by_external_references() {
        let mut environment = EnvironmentDraft::new("Development");
        environment.variables.insert(
            0,
            EditableVariable {
                value: "plain-text-secret".into(),
                secret: true,
                ..EditableVariable::empty()
            },
        );
        let environment_id = environment.id();
        environment.variables[0].ensure_reference(environment_id);

        let stored = environment.to_environment();

        assert_eq!(stored.variables[0].value, "");
        assert!(
            stored.variables[0]
                .secret_ref
                .as_ref()
                .is_some_and(|reference| reference.as_str().starts_with("keyring://environment-"))
        );
        assert!(
            !serde_json::to_string(&stored)
                .expect("environment should serialize")
                .contains("plain-text-secret")
        );
    }

    #[test]
    fn environment_edits_use_debounced_revision_tracking() {
        let start = Instant::now();
        let mut environment = EnvironmentDraft::new("Development");
        let initial_revision = environment.autosave.current_revision();
        environment.name = "Staging".into();

        environment.sync_edit_revision(start);

        assert!(environment.autosave.current_revision() > initial_revision);
        assert!(
            !environment
                .autosave
                .is_due(start + Duration::from_millis(499))
        );
        assert!(
            environment
                .autosave
                .is_due(start + Duration::from_millis(500))
        );
    }
}
