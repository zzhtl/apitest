use std::collections::HashMap;

use apitest_core::{EntityId, Environment, MockProfile, Project, TestScenario};
use apitest_storage::{Database, PageRequest};

use crate::app::{ACTIVE_ENVIRONMENT_SETTING, ACTIVE_PROJECT_SETTING, DOCUMENT_TABS_SETTING};
use crate::environment::EnvironmentDraft;
use crate::i18n::Language;
use crate::persistence::open_workspace_database;
use crate::state::workspace::{ResourcePage, WorkspaceRequest};
use crate::workbench::{DocumentId, DocumentKind, DocumentTabs};

pub(crate) const RESOURCE_PAGE_SIZE: usize = 100;

pub(crate) fn open_database() -> Result<Database, String> {
    let directory = directories::ProjectDirs::from("io.github", "zzhtl", "ApiTest")
        .ok_or_else(|| "failed to resolve the application data directory".to_owned())?;
    let startup =
        open_workspace_database(directory.data_local_dir()).map_err(|error| error.to_string())?;
    if let Some(path) = startup.legacy_archive {
        tracing::info!(archive = %path.display(), "archived ApiTest v1 database");
    }
    Ok(startup.database)
}

pub(crate) fn load_setting<T>(database: Option<&Database>, key: &str, default: T) -> T
where
    T: serde::de::DeserializeOwned,
{
    database
        .and_then(|database| database.get_setting(key).ok().flatten())
        .unwrap_or(default)
}

pub(crate) type WorkspaceLoad = (
    Vec<Project>,
    Project,
    Vec<WorkspaceRequest>,
    Vec<EnvironmentDraft>,
    HashMap<Option<EntityId>, ResourcePage>,
    Vec<String>,
);

pub(crate) type ProjectContent = (
    Vec<WorkspaceRequest>,
    Vec<EnvironmentDraft>,
    HashMap<Option<EntityId>, ResourcePage>,
    Vec<String>,
);

pub(crate) fn load_workspace(database: Option<&Database>, language: Language) -> WorkspaceLoad {
    let mut errors = Vec::new();
    let mut projects = database
        .map(|database| match database.list_projects() {
            Ok(projects) => projects,
            Err(error) => {
                errors.push(error.to_string());
                Vec::new()
            }
        })
        .unwrap_or_default();
    if projects.is_empty() {
        let project = Project::new("My Workspace");
        if let Some(database) = database
            && let Err(error) = database.save_project(&project)
        {
            errors.push(error.to_string());
        }
        projects.push(project);
    }
    let active_project = load_setting(database, ACTIVE_PROJECT_SETTING, projects[0].id);
    let project = projects
        .iter()
        .find(|project| project.id == active_project)
        .unwrap_or(&projects[0])
        .clone();
    let (requests, environments, resource_pages, project_errors) =
        load_project_content(database, &project, language);
    errors.extend(project_errors);
    (
        projects,
        project,
        requests,
        environments,
        resource_pages,
        errors,
    )
}

