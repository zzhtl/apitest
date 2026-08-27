use std::collections::HashSet;

use apitest_core::EntityId;

use crate::app::ApiTestApp;
use crate::draft::AuthDraft;
use crate::services::document::document_snapshot;
use crate::services::loader::active_environment_setting;
use crate::state::action::ToastKind;
use crate::state::workspace::Navigation;
use crate::workbench::{DocumentId, DocumentKind};

impl ApiTestApp {
    pub(crate) fn discard_current(&mut self) -> bool {
        match self.navigation {
            Navigation::Api => self.discard_request(self.selected),
            Navigation::Environment => self.discard_environment(self.selected_environment),
            Navigation::Scenario => {
                let Some(scenario) = self.scenarios.get(self.selected_scenario) else {
                    return false;
                };
                if let Some(saved) = self.scenario_snapshots.get(&scenario.id) {
                    let restored = match serde_json::from_slice(saved) {
                        Ok(restored) => restored,
                        Err(error) => {
                            self.toast(ToastKind::Error, error.to_string());
                            return false;
                        }
                    };
                    self.scenarios[self.selected_scenario] = restored;
                } else {
                    self.scenarios.remove(self.selected_scenario);
                    self.selected_scenario = self
                        .selected_scenario
                        .min(self.scenarios.len().saturating_sub(1));
                }
                true
            }
            Navigation::Mock => {
                let Some(profile) = self.mock_profiles.get(self.selected_mock) else {
                    return false;
                };
                if let Some(saved) = self.mock_snapshots.get(&profile.id) {
                    let restored = match serde_json::from_slice(saved) {
                        Ok(restored) => restored,
                        Err(error) => {
                            self.toast(ToastKind::Error, error.to_string());
                            return false;
                        }
                    };
                    self.mock_profiles[self.selected_mock] = restored;
                } else {
                    self.mock_profiles.remove(self.selected_mock);
                    self.selected_mock = self
                        .selected_mock
                        .min(self.mock_profiles.len().saturating_sub(1));
                }
                true
            }
            Navigation::History => true,
        }
    }

