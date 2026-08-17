use std::collections::HashSet;

use apitest_core::{EntityId, MockProfile, Project, ScenarioNode, TestScenario};
use apitest_storage::PageRequest;

use crate::app::{ACTIVE_PROJECT_SETTING, ApiTestApp};
use crate::environment::EnvironmentDraft;
use crate::services::document::document_snapshot;
use crate::services::history::HISTORY_MAX_RECORDS;
use crate::services::loader::{
    RESOURCE_PAGE_SIZE, active_environment_setting, load_automation, load_document_tabs,
    load_project_content, load_setting,
};
use crate::state::action::{Confirmation, PendingAction, ToastKind};
use crate::state::workspace::{
    Navigation, ResourcePage, ResourceRow, WorkspaceRequest, collect_resource_rows,
};
use crate::workbench::{DocumentId, DocumentKind};

impl ApiTestApp {
    pub(crate) fn queue_action(&mut self, action: PendingAction) {
        let dirty = if action.leaves_workspace() {
            self.workspace_dirty()
        } else if let PendingAction::CloseDocument(id) = action {
            self.document_dirty(id)
        } else {
            self.current_dirty()
        };
        if dirty {
            self.confirmation = Some(Confirmation::Unsaved(action));
        } else {
            self.perform_action(action);
        }
    }