pub(crate) fn load_project_content(
    database: Option<&Database>,
    project: &Project,
    language: Language,
) -> ProjectContent {
    let mut errors = Vec::new();
    let definitions = database
        .map(|database| match database.list_definitions(project.id) {
            Ok(definitions) => definitions,
            Err(error) => {
                errors.push(error.to_string());
                Vec::new()
            }
        })
        .unwrap_or_default();
    let mut requests = Vec::with_capacity(definitions.len());
    for definition in definitions {
        if let Some(database) = database
            && let Err(error) = database.ensure_definition_node(project.id, &definition)
        {
            errors.push(error.to_string());
        }
        let request_case = database.and_then(|database| {
            match database.list_request_cases(project.id, definition.id, PageRequest::new(0, 1)) {
                Ok(page) => page.items.into_iter().next(),
                Err(error) => {
                    errors.push(error.to_string());
                    None
                }
            }
        });
        requests.push(WorkspaceRequest::from_definition(definition, request_case));
    }
    if requests.is_empty() {
        let mut request = WorkspaceRequest::new(match language {
            Language::Chinese => "未命名请求",
            Language::English => "Untitled request",
        });
        request.draft.url = "https://httpbin.org/get".into();
        requests.push(request);
    }

    let mut environments = database
        .map(|database| match database.list_environments(project.id) {
            Ok(environments) => environments,
            Err(error) => {
                errors.push(error.to_string());
                Vec::new()
            }
        })
        .unwrap_or_default()
        .into_iter()
        .map(EnvironmentDraft::from_environment)
        .collect::<Vec<_>>();
    if environments.is_empty() {
        let environment = Environment::new("Local");
        let persisted = if let Some(database) = database {
            match database.save_environment(project.id, &environment) {
                Ok(()) => true,
                Err(error) => {
                    errors.push(error.to_string());
                    false
                }
            }
        } else {
            false
        };
        let mut draft = EnvironmentDraft::from_environment(environment);
        draft.persisted = persisted;
        environments.push(draft);
    }
    let mut resource_pages = HashMap::new();
    let root = database
        .map(|database| {
            database.list_project_nodes(project.id, None, PageRequest::new(0, RESOURCE_PAGE_SIZE))
        })
        .transpose();
    match root {
        Ok(Some(page)) => {
            resource_pages.insert(
                None,
                ResourcePage {
                    items: page.items,
                    total: page.total,
                },
            );
        }
        Ok(None) => {
            resource_pages.insert(None, ResourcePage::default());
        }
        Err(error) => {
            errors.push(error.to_string());
            resource_pages.insert(None, ResourcePage::default());
        }
    }
    (requests, environments, resource_pages, errors)
}

pub(crate) fn load_automation(
    database: Option<&Database>,
    project_id: EntityId,
) -> (Vec<TestScenario>, Vec<MockProfile>, Vec<String>) {
    let mut errors = Vec::new();
    let scenarios = database
        .map(|database| match database.list_scenarios(project_id) {
            Ok(scenarios) => scenarios,
            Err(error) => {
                errors.push(error.to_string());
                Vec::new()
            }
        })
        .unwrap_or_default();
    let mock_profiles = database
        .map(|database| match database.list_mock_profiles(project_id) {
            Ok(profiles) => profiles,
            Err(error) => {
                errors.push(error.to_string());
                Vec::new()
            }
        })
        .unwrap_or_default();
    (scenarios, mock_profiles, errors)
}

pub(crate) fn active_environment_setting(project_id: EntityId) -> String {
    format!("{ACTIVE_ENVIRONMENT_SETTING}.{project_id}")
}

pub(crate) fn document_tabs_setting(project_id: EntityId) -> String {
    format!("{DOCUMENT_TABS_SETTING}.{project_id}")
}

pub(crate) fn load_document_tabs(
    database: Option<&Database>,
    project_id: EntityId,
    requests: &[WorkspaceRequest],
    environments: &[EnvironmentDraft],
    scenarios: &[TestScenario],
    mock_profiles: &[MockProfile],
) -> DocumentTabs {
    let saved = load_setting(
        database,
        &document_tabs_setting(project_id),
        DocumentTabs::default(),
    );
    let active = saved.active();
    let mut tabs = DocumentTabs::default();
    for tab in saved.items() {
        let title = match tab.id.kind {
            DocumentKind::Api => requests
                .iter()
                .find(|request| request.id() == tab.id.entity_id)
                .map(|request| request.name.clone()),
            DocumentKind::Environment => environments
                .iter()
                .find(|environment| environment.id() == tab.id.entity_id)
                .map(|environment| environment.name.clone()),
            DocumentKind::Scenario => scenarios
                .iter()
                .find(|scenario| scenario.id == tab.id.entity_id)
                .map(|scenario| scenario.name.clone()),
            DocumentKind::Mock => mock_profiles
                .iter()
                .find(|profile| profile.id == tab.id.entity_id)
                .map(|profile| profile.name.clone()),
            DocumentKind::History => None,
        };
        if let Some(title) = title {
            tabs.open(tab.id, title);
        }
    }
    if let Some(active) = active {
        tabs.activate(active);
    }
    if tabs.items().is_empty()
        && let Some(request) = requests.first()
    {
        tabs.open(
            DocumentId {
                kind: DocumentKind::Api,
                entity_id: request.id(),
            },
            request.name.clone(),
        );
    }
    tabs
}