    pub(crate) fn discard_request(&mut self, index: usize) -> bool {
        let Some(database) = self.database.clone() else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return false;
        };
        let Some(request) = self.requests.get(index) else {
            return false;
        };
        let id = request.id();
        let persisted = request.persisted;
        let definition = request.definition.clone();
        let request_case = request.request_case.clone();
        let references = (!persisted).then(|| {
            let mut references = AuthDraft::references_for_request(id);
            references.extend(
                request_case
                    .local_variables
                    .iter()
                    .filter_map(|variable| variable.secret_ref.clone()),
            );
            references
        });
        if !self.settle_storage() {
            return false;
        }
        let result = if persisted {
            database.save_definition_and_case(self.project.id, &definition, &request_case)
        } else {
            database.delete_definition(self.project.id, id).map(|_| ())
        };
        if let Err(error) = result {
            self.toast(ToastKind::Error, error.to_string());
            return false;
        }
        if persisted {
            let Some(request) = self.requests.iter_mut().find(|request| request.id() == id) else {
                return false;
            };
            request.definition = definition;
            request.request_case = request_case;
            request.persisted = true;
            request.sync_contract = false;
            request.discard();
        } else {
            if let Some(references) = references {
                self.cleanup_secret_references(references);
            }
            if let Some(index) = self.requests.iter().position(|request| request.id() == id) {
                self.requests.remove(index);
            }
            self.sessions.close(DocumentId {
                kind: DocumentKind::Api,
                entity_id: id,
            });
            self.document_tabs.close(DocumentId {
                kind: DocumentKind::Api,
                entity_id: id,
            });
            self.persist_document_tabs();
            self.selected = self.selected.min(self.requests.len().saturating_sub(1));
        }
        true
    }

    pub(crate) fn discard_environment(&mut self, index: usize) -> bool {
        let Some(database) = self.database.clone() else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return false;
        };
        let Some(environment) = self.environments.get(index) else {
            return false;
        };
        let id = environment.id();
        let persisted = environment.persisted;
        let saved = environment.saved.clone();
        let references = (!persisted).then(|| {
            environment
                .variables
                .iter()
                .filter_map(|variable| variable.reference.clone())
                .collect::<Vec<_>>()
        });
        let protected = saved
            .variables
            .iter()
            .filter_map(|variable| variable.secret_ref.clone())
            .collect::<HashSet<_>>();
        if !self.wait_storage() {
            return false;
        }
        let _ = self.drain_storage_protected(&protected);
        let result = if persisted {
            database.save_environment(self.project.id, &saved)
        } else {
            database.delete_environment(self.project.id, id).map(|_| ())
        };
        if let Err(error) = result {
            self.toast(ToastKind::Error, error.to_string());
            return false;
        }
        if persisted {
            let Some(environment) = self
                .environments
                .iter_mut()
                .find(|environment| environment.id() == id)
            else {
                return false;
            };
            environment.saved = saved;
            environment.persisted = true;
            environment.discard();
        } else {
            if let Some(references) = references {
                self.cleanup_secret_references(references);
            }
            if let Some(index) = self
                .environments
                .iter()
                .position(|environment| environment.id() == id)
            {
                self.environments.remove(index);
            }
            self.document_tabs.close(DocumentId {
                kind: DocumentKind::Environment,
                entity_id: id,
            });
            self.persist_document_tabs();
            self.selected_environment = self
                .selected_environment
                .min(self.environments.len().saturating_sub(1));
        }
        true
    }

    pub(crate) fn discard_workspace(&mut self) -> bool {
        // The per-document discard below only touches documents whose dirty
        // flag is set, so every snapshot must be current first.
        self.sync_all_edit_snapshots(std::time::Instant::now());
        let protected = self
            .environments
            .iter()
            .flat_map(|environment| &environment.saved.variables)
            .filter_map(|variable| variable.secret_ref.clone())
            .collect::<HashSet<_>>();
        if !self.wait_storage() {
            return false;
        }
        let _ = self.drain_storage_protected(&protected);
        for index in (0..self.requests.len()).rev() {
            if self.requests[index].is_dirty() && !self.discard_request(index) {
                return false;
            }
        }
        for index in (0..self.environments.len()).rev() {
            if self.environments[index].is_dirty() && !self.discard_environment(index) {
                return false;
            }
        }
        for index in (0..self.scenarios.len()).rev() {
            let dirty = self
                .scenario_snapshots
                .get(&self.scenarios[index].id)
                .is_none_or(|saved| saved != &document_snapshot(&self.scenarios[index]));
            if !dirty {
                continue;
            }
            if let Some(saved) = self.scenario_snapshots.get(&self.scenarios[index].id) {
                match serde_json::from_slice(saved) {
                    Ok(restored) => self.scenarios[index] = restored,
                    Err(error) => {
                        self.toast(ToastKind::Error, error.to_string());
                        return false;
                    }
                }
            } else {
                self.scenarios.remove(index);
            }
        }
        for index in (0..self.mock_profiles.len()).rev() {
            let dirty = self
                .mock_snapshots
                .get(&self.mock_profiles[index].id)
                .is_none_or(|saved| saved != &document_snapshot(&self.mock_profiles[index]));
            if !dirty {
                continue;
            }
            if let Some(saved) = self.mock_snapshots.get(&self.mock_profiles[index].id) {
                match serde_json::from_slice(saved) {
                    Ok(restored) => self.mock_profiles[index] = restored,
                    Err(error) => {
                        self.toast(ToastKind::Error, error.to_string());
                        return false;
                    }
                }
            } else {
                self.mock_profiles.remove(index);
            }
        }
        true
    }

    pub(crate) fn delete_request(&mut self, id: EntityId) {
        if self.delete_requests_batch(&[id]) > 0 {
            self.toast(ToastKind::Success, self.tr("请求已删除", "Request deleted"));
        }
    }

    /// Delete several requests with one storage transaction, one flush and one
    /// in-memory cleanup pass, and no per-item toasts. Returns how many
    /// requests the database dropped; the caller reports the outcome.
    ///
    /// Deleting a folder used to call the single-request path in a loop: a
    /// 200-request folder meant 200 blocking storage round-trips and 200
    /// stacked toasts.
    pub(crate) fn delete_requests_batch(&mut self, ids: &[EntityId]) -> usize {
        if ids.is_empty() {
            return 0;
        }
        let Some(database) = self.database.clone() else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return 0;
        };
        if !self.wait_storage() {
            return 0;
        }
        let deleted = match database.delete_definitions(self.project.id, ids) {
            Ok(deleted) => deleted,
            Err(error) => {
                self.toast(ToastKind::Error, error.to_string());
                return 0;
            }
        };
        let _ = self.drain_storage();
        let removing = ids.iter().copied().collect::<HashSet<_>>();
        for id in &removing {
            let Some(index) = self.requests.iter().position(|request| request.id() == *id) else {
                continue;
            };
            let mut references = AuthDraft::references_for_request(*id);
            references.extend(
                self.requests[index]
                    .request_case
                    .local_variables
                    .iter()
                    .filter_map(|variable| variable.secret_ref.clone()),
            );
            self.cleanup_secret_references(references);
            self.requests.remove(index);
            self.sessions.close(DocumentId {
                kind: DocumentKind::Api,
                entity_id: *id,
            });
        }
        self.document_tabs
            .retain(|doc| !(doc.kind == DocumentKind::Api && removing.contains(&doc.entity_id)));
        self.persist_document_tabs();
        if let Some(active) = self.document_tabs.active() {
            self.activate_document(active);
        }
        self.invalidate_scenario_run();
        self.selected = self.selected.min(self.requests.len().saturating_sub(1));
        self.reload_resource_page(None);
        deleted
    }

    pub(crate) fn delete_environment(&mut self, id: EntityId) {
        if self.environments.len() <= 1 {
            self.toast(
                ToastKind::Info,
                self.tr("至少保留一个环境", "Keep at least one environment"),
            );
            return;
        }
        if !self
            .environments
            .iter()
            .any(|environment| environment.id() == id)
        {
            return;
        }
        let Some(database) = self.database.clone() else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return;
        };
        if !self.wait_storage() {
            return;
        }
        if let Err(error) = database.delete_environment(self.project.id, id) {
            self.toast(ToastKind::Error, error.to_string());
            return;
        }
        let _ = self.drain_storage();
        let Some(index) = self
            .environments
            .iter()
            .position(|environment| environment.id() == id)
        else {
            return;
        };
        let prefix = format!("keyring://environment-{id}-");
        let references = self.environments[index]
            .variables
            .iter()
            .filter_map(|variable| variable.reference.clone())
            .chain(
                self.environments[index]
                    .saved
                    .variables
                    .iter()
                    .filter_map(|variable| variable.secret_ref.clone()),
            )
            .filter(|reference| reference.as_str().starts_with(&prefix))
            .collect::<HashSet<_>>();
        self.cleanup_secret_references(references);
        self.environments.remove(index);
        self.selected_environment = self
            .selected_environment
            .min(self.environments.len().saturating_sub(1));
        if self.active_environment == id {
            self.active_environment = self.environments[0].id();
            let active = self.active_environment;
            let key = active_environment_setting(self.project.id);
            self.persist_setting(&key, &active);
        }
        self.close_document(DocumentId {
            kind: DocumentKind::Environment,
            entity_id: id,
        });
        self.toast(
            ToastKind::Success,
            self.tr("环境已删除", "Environment deleted"),
        );
    }
}