    pub(crate) fn perform_action(&mut self, action: PendingAction) {
        match action {
            PendingAction::SelectRequest(id) => {
                if let Some(index) = self.requests.iter().position(|request| request.id() == id) {
                    self.invalidate_run();
                    self.selected = index;
                    self.navigation = Navigation::Api;
                    self.open_document(
                        DocumentId {
                            kind: DocumentKind::Api,
                            entity_id: id,
                        },
                        self.requests[index].name.clone(),
                    );
                }
            }
            PendingAction::NewRequest(kind) => {
                self.invalidate_run();
                self.requests.push(WorkspaceRequest::new_protocol(
                    self.tr("未命名请求", "Untitled request"),
                    kind,
                ));
                self.selected = self.requests.len() - 1;
                self.navigation = Navigation::Api;
                let request_id = self.requests[self.selected].id();
                let request_name = self.requests[self.selected].name.clone();
                self.open_document(
                    DocumentId {
                        kind: DocumentKind::Api,
                        entity_id: request_id,
                    },
                    request_name,
                );
            }
            PendingAction::SelectScenario(id) => {
                if let Some(index) = self.scenarios.iter().position(|scenario| scenario.id == id) {
                    self.invalidate_run();
                    self.selected_scenario = index;
                    self.navigation = Navigation::Scenario;
                    let name = self.scenarios[index].name.clone();
                    self.open_document(
                        DocumentId {
                            kind: DocumentKind::Scenario,
                            entity_id: id,
                        },
                        name,
                    );
                }
            }
            PendingAction::NewScenario => {
                let mut scenario = TestScenario {
                    name: self.tr("新建场景", "New scenario").into(),
                    ..TestScenario::default()
                };
                scenario.nodes.push(ScenarioNode::Delay { millis: 100 });
                let id = scenario.id;
                let name = scenario.name.clone();
                self.scenarios.push(scenario);
                self.selected_scenario = self.scenarios.len() - 1;
                self.navigation = Navigation::Scenario;
                self.open_document(
                    DocumentId {
                        kind: DocumentKind::Scenario,
                        entity_id: id,
                    },
                    name,
                );
            }
            PendingAction::SelectMock(id) => {
                if let Some(index) = self
                    .mock_profiles
                    .iter()
                    .position(|profile| profile.id == id)
                {
                    self.invalidate_run();
                    self.selected_mock = index;
                    self.navigation = Navigation::Mock;
                    let name = self.mock_profiles[index].name.clone();
                    self.open_document(
                        DocumentId {
                            kind: DocumentKind::Mock,
                            entity_id: id,
                        },
                        name,
                    );
                }
            }
            PendingAction::NewMock => {
                let profile = MockProfile {
                    name: self.tr("新建 Mock", "New mock").into(),
                    ..MockProfile::default()
                };
                let id = profile.id;
                let name = profile.name.clone();
                self.mock_profiles.push(profile);
                self.selected_mock = self.mock_profiles.len() - 1;
                self.navigation = Navigation::Mock;
                self.open_document(
                    DocumentId {
                        kind: DocumentKind::Mock,
                        entity_id: id,
                    },
                    name,
                );
            }
            PendingAction::SelectProject(id) => self.switch_project(id),
            PendingAction::NewProject => self.create_project(),
            PendingAction::Navigate(navigation) => {
                if navigation == Navigation::Environment {
                    self.invalidate_run();
                }
                self.navigation = navigation;
                match navigation {
                    Navigation::Api => {
                        if let Some((id, name)) = self
                            .requests
                            .get(self.selected)
                            .map(|request| (request.id(), request.name.clone()))
                        {
                            self.open_document(
                                DocumentId {
                                    kind: DocumentKind::Api,
                                    entity_id: id,
                                },
                                name,
                            );
                        }
                    }
                    Navigation::Environment => {
                        if let Some((id, name)) = self
                            .environments
                            .get(self.selected_environment)
                            .map(|environment| (environment.id(), environment.name.clone()))
                        {
                            self.open_document(
                                DocumentId {
                                    kind: DocumentKind::Environment,
                                    entity_id: id,
                                },
                                name,
                            );
                        }
                    }
                    Navigation::Scenario => {
                        if let Some(scenario) = self.scenarios.get(self.selected_scenario) {
                            self.open_document(
                                DocumentId {
                                    kind: DocumentKind::Scenario,
                                    entity_id: scenario.id,
                                },
                                scenario.name.clone(),
                            );
                        }
                    }
                    Navigation::Mock => {
                        if let Some(profile) = self.mock_profiles.get(self.selected_mock) {
                            self.open_document(
                                DocumentId {
                                    kind: DocumentKind::Mock,
                                    entity_id: profile.id,
                                },
                                profile.name.clone(),
                            );
                        }
                    }
                    Navigation::History => {
                        self.reload_run_history();
                    }
                }
            }
            PendingAction::SelectEnvironment(id) => {
                if let Some(index) = self
                    .environments
                    .iter()
                    .position(|environment| environment.id() == id)
                {
                    self.selected_environment = index;
                    self.navigation = Navigation::Environment;
                    let name = self.environments[index].name.clone();
                    self.open_document(
                        DocumentId {
                            kind: DocumentKind::Environment,
                            entity_id: id,
                        },
                        name,
                    );
                }
            }
            PendingAction::NewEnvironment => {
                let base = self.tr("新环境", "New environment");
                let mut suffix = 1;
                let mut name = base.to_owned();
                while self
                    .environments
                    .iter()
                    .any(|environment| environment.name.eq_ignore_ascii_case(&name))
                {
                    suffix += 1;
                    name = format!("{base} {suffix}");
                }
                self.environments.push(EnvironmentDraft::new(name));
                self.selected_environment = self.environments.len() - 1;
                self.navigation = Navigation::Environment;
                let environment_id = self.environments[self.selected_environment].id();
                let environment_name = self.environments[self.selected_environment].name.clone();
                self.open_document(
                    DocumentId {
                        kind: DocumentKind::Environment,
                        entity_id: environment_id,
                    },
                    environment_name,
                );
            }
            PendingAction::CloseDocument(id) => self.close_document(id),
            PendingAction::Close => {
                self.allow_close = true;
                self.confirmation = None;
            }
        }
    }

