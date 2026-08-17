use apitest_core::ApiDefinition;
use apitest_interop::{
    export_curl, export_har, export_openapi, export_openapi_documentation, export_postman,
    import_curl, import_har, import_openapi_document, import_postman, validate_openapi,
};

use crate::app::ApiTestApp;
use crate::i18n::Language;
use crate::services::secrets::{delete_secret_refs, secure_imported_request};
use crate::state::action::{InteropAction, ToastKind};
use crate::state::workspace::WorkspaceRequest;

impl ApiTestApp {
    pub(crate) fn handle_interop_action(&mut self, action: InteropAction) {
        match action {
            InteropAction::ImportOpenApi
            | InteropAction::ImportPostman
            | InteropAction::ImportHar => self.import_file(action),
            InteropAction::ImportCurl => self.show_curl_import = true,
            InteropAction::PreviewOpenApi => match self.refresh_openapi_preview() {
                Ok(()) => self.show_openapi_preview = true,
                Err(error) => self.toast(ToastKind::Error, error),
            },
            InteropAction::ExportOpenApi => {
                match export_openapi(&self.project, &self.contract_definitions()) {
                    Ok(source) => self.export_text("openapi.yaml", "OpenAPI", &source),
                    Err(error) => self.toast(ToastKind::Error, error.to_string()),
                }
            }
            InteropAction::ExportPostman => {
                match export_postman(&self.project, &self.example_definitions()) {
                    Ok(source) => self.export_text("postman-collection.json", "JSON", &source),
                    Err(error) => self.toast(ToastKind::Error, error.to_string()),
                }
            }
            InteropAction::ExportHar => match export_har(&self.example_definitions()) {
                Ok(source) => self.export_text("requests.har", "HAR", &source),
                Err(error) => self.toast(ToastKind::Error, error.to_string()),
            },
            InteropAction::ExportCurl => {
                let Some(request) = self.requests.get(self.selected) else {
                    self.toast(
                        ToastKind::Error,
                        self.tr("没有可导出的请求", "No request to export"),
                    );
                    return;
                };
                let mut definition = request.definition.clone();
                definition.name = request.name.clone();
                definition.contract = request.edited_protocol().into();
                match export_curl(&definition) {
                    Ok(source) => self.export_text("request.curl.txt", "Text", &source),
                    Err(error) => self.toast(ToastKind::Error, error.to_string()),
                }
            }
            InteropAction::ExportDocumentation => {
                let html =
                    export_openapi_documentation(&self.project, &self.contract_definitions());
                self.export_text("api-documentation.html", "HTML", &html);
            }
        }
    }

