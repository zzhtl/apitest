use std::time::Instant;

use serde::Serialize;

use crate::app::ApiTestApp;
use crate::environment::EnvironmentDraft;
use crate::i18n::Language;
use crate::services::loader::document_tabs_setting;
use crate::state::action::{Toast, ToastKind};
use crate::state::workspace::{Navigation, WorkspaceRequest};
use crate::workbench::{DocumentId, DocumentKind};

impl ApiTestApp {
    pub(crate) fn tr<'a>(&self, chinese: &'a str, english: &'a str) -> &'a str {
        match self.language {
            Language::Chinese => chinese,
            Language::English => english,
        }
    }

    pub(crate) fn toast(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.toast = Some(Toast {
            message: message.into(),
            kind,
            created_at: Instant::now(),
        });
    }

    pub(crate) fn persist_setting<T: Serialize>(&mut self, key: &str, value: &T) {
        let Some(database) = &self.database else {
            return;
        };
        if let Err(error) = database.set_setting(key, value) {
            self.toast(ToastKind::Error, error.to_string());
        }
    }

    pub(crate) fn persist_document_tabs(&mut self) {
        let key = document_tabs_setting(self.project.id);
        let tabs = self.document_tabs.clone();
        self.persist_setting(&key, &tabs);
    }

    pub(crate) fn open_document(&mut self, id: DocumentId, title: impl Into<String>) {
        self.document_tabs.open(id, title);
        self.persist_document_tabs();
    }

    pub(crate) fn activate_document(&mut self, id: DocumentId) -> bool {
        let activated = match id.kind {
            DocumentKind::Api => {
                let Some(index) = self
                    .requests
                    .iter()
                    .position(|request| request.id() == id.entity_id)
                else {
                    return false;
                };
                self.invalidate_run();
                self.selected = index;
                self.navigation = Navigation::Api;
                true
            }
            DocumentKind::Environment => {
                let Some(index) = self
                    .environments
                    .iter()
                    .position(|environment| environment.id() == id.entity_id)
                else {
                    return false;
                };
                self.invalidate_run();
                self.selected_environment = index;
                self.navigation = Navigation::Environment;
                true
            }
            DocumentKind::Scenario => {
                let Some(index) = self
                    .scenarios
                    .iter()
                    .position(|scenario| scenario.id == id.entity_id)
                else {
                    return false;
                };
                self.invalidate_run();
                self.selected_scenario = index;
                self.navigation = Navigation::Scenario;
                true
            }
            DocumentKind::Mock => {
                let Some(index) = self
                    .mock_profiles
                    .iter()
                    .position(|profile| profile.id == id.entity_id)
                else {
                    return false;
                };
                self.invalidate_run();
                self.selected_mock = index;
                self.navigation = Navigation::Mock;
                true
            }
            DocumentKind::History => false,
        };
        if activated {
            self.document_tabs.activate(id);
            self.persist_document_tabs();
        }
        activated
    }

    pub(crate) fn close_document(&mut self, id: DocumentId) {
        let active = self.document_tabs.close(id);
        self.persist_document_tabs();
        if let Some(active) = active {
            self.activate_document(active);
        }
    }

    pub(crate) fn current_dirty(&self) -> bool {
        match self.navigation {
            Navigation::Api => self
                .requests
                .get(self.selected)
                .is_some_and(WorkspaceRequest::is_dirty),
            Navigation::Environment => self
                .environments
                .get(self.selected_environment)
                .is_some_and(EnvironmentDraft::is_dirty),
            Navigation::Scenario => {
                self.scenarios
                    .get(self.selected_scenario)
                    .is_some_and(|scenario| {
                        self.scenario_snapshots
                            .get(&scenario.id)
                            .is_none_or(|saved| saved != &document_snapshot(scenario))
                    })
            }
            Navigation::Mock => self
                .mock_profiles
                .get(self.selected_mock)
                .is_some_and(|profile| {
                    self.mock_snapshots
                        .get(&profile.id)
                        .is_none_or(|saved| saved != &document_snapshot(profile))
                }),
            Navigation::History => false,
        }
    }

    pub(crate) fn workspace_dirty(&self) -> bool {
        self.requests.iter().any(WorkspaceRequest::is_dirty)
            || self.environments.iter().any(EnvironmentDraft::is_dirty)
            || self.scenarios.iter().any(|scenario| {
                self.scenario_snapshots
                    .get(&scenario.id)
                    .is_none_or(|saved| saved != &document_snapshot(scenario))
            })
            || self.mock_profiles.iter().any(|profile| {
                self.mock_snapshots
                    .get(&profile.id)
                    .is_none_or(|saved| saved != &document_snapshot(profile))
            })
    }

    pub(crate) fn document_dirty(&self, id: DocumentId) -> bool {
        match id.kind {
            DocumentKind::Api => self
                .requests
                .iter()
                .find(|request| request.id() == id.entity_id)
                .is_some_and(WorkspaceRequest::is_dirty),
            DocumentKind::Environment => self
                .environments
                .iter()
                .find(|environment| environment.id() == id.entity_id)
                .is_some_and(EnvironmentDraft::is_dirty),
            DocumentKind::Scenario => self
                .scenarios
                .iter()
                .find(|scenario| scenario.id == id.entity_id)
                .is_some_and(|scenario| {
                    self.scenario_snapshots
                        .get(&scenario.id)
                        .is_none_or(|saved| saved != &document_snapshot(scenario))
                }),
            DocumentKind::Mock => self
                .mock_profiles
                .iter()
                .find(|profile| profile.id == id.entity_id)
                .is_some_and(|profile| {
                    self.mock_snapshots
                        .get(&profile.id)
                        .is_none_or(|saved| saved != &document_snapshot(profile))
                }),
            DocumentKind::History => false,
        }
    }
}

pub(crate) fn document_snapshot(value: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("workspace document should serialize")
}