    pub(crate) fn create_project(&mut self) {
        let Some(database) = self.database.clone() else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return;
        };
        let base = self.tr("新项目", "New project");
        let mut suffix = 1;
        let mut name = base.to_owned();
        while self
            .projects
            .iter()
            .any(|project| project.name.eq_ignore_ascii_case(&name))
        {
            suffix += 1;
            name = format!("{base} {suffix}");
        }
        let project = Project::new(name);
        if let Err(error) = database.save_project(&project) {
            self.toast(ToastKind::Error, error.to_string());
            return;
        }
        let id = project.id;
        self.projects.push(project);
        self.projects
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.switch_project(id);
    }

    pub(crate) fn switch_project(&mut self, id: EntityId) {
        if self.project.id == id {
            return;
        }
        let Some(project) = self
            .projects
            .iter()
            .find(|project| project.id == id)
            .cloned()
        else {
            return;
        };
        self.invalidate_run();
        let (requests, environments, mut resource_pages, mut errors) =
            load_project_content(self.database.as_deref(), &project, self.language);
        let (scenarios, mock_profiles, automation_errors) =
            load_automation(self.database.as_deref(), project.id);
        errors.extend(automation_errors);
        let run_records = self
            .database
            .as_deref()
            .map(|database| {
                database.list_run_records(project.id, PageRequest::new(0, HISTORY_MAX_RECORDS))
            })
            .transpose();
        let run_records = match run_records {
            Ok(Some(page)) => page.items,
            Ok(None) => Vec::new(),
            Err(error) => {
                errors.push(error.to_string());
                Vec::new()
            }
        };
        resource_pages.entry(None).or_default();
        let active_environment = load_setting(
            self.database.as_deref(),
            &active_environment_setting(project.id),
            environments[0].id(),
        );
        let active_environment = if environments
            .iter()
            .any(|environment| environment.id() == active_environment)
        {
            active_environment
        } else {
            environments[0].id()
        };
        let document_tabs = load_document_tabs(
            self.database.as_deref(),
            project.id,
            &requests,
            &environments,
            &scenarios,
            &mock_profiles,
        );
        let active_document = document_tabs.active();

        self.project = project;
        self.requests = requests;
        self.environments = environments;
        self.scenario_snapshots = scenarios
            .iter()
            .map(|scenario| (scenario.id, document_snapshot(scenario)))
            .collect();
        self.scenarios = scenarios;
        self.scenario_report = None;
        self.mock_snapshots = mock_profiles
            .iter()
            .map(|profile| (profile.id, document_snapshot(profile)))
            .collect();
        self.mock_profiles = mock_profiles;
        self.run_records = run_records;
        self.selected_history = 0;
        self.history_body_preview.clear();
        self.history_body_truncated = false;
        self.mock_server = None;
        self.mock_run_id = self.mock_run_id.wrapping_add(1);
        self.resource_pages = resource_pages;
        self.expanded_resources.clear();
        self.active_environment = active_environment;
        self.selected = 0;
        self.selected_environment = self
            .environments
            .iter()
            .position(|environment| environment.id() == active_environment)
            .unwrap_or_default();
        self.selected_scenario = 0;
        self.selected_mock = 0;
        self.navigation = Navigation::Api;
        self.document_tabs = document_tabs;
        if let Some(active) = active_document {
            self.activate_document(active);
        }
        let project_id = self.project.id;
        self.persist_setting(ACTIVE_PROJECT_SETTING, &project_id);
        if let Some(error) = errors.into_iter().last() {
            self.toast(ToastKind::Error, error);
        }
    }

    pub(crate) fn reload_resource_page(&mut self, parent_id: Option<EntityId>) {
        let Some(database) = self.database.clone() else {
            return;
        };
        match database.list_project_nodes(
            self.project.id,
            parent_id,
            PageRequest::new(0, RESOURCE_PAGE_SIZE),
        ) {
            Ok(page) => {
                self.resource_pages.insert(
                    parent_id,
                    ResourcePage {
                        items: page.items,
                        total: page.total,
                    },
                );
            }
            Err(error) => self.toast(ToastKind::Error, error.to_string()),
        }
    }

    pub(crate) fn load_more_resources(&mut self, parent_id: Option<EntityId>) {
        let Some(database) = self.database.clone() else {
            return;
        };
        let offset = self
            .resource_pages
            .get(&parent_id)
            .map(|page| page.items.len())
            .unwrap_or_default();
        match database.list_project_nodes(
            self.project.id,
            parent_id,
            PageRequest::new(offset, RESOURCE_PAGE_SIZE),
        ) {
            Ok(page) => {
                let branch = self.resource_pages.entry(parent_id).or_default();
                for node in page.items {
                    if !branch.items.iter().any(|existing| existing.id == node.id) {
                        branch.items.push(node);
                    }
                }
                branch.total = page.total;
            }
            Err(error) => self.toast(ToastKind::Error, error.to_string()),
        }
    }

    pub(crate) fn toggle_resource(&mut self, id: EntityId) {
        if self.expanded_resources.remove(&id) {
            return;
        }
        self.expanded_resources.insert(id);
        if !self.resource_pages.contains_key(&Some(id)) {
            self.reload_resource_page(Some(id));
        }
    }

    pub(crate) fn visible_resource_rows(&self) -> Vec<ResourceRow> {
        let mut rows = Vec::new();
        let mut visiting = HashSet::new();
        collect_resource_rows(
            None,
            0,
            &self.resource_pages,
            &self.expanded_resources,
            &mut visiting,
            &mut rows,
        );
        rows
    }
}