    pub(crate) fn import_file(&mut self, action: InteropAction) {
        let (label, extensions): (&str, &[&str]) = match action {
            InteropAction::ImportOpenApi => ("OpenAPI", &["yaml", "yml", "json"]),
            InteropAction::ImportPostman => ("Postman", &["json"]),
            InteropAction::ImportHar => ("HAR", &["har", "json"]),
            _ => return,
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(label, extensions)
            .pick_file()
        else {
            return;
        };
        let result = std::fs::read_to_string(&path)
            .map_err(|error| error.to_string())
            .and_then(|source| self.import_source(action, &source));
        match result {
            Ok(count) => self.toast(
                ToastKind::Success,
                match self.language {
                    Language::Chinese => format!("已导入 {count} 个请求"),
                    Language::English => format!("Imported {count} requests"),
                },
            ),
            Err(error) => self.toast(ToastKind::Error, error),
        }
    }

    pub(crate) fn import_source(
        &mut self,
        action: InteropAction,
        source: &str,
    ) -> Result<usize, String> {
        let definitions = match action {
            InteropAction::ImportOpenApi => {
                let imported =
                    import_openapi_document(source).map_err(|error| error.to_string())?;
                let mut project = self.project.clone();
                project
                    .components
                    .schemas
                    .extend(imported.components.schemas);
                project
                    .components
                    .examples
                    .extend(imported.components.examples);
                project
                    .components
                    .security_schemes
                    .extend(imported.components.security_schemes);
                let database = self.database.as_ref().ok_or_else(|| {
                    self.tr("本地数据库不可用", "Local database unavailable")
                        .to_owned()
                })?;
                database
                    .save_project(&project)
                    .map_err(|error| error.to_string())?;
                self.project = project.clone();
                if let Some(existing) = self
                    .projects
                    .iter_mut()
                    .find(|existing| existing.id == project.id)
                {
                    *existing = project;
                }
                imported.definitions
            }
            InteropAction::ImportPostman => {
                import_postman(source).map_err(|error| error.to_string())?
            }
            InteropAction::ImportHar => import_har(source).map_err(|error| error.to_string())?,
            InteropAction::ImportCurl => {
                vec![import_curl(source).map_err(|error| error.to_string())?]
            }
            _ => return Err("unsupported import action".into()),
        };
        self.queue_imported_definitions(definitions)
    }

    pub(crate) fn queue_imported_definitions(
        &mut self,
        definitions: Vec<ApiDefinition>,
    ) -> Result<usize, String> {
        if definitions.is_empty() {
            return Err(self
                .tr(
                    "文档中没有可导入的请求",
                    "The document contains no requests",
                )
                .into());
        }
        if self.storage_worker.is_none() {
            return Err(self
                .tr("本地数据库不可用", "Local database unavailable")
                .into());
        }
        let mut requests = definitions
            .into_iter()
            .map(|definition| WorkspaceRequest::from_definition(definition, None))
            .collect::<Vec<_>>();
        let mut imported_secret_refs = Vec::new();
        for request in &mut requests {
            match secure_imported_request(request, self.secrets.as_ref()) {
                Ok(references) => imported_secret_refs.extend(references),
                Err(error) => {
                    delete_secret_refs(self.secrets.as_ref(), &imported_secret_refs);
                    return Err(error);
                }
            }
        }
        let payload = requests
            .iter()
            .map(|request| {
                let (definition, request_case) = request.save_snapshot();
                (
                    definition,
                    request_case,
                    request.autosave.current_revision(),
                )
            })
            .collect::<Vec<_>>();
        if let Err(error) = self
            .storage_worker
            .as_ref()
            .expect("storage worker was checked above")
            .queue_import(self.project.id, payload)
        {
            delete_secret_refs(self.secrets.as_ref(), &imported_secret_refs);
            return Err(error.to_string());
        }
        for request in &mut requests {
            request
                .autosave
                .mark_queued(request.autosave.current_revision());
        }
        let count = requests.len();
        self.requests.extend(requests);
        Ok(count)
    }

    pub(crate) fn contract_definitions(&self) -> Vec<ApiDefinition> {
        self.requests
            .iter()
            .map(|request| request.save_snapshot().0)
            .collect()
    }

    pub(crate) fn example_definitions(&self) -> Vec<ApiDefinition> {
        self.requests
            .iter()
            .map(|request| {
                let mut definition = request.definition.clone();
                definition.name = request.name.clone();
                definition.contract = request.edited_protocol().into();
                definition
            })
            .collect()
    }

    pub(crate) fn refresh_openapi_preview(&mut self) -> Result<(), String> {
        let definitions = self.contract_definitions();
        let source =
            export_openapi(&self.project, &definitions).map_err(|error| error.to_string())?;
        let issues = validate_openapi(&source).map_err(|error| error.to_string())?;
        self.openapi_html = export_openapi_documentation(&self.project, &definitions);
        self.openapi_source = source;
        self.openapi_issues = issues;
        Ok(())
    }

    pub(crate) fn export_text(&mut self, file_name: &str, label: &str, source: &str) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(label, &[file_name.rsplit('.').next().unwrap_or("txt")])
            .set_file_name(file_name)
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, source) {
            Ok(()) => self.toast(
                ToastKind::Success,
                match self.language {
                    Language::Chinese => format!("已导出到 {}", path.display()),
                    Language::English => format!("Exported to {}", path.display()),
                },
            ),
            Err(error) => self.toast(ToastKind::Error, error.to_string()),
        }
    }
}
