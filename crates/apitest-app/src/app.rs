use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::Write,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use apitest_core::{
    ApiContract, ApiDefinition, ApiKeyLocation, ApiParameterLocation, AssertionRule, BodySpec,
    EntityId, Environment, ExecutionCommand, ExecutionError, ExecutionEvent, ExecutionMetrics,
    ExecutionRequest, ExtractorRule, GraphQlSpec, GrpcCallKind, GrpcSpec, HttpMethod, KeyValue,
    MockProfile, MockRule, MultipartPart, OAuth2Grant, Project, ProjectNode, ProjectNodeKind,
    ProtocolExecutor, ProtocolKind, ProtocolSpec, RequestCase, ResponseHead, RunRecord,
    RunState as HistoryRunState, ScenarioNode, SecretRef, SseSpec, TestScenario, Variable,
    VariableScope, VariableSource, WebSocketSpec, generate_mock_rules,
};
use apitest_interop::{
    IMPORTED_AUTH_MARKER, OpenApiIssue, OpenApiIssueLevel, export_curl, export_har, export_openapi,
    export_openapi_documentation, export_postman, import_curl, import_har, import_openapi_document,
    import_postman, validate_openapi,
};
use apitest_runtime::{
    ExecutorRegistry, GrpcExecutor, HttpExecutor, MockRoute, MockServer, ResponseSchemaContract,
    ScenarioCase, ScenarioReport, ScenarioRunner, WebSocketExecutor,
};
use apitest_storage::{
    BodyRef, BodyStore, Database, PageRequest, RedactingBodySink, SecretStore, SystemSecretStore,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use eframe::egui::{
    self, Color32, CornerRadius, RichText, Stroke, TextFormat, Vec2, text::LayoutJob,
};
use egui_extras::{Column, TableBuilder};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    draft::{
        AuthDraft, AuthMode, BodyMode, EditableMultipartPart, EditablePair, MultipartValueMode,
        ProxyDraft, RequestDraft, SecretDraft,
    },
    environment::EnvironmentDraft,
    persistence::{StorageEvent, StorageWorker, open_workspace_database},
    theme::{self, Palette, ThemeMode},
    workbench::{AutoSaveState, DocumentId, DocumentKind, DocumentTabs},
};

const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
const THEME_SETTING: &str = "ui.theme";
const LANGUAGE_SETTING: &str = "ui.language";
const ACTIVE_ENVIRONMENT_SETTING: &str = "ui.active_environment";
const ACTIVE_PROJECT_SETTING: &str = "ui.active_project";
const DOCUMENT_TABS_SETTING: &str = "ui.document_tabs";
const SEARCH_FIELD_ID: &str = "global_api_search";
const RESOURCE_PAGE_SIZE: usize = 100;
const HISTORY_MAX_RECORDS: usize = 200;
const HISTORY_MAX_AGE_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Language {
    Chinese,
    English,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Navigation {
    Api,
    Scenario,
    Mock,
    History,
    Environment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorTab {
    Params,
    Headers,
    Cookies,
    Body,
    Auth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseTab {
    Body,
    Headers,
    Timeline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseBodyMode {
    Pretty,
    Raw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenApiPreviewTab {
    Yaml,
    Validation,
    Documentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InteropAction {
    ImportOpenApi,
    ImportPostman,
    ImportHar,
    ImportCurl,
    PreviewOpenApi,
    ExportOpenApi,
    ExportPostman,
    ExportHar,
    ExportCurl,
    ExportDocumentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunState {
    Idle,
    Running,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimelinePhase {
    Started,
    ResponseHead,
    FirstByte,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy)]
struct TimelineEntry {
    phase: TimelinePhase,
    elapsed_ms: u64,
}

#[derive(Default)]
struct ResourcePage {
    items: Vec<ProjectNode>,
    total: usize,
}

#[derive(Clone)]
enum ResourceRow {
    Node {
        node: ProjectNode,
        depth: usize,
    },
    More {
        parent_id: Option<EntityId>,
        depth: usize,
    },
}

struct WorkspaceRequest {
    definition: ApiDefinition,
    request_case: RequestCase,
    name: String,
    draft: RequestDraft,
    alternate_protocol: Option<ProtocolSpec>,
    persisted: bool,
    sync_contract: bool,
    autosave: AutoSaveState,
    observed_snapshot: Vec<u8>,
}

impl WorkspaceRequest {
    fn new(name: impl Into<String>) -> Self {
        Self::new_protocol(name, ProtocolKind::Http)
    }

    fn new_protocol(name: impl Into<String>, kind: ProtocolKind) -> Self {
        let protocol = default_protocol(kind);
        let definition = ApiDefinition::new(name, protocol.clone());
        let request_case = RequestCase::for_definition(&definition, "Default");
        let request_id = definition.id;
        let (draft, alternate_protocol) = match protocol {
            ProtocolSpec::Http(spec) => (RequestDraft::from_http_spec(&spec, request_id), None),
            protocol => (RequestDraft::default(), Some(protocol)),
        };
        let name = definition.name.clone();
        let observed_snapshot = request_snapshot(
            &name,
            alternate_protocol
                .clone()
                .unwrap_or_else(|| ProtocolSpec::Http(draft.to_http_spec())),
        );
        let mut autosave = AutoSaveState::new(Duration::from_millis(500));
        autosave.mark_changed(Instant::now());
        Self {
            request_case,
            name,
            draft,
            alternate_protocol,
            definition,
            persisted: false,
            sync_contract: true,
            autosave,
            observed_snapshot,
        }
    }

    fn from_definition(definition: ApiDefinition, request_case: Option<RequestCase>) -> Self {
        let persisted = request_case.is_some();
        let request_case =
            request_case.unwrap_or_else(|| RequestCase::for_definition(&definition, "Default"));
        let (draft, alternate_protocol) = match &request_case.protocol {
            ProtocolSpec::Http(spec) => (RequestDraft::from_http_spec(spec, definition.id), None),
            protocol => (RequestDraft::default(), Some(protocol.clone())),
        };
        let name = definition.name.clone();
        let observed_snapshot = request_snapshot(&name, request_case.protocol.clone());
        let mut autosave = AutoSaveState::new(Duration::from_millis(500));
        if !persisted {
            autosave.mark_changed(Instant::now());
        }
        Self {
            request_case,
            name,
            draft,
            alternate_protocol,
            definition,
            persisted,
            sync_contract: false,
            autosave,
            observed_snapshot,
        }
    }

    fn id(&self) -> EntityId {
        self.definition.id
    }

    fn is_dirty(&self) -> bool {
        !self.persisted
            || self.name != self.definition.name
            || self.edited_protocol() != self.request_case.protocol
            || self.draft.has_pending_secret()
            || self.autosave.is_dirty()
    }

    fn discard(&mut self) {
        self.name = self.definition.name.clone();
        match &self.request_case.protocol {
            ProtocolSpec::Http(spec) => {
                self.draft = RequestDraft::from_http_spec(spec, self.definition.id);
                self.alternate_protocol = None;
            }
            protocol => {
                self.draft = RequestDraft::default();
                self.alternate_protocol = Some(protocol.clone());
            }
        }
        self.autosave = AutoSaveState::new(Duration::from_millis(500));
        self.observed_snapshot = request_snapshot(&self.name, self.edited_protocol());
    }

    fn sync_edit_revision(&mut self, now: Instant) {
        let snapshot = request_snapshot(&self.name, self.edited_protocol());
        if snapshot != self.observed_snapshot {
            self.observed_snapshot = snapshot;
            self.autosave.mark_changed(now);
        }
    }

    fn save_snapshot(&self) -> (ApiDefinition, RequestCase) {
        let mut definition = self.definition.clone();
        definition.name = self.name.clone();
        definition.updated_at = Utc::now();
        let protocol = self.edited_protocol();
        if self.sync_contract {
            definition.contract = protocol.clone().into();
        }
        let mut request_case = self.request_case.clone();
        request_case.protocol = protocol;
        request_case.updated_at = Utc::now();
        (definition, request_case)
    }

    fn edited_protocol(&self) -> ProtocolSpec {
        self.alternate_protocol
            .clone()
            .unwrap_or_else(|| ProtocolSpec::Http(self.draft.to_http_spec()))
    }

    fn protocol_kind(&self) -> ProtocolKind {
        self.alternate_protocol
            .as_ref()
            .map(ProtocolSpec::kind)
            .unwrap_or(ProtocolKind::Http)
    }

    fn endpoint(&self) -> &str {
        match self.alternate_protocol.as_ref() {
            None => &self.draft.url,
            Some(ProtocolSpec::GraphQl(spec)) => &spec.endpoint,
            Some(ProtocolSpec::Sse(spec)) => &spec.request.url,
            Some(ProtocolSpec::WebSocket(spec)) => &spec.url,
            Some(ProtocolSpec::Grpc(spec)) => &spec.endpoint,
            Some(ProtocolSpec::Http(_)) => &self.draft.url,
        }
    }

    fn mark_saved(&mut self, definition: ApiDefinition, request_case: RequestCase, revision: u64) {
        self.definition = definition;
        self.request_case = request_case;
        self.persisted = true;
        self.sync_contract = false;
        self.autosave.mark_saved(revision);
    }
}

fn request_snapshot(name: &str, protocol: ProtocolSpec) -> Vec<u8> {
    serde_json::to_vec(&(name, protocol)).expect("request editor state should serialize")
}

fn persist_request_draft_secrets(
    secrets: &dyn SecretStore,
    draft: &mut RequestDraft,
) -> Result<(), String> {
    let mut pending = draft
        .auth
        .secrets()
        .into_iter()
        .filter(|secret| secret.has_pending_value())
        .map(|secret| (secret.reference.clone(), secret.replacement.clone()))
        .collect::<Vec<_>>();
    if let Some(proxy) = &draft.proxy
        && proxy.password.has_pending_value()
    {
        pending.push((
            proxy.password.reference.clone(),
            proxy.password.replacement.clone(),
        ));
    }
    for (reference, value) in &pending {
        secrets
            .set(reference, value)
            .map_err(|error| error.to_string())?;
    }
    for secret in draft.auth.secrets_mut() {
        if secret.has_pending_value() {
            secret.mark_saved();
        }
    }
    if let Some(proxy) = draft.proxy.as_mut()
        && proxy.password.has_pending_value()
    {
        proxy.password.mark_saved();
    }
    Ok(())
}

fn secure_imported_request(
    request: &mut WorkspaceRequest,
    secrets: &dyn SecretStore,
) -> Result<Vec<SecretRef>, String> {
    let request_id = request.id();
    let mut pending = Vec::<(SecretRef, String)>::new();
    let mut protocol_changed = false;
    if let Some(index) = request
        .draft
        .headers
        .iter()
        .position(|header| header.enabled && header.name.eq_ignore_ascii_case("authorization"))
    {
        let value = request.draft.headers[index].value.trim().to_owned();
        if !is_variable_template(&value)
            && let Some((scheme, credential)) = value.split_once(' ')
        {
            if scheme.eq_ignore_ascii_case("bearer") && !credential.trim().is_empty() {
                let reference = imported_secret_ref(request_id, "bearer-token");
                pending.push((reference.clone(), credential.trim().to_owned()));
                request.draft.auth = AuthDraft::Bearer {
                    token: SecretDraft {
                        reference,
                        replacement: credential.trim().to_owned(),
                        configured: false,
                    },
                };
                request.draft.headers.remove(index);
            } else if scheme.eq_ignore_ascii_case("basic")
                && let Ok(decoded) = STANDARD.decode(credential.trim())
                && let Ok(decoded) = String::from_utf8(decoded)
                && let Some((username, password)) = decoded.split_once(':')
            {
                let reference = imported_secret_ref(request_id, "basic-password");
                pending.push((reference.clone(), password.to_owned()));
                request.draft.auth = AuthDraft::Basic {
                    username: username.to_owned(),
                    password: SecretDraft {
                        reference,
                        replacement: password.to_owned(),
                        configured: false,
                    },
                };
                request.draft.headers.remove(index);
            }
        }
    }

    let mut secret_index = 0usize;
    for pair in &mut request.draft.headers {
        if sensitive_name(&pair.name) || pair.description == IMPORTED_AUTH_MARKER {
            externalize_imported_value(
                request_id,
                &mut secret_index,
                &mut pair.value,
                &mut request.request_case.local_variables,
                &mut pending,
            );
            if pair.description == IMPORTED_AUTH_MARKER {
                pair.description.clear();
                protocol_changed = true;
            }
        }
    }
    for pair in &mut request.draft.query {
        if sensitive_name(&pair.name) || pair.description == IMPORTED_AUTH_MARKER {
            externalize_imported_value(
                request_id,
                &mut secret_index,
                &mut pair.value,
                &mut request.request_case.local_variables,
                &mut pending,
            );
            if pair.description == IMPORTED_AUTH_MARKER {
                pair.description.clear();
                protocol_changed = true;
            }
        }
    }
    for pair in &mut request.draft.cookies {
        externalize_imported_value(
            request_id,
            &mut secret_index,
            &mut pair.value,
            &mut request.request_case.local_variables,
            &mut pending,
        );
    }
    for pair in &mut request.draft.form_fields {
        if sensitive_name(&pair.name) {
            externalize_imported_value(
                request_id,
                &mut secret_index,
                &mut pair.value,
                &mut request.request_case.local_variables,
                &mut pending,
            );
        }
    }
    for part in &mut request.draft.multipart_parts {
        if part.mode == MultipartValueMode::Text && sensitive_name(&part.name) {
            externalize_imported_value(
                request_id,
                &mut secret_index,
                &mut part.value,
                &mut request.request_case.local_variables,
                &mut pending,
            );
        }
    }
    if request.draft.body_mode == BodyMode::Json
        && let Ok(mut body) = serde_json::from_str::<serde_json::Value>(&request.draft.body)
    {
        let pending_before = pending.len();
        externalize_imported_json(
            &mut body,
            request_id,
            &mut secret_index,
            &mut request.request_case.local_variables,
            &mut pending,
        );
        if pending.len() > pending_before {
            request.draft.body = serde_json::to_string_pretty(&body)
                .expect("sanitized imported JSON body should serialize");
        }
    }
    let secured_body = match request.edited_protocol() {
        ProtocolSpec::Http(spec) => spec.body,
        _ => BodySpec::None,
    };
    secure_contract_body_examples(
        &mut request.definition.contract,
        &secured_body,
        request_id,
        &mut secret_index,
        &mut request.request_case.local_variables,
        &mut pending,
    );

    let mut saved = Vec::new();
    for (reference, value) in &pending {
        if let Err(error) = secrets.set(reference, value) {
            delete_secret_refs(secrets, &saved);
            return Err(error.to_string());
        }
        saved.push(reference.clone());
    }
    for secret in request.draft.auth.secrets_mut() {
        if pending
            .iter()
            .any(|(reference, _)| reference == &secret.reference)
        {
            secret.mark_saved();
        }
    }
    if !pending.is_empty() || protocol_changed {
        let protocol = request.edited_protocol();
        sync_contract_examples(&mut request.definition.contract, &protocol);
        request.request_case.protocol = protocol;
        request.sync_contract = false;
        request.observed_snapshot = request_snapshot(&request.name, request.edited_protocol());
    }
    Ok(saved)
}

fn secure_contract_body_examples(
    contract: &mut ApiContract,
    body: &BodySpec,
    request_id: EntityId,
    secret_index: &mut usize,
    variables: &mut Vec<Variable>,
    pending: &mut Vec<(SecretRef, String)>,
) {
    if matches!(body, BodySpec::None) {
        return;
    }
    let ApiContract::Http(contract) = contract else {
        return;
    };
    let Some(content) = contract.request_body.as_mut() else {
        return;
    };
    let mut examples = content.examples.values_mut();
    if let Some(first) = examples.next() {
        *first = body_example_value(body);
    }
    for example in examples {
        externalize_imported_json(example, request_id, secret_index, variables, pending);
    }
}

fn body_example_value(body: &BodySpec) -> serde_json::Value {
    match body {
        BodySpec::None => serde_json::Value::Null,
        BodySpec::Json(value) => {
            serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.clone()))
        }
        BodySpec::Text(value) | BodySpec::Xml(value) => serde_json::Value::String(value.clone()),
        BodySpec::FormUrlEncoded(fields) => serde_json::Value::Object(
            fields
                .iter()
                .filter(|field| field.enabled)
                .map(|field| {
                    (
                        field.name.clone(),
                        serde_json::Value::String(field.value.clone()),
                    )
                })
                .collect(),
        ),
        BodySpec::Multipart(parts) => serde_json::Value::Object(
            parts
                .iter()
                .filter_map(|part| match part {
                    MultipartPart::Text(field) if field.enabled => Some((
                        field.name.clone(),
                        serde_json::Value::String(field.value.clone()),
                    )),
                    MultipartPart::File {
                        name,
                        path,
                        enabled: true,
                        ..
                    } => Some((
                        name.clone(),
                        serde_json::Value::String(path.display().to_string()),
                    )),
                    _ => None,
                })
                .collect(),
        ),
        BodySpec::BinaryFile(path) => serde_json::Value::String(path.display().to_string()),
    }
}

fn sync_contract_examples(contract: &mut ApiContract, protocol: &ProtocolSpec) {
    let (ApiContract::Http(contract), ProtocolSpec::Http(spec)) = (contract, protocol) else {
        return;
    };
    contract.parameters.retain_mut(|parameter| {
        let values = match parameter.location {
            ApiParameterLocation::Query => &spec.query,
            ApiParameterLocation::Header => &spec.headers,
            ApiParameterLocation::Cookie => &spec.cookies,
            ApiParameterLocation::Path => return true,
        };
        if let Some(value) = values
            .iter()
            .find(|value| value.name.eq_ignore_ascii_case(&parameter.name))
        {
            parameter.example = Some(serde_json::Value::String(value.value.clone()));
            parameter.enabled = value.enabled;
            true
        } else {
            !sensitive_name(&parameter.name) && parameter.description != IMPORTED_AUTH_MARKER
        }
    });
    contract.body_example = spec.body.clone();
    contract.default_auth = spec.auth.clone();
    contract.timeout_ms = spec.timeout_ms;
    contract.follow_redirects = spec.follow_redirects;
    contract.validate_tls = spec.validate_tls;
    contract.proxy = spec.proxy.clone();
    contract.client_certificate = spec.client_certificate.clone();
}

fn imported_secret_ref(request_id: EntityId, suffix: &str) -> SecretRef {
    SecretRef::new(format!("keyring://request-{request_id}-{suffix}"))
}

fn externalize_imported_value(
    request_id: EntityId,
    index: &mut usize,
    value: &mut String,
    variables: &mut Vec<Variable>,
    pending: &mut Vec<(SecretRef, String)>,
) {
    if value.is_empty() || is_variable_template(value) {
        return;
    }
    let name = format!("__import_secret_{index}");
    let reference = imported_secret_ref(request_id, &format!("import-{index}"));
    pending.push((reference.clone(), std::mem::take(value)));
    let mut variable = Variable::secret(&name, reference.0.clone());
    variable.scope = VariableScope::Local;
    variables.push(variable);
    *value = format!("{{{{{name}}}}}");
    *index += 1;
}

fn externalize_imported_json(
    value: &mut serde_json::Value,
    request_id: EntityId,
    index: &mut usize,
    variables: &mut Vec<Variable>,
    pending: &mut Vec<(SecretRef, String)>,
) {
    match value {
        serde_json::Value::Object(object) => {
            for (name, value) in object {
                if sensitive_name(name) {
                    if let serde_json::Value::String(secret) = value {
                        externalize_imported_value(request_id, index, secret, variables, pending);
                    }
                } else {
                    externalize_imported_json(value, request_id, index, variables, pending);
                }
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                externalize_imported_json(value, request_id, index, variables, pending);
            }
        }
        _ => {}
    }
}

fn is_variable_template(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("{{") && value.ends_with("}}")
}

fn delete_secret_refs(secrets: &dyn SecretStore, references: &[SecretRef]) {
    for reference in references {
        if let Err(error) = secrets.delete(reference) {
            tracing::warn!(%error, "failed to clean imported secret");
        }
    }
}

fn default_protocol(kind: ProtocolKind) -> ProtocolSpec {
    match kind {
        ProtocolKind::Http => ProtocolSpec::Http(apitest_core::HttpSpec::new(HttpMethod::Get, "")),
        ProtocolKind::GraphQl => ProtocolSpec::GraphQl(GraphQlSpec {
            endpoint: String::new(),
            query: "query {\n  __typename\n}".into(),
            variables: "{}".into(),
            operation_name: None,
            headers: Vec::new(),
            auth: Default::default(),
            timeout_ms: 30_000,
        }),
        ProtocolKind::Sse => ProtocolSpec::Sse(SseSpec {
            request: apitest_core::HttpSpec::new(HttpMethod::Get, ""),
            reconnect: true,
        }),
        ProtocolKind::WebSocket => ProtocolSpec::WebSocket(WebSocketSpec {
            url: String::new(),
            query: Vec::new(),
            headers: Vec::new(),
            subprotocols: Vec::new(),
            validate_tls: true,
            connect_timeout_ms: 30_000,
        }),
        ProtocolKind::Grpc => ProtocolSpec::Grpc(GrpcSpec {
            endpoint: String::new(),
            service: String::new(),
            method: String::new(),
            call_kind: GrpcCallKind::Unary,
            descriptor_set: None,
            proto_files: Vec::new(),
            import_paths: Vec::new(),
            use_reflection: true,
            metadata: Vec::new(),
            message_json: "{}".into(),
            validate_tls: true,
            timeout_ms: 30_000,
        }),
    }
}

struct ResponseView {
    state: RunState,
    status: Option<u16>,
    version: Option<String>,
    headers: Vec<(String, String)>,
    body: String,
    pending_utf8: Vec<u8>,
    preview_bytes: usize,
    pretty_body: Option<String>,
    metrics: Option<ExecutionMetrics>,
    error: Option<String>,
    truncated: bool,
    started_at: Option<Instant>,
    timeline: Vec<TimelineEntry>,
    saw_first_byte: bool,
}

impl Default for ResponseView {
    fn default() -> Self {
        Self {
            state: RunState::Idle,
            status: None,
            version: None,
            headers: Vec::new(),
            body: String::new(),
            pending_utf8: Vec::new(),
            preview_bytes: 0,
            pretty_body: None,
            metrics: None,
            error: None,
            truncated: false,
            started_at: None,
            timeline: Vec::new(),
            saw_first_byte: false,
        }
    }
}

impl ResponseView {
    fn running() -> Self {
        Self {
            state: RunState::Running,
            started_at: Some(Instant::now()),
            timeline: vec![TimelineEntry {
                phase: TimelinePhase::Started,
                elapsed_ms: 0,
            }],
            ..Default::default()
        }
    }

    fn is_active(&self) -> bool {
        matches!(self.state, RunState::Running | RunState::Cancelling)
    }

    fn elapsed_ms(&self) -> u64 {
        self.started_at
            .map(|started| started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or_default()
    }

    fn push_timeline(&mut self, phase: TimelinePhase) {
        if self.timeline.iter().any(|entry| entry.phase == phase) {
            return;
        }
        self.timeline.push(TimelineEntry {
            phase,
            elapsed_ms: self.elapsed_ms(),
        });
    }

    fn append_body(&mut self, bytes: &[u8]) {
        if self.preview_bytes >= MAX_RESPONSE_BYTES {
            self.truncated = true;
            return;
        }
        if !bytes.is_empty() && !self.saw_first_byte {
            self.saw_first_byte = true;
            self.push_timeline(TimelinePhase::FirstByte);
        }
        let available = MAX_RESPONSE_BYTES - self.preview_bytes;
        let accepted = bytes.len().min(available);
        self.preview_bytes += accepted;
        self.truncated |= accepted < bytes.len();

        let mut data = std::mem::take(&mut self.pending_utf8);
        data.extend_from_slice(&bytes[..accepted]);
        let mut cursor = 0;
        while cursor < data.len() {
            match std::str::from_utf8(&data[cursor..]) {
                Ok(valid) => {
                    self.body.push_str(valid);
                    cursor = data.len();
                }
                Err(error) => {
                    let valid_end = cursor + error.valid_up_to();
                    if let Ok(valid) = std::str::from_utf8(&data[cursor..valid_end]) {
                        self.body.push_str(valid);
                    }
                    if let Some(error_len) = error.error_len() {
                        self.body.push('\u{fffd}');
                        cursor = valid_end + error_len;
                    } else {
                        self.pending_utf8.extend_from_slice(&data[valid_end..]);
                        cursor = data.len();
                    }
                }
            }
        }
    }

    fn finish_body(&mut self) {
        if !self.pending_utf8.is_empty() {
            self.body.push('\u{fffd}');
            self.pending_utf8.clear();
        }
        self.pretty_body = serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|value| serde_json::to_string_pretty(&value).ok());
    }
}

enum RuntimeMessage {
    Event(u64, Result<ExecutionEvent, ExecutionError>),
    Closed(u64),
    ScenarioFinished(u64, Result<ScenarioReport, String>),
    MockStarted(u64, Result<MockServer, String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToastKind {
    Success,
    Info,
    Error,
}

struct Toast {
    message: String,
    kind: ToastKind,
    created_at: Instant,
}

#[derive(Debug, Clone, Copy)]
enum PendingAction {
    SelectRequest(EntityId),
    NewRequest(ProtocolKind),
    SelectScenario(EntityId),
    NewScenario,
    SelectMock(EntityId),
    NewMock,
    SelectProject(EntityId),
    NewProject,
    Navigate(Navigation),
    SelectEnvironment(EntityId),
    NewEnvironment,
    CloseDocument(DocumentId),
    Close,
}

impl PendingAction {
    fn leaves_workspace(self) -> bool {
        matches!(
            self,
            Self::SelectProject(_) | Self::NewProject | Self::Close
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum Confirmation {
    Unsaved(PendingAction),
    DeleteRequest(EntityId),
    DeleteEnvironment(EntityId),
}

pub struct ApiTestApp {
    runtime: Arc<tokio::runtime::Runtime>,
    executors: Arc<ExecutorRegistry>,
    secrets: Arc<dyn SecretStore>,
    database: Option<Arc<Database>>,
    body_store: Option<BodyStore>,
    history_body: Option<RedactingBodySink>,
    history_record: Option<RunRecord>,
    history_redactions: Vec<String>,
    run_records: Vec<RunRecord>,
    selected_history: usize,
    history_body_preview: String,
    history_body_truncated: bool,
    storage_worker: Option<StorageWorker>,
    projects: Vec<Project>,
    project: Project,
    requests: Vec<WorkspaceRequest>,
    resource_pages: HashMap<Option<EntityId>, ResourcePage>,
    expanded_resources: HashSet<EntityId>,
    environments: Vec<EnvironmentDraft>,
    scenarios: Vec<TestScenario>,
    selected_scenario: usize,
    scenario_snapshots: HashMap<EntityId, Vec<u8>>,
    scenario_report: Option<ScenarioReport>,
    scenario_cancellation: Option<CancellationToken>,
    scenario_run_id: u64,
    mock_profiles: Vec<MockProfile>,
    selected_mock: usize,
    mock_snapshots: HashMap<EntityId, Vec<u8>>,
    mock_server: Option<MockServer>,
    mock_run_id: u64,
    selected: usize,
    selected_environment: usize,
    active_environment: EntityId,
    navigation: Navigation,
    editor_tab: EditorTab,
    response_tab: ResponseTab,
    response_body_mode: ResponseBodyMode,
    response: ResponseView,
    sender: mpsc::Sender<RuntimeMessage>,
    receiver: mpsc::Receiver<RuntimeMessage>,
    cancellation: Option<CancellationToken>,
    execution_commands: Option<tokio::sync::mpsc::Sender<ExecutionCommand>>,
    websocket_message: String,
    run_id: u64,
    theme: ThemeMode,
    language: Language,
    search: String,
    toast: Option<Toast>,
    confirmation: Option<Confirmation>,
    document_tabs: DocumentTabs,
    show_settings: bool,
    show_curl_import: bool,
    curl_import_source: String,
    show_openapi_preview: bool,
    openapi_preview_tab: OpenApiPreviewTab,
    openapi_source: String,
    openapi_html: String,
    openapi_issues: Vec<OpenApiIssue>,
    allow_close: bool,
}

impl ApiTestApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        let mut startup_errors = Vec::new();
        let database = match open_database() {
            Ok(database) => Some(Arc::new(database)),
            Err(error) => {
                tracing::error!(%error, "failed to open ApiTest database");
                startup_errors.push(error);
                None
            }
        };
        let body_store = database
            .as_ref()
            .and_then(|database| database.path())
            .and_then(std::path::Path::parent)
            .map(|directory| BodyStore::new(directory.join("response-bodies")))
            .transpose()
            .unwrap_or_else(|error| {
                startup_errors.push(error.to_string());
                None
            });
        let theme = load_setting(database.as_deref(), THEME_SETTING, ThemeMode::Dark);
        let language = load_setting(database.as_deref(), LANGUAGE_SETTING, Language::Chinese);
        theme::install_fonts(&context.egui_ctx);
        theme::apply(&context.egui_ctx, theme);

        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("apitest-runtime")
                .build()
                .expect("ApiTest async runtime should initialize"),
        );
        let secrets: Arc<dyn SecretStore> = Arc::new(SystemSecretStore::new("ApiTest"));
        let http: Arc<dyn ProtocolExecutor> = Arc::new(HttpExecutor::new(Arc::clone(&secrets)));
        let websocket: Arc<dyn ProtocolExecutor> =
            Arc::new(WebSocketExecutor::new(Arc::clone(&secrets)));
        let grpc: Arc<dyn ProtocolExecutor> = Arc::new(GrpcExecutor::new(Arc::clone(&secrets)));
        let mut executors = ExecutorRegistry::new();
        executors.register(ProtocolKind::Http, Arc::clone(&http));
        executors.register(ProtocolKind::GraphQl, Arc::clone(&http));
        executors.register(ProtocolKind::Sse, http);
        executors.register(ProtocolKind::WebSocket, websocket);
        executors.register(ProtocolKind::Grpc, grpc);
        let (projects, project, requests, environments, mut resource_pages, load_errors) =
            load_workspace(database.as_deref(), language);
        startup_errors.extend(load_errors);
        let (scenarios, mock_profiles, automation_errors) =
            load_automation(database.as_deref(), project.id);
        startup_errors.extend(automation_errors);
        let run_records = database
            .as_deref()
            .map(|database| {
                database
                    .list_run_records(project.id, PageRequest::new(0, HISTORY_MAX_RECORDS))
                    .map(|page| page.items)
            })
            .transpose()
            .unwrap_or_else(|error| {
                startup_errors.push(error.to_string());
                None
            })
            .unwrap_or_default();
        let scenario_snapshots = scenarios
            .iter()
            .map(|scenario| (scenario.id, document_snapshot(scenario)))
            .collect();
        let mock_snapshots = mock_profiles
            .iter()
            .map(|profile| (profile.id, document_snapshot(profile)))
            .collect();
        let active_environment_key = active_environment_setting(project.id);
        let active_environment = database
            .as_deref()
            .and_then(|database| database.get_setting(&active_environment_key).ok().flatten())
            .filter(|id| {
                environments
                    .iter()
                    .any(|environment| environment.id() == *id)
            })
            .unwrap_or_else(|| environments[0].id());
        let mut selected_environment = environments
            .iter()
            .position(|environment| environment.id() == active_environment)
            .unwrap_or_default();
        let (sender, receiver) = mpsc::channel();
        let storage_worker = database
            .as_ref()
            .map(|database| StorageWorker::new(Arc::clone(database)));
        resource_pages.entry(None).or_default();
        let document_tabs = load_document_tabs(
            database.as_deref(),
            project.id,
            &requests,
            &environments,
            &scenarios,
            &mock_profiles,
        );
        let mut selected = 0;
        let mut selected_scenario = 0;
        let mut selected_mock = 0;
        let mut navigation = Navigation::Api;
        if let Some(active) = document_tabs.active() {
            match active.kind {
                DocumentKind::Api => {
                    selected = requests
                        .iter()
                        .position(|request| request.id() == active.entity_id)
                        .unwrap_or_default();
                }
                DocumentKind::Environment => {
                    selected_environment = environments
                        .iter()
                        .position(|environment| environment.id() == active.entity_id)
                        .unwrap_or(selected_environment);
                    navigation = Navigation::Environment;
                }
                DocumentKind::Scenario => {
                    selected_scenario = scenarios
                        .iter()
                        .position(|scenario| scenario.id == active.entity_id)
                        .unwrap_or_default();
                    navigation = Navigation::Scenario;
                }
                DocumentKind::Mock => {
                    selected_mock = mock_profiles
                        .iter()
                        .position(|profile| profile.id == active.entity_id)
                        .unwrap_or_default();
                    navigation = Navigation::Mock;
                }
                DocumentKind::History => {}
            }
        }

        Self {
            runtime,
            executors: Arc::new(executors),
            secrets,
            database,
            body_store,
            history_body: None,
            history_record: None,
            history_redactions: Vec::new(),
            run_records,
            selected_history: 0,
            history_body_preview: String::new(),
            history_body_truncated: false,
            storage_worker,
            projects,
            project,
            requests,
            resource_pages,
            expanded_resources: HashSet::new(),
            environments,
            scenarios,
            selected_scenario,
            scenario_snapshots,
            scenario_report: None,
            scenario_cancellation: None,
            scenario_run_id: 0,
            mock_profiles,
            selected_mock,
            mock_snapshots,
            mock_server: None,
            mock_run_id: 0,
            selected,
            selected_environment,
            active_environment,
            navigation,
            editor_tab: EditorTab::Params,
            response_tab: ResponseTab::Body,
            response_body_mode: ResponseBodyMode::Pretty,
            response: ResponseView::default(),
            sender,
            receiver,
            cancellation: None,
            execution_commands: None,
            websocket_message: String::new(),
            run_id: 0,
            theme,
            language,
            search: String::new(),
            toast: startup_errors.pop().map(|message| Toast {
                message,
                kind: ToastKind::Error,
                created_at: Instant::now(),
            }),
            confirmation: None,
            document_tabs,
            show_settings: false,
            show_curl_import: false,
            curl_import_source: String::new(),
            show_openapi_preview: false,
            openapi_preview_tab: OpenApiPreviewTab::Yaml,
            openapi_source: String::new(),
            openapi_html: String::new(),
            openapi_issues: Vec::new(),
            allow_close: false,
        }
    }

    fn tr<'a>(&self, chinese: &'a str, english: &'a str) -> &'a str {
        match self.language {
            Language::Chinese => chinese,
            Language::English => english,
        }
    }

    fn toast(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.toast = Some(Toast {
            message: message.into(),
            kind,
            created_at: Instant::now(),
        });
    }

    fn persist_setting<T: Serialize>(&mut self, key: &str, value: &T) {
        let Some(database) = &self.database else {
            return;
        };
        if let Err(error) = database.set_setting(key, value) {
            self.toast(ToastKind::Error, error.to_string());
        }
    }

    fn persist_document_tabs(&mut self) {
        let key = document_tabs_setting(self.project.id);
        let tabs = self.document_tabs.clone();
        self.persist_setting(&key, &tabs);
    }

    fn open_document(&mut self, id: DocumentId, title: impl Into<String>) {
        self.document_tabs.open(id, title);
        self.persist_document_tabs();
    }

    fn activate_document(&mut self, id: DocumentId) -> bool {
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

    fn close_document(&mut self, id: DocumentId) {
        let active = self.document_tabs.close(id);
        self.persist_document_tabs();
        if let Some(active) = active {
            self.activate_document(active);
        }
    }

    fn current_dirty(&self) -> bool {
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

    fn workspace_dirty(&self) -> bool {
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

    fn document_dirty(&self, id: DocumentId) -> bool {
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

    fn queue_action(&mut self, action: PendingAction) {
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

    fn perform_action(&mut self, action: PendingAction) {
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

    fn create_project(&mut self) {
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

    fn switch_project(&mut self, id: EntityId) {
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

    fn reload_resource_page(&mut self, parent_id: Option<EntityId>) {
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

    fn load_more_resources(&mut self, parent_id: Option<EntityId>) {
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

    fn toggle_resource(&mut self, id: EntityId) {
        if self.expanded_resources.remove(&id) {
            return;
        }
        self.expanded_resources.insert(id);
        if !self.resource_pages.contains_key(&Some(id)) {
            self.reload_resource_page(Some(id));
        }
    }

    fn visible_resource_rows(&self) -> Vec<ResourceRow> {
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

    fn discard_current(&mut self) -> bool {
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

    fn discard_request(&mut self, index: usize) -> bool {
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
            self.document_tabs.close(DocumentId {
                kind: DocumentKind::Api,
                entity_id: id,
            });
            self.persist_document_tabs();
            self.selected = self.selected.min(self.requests.len().saturating_sub(1));
        }
        true
    }

    fn discard_environment(&mut self, index: usize) -> bool {
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

    fn discard_workspace(&mut self) -> bool {
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

    fn save_current(&mut self) -> bool {
        match self.navigation {
            Navigation::Api => self.save_current_request(),
            Navigation::Environment => self.save_current_environment(),
            Navigation::Scenario => self.save_current_scenario(),
            Navigation::Mock => self.save_current_mock(),
            Navigation::History => true,
        }
    }

    fn save_current_and_wait(&mut self) -> bool {
        if !self.save_current() {
            return false;
        }
        match self.navigation {
            Navigation::Api | Navigation::Environment => {
                self.flush_storage() && !self.current_dirty()
            }
            Navigation::Scenario | Navigation::Mock | Navigation::History => !self.current_dirty(),
        }
    }

    fn save_workspace_and_wait(&mut self) -> bool {
        for index in 0..self.requests.len() {
            if self.requests[index].is_dirty()
                && (!self.queue_request_save(index, true, false) || !self.flush_storage())
            {
                return false;
            }
        }
        for index in 0..self.environments.len() {
            if self.environments[index].is_dirty()
                && (!self.queue_environment_save(index, true, false) || !self.flush_storage())
            {
                return false;
            }
        }
        let selected_scenario = self.selected_scenario;
        for index in 0..self.scenarios.len() {
            let dirty = self
                .scenario_snapshots
                .get(&self.scenarios[index].id)
                .is_none_or(|saved| saved != &document_snapshot(&self.scenarios[index]));
            if dirty {
                self.selected_scenario = index;
                if !self.save_current_scenario() {
                    self.selected_scenario = selected_scenario;
                    return false;
                }
            }
        }
        self.selected_scenario = selected_scenario.min(self.scenarios.len().saturating_sub(1));
        let selected_mock = self.selected_mock;
        for index in 0..self.mock_profiles.len() {
            let dirty = self
                .mock_snapshots
                .get(&self.mock_profiles[index].id)
                .is_none_or(|saved| saved != &document_snapshot(&self.mock_profiles[index]));
            if dirty {
                self.selected_mock = index;
                if !self.save_current_mock() {
                    self.selected_mock = selected_mock;
                    return false;
                }
            }
        }
        self.selected_mock = selected_mock.min(self.mock_profiles.len().saturating_sub(1));
        !self.workspace_dirty()
    }

    fn cleanup_secret_references<I>(&self, references: I)
    where
        I: IntoIterator<Item = SecretRef>,
    {
        for reference in references {
            if let Err(error) = self.secrets.delete(&reference) {
                tracing::warn!(%error, reference = reference.as_str(), "failed to clean unused secret");
            }
        }
    }

    fn validate_auth(&self, auth: &AuthDraft) -> Result<(), String> {
        match auth {
            AuthDraft::None | AuthDraft::Unsupported(_) => Ok(()),
            AuthDraft::Basic { username, password } => {
                if username.trim().is_empty() {
                    return Err(self
                        .tr("请输入 Basic 用户名", "Enter the Basic username")
                        .into());
                }
                if !password.is_ready() {
                    return Err(self
                        .tr("请输入 Basic 密码", "Enter the Basic password")
                        .into());
                }
                Ok(())
            }
            AuthDraft::Bearer { token } => {
                if token.is_ready() {
                    Ok(())
                } else {
                    Err(self
                        .tr("请输入 Bearer Token", "Enter the Bearer token")
                        .into())
                }
            }
            AuthDraft::ApiKey { name, value, .. } => {
                if name.trim().is_empty() {
                    return Err(self
                        .tr("请输入 API Key 名称", "Enter the API key name")
                        .into());
                }
                if !value.is_ready() {
                    return Err(self
                        .tr("请输入 API Key 值", "Enter the API key value")
                        .into());
                }
                Ok(())
            }
            AuthDraft::OAuth2 {
                grant,
                token_url,
                client_id,
                username,
                password,
                access_token,
                ..
            } => {
                if access_token.is_ready() {
                    return Ok(());
                }
                if *grant == OAuth2Grant::AuthorizationCodePkce {
                    return Err(self
                        .tr(
                            "授权码 PKCE 流程需要先配置访问令牌",
                            "Authorization Code PKCE requires a cached access token",
                        )
                        .into());
                }
                if token_url.trim().is_empty() {
                    return Err(self
                        .tr("请输入 OAuth2 Token URL", "Enter the OAuth2 token URL")
                        .into());
                }
                if client_id.trim().is_empty() {
                    return Err(self
                        .tr("请输入 OAuth2 Client ID", "Enter the OAuth2 client ID")
                        .into());
                }
                if *grant == OAuth2Grant::Password {
                    if username.trim().is_empty() {
                        return Err(self
                            .tr("请输入 OAuth2 用户名", "Enter the OAuth2 username")
                            .into());
                    }
                    if !password.is_ready() {
                        return Err(self
                            .tr("请输入 OAuth2 密码", "Enter the OAuth2 password")
                            .into());
                    }
                }
                Ok(())
            }
            AuthDraft::Digest { username, password } => {
                if username.trim().is_empty() {
                    return Err(self
                        .tr("请输入 Digest 用户名", "Enter the Digest username")
                        .into());
                }
                if !password.is_ready() {
                    return Err(self
                        .tr("请输入 Digest 密码", "Enter the Digest password")
                        .into());
                }
                Ok(())
            }
            AuthDraft::AwsSigV4 {
                access_key,
                secret_key,
                region,
                service,
                ..
            } => {
                if !access_key.is_ready() {
                    return Err(self
                        .tr("请输入 AWS Access Key", "Enter the AWS access key")
                        .into());
                }
                if !secret_key.is_ready() {
                    return Err(self
                        .tr("请输入 AWS Secret Key", "Enter the AWS secret key")
                        .into());
                }
                if region.trim().is_empty() || service.trim().is_empty() {
                    return Err(self
                        .tr(
                            "请输入 AWS Region 和 Service",
                            "Enter the AWS region and service",
                        )
                        .into());
                }
                Ok(())
            }
        }
    }

    fn validate_proxy(&self, proxy: Option<&ProxyDraft>) -> Result<(), String> {
        let Some(proxy) = proxy else {
            return Ok(());
        };
        if proxy.url.trim().is_empty() {
            return Err(self.tr("请输入代理地址", "Enter the proxy URL").into());
        }
        if proxy.password.is_ready() && proxy.username.trim().is_empty() {
            return Err(self
                .tr(
                    "配置代理密码时必须填写用户名",
                    "A proxy username is required when a password is configured",
                )
                .into());
        }
        Ok(())
    }

    fn persist_request_secret(&mut self, index: usize) -> Result<(), String> {
        let Some(request) = self.requests.get_mut(index) else {
            return Ok(());
        };
        persist_request_draft_secrets(self.secrets.as_ref(), &mut request.draft)
    }

    fn save_current_request(&mut self) -> bool {
        self.queue_request_save(self.selected, true, true)
    }

    fn queue_request_save(&mut self, index: usize, validate: bool, notify: bool) -> bool {
        if self.storage_worker.is_none() {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return false;
        }
        let Some(request) = self.requests.get(index) else {
            return false;
        };
        if validate && request.name.trim().is_empty() {
            self.toast(
                ToastKind::Error,
                self.tr("请求名称不能为空", "Request name cannot be empty"),
            );
            return false;
        }
        if validate {
            if let Err(error) = self.validate_auth(&request.draft.auth) {
                self.toast(ToastKind::Error, error);
                return false;
            }
            if let Err(error) = self.validate_proxy(request.draft.proxy.as_ref()) {
                self.toast(ToastKind::Error, error);
                return false;
            }
        }
        if let Err(error) = self.persist_request_secret(index) {
            self.toast(ToastKind::Error, error);
            return false;
        }
        self.requests[index].sync_edit_revision(Instant::now());
        let revision = self.requests[index].autosave.current_revision();
        if revision == 0 && self.requests[index].persisted {
            if notify {
                self.toast(
                    ToastKind::Info,
                    self.tr("没有需要保存的更改", "No changes to save"),
                );
            }
            return true;
        }
        let (definition, request_case) = self.requests[index].save_snapshot();
        let result = self
            .storage_worker
            .as_ref()
            .expect("storage worker was checked above")
            .queue_request(self.project.id, definition, request_case, revision, notify);
        match result {
            Ok(()) => {
                self.requests[index].autosave.mark_queued(revision);
                true
            }
            Err(error) => {
                self.requests[index]
                    .autosave
                    .mark_failed(revision, Instant::now());
                self.toast(ToastKind::Error, error.to_string());
                false
            }
        }
    }

    fn schedule_request_autosaves(&mut self, context: &egui::Context) {
        let now = Instant::now();
        for request in &mut self.requests {
            request.sync_edit_revision(now);
        }
        let due = self
            .requests
            .iter()
            .enumerate()
            .filter_map(|(index, request)| request.autosave.is_due(now).then_some(index))
            .collect::<Vec<_>>();
        for index in due {
            self.queue_request_save(index, false, false);
        }
        if self.requests.iter().any(WorkspaceRequest::is_dirty) {
            context.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn drain_storage(&mut self) -> bool {
        self.drain_storage_protected(&HashSet::new())
    }

    fn drain_storage_protected(
        &mut self,
        protected_environment_secrets: &HashSet<SecretRef>,
    ) -> bool {
        let mut succeeded = true;
        loop {
            let Some(event) = self
                .storage_worker
                .as_ref()
                .and_then(StorageWorker::try_recv)
            else {
                break;
            };
            match event {
                StorageEvent::RequestSaved {
                    revision,
                    definition,
                    request_case,
                    notify,
                } => {
                    let mut renamed = None;
                    if let Some(request) = self
                        .requests
                        .iter_mut()
                        .find(|request| request.id() == definition.id)
                    {
                        request.mark_saved(definition, request_case, revision);
                        renamed = Some((request.id(), request.name.clone()));
                    }
                    if let Some((id, name)) = renamed {
                        self.document_tabs.rename(
                            DocumentId {
                                kind: DocumentKind::Api,
                                entity_id: id,
                            },
                            name,
                        );
                        self.persist_document_tabs();
                        self.reload_resource_page(None);
                    }
                    if notify {
                        self.toast(ToastKind::Success, self.tr("已保存", "Saved"));
                    }
                }
                StorageEvent::EnvironmentSaved {
                    revision,
                    environment,
                    notify,
                } => {
                    let mut renamed = None;
                    if let Some(draft) = self
                        .environments
                        .iter_mut()
                        .find(|draft| draft.id() == environment.id)
                    {
                        let old_references = draft
                            .saved
                            .variables
                            .iter()
                            .filter_map(|variable| variable.secret_ref.clone())
                            .collect::<HashSet<_>>();
                        let new_references = environment
                            .variables
                            .iter()
                            .filter_map(|variable| variable.secret_ref.clone())
                            .collect::<HashSet<_>>();
                        let current_references = draft
                            .variables
                            .iter()
                            .filter(|variable| variable.secret)
                            .filter_map(|variable| variable.reference.clone())
                            .collect::<HashSet<_>>();
                        for reference in old_references.difference(&new_references) {
                            if current_references.contains(reference)
                                || protected_environment_secrets.contains(reference)
                            {
                                continue;
                            }
                            if let Err(error) = self.secrets.delete(reference) {
                                tracing::warn!(%error, "failed to clean unused environment secret");
                            }
                        }
                        draft.mark_saved(environment, revision);
                        renamed = Some((draft.id(), draft.name.clone()));
                    }
                    if let Some((id, name)) = renamed {
                        self.document_tabs.rename(
                            DocumentId {
                                kind: DocumentKind::Environment,
                                entity_id: id,
                            },
                            name,
                        );
                        self.persist_document_tabs();
                    }
                    if notify {
                        self.toast(
                            ToastKind::Success,
                            self.tr("环境已保存", "Environment saved"),
                        );
                    }
                }
                StorageEvent::SaveFailed {
                    revision,
                    entity_id,
                    error,
                } => {
                    succeeded = false;
                    if let Some(request) = self
                        .requests
                        .iter_mut()
                        .find(|request| request.id() == entity_id)
                    {
                        request.autosave.mark_failed(revision, Instant::now());
                    } else if let Some(environment) = self
                        .environments
                        .iter_mut()
                        .find(|environment| environment.id() == entity_id)
                    {
                        environment.autosave.mark_failed(revision, Instant::now());
                    }
                    self.toast(ToastKind::Error, error);
                }
            }
        }
        succeeded
    }

    fn wait_storage(&mut self) -> bool {
        let result = self
            .storage_worker
            .as_ref()
            .map(StorageWorker::flush)
            .transpose();
        if let Err(error) = result {
            self.toast(ToastKind::Error, error.to_string());
            return false;
        }
        true
    }

    fn flush_storage(&mut self) -> bool {
        self.wait_storage() && self.drain_storage()
    }

    fn settle_storage(&mut self) -> bool {
        if !self.wait_storage() {
            return false;
        }
        let _ = self.drain_storage();
        true
    }

    fn validate_environment(&self, index: usize) -> Result<(), String> {
        let environment = &self.environments[index];
        if environment.name.trim().is_empty() {
            return Err(self
                .tr("环境名称不能为空", "Environment name cannot be empty")
                .into());
        }
        if self.environments.iter().enumerate().any(|(other, value)| {
            other != index && value.name.eq_ignore_ascii_case(&environment.name)
        }) {
            return Err(self
                .tr("环境名称不能重复", "Environment names must be unique")
                .into());
        }
        let mut names = HashSet::new();
        for variable in environment
            .variables
            .iter()
            .filter(|variable| !variable.is_empty())
        {
            if variable.name.trim().is_empty() {
                return Err(self
                    .tr("变量名称不能为空", "Variable name cannot be empty")
                    .into());
            }
            if !names.insert(variable.name.trim().to_owned()) {
                return Err(self
                    .tr(
                        "同一环境中的变量名称不能重复",
                        "Variable names must be unique",
                    )
                    .into());
            }
            if !variable.is_ready() {
                return Err(self.tr("请输入密钥值", "Enter the secret value").into());
            }
        }
        Ok(())
    }

    fn persist_environment_secrets(&mut self, index: usize) -> Result<(), String> {
        let environment_id = self.environments[index].id();
        let mut pending = Vec::new();
        for (variable_index, variable) in self.environments[index].variables.iter_mut().enumerate()
        {
            if variable.has_pending_secret() {
                let reference = variable.ensure_reference(environment_id).clone();
                pending.push((variable_index, reference, variable.value.clone()));
            }
        }
        for (_, reference, value) in &pending {
            self.secrets
                .set(reference, value)
                .map_err(|error| error.to_string())?;
        }
        for (variable_index, _, _) in pending {
            self.environments[index].variables[variable_index].mark_secret_saved();
        }
        Ok(())
    }

    fn save_current_environment(&mut self) -> bool {
        self.queue_environment_save(self.selected_environment, true, true)
    }

    fn save_current_scenario(&mut self) -> bool {
        let Some(scenario) = self.scenarios.get(self.selected_scenario) else {
            return false;
        };
        if scenario.name.trim().is_empty() {
            self.toast(
                ToastKind::Error,
                self.tr("场景名称不能为空", "Scenario name cannot be empty"),
            );
            return false;
        }
        if let Err(error) = validate_scenario_nodes(&scenario.nodes, &self.requests) {
            self.toast(ToastKind::Error, error);
            return false;
        }
        let Some(database) = &self.database else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return false;
        };
        if let Err(error) = database.save_scenario(self.project.id, scenario) {
            self.toast(ToastKind::Error, error.to_string());
            return false;
        }
        let id = scenario.id;
        let name = scenario.name.clone();
        self.scenario_snapshots
            .insert(id, document_snapshot(scenario));
        self.document_tabs.rename(
            DocumentId {
                kind: DocumentKind::Scenario,
                entity_id: id,
            },
            name,
        );
        self.persist_document_tabs();
        self.toast(ToastKind::Success, self.tr("场景已保存", "Scenario saved"));
        true
    }

    fn run_current_scenario(&mut self, context: &egui::Context) {
        let Some(scenario) = self.scenarios.get(self.selected_scenario).cloned() else {
            return;
        };
        if scenario.name.trim().is_empty() {
            self.toast(
                ToastKind::Error,
                self.tr("场景名称不能为空", "Scenario name cannot be empty"),
            );
            return;
        }
        if let Err(error) = validate_scenario_nodes(&scenario.nodes, &self.requests) {
            self.toast(ToastKind::Error, error);
            return;
        }
        let case_ids = scenario_case_ids(&scenario.nodes);
        let request_indices = self
            .requests
            .iter()
            .enumerate()
            .filter_map(|(index, request)| {
                case_ids.contains(&request.request_case.id).then_some(index)
            })
            .collect::<Vec<_>>();
        for index in request_indices {
            let validation = self
                .validate_auth(&self.requests[index].draft.auth)
                .and_then(|_| self.validate_proxy(self.requests[index].draft.proxy.as_ref()));
            if let Err(error) = validation {
                self.toast(ToastKind::Error, error);
                return;
            }
            if let Err(error) = self.persist_request_secret(index) {
                self.toast(ToastKind::Error, error);
                return;
            }
        }
        let active_index = self
            .environments
            .iter()
            .position(|environment| environment.id() == self.active_environment)
            .unwrap_or_default();
        if let Err(error) = self.persist_environment_secrets(active_index) {
            self.toast(ToastKind::Error, error);
            return;
        }
        let datasets = match load_scenario_datasets(scenario.dataset_path.as_deref()) {
            Ok(datasets) => datasets,
            Err(error) => {
                self.toast(ToastKind::Error, error);
                return;
            }
        };
        let environment = self.environments[active_index].to_environment();
        let initial_variables = environment
            .variables
            .iter()
            .filter(|variable| variable.enabled && !variable.is_secret())
            .map(|variable| (variable.name.clone(), variable.value.clone()))
            .collect::<BTreeMap<_, _>>();
        let cases = self
            .requests
            .iter()
            .map(|request| {
                let mut execution =
                    ExecutionRequest::new(request.edited_protocol(), environment.clone());
                execution.case_id = Some(request.request_case.id);
                execution.local_variables = request.request_case.local_variables.clone();
                (
                    request.request_case.id,
                    ScenarioCase {
                        name: request.name.clone(),
                        request: execution,
                        pre_script: request.request_case.pre_request_script.clone(),
                        post_script: request.request_case.post_response_script.clone(),
                        assertions: request.request_case.assertions.clone(),
                        extractors: request.request_case.extractors.clone(),
                        response_schema: match &request.definition.contract {
                            ApiContract::Http(contract) => Some(ResponseSchemaContract {
                                responses: contract.responses.clone(),
                                components: self.project.components.clone(),
                            }),
                            _ => None,
                        },
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let mut runner = ScenarioRunner::new();
        for kind in [
            ProtocolKind::Http,
            ProtocolKind::GraphQl,
            ProtocolKind::Sse,
            ProtocolKind::WebSocket,
            ProtocolKind::Grpc,
        ] {
            if let Some(executor) = self.executors.executor(kind) {
                runner.register_executor_for(kind, executor);
            }
        }
        if let Some(cancellation) = self.scenario_cancellation.take() {
            cancellation.cancel();
        }
        self.scenario_run_id = self.scenario_run_id.wrapping_add(1);
        let run_id = self.scenario_run_id;
        let cancellation = CancellationToken::new();
        self.scenario_cancellation = Some(cancellation.clone());
        self.scenario_report = None;
        let sender = self.sender.clone();
        let context = context.clone();
        self.runtime.spawn(async move {
            let started = Instant::now();
            let mut passed = true;
            let mut steps = Vec::new();
            let iterations = datasets.len();
            let mut result = Ok(());
            for (index, dataset) in datasets.into_iter().enumerate() {
                let mut variables = initial_variables.clone();
                variables.extend(dataset);
                match runner
                    .run_test_scenario(
                        scenario.clone(),
                        cases.clone(),
                        variables,
                        cancellation.child_token(),
                    )
                    .await
                {
                    Ok(mut report) => {
                        if iterations > 1 {
                            for step in &mut report.steps {
                                step.name = format!("[{}] {}", index + 1, step.name);
                            }
                        }
                        passed &= report.passed;
                        steps.append(&mut report.steps);
                        if !report.passed && scenario.stop_on_failure {
                            break;
                        }
                    }
                    Err(error) => {
                        result = Err(error.to_string());
                        break;
                    }
                }
            }
            let report = result.map(|()| ScenarioReport {
                name: scenario.name,
                passed,
                elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
                steps,
            });
            let _ = sender.send(RuntimeMessage::ScenarioFinished(run_id, report));
            context.request_repaint();
        });
    }

    fn stop_current_scenario(&mut self) {
        if let Some(cancellation) = self.scenario_cancellation.take() {
            cancellation.cancel();
            self.scenario_run_id = self.scenario_run_id.wrapping_add(1);
            self.toast(
                ToastKind::Info,
                self.tr("场景执行已停止", "Scenario stopped"),
            );
        }
    }

    fn save_current_mock(&mut self) -> bool {
        let Some(profile) = self.mock_profiles.get(self.selected_mock) else {
            return false;
        };
        if let Err(error) = validate_mock_profile(profile) {
            self.toast(ToastKind::Error, error);
            return false;
        }
        let Some(database) = &self.database else {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return false;
        };
        if let Err(error) = database.save_mock_profile(self.project.id, profile) {
            self.toast(ToastKind::Error, error.to_string());
            return false;
        }
        let id = profile.id;
        let name = profile.name.clone();
        self.mock_snapshots.insert(id, document_snapshot(profile));
        self.document_tabs.rename(
            DocumentId {
                kind: DocumentKind::Mock,
                entity_id: id,
            },
            name,
        );
        self.persist_document_tabs();
        self.toast(ToastKind::Success, self.tr("Mock 已保存", "Mock saved"));
        true
    }

    fn generate_current_mock_rules(&mut self) {
        let definitions = self.contract_definitions();
        let rules = generate_mock_rules(&definitions, &self.project.components);
        if rules.is_empty() {
            self.toast(
                ToastKind::Error,
                self.tr(
                    "当前项目没有可生成 Mock 的 HTTP 契约",
                    "The project has no HTTP contracts to mock",
                ),
            );
            return;
        }
        let mut count = 0;
        if let Some(profile) = self.mock_profiles.get_mut(self.selected_mock) {
            for rule in rules {
                if profile
                    .rules
                    .iter()
                    .any(|existing| existing.method == rule.method && existing.path == rule.path)
                {
                    continue;
                }
                profile.rules.push(rule);
                count += 1;
            }
        }
        self.toast(
            ToastKind::Info,
            match self.language {
                Language::Chinese => format!("已根据契约新增 {count} 条 Mock 规则"),
                Language::English => format!("Added {count} mock rules from contracts"),
            },
        );
    }

    fn start_current_mock(&mut self, context: &egui::Context) {
        if self.mock_server.is_some() {
            return;
        }
        let Some(profile) = self.mock_profiles.get(self.selected_mock) else {
            return;
        };
        if let Err(error) = validate_mock_profile(profile) {
            self.toast(ToastKind::Error, error);
            return;
        }
        let address = match profile.bind_address.parse::<std::net::IpAddr>() {
            Ok(address) => std::net::SocketAddr::new(address, profile.port),
            Err(error) => {
                self.toast(ToastKind::Error, error.to_string());
                return;
            }
        };
        let routes = profile
            .rules
            .iter()
            .filter(|rule| rule.enabled)
            .map(MockRoute::from)
            .collect::<Vec<_>>();
        self.mock_run_id = self.mock_run_id.wrapping_add(1);
        let run_id = self.mock_run_id;
        let sender = self.sender.clone();
        let context = context.clone();
        self.runtime.spawn(async move {
            let result = MockServer::start(address, routes)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(RuntimeMessage::MockStarted(run_id, result));
            context.request_repaint();
        });
    }

    fn stop_current_mock(&mut self) {
        self.mock_run_id = self.mock_run_id.wrapping_add(1);
        if let Some(server) = self.mock_server.take() {
            self.runtime.spawn(server.shutdown());
            self.toast(
                ToastKind::Info,
                self.tr("Mock 服务已停止", "Mock server stopped"),
            );
        }
    }

    fn queue_environment_save(&mut self, index: usize, validate: bool, notify: bool) -> bool {
        if self.storage_worker.is_none() {
            self.toast(
                ToastKind::Error,
                self.tr("本地数据库不可用", "Local database unavailable"),
            );
            return false;
        }
        if self.environments.get(index).is_none() {
            return false;
        }
        if let Err(error) = self.validate_environment(index) {
            if validate {
                self.toast(ToastKind::Error, error);
            }
            return false;
        }
        if let Err(error) = self.persist_environment_secrets(index) {
            self.toast(ToastKind::Error, error);
            return false;
        }
        self.environments[index].sync_edit_revision(Instant::now());
        let revision = self.environments[index].autosave.current_revision();
        if revision == 0 && self.environments[index].persisted {
            if notify {
                self.toast(
                    ToastKind::Info,
                    self.tr("没有需要保存的更改", "No changes to save"),
                );
            }
            return true;
        }
        let environment = self.environments[index].to_environment();
        let result = self
            .storage_worker
            .as_ref()
            .expect("storage worker was checked above")
            .queue_environment(self.project.id, environment, revision, notify);
        match result {
            Ok(()) => {
                self.environments[index].autosave.mark_queued(revision);
                true
            }
            Err(error) => {
                self.environments[index]
                    .autosave
                    .mark_failed(revision, Instant::now());
                self.toast(ToastKind::Error, error.to_string());
                false
            }
        }
    }

    fn schedule_environment_autosaves(&mut self, context: &egui::Context) {
        let now = Instant::now();
        for environment in &mut self.environments {
            environment.sync_edit_revision(now);
        }
        let due = self
            .environments
            .iter()
            .enumerate()
            .filter_map(|(index, environment)| environment.autosave.is_due(now).then_some(index))
            .collect::<Vec<_>>();
        for index in due {
            self.queue_environment_save(index, false, false);
        }
        if self.environments.iter().any(EnvironmentDraft::is_dirty) {
            context.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn validate_send(&self) -> Result<(), String> {
        let Some(request) = self.requests.get(self.selected) else {
            return Err(self.tr("没有可发送的请求", "No request to send").into());
        };
        match request.alternate_protocol.as_ref() {
            None | Some(ProtocolSpec::Http(_)) => {
                if request.draft.url.trim().is_empty() {
                    return Err(self.tr("请输入请求地址", "Enter a request URL").into());
                }
                self.validate_auth(&request.draft.auth)?;
                self.validate_proxy(request.draft.proxy.as_ref())?;
                match request.draft.body_mode {
                    BodyMode::Binary if request.draft.binary_file.is_none() => {
                        Err(self.tr("请选择二进制文件", "Select a binary file").into())
                    }
                    BodyMode::Multipart
                        if request.draft.multipart_parts.iter().any(|part| {
                            part.enabled
                                && part.mode == MultipartValueMode::File
                                && !part.is_empty()
                                && part.file.is_none()
                        }) =>
                    {
                        Err(self
                            .tr("请选择 Multipart 文件", "Select the multipart file")
                            .into())
                    }
                    _ => Ok(()),
                }
            }
            Some(ProtocolSpec::GraphQl(spec)) => {
                if spec.endpoint.trim().is_empty() || spec.query.trim().is_empty() {
                    return Err(self
                        .tr(
                            "请输入 GraphQL 地址和查询",
                            "Enter the GraphQL endpoint and query",
                        )
                        .into());
                }
                if !spec.variables.trim().is_empty() {
                    serde_json::from_str::<serde_json::Value>(&spec.variables)
                        .map_err(|error| format!("GraphQL variables: {error}"))?;
                }
                Ok(())
            }
            Some(ProtocolSpec::Sse(spec)) => {
                if spec.request.url.trim().is_empty() {
                    Err(self.tr("请输入 SSE 地址", "Enter the SSE URL").into())
                } else {
                    Ok(())
                }
            }
            Some(ProtocolSpec::WebSocket(spec)) => {
                if spec.url.trim().is_empty() {
                    Err(self
                        .tr("请输入 WebSocket 地址", "Enter the WebSocket URL")
                        .into())
                } else {
                    Ok(())
                }
            }
            Some(ProtocolSpec::Grpc(spec)) => {
                if spec.endpoint.trim().is_empty()
                    || spec.service.trim().is_empty()
                    || spec.method.trim().is_empty()
                {
                    Err(self
                        .tr(
                            "请输入 gRPC 地址、服务和方法",
                            "Enter the gRPC endpoint, service and method",
                        )
                        .into())
                } else if spec.descriptor_set.is_none()
                    && spec.proto_files.is_empty()
                    && !spec.use_reflection
                {
                    Err(self
                        .tr(
                            "请选择 proto/descriptor 或启用反射",
                            "Choose proto/descriptor files or enable reflection",
                        )
                        .into())
                } else {
                    serde_json::from_str::<serde_json::Value>(&spec.message_json)
                        .map(|_| ())
                        .map_err(|error| format!("gRPC JSON: {error}"))
                }
            }
        }
    }

    fn history_redaction_values(
        &self,
        request_index: usize,
        environment_index: usize,
    ) -> Vec<String> {
        let mut values = Vec::new();
        let request = &self.requests[request_index];
        for secret in request.draft.auth.secrets() {
            if let Ok(Some(value)) = self.secrets.get(&secret.reference) {
                values.push(value);
            }
        }
        for variable in &request.request_case.local_variables {
            if let Some(reference) = &variable.secret_ref
                && let Ok(Some(value)) = self.secrets.get(reference)
            {
                values.push(value);
            }
        }
        if let Some(proxy) = &request.draft.proxy
            && let Ok(Some(value)) = self.secrets.get(&proxy.password.reference)
        {
            values.push(value);
        }
        for variable in &self.environments[environment_index].variables {
            if variable.secret {
                if let Some(reference) = &variable.reference
                    && let Ok(Some(value)) = self.secrets.get(reference)
                {
                    values.push(value);
                }
            } else if sensitive_name(&variable.name) && !variable.value.is_empty() {
                values.push(variable.value.clone());
            }
        }
        for pair in request
            .draft
            .query
            .iter()
            .chain(&request.draft.headers)
            .chain(&request.draft.cookies)
            .chain(&request.draft.form_fields)
        {
            if pair.enabled && sensitive_name(&pair.name) && !pair.value.is_empty() {
                values.push(pair.value.clone());
                if let Some((_, credential)) = pair.value.split_once(' ')
                    && !credential.is_empty()
                {
                    values.push(credential.to_owned());
                }
            }
        }
        for part in &request.draft.multipart_parts {
            if part.enabled && sensitive_name(&part.name) && !part.value.is_empty() {
                values.push(part.value.clone());
            }
        }
        if request.draft.body_mode == BodyMode::Json
            && let Ok(body) = serde_json::from_str::<serde_json::Value>(&request.draft.body)
        {
            collect_sensitive_json_values(&body, &mut values);
        }
        values.retain(|value| !value.is_empty());
        values.sort_by_key(|value| std::cmp::Reverse(value.len()));
        values.dedup();
        values
    }

    fn begin_run_history(&mut self, request_case_id: EntityId, redactions: Vec<String>) {
        self.history_record = Some(RunRecord {
            id: EntityId::new(),
            request_case_id: Some(request_case_id),
            state: HistoryRunState::Running,
            status_code: None,
            elapsed_ms: 0,
            response_bytes: 0,
            body_path: None,
            error: None,
            started_at: Utc::now(),
            finished_at: None,
        });
        self.history_redactions = redactions;
        self.history_body = match self.body_store.as_ref() {
            Some(store) => match store.begin_redacted(&self.history_redactions) {
                Ok(sink) => Some(sink),
                Err(error) => {
                    self.toast(ToastKind::Error, error.to_string());
                    None
                }
            },
            None => None,
        };
    }

    fn write_run_history_body(&mut self, bytes: &[u8]) {
        let error = self
            .history_body
            .as_mut()
            .and_then(|sink| sink.write_all(bytes).err());
        if let Some(error) = error {
            self.history_body = None;
            self.toast(
                ToastKind::Error,
                format!("failed to store response body: {error}"),
            );
        }
    }

    fn finish_run_history(
        &mut self,
        state: HistoryRunState,
        metrics: Option<ExecutionMetrics>,
        error: Option<String>,
    ) {
        let Some(mut record) = self.history_record.take() else {
            self.history_body = None;
            self.history_redactions.clear();
            return;
        };
        let body = match self.history_body.take() {
            Some(sink) => match sink.commit() {
                Ok(body) => Some(body),
                Err(storage_error) => {
                    self.toast(ToastKind::Error, storage_error.to_string());
                    None
                }
            },
            None => None,
        };
        let finished_at = Utc::now();
        record.state = state;
        record.status_code = self.response.status;
        record.elapsed_ms = metrics
            .map(|metrics| metrics.elapsed_ms)
            .unwrap_or_else(|| (finished_at - record.started_at).num_milliseconds().max(0) as u64);
        record.response_bytes = metrics
            .map(|metrics| metrics.received_bytes)
            .or_else(|| body.as_ref().map(|body| body.size))
            .unwrap_or_default();
        record.body_path = body.as_ref().map(|body| body.path.display().to_string());
        record.error = error.map(|error| redact_text(&error, &self.history_redactions));
        record.finished_at = Some(finished_at);
        self.history_redactions.clear();

        let Some(database) = self.database.clone() else {
            if let (Some(store), Some(body)) = (&self.body_store, body.as_ref()) {
                let _ = store.delete(body);
            }
            return;
        };
        if let Err(storage_error) = database.save_run_record(self.project.id, &record) {
            if let (Some(store), Some(body)) = (&self.body_store, body.as_ref()) {
                let _ = store.delete(body);
            }
            self.toast(ToastKind::Error, storage_error.to_string());
            return;
        }
        match database.prune_run_records_with_body_paths(
            self.project.id,
            HISTORY_MAX_RECORDS,
            HISTORY_MAX_AGE_DAYS,
            finished_at,
        ) {
            Ok((_, paths)) => {
                if let Some(store) = &self.body_store {
                    for path in paths {
                        if let Err(storage_error) = store.delete(&BodyRef { path, size: 0 }) {
                            tracing::warn!(%storage_error, "failed to delete expired response body");
                        }
                    }
                }
            }
            Err(storage_error) => self.toast(ToastKind::Error, storage_error.to_string()),
        }
        self.reload_run_history();
    }

    fn reload_run_history(&mut self) {
        let Some(database) = self.database.clone() else {
            self.run_records.clear();
            self.history_body_preview.clear();
            self.history_body_truncated = false;
            return;
        };
        let selected_id = self
            .run_records
            .get(self.selected_history)
            .map(|record| record.id);
        match database.list_run_records(self.project.id, PageRequest::new(0, HISTORY_MAX_RECORDS)) {
            Ok(page) => {
                self.run_records = page.items;
                self.selected_history = selected_id
                    .and_then(|id| self.run_records.iter().position(|record| record.id == id))
                    .unwrap_or_default();
                self.load_selected_history_body();
            }
            Err(error) => self.toast(ToastKind::Error, error.to_string()),
        }
    }

    fn select_run_history(&mut self, index: usize) {
        if self.run_records.get(index).is_none() {
            return;
        }
        self.selected_history = index;
        self.load_selected_history_body();
    }

    fn load_selected_history_body(&mut self) {
        self.history_body_preview.clear();
        self.history_body_truncated = false;
        let Some(path) = self
            .run_records
            .get(self.selected_history)
            .and_then(|record| record.body_path.as_deref())
        else {
            return;
        };
        let Some(store) = &self.body_store else {
            self.history_body_preview = "response body store is unavailable".into();
            return;
        };
        let path = std::path::PathBuf::from(path);
        let size = match std::fs::metadata(&path) {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                self.history_body_preview = format!("failed to inspect response body: {error}");
                return;
            }
        };
        match store.read_range(&BodyRef { path, size }, 0, MAX_RESPONSE_BYTES) {
            Ok(bytes) => {
                self.history_body_preview = String::from_utf8_lossy(&bytes).into_owned();
                self.history_body_truncated = size > bytes.len() as u64;
            }
            Err(error) => {
                self.history_body_preview = format!("failed to read response body: {error}");
            }
        }
    }

    fn send_current(&mut self, context: &egui::Context) {
        if let Err(error) = self.validate_send() {
            self.toast(ToastKind::Error, error);
            return;
        }
        if let Err(error) = self.persist_request_secret(self.selected) {
            self.toast(ToastKind::Error, error);
            return;
        }
        let active_index = self
            .environments
            .iter()
            .position(|environment| environment.id() == self.active_environment)
            .unwrap_or_default();
        if let Err(error) = self.persist_environment_secrets(active_index) {
            self.toast(ToastKind::Error, error);
            return;
        }

        if self.history_record.is_some() {
            self.finish_run_history(
                HistoryRunState::Cancelled,
                self.response.metrics,
                Some("superseded by a new execution".into()),
            );
        }
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        self.execution_commands = None;
        self.run_id = self.run_id.wrapping_add(1);
        let run_id = self.run_id;
        self.response = ResponseView::running();
        self.response_tab = ResponseTab::Body;
        self.response_body_mode = ResponseBodyMode::Pretty;

        let redactions = self.history_redaction_values(self.selected, active_index);
        let request_case_id = self.requests[self.selected].request_case.id;
        let mut request = ExecutionRequest::new(
            self.requests[self.selected].edited_protocol(),
            self.environments[active_index].to_environment(),
        );
        request.case_id = Some(request_case_id);
        request.local_variables = self.requests[self.selected]
            .request_case
            .local_variables
            .clone();
        let handle = match self.executors.start(request) {
            Ok(handle) => handle,
            Err(error) => {
                self.response = ResponseView::default();
                self.toast(ToastKind::Error, error.to_string());
                return;
            }
        };
        self.begin_run_history(request_case_id, redactions);
        self.execution_commands = handle.command_sender();
        let cancellation = handle.cancellation_token();
        self.cancellation = Some(cancellation);
        let mut stream = handle.events;
        let sender = self.sender.clone();
        let context = context.clone();
        self.runtime.spawn(async move {
            while let Some(event) = stream.next().await {
                if sender.send(RuntimeMessage::Event(run_id, event)).is_err() {
                    return;
                }
                context.request_repaint();
            }
            let _ = sender.send(RuntimeMessage::Closed(run_id));
            context.request_repaint();
        });
    }

    fn stop(&mut self) {
        if let Some(commands) = &self.execution_commands {
            let _ = commands.try_send(ExecutionCommand::Cancel);
        }
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
            self.response.state = RunState::Cancelling;
        }
        self.execution_commands = None;
    }

    fn invalidate_run(&mut self) {
        if self.history_record.is_some() {
            self.finish_run_history(
                HistoryRunState::Cancelled,
                self.response.metrics,
                Some("execution was interrupted".into()),
            );
        }
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        if let Some(cancellation) = self.scenario_cancellation.take() {
            cancellation.cancel();
        }
        self.execution_commands = None;
        self.run_id = self.run_id.wrapping_add(1);
        self.scenario_run_id = self.scenario_run_id.wrapping_add(1);
        self.response = ResponseView::default();
    }

    fn send_websocket_message(&mut self) {
        if self.websocket_message.trim().is_empty() {
            return;
        }
        let message = self.websocket_message.clone();
        match self.try_execution_command(ExecutionCommand::SendMessage {
            media_type: Some("text/plain; charset=utf-8".to_owned()),
            data: message.into(),
        }) {
            Ok(()) => self.websocket_message.clear(),
            Err(error) => self.toast(ToastKind::Error, error),
        }
    }

    fn close_websocket_input(&mut self) {
        match self.try_execution_command(ExecutionCommand::CompleteInput) {
            Ok(()) => self.execution_commands = None,
            Err(error) => self.toast(ToastKind::Error, error),
        }
    }

    fn try_execution_command(&mut self, command: ExecutionCommand) -> Result<(), String> {
        let Some(commands) = self.execution_commands.clone() else {
            return Err(self
                .tr("连接尚未建立或已经关闭", "The connection is not open")
                .into());
        };
        match commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => Err(self
                .tr(
                    "消息队列已满，请稍后重试",
                    "The message queue is full; try again",
                )
                .into()),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                self.execution_commands = None;
                Err(self
                    .tr("连接已经关闭", "The connection is already closed")
                    .into())
            }
        }
    }

    fn drain_runtime(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                RuntimeMessage::Event(run_id, event) if run_id == self.run_id => {
                    self.apply_runtime_event(event)
                }
                RuntimeMessage::Closed(run_id) if run_id == self.run_id => {
                    if self.response.state == RunState::Cancelling {
                        self.response.state = RunState::Cancelled;
                        self.response.push_timeline(TimelinePhase::Cancelled);
                        self.response.finish_body();
                    } else if self.response.state == RunState::Running {
                        self.response.state = RunState::Failed;
                        self.response.error = Some(
                            self.tr(
                                "请求流意外结束，未收到完成事件",
                                "The request stream ended without a completion event",
                            )
                            .into(),
                        );
                        self.response.push_timeline(TimelinePhase::Failed);
                        self.response.finish_body();
                    }
                    if self.history_record.is_some() {
                        let state = if self.response.state == RunState::Cancelled {
                            HistoryRunState::Cancelled
                        } else {
                            HistoryRunState::Failed
                        };
                        self.finish_run_history(
                            state,
                            self.response.metrics,
                            self.response.error.clone(),
                        );
                    }
                    self.cancellation = None;
                    self.execution_commands = None;
                }
                RuntimeMessage::ScenarioFinished(run_id, result)
                    if run_id == self.scenario_run_id =>
                {
                    self.scenario_cancellation = None;
                    match result {
                        Ok(report) => {
                            let passed = report.passed;
                            self.scenario_report = Some(report);
                            self.toast(
                                if passed {
                                    ToastKind::Success
                                } else {
                                    ToastKind::Error
                                },
                                match (self.language, passed) {
                                    (Language::Chinese, true) => "场景执行通过",
                                    (Language::Chinese, false) => "场景执行失败",
                                    (Language::English, true) => "Scenario passed",
                                    (Language::English, false) => "Scenario failed",
                                },
                            );
                        }
                        Err(error) => self.toast(ToastKind::Error, error),
                    }
                }
                RuntimeMessage::MockStarted(run_id, result) if run_id == self.mock_run_id => {
                    match result {
                        Ok(server) => {
                            let address = server.base_url();
                            self.mock_server = Some(server);
                            self.toast(
                                ToastKind::Success,
                                match self.language {
                                    Language::Chinese => format!("Mock 服务已启动：{address}"),
                                    Language::English => format!("Mock server started: {address}"),
                                },
                            );
                        }
                        Err(error) => self.toast(ToastKind::Error, error),
                    }
                }
                RuntimeMessage::Event(_, _)
                | RuntimeMessage::Closed(_)
                | RuntimeMessage::ScenarioFinished(_, _)
                | RuntimeMessage::MockStarted(_, _) => {}
            }
        }
    }

    fn apply_runtime_event(&mut self, event: Result<ExecutionEvent, ExecutionError>) {
        match event {
            Ok(ExecutionEvent::Started { .. }) => {}
            Ok(ExecutionEvent::ResponseHead(head)) => {
                self.apply_response_head(head);
                self.response.push_timeline(TimelinePhase::ResponseHead);
            }
            Ok(ExecutionEvent::Data(data)) => {
                self.write_run_history_body(&data);
                self.response.append_body(&data);
            }
            Ok(ExecutionEvent::Message { data, .. }) => {
                self.write_run_history_body(&data);
                self.write_run_history_body(b"\n");
                self.response.append_body(&data);
                self.response.append_body(b"\n");
            }
            Ok(ExecutionEvent::Metrics(metrics)) => self.response.metrics = Some(metrics),
            Ok(ExecutionEvent::Completed(metrics)) => {
                self.response.state = RunState::Completed;
                self.response.metrics = Some(metrics);
                self.response.finish_body();
                self.response.push_timeline(TimelinePhase::Completed);
                self.cancellation = None;
                self.execution_commands = None;
                self.finish_run_history(HistoryRunState::Passed, Some(metrics), None);
            }
            Err(ExecutionError::Cancelled) => {
                self.response.state = RunState::Cancelled;
                self.response.finish_body();
                self.response.push_timeline(TimelinePhase::Cancelled);
                self.cancellation = None;
                self.execution_commands = None;
                self.finish_run_history(HistoryRunState::Cancelled, self.response.metrics, None);
            }
            Err(error) => {
                let error = error.to_string();
                self.response.state = RunState::Failed;
                self.response.error = Some(error.clone());
                self.response.finish_body();
                self.response.push_timeline(TimelinePhase::Failed);
                self.cancellation = None;
                self.execution_commands = None;
                self.finish_run_history(
                    HistoryRunState::Failed,
                    self.response.metrics,
                    Some(error),
                );
            }
        }
    }

    fn apply_response_head(&mut self, head: ResponseHead) {
        self.response.status = head.status;
        self.response.version = head.version;
        self.response.headers = head
            .headers
            .into_iter()
            .flat_map(|(name, values)| values.into_iter().map(move |value| (name.clone(), value)))
            .collect();
    }

    fn delete_request(&mut self, id: EntityId) {
        if !self.requests.iter().any(|request| request.id() == id) {
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
        if let Err(error) = database.delete_definition(self.project.id, id) {
            self.toast(ToastKind::Error, error.to_string());
            return;
        }
        let _ = self.drain_storage();
        let Some(index) = self.requests.iter().position(|request| request.id() == id) else {
            return;
        };
        let mut references = AuthDraft::references_for_request(id);
        references.extend(
            self.requests[index]
                .request_case
                .local_variables
                .iter()
                .filter_map(|variable| variable.secret_ref.clone()),
        );
        self.cleanup_secret_references(references);
        self.invalidate_run();
        self.requests.remove(index);
        self.selected = self.selected.min(self.requests.len().saturating_sub(1));
        self.close_document(DocumentId {
            kind: DocumentKind::Api,
            entity_id: id,
        });
        self.reload_resource_page(None);
        self.toast(ToastKind::Success, self.tr("请求已删除", "Request deleted"));
    }

    fn delete_environment(&mut self, id: EntityId) {
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

    fn keyboard_shortcuts(&mut self, context: &egui::Context) {
        if self.confirmation.is_some() {
            return;
        }
        let (send, save, new_request, search) = context.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::Enter),
                input.modifiers.command && input.key_pressed(egui::Key::S),
                input.modifiers.command && input.key_pressed(egui::Key::N),
                input.modifiers.command && input.key_pressed(egui::Key::K),
            )
        });
        if search {
            self.navigation = Navigation::Api;
            context.memory_mut(|memory| memory.request_focus(egui::Id::new(SEARCH_FIELD_ID)));
        }
        if send && self.navigation == Navigation::Api {
            self.send_current(context);
        }
        if save {
            self.save_current();
        }
        if new_request {
            self.queue_action(PendingAction::NewRequest(ProtocolKind::Http));
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let search_hint = self.tr("搜索接口  Ctrl K", "Search APIs  Ctrl K");
        let new_label = self.tr("新建", "New");
        let settings_tip = self.tr("设置", "Settings");
        let theme_tip = self.tr("切换主题", "Toggle theme");
        let mut new_protocol = None;
        let mut toggle_theme = false;
        let mut show_settings = false;
        let mut selected_environment = None;
        let mut selected_project = None;
        let mut new_project = false;
        let mut interop_action = None;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let logo = egui::Button::new(RichText::new("A").strong().color(Color32::WHITE))
                        .fill(palette.primary)
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(4));
                    ui.add_sized([28.0, 28.0], logo);
                    ui.label(RichText::new("ApiTest").size(16.0).strong());
                    ui.separator();
                    egui::ComboBox::from_id_salt("active_project")
                        .selected_text(&self.project.name)
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            for project in &self.projects {
                                if ui
                                    .selectable_label(project.id == self.project.id, &project.name)
                                    .clicked()
                                {
                                    selected_project = Some(project.id);
                                }
                            }
                            ui.separator();
                            if ui
                                .button(theme::icon_label(
                                    "plus",
                                    self.tr("新建项目", "New project"),
                                    12.0,
                                    palette.text,
                                ))
                                .clicked()
                            {
                                new_project = true;
                            }
                        });
                    ui.add_space(8.0);
                    ui.add_sized(
                        [220.0, 32.0],
                        egui::TextEdit::singleline(&mut self.search)
                            .id_source(SEARCH_FIELD_ID)
                            .hint_text(search_hint),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [32.0, 32.0],
                                egui::Button::new(theme::icon("settings", 15.0)),
                            )
                            .on_hover_text(settings_tip)
                            .clicked()
                        {
                            show_settings = true;
                        }
                        let theme_icon = match self.theme {
                            ThemeMode::Dark => "sun",
                            ThemeMode::Light => "moon",
                        };
                        if ui
                            .add_sized(
                                [32.0, 32.0],
                                egui::Button::new(theme::icon(theme_icon, 15.0)),
                            )
                            .on_hover_text(theme_tip)
                            .clicked()
                        {
                            toggle_theme = true;
                        }
                        ui.menu_button(
                            theme::icon_label("plus", new_label, 13.0, Color32::WHITE),
                            |ui| protocol_creation_menu(ui, self.language, &mut new_protocol),
                        );
                        ui.menu_button(self.tr("导入 / 导出", "Import / Export"), |ui| {
                            ui.label(RichText::new(self.tr("导入", "Import")).strong());
                            for (action, chinese, english) in [
                                (InteropAction::ImportOpenApi, "OpenAPI 文件", "OpenAPI file"),
                                (
                                    InteropAction::ImportPostman,
                                    "Postman 集合",
                                    "Postman collection",
                                ),
                                (InteropAction::ImportHar, "HAR 文件", "HAR file"),
                                (InteropAction::ImportCurl, "粘贴 cURL", "Paste cURL"),
                            ] {
                                if ui
                                    .button(match self.language {
                                        Language::Chinese => chinese,
                                        Language::English => english,
                                    })
                                    .clicked()
                                {
                                    interop_action = Some(action);
                                    ui.close();
                                }
                            }
                            ui.separator();
                            ui.label(RichText::new(self.tr("导出", "Export")).strong());
                            for (action, chinese, english) in [
                                (
                                    InteropAction::PreviewOpenApi,
                                    "OpenAPI 设计与预览",
                                    "OpenAPI design & preview",
                                ),
                                (InteropAction::ExportOpenApi, "OpenAPI YAML", "OpenAPI YAML"),
                                (
                                    InteropAction::ExportPostman,
                                    "Postman 集合",
                                    "Postman collection",
                                ),
                                (InteropAction::ExportHar, "HAR 文件", "HAR file"),
                                (
                                    InteropAction::ExportCurl,
                                    "当前请求 cURL",
                                    "Current request cURL",
                                ),
                                (
                                    InteropAction::ExportDocumentation,
                                    "静态 HTML 文档",
                                    "Static HTML documentation",
                                ),
                            ] {
                                if ui
                                    .button(match self.language {
                                        Language::Chinese => chinese,
                                        Language::English => english,
                                    })
                                    .clicked()
                                {
                                    interop_action = Some(action);
                                    ui.close();
                                }
                            }
                        });
                        let active_name = self
                            .environments
                            .iter()
                            .find(|environment| environment.id() == self.active_environment)
                            .map(|environment| environment.name.as_str())
                            .unwrap_or("-");
                        egui::ComboBox::from_id_salt("active_environment")
                            .selected_text(theme::icon_label(
                                "globe",
                                active_name,
                                12.0,
                                palette.text,
                            ))
                            .width(126.0)
                            .show_ui(ui, |ui| {
                                for environment in &self.environments {
                                    if ui
                                        .selectable_label(
                                            environment.id() == self.active_environment,
                                            &environment.name,
                                        )
                                        .clicked()
                                    {
                                        selected_environment = Some(environment.id());
                                    }
                                }
                            });
                    });
                });
            });
        if let Some(kind) = new_protocol {
            self.queue_action(PendingAction::NewRequest(kind));
        }
        if let Some(id) = selected_project
            && id != self.project.id
        {
            self.queue_action(PendingAction::SelectProject(id));
        }
        if new_project {
            self.queue_action(PendingAction::NewProject);
        }
        if toggle_theme {
            self.theme = match self.theme {
                ThemeMode::Dark => ThemeMode::Light,
                ThemeMode::Light => ThemeMode::Dark,
            };
            theme::apply(ui.ctx(), self.theme);
            let theme = self.theme;
            self.persist_setting(THEME_SETTING, &theme);
        }
        if show_settings {
            self.show_settings = true;
        }
        if let Some(id) = selected_environment {
            self.active_environment = id;
            let key = active_environment_setting(self.project.id);
            self.persist_setting(&key, &id);
        }
        if let Some(action) = interop_action {
            self.handle_interop_action(action);
        }
    }

    fn handle_interop_action(&mut self, action: InteropAction) {
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

    fn import_file(&mut self, action: InteropAction) {
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

    fn import_source(&mut self, action: InteropAction, source: &str) -> Result<usize, String> {
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

    fn queue_imported_definitions(
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

    fn contract_definitions(&self) -> Vec<ApiDefinition> {
        self.requests
            .iter()
            .map(|request| request.save_snapshot().0)
            .collect()
    }

    fn example_definitions(&self) -> Vec<ApiDefinition> {
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

    fn refresh_openapi_preview(&mut self) -> Result<(), String> {
        let definitions = self.contract_definitions();
        let source =
            export_openapi(&self.project, &definitions).map_err(|error| error.to_string())?;
        let issues = validate_openapi(&source).map_err(|error| error.to_string())?;
        self.openapi_html = export_openapi_documentation(&self.project, &definitions);
        self.openapi_source = source;
        self.openapi_issues = issues;
        Ok(())
    }

    fn export_text(&mut self, file_name: &str, label: &str, source: &str) {
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

    fn activity_bar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut navigate = None;
        egui::Frame::new()
            .fill(palette.rail_bg)
            .inner_margin(egui::Margin::symmetric(6, 10))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    for (target, icon, label, tooltip) in [
                        (Navigation::Api, "braces", "API", self.tr("接口", "APIs")),
                        (
                            Navigation::Scenario,
                            "workflow",
                            "RUN",
                            self.tr("测试场景", "Scenarios"),
                        ),
                        (
                            Navigation::Mock,
                            "server",
                            "MOCK",
                            self.tr("Mock 服务", "Mock servers"),
                        ),
                        (
                            Navigation::History,
                            "history",
                            "HIST",
                            self.tr("运行历史", "Run history"),
                        ),
                        (
                            Navigation::Environment,
                            "database",
                            "ENV",
                            self.tr("环境", "Environments"),
                        ),
                    ] {
                        let selected = self.navigation == target;
                        let color = if selected {
                            palette.accent_text
                        } else {
                            palette.muted
                        };
                        let button = egui::Button::new(theme::icon_label(icon, label, 11.0, color))
                            .fill(if selected {
                                palette.primary_soft
                            } else {
                                Color32::TRANSPARENT
                            })
                            .stroke(Stroke::NONE);
                        if ui
                            .add_sized([48.0, 42.0], button)
                            .on_hover_text(tooltip)
                            .clicked()
                        {
                            navigate = Some(target);
                        }
                    }
                });
            });
        if let Some(navigation) = navigate
            && navigation != self.navigation
        {
            self.queue_action(PendingAction::Navigate(navigation));
        }
    }

    fn api_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut new_protocol = None;
        let mut selection = None;
        let mut toggle_resource = None;
        let mut load_more = None;
        let resource_rows = self.visible_resource_rows();
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(10, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.tr("接口管理", "APIs"))
                            .strong()
                            .size(14.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button(theme::icon("plus", 14.0), |ui| {
                            protocol_creation_menu(ui, self.language, &mut new_protocol)
                        })
                        .response
                        .on_hover_text(self.tr("新建请求", "New request"));
                    });
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(theme::icon("folder-open", 14.0).color(palette.muted));
                    ui.label(RichText::new(&self.project.name).strong());
                });
                ui.add_space(6.0);
                let search = self.search.to_lowercase();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if search.is_empty() {
                        for row in &resource_rows {
                            match row {
                                ResourceRow::Node { node, depth } => {
                                    ui.horizontal(|ui| {
                                        ui.add_space(*depth as f32 * 14.0);
                                        if node.kind == ProjectNodeKind::Folder {
                                            let marker =
                                                if self.expanded_resources.contains(&node.id) {
                                                    "▾"
                                                } else {
                                                    "▸"
                                                };
                                            if ui
                                                .add_sized(
                                                    [ui.available_width(), 32.0],
                                                    egui::Button::new(format!(
                                                        "{marker}  {}",
                                                        node.name
                                                    ))
                                                    .fill(Color32::TRANSPARENT)
                                                    .stroke(Stroke::NONE),
                                                )
                                                .clicked()
                                            {
                                                toggle_resource = Some(node.id);
                                            }
                                            return;
                                        }
                                        let request = node.entity_id.and_then(|id| {
                                            self.requests.iter().find(|request| request.id() == id)
                                        });
                                        let selected = request.is_some_and(|request| {
                                            self.requests
                                                .get(self.selected)
                                                .is_some_and(|current| current.id() == request.id())
                                        });
                                        let response = if let Some(request) = request {
                                            ui.add_sized(
                                                [ui.available_width(), 34.0],
                                                egui::Button::new(request_row_text(
                                                    request, palette,
                                                ))
                                                .selected(selected)
                                                .fill(if selected {
                                                    palette.primary_soft
                                                } else {
                                                    Color32::TRANSPARENT
                                                })
                                                .stroke(Stroke::NONE),
                                            )
                                        } else {
                                            ui.add_sized(
                                                [ui.available_width(), 34.0],
                                                egui::Button::new(&node.name)
                                                    .fill(Color32::TRANSPARENT)
                                                    .stroke(Stroke::NONE),
                                            )
                                        };
                                        if response.clicked()
                                            && !selected
                                            && let Some(id) = node.entity_id
                                            && node.kind == ProjectNodeKind::ApiDefinition
                                        {
                                            selection = Some(id);
                                        }
                                    });
                                }
                                ResourceRow::More { parent_id, depth } => {
                                    ui.horizontal(|ui| {
                                        ui.add_space(*depth as f32 * 14.0);
                                        if ui.button(self.tr("加载更多…", "Load more…")).clicked()
                                        {
                                            load_more = Some(*parent_id);
                                        }
                                    });
                                }
                            }
                        }
                    } else {
                        for request in &self.requests {
                            if !request.name.to_lowercase().contains(&search)
                                && !request.endpoint().to_lowercase().contains(&search)
                                && !protocol_label(request.protocol_kind())
                                    .to_lowercase()
                                    .contains(&search)
                            {
                                continue;
                            }
                            let selected = self
                                .requests
                                .get(self.selected)
                                .is_some_and(|current| current.id() == request.id());
                            let response = ui.add_sized(
                                [ui.available_width(), 34.0],
                                egui::Button::new(request_row_text(request, palette))
                                    .selected(selected)
                                    .fill(if selected {
                                        palette.primary_soft
                                    } else {
                                        Color32::TRANSPARENT
                                    })
                                    .stroke(Stroke::NONE),
                            );
                            if response.clicked() && !selected {
                                selection = Some(request.id());
                            }
                        }
                    }
                });
            });
        if let Some(kind) = new_protocol {
            self.queue_action(PendingAction::NewRequest(kind));
        }
        if let Some(id) = selection {
            self.queue_action(PendingAction::SelectRequest(id));
        }
        if let Some(id) = toggle_resource {
            self.toggle_resource(id);
        }
        if let Some(parent_id) = load_more {
            self.load_more_resources(parent_id);
        }
    }

    fn scenario_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut select = None;
        let mut create = false;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.tr("测试场景", "Scenarios")).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([28.0, 28.0], egui::Button::new(theme::icon("plus", 14.0)))
                            .on_hover_text(self.tr("新建场景", "New scenario"))
                            .clicked()
                        {
                            create = true;
                        }
                    });
                });
            });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, scenario) in self.scenarios.iter().enumerate() {
                let dirty = self
                    .scenario_snapshots
                    .get(&scenario.id)
                    .is_none_or(|saved| saved != &document_snapshot(scenario));
                let label = if dirty {
                    format!("{}  •", scenario.name)
                } else {
                    scenario.name.clone()
                };
                if ui
                    .selectable_label(index == self.selected_scenario, label)
                    .clicked()
                {
                    select = Some(scenario.id);
                }
            }
        });
        if create {
            self.queue_action(PendingAction::NewScenario);
        }
        if let Some(id) = select {
            self.queue_action(PendingAction::SelectScenario(id));
        }
    }

    fn mock_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut select = None;
        let mut create = false;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.tr("Mock 服务", "Mock servers")).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([28.0, 28.0], egui::Button::new(theme::icon("plus", 14.0)))
                            .on_hover_text(self.tr("新建 Mock", "New mock"))
                            .clicked()
                        {
                            create = true;
                        }
                    });
                });
            });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, profile) in self.mock_profiles.iter().enumerate() {
                let dirty = self
                    .mock_snapshots
                    .get(&profile.id)
                    .is_none_or(|saved| saved != &document_snapshot(profile));
                let label = if dirty {
                    format!("{}  •", profile.name)
                } else {
                    profile.name.clone()
                };
                if ui
                    .selectable_label(index == self.selected_mock, label)
                    .clicked()
                {
                    select = Some(profile.id);
                }
            }
        });
        if create {
            self.queue_action(PendingAction::NewMock);
        }
        if let Some(id) = select {
            self.queue_action(PendingAction::SelectMock(id));
        }
    }

    fn history_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut selection = None;
        let mut refresh = false;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.tr("运行历史", "Run history")).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [28.0, 28.0],
                                egui::Button::new(theme::icon("refresh-cw", 13.0)),
                            )
                            .on_hover_text(self.tr("刷新历史", "Refresh history"))
                            .clicked()
                        {
                            refresh = true;
                        }
                    });
                });
                ui.label(
                    RichText::new(match self.language {
                        Language::Chinese => {
                            format!("保留最近 {HISTORY_MAX_RECORDS} 条 / {HISTORY_MAX_AGE_DAYS} 天")
                        }
                        Language::English => format!(
                            "Keeps {HISTORY_MAX_RECORDS} records / {HISTORY_MAX_AGE_DAYS} days"
                        ),
                    })
                    .small()
                    .color(palette.muted),
                );
            });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, record) in self.run_records.iter().enumerate() {
                let request_name = record
                    .request_case_id
                    .and_then(|case_id| {
                        self.requests
                            .iter()
                            .find(|request| request.request_case.id == case_id)
                    })
                    .map(|request| request.name.as_str())
                    .unwrap_or(match self.language {
                        Language::Chinese => "已删除的请求",
                        Language::English => "Deleted request",
                    });
                let state = history_state_label(record.state, self.language);
                let label = format!(
                    "{}  {}\n{}  ·  {} ms",
                    state,
                    request_name,
                    record.started_at.format("%m-%d %H:%M:%S"),
                    record.elapsed_ms
                );
                if ui
                    .selectable_label(index == self.selected_history, label)
                    .clicked()
                {
                    selection = Some(index);
                }
            }
        });
        if refresh {
            self.reload_run_history();
        }
        if let Some(index) = selection {
            self.select_run_history(index);
        }
    }

    fn environment_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let mut new_environment = false;
        let mut selection = None;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(10, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(self.tr("环境管理", "Environments"))
                            .strong()
                            .size(14.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized([28.0, 28.0], egui::Button::new(theme::icon("plus", 14.0)))
                            .on_hover_text(self.tr("新建环境", "New environment"))
                            .clicked()
                        {
                            new_environment = true;
                        }
                    });
                });
                ui.add_space(10.0);
                for (index, environment) in self.environments.iter().enumerate() {
                    let selected = index == self.selected_environment;
                    let marker = if environment.id() == self.active_environment {
                        "  ●"
                    } else {
                        ""
                    };
                    let dirty = if environment.is_dirty() { "  •" } else { "" };
                    let response = ui.add_sized(
                        [ui.available_width(), 34.0],
                        egui::Button::new(RichText::new(format!(
                            "{}{}{}",
                            environment.name, marker, dirty
                        )))
                        .selected(selected)
                        .fill(if selected {
                            palette.primary_soft
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE),
                    );
                    if response.clicked() && !selected {
                        selection = Some(environment.id());
                    }
                }
            });
        if new_environment {
            self.queue_action(PendingAction::NewEnvironment);
        }
        if let Some(id) = selection {
            self.queue_action(PendingAction::SelectEnvironment(id));
        }
    }

    fn document_tab_bar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let tabs = self.document_tabs.items().to_vec();
        let active = self.document_tabs.active();
        let mut activate = None;
        let mut close = None;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                egui::ScrollArea::horizontal()
                    .id_salt("document_tabs")
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for tab in &tabs {
                                ui.push_id((tab.id.kind, tab.id.entity_id), |ui| {
                                    let selected = active == Some(tab.id);
                                    let dirty = self.document_dirty(tab.id);
                                    egui::Frame::new()
                                        .fill(if selected {
                                            palette.primary_soft
                                        } else {
                                            Color32::TRANSPARENT
                                        })
                                        .corner_radius(CornerRadius::same(4))
                                        .inner_margin(egui::Margin::symmetric(8, 3))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                let title = if dirty {
                                                    format!("{}  •", tab.title)
                                                } else {
                                                    tab.title.clone()
                                                };
                                                if ui
                                                    .add(
                                                        egui::Button::new(title)
                                                            .frame(false)
                                                            .selected(selected),
                                                    )
                                                    .clicked()
                                                {
                                                    activate = Some(tab.id);
                                                }
                                                if ui
                                                    .add_sized(
                                                        [20.0, 20.0],
                                                        egui::Button::new(theme::icon("x", 11.0))
                                                            .frame(false),
                                                    )
                                                    .on_hover_text(self.tr("关闭", "Close"))
                                                    .clicked()
                                                {
                                                    close = Some(tab.id);
                                                }
                                            });
                                        });
                                });
                            }
                        });
                    });
            });
        if let Some(id) = activate {
            self.activate_document(id);
        }
        if let Some(id) = close {
            self.activate_document(id);
            self.queue_action(PendingAction::CloseDocument(id));
        }
    }

    fn request_workspace(&mut self, ui: &mut egui::Ui, palette: Palette) {
        if self.requests.get(self.selected).is_none() {
            ui.centered_and_justified(|ui| {
                if ui
                    .button(theme::icon_label(
                        "plus",
                        self.tr("新建请求", "Create request"),
                        13.0,
                        palette.text,
                    ))
                    .clicked()
                {
                    self.queue_action(PendingAction::NewRequest(ProtocolKind::Http));
                }
            });
            return;
        }
        let maximum = (ui.available_height() - 220.0).max(220.0);
        egui::Panel::top("request_composer")
            .resizable(true)
            .default_size(350.0)
            .size_range(220.0..=maximum)
            .frame(
                egui::Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0, palette.divider)),
            )
            .show(ui, |ui| self.request_composer(ui, palette));
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(16, 10))
            .show(ui, |ui| self.response_panel(ui, palette));
    }

    fn request_composer(&mut self, ui: &mut egui::Ui, palette: Palette) {
        ui.set_min_size(ui.available_size());
        let index = self.selected;
        if self.requests[index].alternate_protocol.is_some() {
            self.protocol_request_composer(ui, palette);
            return;
        }
        let request_id = self.requests[index].id();
        let dirty = self.requests[index].is_dirty();
        let save_label = self.tr("保存", "Save");
        let send_label = self.tr("发送", "Send");
        let stop_label = self.tr("停止", "Stop");
        let delete_tip = self.tr("删除请求", "Delete request");
        let settings_tip = self.tr("请求设置", "Request settings");
        let follow_redirects_label = self.tr("跟随重定向", "Follow redirects");
        let validate_tls_label = self.tr("校验 TLS 证书", "Validate TLS certificates");
        let mut save = false;
        let mut delete = false;
        let mut send = false;
        let mut stop = false;
        let mut editor_error = None;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(16, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.requests[index].name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(280.0),
                    );
                    if dirty {
                        ui.label(RichText::new("●").color(palette.warning).size(8.0));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [32.0, 30.0],
                                egui::Button::new(theme::icon("trash-2", 14.0))
                                    .stroke(Stroke::NONE),
                            )
                            .on_hover_text(delete_tip)
                            .clicked()
                        {
                            delete = true;
                        }
                        if ui
                            .add_enabled(
                                dirty,
                                egui::Button::new(theme::icon_label(
                                    "save",
                                    save_label,
                                    12.0,
                                    palette.text,
                                )),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    method_combo(ui, &mut self.requests[index].draft.method, palette);
                    let reserved = 44.0 + 96.0 + 16.0;
                    let url_width = (ui.available_width() - reserved).max(180.0);
                    ui.add_sized(
                        [url_width, 36.0],
                        egui::TextEdit::singleline(&mut self.requests[index].draft.url)
                            .hint_text("https://api.example.com/v1/users/{{id}}"),
                    );
                    ui.menu_button(theme::icon("settings", 15.0), |ui| {
                        ui.set_min_width(230.0);
                        ui.label(RichText::new(settings_tip).strong());
                        ui.horizontal(|ui| {
                            ui.label(self.tr("超时", "Timeout"));
                            ui.add(
                                egui::DragValue::new(&mut self.requests[index].draft.timeout_ms)
                                    .range(1..=600_000)
                                    .suffix(" ms"),
                            );
                        });
                        ui.checkbox(
                            &mut self.requests[index].draft.follow_redirects,
                            follow_redirects_label,
                        );
                        ui.checkbox(
                            &mut self.requests[index].draft.validate_tls,
                            validate_tls_label,
                        );
                        ui.separator();
                        let mut proxy_enabled = self.requests[index].draft.proxy.is_some();
                        if ui
                            .checkbox(&mut proxy_enabled, self.tr("使用代理", "Use proxy"))
                            .changed()
                        {
                            self.requests[index].draft.proxy = if proxy_enabled {
                                let mut proxy = ProxyDraft::new(request_id);
                                match self.secrets.get(&proxy.password.reference) {
                                    Ok(Some(_)) => proxy.password.configured = true,
                                    Ok(None) => {}
                                    Err(error) => editor_error = Some(error.to_string()),
                                }
                                Some(proxy)
                            } else {
                                None
                            };
                        }
                        let proxy_username_hint = self
                            .tr("代理用户名（可选）", "Proxy username (optional)")
                            .to_owned();
                        let proxy_password_hint = self
                            .tr("代理密码（可选）", "Proxy password (optional)")
                            .to_owned();
                        if let Some(proxy) = self.requests[index].draft.proxy.as_mut() {
                            ui.add(
                                egui::TextEdit::singleline(&mut proxy.url)
                                    .hint_text("http://127.0.0.1:8080 / socks5://..."),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut proxy.username)
                                    .hint_text(proxy_username_hint),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut proxy.password.replacement)
                                    .password(true)
                                    .hint_text(if proxy.password.configured {
                                        "••••••••"
                                    } else {
                                        &proxy_password_hint
                                    }),
                            );
                        }
                        ui.horizontal(|ui| {
                            let certificate = self.requests[index]
                                .draft
                                .client_certificate
                                .as_ref()
                                .map(|certificate| certificate.pem_file.display().to_string())
                                .unwrap_or_else(|| {
                                    self.tr("未配置客户端证书", "No client certificate").into()
                                });
                            ui.label(RichText::new(certificate).small().color(palette.muted));
                            if ui.button(self.tr("选择 PEM", "Choose PEM")).clicked()
                                && let Some(pem_file) = rfd::FileDialog::new()
                                    .add_filter("PEM", &["pem", "crt", "key"])
                                    .pick_file()
                            {
                                self.requests[index].draft.client_certificate =
                                    Some(apitest_core::ClientCertificate { pem_file });
                            }
                            if self.requests[index].draft.client_certificate.is_some()
                                && ui.button(self.tr("清除", "Clear")).clicked()
                            {
                                self.requests[index].draft.client_certificate = None;
                            }
                        });
                    })
                    .response
                    .on_hover_text(settings_tip);
                    if self.response.is_active() {
                        let stopping = self.response.state == RunState::Cancelling;
                        if ui
                            .add_enabled(
                                !stopping,
                                egui::Button::new(theme::icon_label(
                                    "circle-stop",
                                    stop_label,
                                    13.0,
                                    palette.danger,
                                ))
                                .fill(palette.primary_soft)
                                .stroke(Stroke::new(1.0, palette.danger))
                                .min_size(Vec2::new(92.0, 36.0)),
                            )
                            .clicked()
                        {
                            stop = true;
                        }
                    } else if ui
                        .add_sized(
                            [92.0, 36.0],
                            egui::Button::new(theme::icon_label(
                                "send",
                                send_label,
                                13.0,
                                Color32::WHITE,
                            ))
                            .fill(palette.primary)
                            .stroke(Stroke::NONE),
                        )
                        .clicked()
                    {
                        send = true;
                    }
                });
                ui.add_space(6.0);
                editor_tabs(
                    ui,
                    &mut self.editor_tab,
                    self.language,
                    &self.requests[index].draft,
                    palette,
                );
                ui.separator();
                let language = self.language;
                match self.editor_tab {
                    EditorTab::Params => editable_pairs(
                        ui,
                        &mut self.requests[index].draft.query,
                        language,
                        palette,
                        true,
                    ),
                    EditorTab::Headers => editable_pairs(
                        ui,
                        &mut self.requests[index].draft.headers,
                        language,
                        palette,
                        true,
                    ),
                    EditorTab::Cookies => editable_pairs(
                        ui,
                        &mut self.requests[index].draft.cookies,
                        language,
                        palette,
                        true,
                    ),
                    EditorTab::Body => {
                        editor_error =
                            body_editor(ui, &mut self.requests[index].draft, language, palette)
                    }
                    EditorTab::Auth => {
                        editor_error = auth_editor(
                            ui,
                            &mut self.requests[index].draft.auth,
                            request_id,
                            Arc::clone(&self.secrets),
                            language,
                            palette,
                        )
                    }
                }
                self.requests[index].draft.ensure_empty_rows();
            });
        if let Some(error) = editor_error {
            self.toast(ToastKind::Error, error);
        }
        if save {
            self.save_current_request();
        }
        if delete {
            self.confirmation = Some(Confirmation::DeleteRequest(request_id));
        }
        if send {
            self.send_current(ui.ctx());
        }
        if stop {
            self.stop();
        }
    }

    fn protocol_request_composer(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let index = self.selected;
        let request_id = self.requests[index].id();
        let kind = self.requests[index].protocol_kind();
        let dirty = self.requests[index].is_dirty();
        let mut save = false;
        let mut delete = false;
        let mut send = false;
        let mut stop = false;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(16, 10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(protocol_label(kind))
                            .strong()
                            .color(protocol_color(kind, palette)),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.requests[index].name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(280.0),
                    );
                    if dirty {
                        ui.label(RichText::new("●").color(palette.warning).size(8.0));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [32.0, 30.0],
                                egui::Button::new(theme::icon("trash-2", 14.0))
                                    .stroke(Stroke::NONE),
                            )
                            .on_hover_text(self.tr("删除请求", "Delete request"))
                            .clicked()
                        {
                            delete = true;
                        }
                        if ui
                            .add_enabled(
                                dirty,
                                egui::Button::new(theme::icon_label(
                                    "save",
                                    self.tr("保存", "Save"),
                                    12.0,
                                    palette.text,
                                )),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                        if self.response.is_active() {
                            if ui
                                .button(theme::icon_label(
                                    "circle-stop",
                                    self.tr("停止", "Stop"),
                                    13.0,
                                    palette.danger,
                                ))
                                .clicked()
                            {
                                stop = true;
                            }
                        } else if ui
                            .add(
                                egui::Button::new(theme::icon_label(
                                    "send",
                                    self.tr("连接 / 发送", "Connect / Send"),
                                    13.0,
                                    Color32::WHITE,
                                ))
                                .fill(palette.primary)
                                .stroke(Stroke::NONE),
                            )
                            .clicked()
                        {
                            send = true;
                        }
                    });
                });
                ui.separator();
                if let Some(protocol) = self.requests[index].alternate_protocol.as_mut() {
                    protocol_editor(ui, protocol, self.language, palette);
                }
            });
        if save {
            self.save_current_request();
        }
        if delete {
            self.confirmation = Some(Confirmation::DeleteRequest(request_id));
        }
        if send {
            self.send_current(ui.ctx());
        }
        if stop {
            self.stop();
        }
    }

    fn response_panel(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let websocket_selected = self
            .requests
            .get(self.selected)
            .is_some_and(|request| request.protocol_kind() == ProtocolKind::WebSocket);
        ui.horizontal(|ui| {
            for (tab, chinese, english) in [
                (ResponseTab::Body, "响应体", "Body"),
                (ResponseTab::Headers, "响应头", "Headers"),
                (ResponseTab::Timeline, "时间线", "Timeline"),
            ] {
                if tab_button(
                    ui,
                    self.response_tab == tab,
                    self.tr(chinese, english),
                    palette,
                )
                .clicked()
                {
                    self.response_tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.response.is_active() {
                    ui.spinner();
                }
                if let Some(metrics) = self.response.metrics {
                    ui.label(
                        RichText::new(format!(
                            "{} ms  ·  ↓ {} B  ·  ↑ {} B",
                            metrics.elapsed_ms, metrics.received_bytes, metrics.sent_bytes
                        ))
                        .color(palette.muted),
                    );
                }
                if let Some(version) = &self.response.version {
                    ui.label(RichText::new(version).color(palette.muted));
                }
                if let Some(status) = self.response.status {
                    let color = if status < 400 {
                        palette.success
                    } else {
                        palette.danger
                    };
                    ui.label(RichText::new(status.to_string()).strong().color(color));
                }
            });
        });
        if websocket_selected {
            self.websocket_message_bar(ui, palette);
        }
        ui.separator();
        if let Some(error) = &self.response.error {
            egui::Frame::new()
                .fill(palette.primary_soft)
                .stroke(Stroke::new(1.0, palette.danger))
                .corner_radius(CornerRadius::same(5))
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.label(RichText::new(error).color(palette.danger));
                });
            ui.add_space(6.0);
        }
        match self.response_tab {
            ResponseTab::Body => self.response_body(ui, palette),
            ResponseTab::Headers => {
                response_headers(ui, &self.response.headers, self.language, palette)
            }
            ResponseTab::Timeline => {
                response_timeline(ui, &self.response.timeline, self.language, palette)
            }
        }
    }

    fn websocket_message_bar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let connected = self.execution_commands.is_some();
        let can_send = connected && !self.websocket_message.trim().is_empty();
        let message_hint = self
            .tr("输入 WebSocket 文本消息", "Enter a WebSocket text message")
            .to_owned();
        let mut send = false;
        let mut close = false;
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(if connected {
                    self.tr("已连接", "Connected")
                } else {
                    self.tr("未连接", "Disconnected")
                })
                .small()
                .color(if connected {
                    palette.success
                } else {
                    palette.muted
                }),
            );
            let width = (ui.available_width() - 220.0).max(160.0);
            let response = ui.add_sized(
                [width, 30.0],
                egui::TextEdit::singleline(&mut self.websocket_message).hint_text(message_hint),
            );
            if response.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                && can_send
            {
                send = true;
            }
            if ui
                .add_enabled(
                    can_send,
                    egui::Button::new(theme::icon_label(
                        "send",
                        self.tr("发送消息", "Send message"),
                        12.0,
                        palette.text,
                    )),
                )
                .clicked()
            {
                send = true;
            }
            if ui
                .add_enabled(
                    connected,
                    egui::Button::new(self.tr("关闭连接", "Close connection")),
                )
                .clicked()
            {
                close = true;
            }
        });
        if send {
            self.send_websocket_message();
        }
        if close {
            self.close_websocket_input();
        }
    }

    fn response_body(&mut self, ui: &mut egui::Ui, palette: Palette) {
        if self.response.body.is_empty() && !self.response.is_active() {
            empty_state(
                ui,
                self.tr("暂无响应", "No response yet"),
                self.tr(
                    "发送请求后将在此显示结果",
                    "Send a request to inspect the result",
                ),
                palette,
            );
            return;
        }
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.response_body_mode,
                ResponseBodyMode::Pretty,
                "Pretty",
            );
            ui.selectable_value(&mut self.response_body_mode, ResponseBodyMode::Raw, "Raw");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_sized([28.0, 28.0], egui::Button::new(theme::icon("copy", 14.0)))
                    .on_hover_text(self.tr("复制响应", "Copy response"))
                    .clicked()
                {
                    let text = self
                        .response
                        .pretty_body
                        .as_ref()
                        .filter(|_| self.response_body_mode == ResponseBodyMode::Pretty)
                        .unwrap_or(&self.response.body)
                        .clone();
                    ui.ctx().copy_text(text);
                }
            });
        });
        let body = self
            .response
            .pretty_body
            .as_ref()
            .filter(|_| self.response_body_mode == ResponseBodyMode::Pretty)
            .unwrap_or(&self.response.body);
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(
                egui::Label::new(RichText::new(body).monospace().color(palette.text))
                    .selectable(true)
                    .wrap_mode(egui::TextWrapMode::Extend),
            );
        });
        if self.response.truncated {
            ui.colored_label(
                palette.warning,
                self.tr(
                    "响应超过 10 MiB，预览已截断",
                    "The response preview was truncated at 10 MiB",
                ),
            );
        }
    }

    fn scenario_workspace(&mut self, ui: &mut egui::Ui, palette: Palette) {
        if self.scenarios.get(self.selected_scenario).is_none() {
            empty_state(
                ui,
                self.tr("暂无测试场景", "No scenarios"),
                self.tr(
                    "新建场景后可视化编排请求和控制节点",
                    "Create a scenario to arrange requests and control nodes",
                ),
                palette,
            );
            if ui.button(self.tr("新建场景", "New scenario")).clicked() {
                self.perform_action(PendingAction::NewScenario);
            }
            return;
        }
        let dirty = self.current_dirty();
        let request_options = self
            .requests
            .iter()
            .map(|request| (request.request_case.id, request.name.clone()))
            .collect::<Vec<_>>();
        let running = self.scenario_cancellation.is_some();
        let report = self.scenario_report.as_ref();
        let mut save = false;
        let mut run = false;
        let mut stop = false;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                let scenario = &mut self.scenarios[self.selected_scenario];
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut scenario.name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(320.0),
                    );
                    if dirty {
                        ui.label(RichText::new("●").color(palette.warning).size(8.0));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if running {
                            if ui
                                .button(match self.language {
                                    Language::Chinese => "停止",
                                    Language::English => "Stop",
                                })
                                .clicked()
                            {
                                stop = true;
                            }
                            ui.spinner();
                        } else if ui
                            .button(theme::icon_label(
                                "play",
                                match self.language {
                                    Language::Chinese => "运行场景",
                                    Language::English => "Run scenario",
                                },
                                12.0,
                                palette.text,
                            ))
                            .clicked()
                        {
                            run = true;
                        }
                        if ui
                            .add_enabled(
                                dirty,
                                egui::Button::new(theme::icon_label(
                                    "save",
                                    match self.language {
                                        Language::Chinese => "保存",
                                        Language::English => "Save",
                                    },
                                    12.0,
                                    palette.text,
                                )),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.checkbox(
                        &mut scenario.stop_on_failure,
                        match self.language {
                            Language::Chinese => "失败时停止",
                            Language::English => "Stop on failure",
                        },
                    );
                    let dataset = scenario
                        .dataset_path
                        .as_deref()
                        .unwrap_or(match self.language {
                            Language::Chinese => "未选择数据集",
                            Language::English => "No dataset",
                        });
                    ui.label(RichText::new(dataset).small().color(palette.muted));
                    if ui
                        .button(match self.language {
                            Language::Chinese => "选择 CSV/JSON",
                            Language::English => "Choose CSV/JSON",
                        })
                        .clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("Dataset", &["csv", "json"])
                            .pick_file()
                    {
                        scenario.dataset_path = Some(path.display().to_string());
                    }
                    if scenario.dataset_path.is_some()
                        && ui
                            .button(match self.language {
                                Language::Chinese => "清除",
                                Language::English => "Clear",
                            })
                            .clicked()
                    {
                        scenario.dataset_path = None;
                    }
                });
                if let Some(report) = report {
                    ui.separator();
                    scenario_report_view(ui, report, self.language, palette);
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    scenario_nodes_editor(
                        ui,
                        &mut scenario.nodes,
                        &request_options,
                        self.language,
                        palette,
                        0,
                    );
                });
            });
        if save {
            self.save_current_scenario();
        }
        if run {
            self.run_current_scenario(ui.ctx());
        }
        if stop {
            self.stop_current_scenario();
        }
    }

    fn mock_workspace(&mut self, ui: &mut egui::Ui, palette: Palette) {
        if self.mock_profiles.get(self.selected_mock).is_none() {
            empty_state(
                ui,
                self.tr("暂无 Mock 服务", "No mock servers"),
                self.tr(
                    "新建 Mock 后可从 API 契约智能生成响应",
                    "Create a mock to generate responses from API contracts",
                ),
                palette,
            );
            if ui.button(self.tr("新建 Mock", "New mock")).clicked() {
                self.perform_action(PendingAction::NewMock);
            }
            return;
        }
        let dirty = self.current_dirty();
        let running = self.mock_server.is_some();
        let base_url = self.mock_server.as_ref().map(MockServer::base_url);
        let mut save = false;
        let mut generate = false;
        let mut start = false;
        let mut stop = false;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                let profile = &mut self.mock_profiles[self.selected_mock];
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut profile.name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(300.0),
                    );
                    if dirty {
                        ui.label(RichText::new("●").color(palette.warning).size(8.0));
                    }
                    if let Some(base_url) = &base_url {
                        ui.label(RichText::new(base_url).color(palette.success));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if running {
                            if ui
                                .button(match self.language {
                                    Language::Chinese => "停止服务",
                                    Language::English => "Stop server",
                                })
                                .clicked()
                            {
                                stop = true;
                            }
                        } else if ui
                            .button(match self.language {
                                Language::Chinese => "启动服务",
                                Language::English => "Start server",
                            })
                            .clicked()
                        {
                            start = true;
                        }
                        if ui
                            .button(match self.language {
                                Language::Chinese => "智能生成",
                                Language::English => "Smart generate",
                            })
                            .on_hover_text(match self.language {
                                Language::Chinese => "从 HTTP 契约新增尚不存在的规则",
                                Language::English => "Add missing rules from HTTP contracts",
                            })
                            .clicked()
                        {
                            generate = true;
                        }
                        if ui
                            .add_enabled(
                                dirty,
                                egui::Button::new(match self.language {
                                    Language::Chinese => "保存",
                                    Language::English => "Save",
                                }),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label(match self.language {
                        Language::Chinese => "监听地址",
                        Language::English => "Bind address",
                    });
                    ui.add_enabled(
                        !running,
                        egui::TextEdit::singleline(&mut profile.bind_address).desired_width(150.0),
                    );
                    ui.label(match self.language {
                        Language::Chinese => "端口",
                        Language::English => "Port",
                    });
                    ui.add_enabled(
                        !running,
                        egui::DragValue::new(&mut profile.port).range(0..=u16::MAX),
                    );
                    ui.label(
                        RichText::new(format!("{} rules", profile.rules.len()))
                            .color(palette.muted),
                    );
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    mock_rules_editor(ui, &mut profile.rules, self.language, palette, running);
                });
            });
        if save {
            self.save_current_mock();
        }
        if generate {
            self.generate_current_mock_rules();
        }
        if start {
            self.start_current_mock(ui.ctx());
        }
        if stop {
            self.stop_current_mock();
        }
    }

    fn history_workspace(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let Some(record) = self.run_records.get(self.selected_history).cloned() else {
            empty_state(
                ui,
                self.tr("暂无运行历史", "No run history"),
                self.tr(
                    "发送请求后会在本地保存脱敏的执行记录",
                    "Run a request to save a redacted local record",
                ),
                palette,
            );
            return;
        };
        let request_name = record
            .request_case_id
            .and_then(|case_id| {
                self.requests
                    .iter()
                    .find(|request| request.request_case.id == case_id)
            })
            .map(|request| request.name.as_str())
            .unwrap_or(self.tr("已删除的请求", "Deleted request"));
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(request_name).heading());
                    ui.colored_label(
                        history_state_color(record.state, palette),
                        history_state_label(record.state, self.language),
                    );
                });
                ui.horizontal_wrapped(|ui| {
                    history_metric(
                        ui,
                        self.tr("状态码", "Status"),
                        record
                            .status_code
                            .map(|status| status.to_string())
                            .unwrap_or_else(|| "—".into()),
                        palette,
                    );
                    history_metric(
                        ui,
                        self.tr("耗时", "Elapsed"),
                        format!("{} ms", record.elapsed_ms),
                        palette,
                    );
                    history_metric(
                        ui,
                        self.tr("响应大小", "Response size"),
                        format_bytes(record.response_bytes),
                        palette,
                    );
                    history_metric(
                        ui,
                        self.tr("开始时间", "Started"),
                        record.started_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                        palette,
                    );
                });
                if let Some(error) = &record.error {
                    ui.colored_label(palette.danger, error);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(self.tr("响应正文", "Response body")).strong());
                    ui.label(
                        RichText::new(self.tr("已脱敏并外置存储", "Redacted external storage"))
                            .small()
                            .color(palette.muted),
                    );
                });
                if record.body_path.is_none() {
                    empty_state(ui, self.tr("没有响应正文", "No response body"), "", palette);
                } else {
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.history_body_preview)
                                    .monospace()
                                    .color(palette.text),
                            )
                            .selectable(true)
                            .wrap_mode(egui::TextWrapMode::Extend),
                        );
                    });
                    if self.history_body_truncated {
                        ui.colored_label(
                            palette.warning,
                            self.tr(
                                "历史正文超过 10 MiB，预览已截断",
                                "History body preview was truncated at 10 MiB",
                            ),
                        );
                    }
                }
            });
    }

    fn environment_workspace(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let Some(environment) = self.environments.get(self.selected_environment) else {
            return;
        };
        let environment_id = environment.id();
        let dirty = environment.is_dirty();
        let save_label = self.tr("保存", "Save");
        let delete_tip = self.tr("删除环境", "Delete environment");
        let mut save = false;
        let mut delete = false;
        let mut variable_error = None;
        egui::Frame::new()
            .fill(palette.surface)
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(
                            &mut self.environments[self.selected_environment].name,
                        )
                        .font(egui::TextStyle::Heading)
                        .desired_width(300.0),
                    );
                    if dirty {
                        ui.label(RichText::new("●").color(palette.warning).size(8.0));
                    }
                    if environment_id == self.active_environment {
                        ui.label(
                            RichText::new(self.tr("当前环境", "Active")).color(palette.success),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [32.0, 30.0],
                                egui::Button::new(theme::icon("trash-2", 14.0)),
                            )
                            .on_hover_text(delete_tip)
                            .clicked()
                        {
                            delete = true;
                        }
                        if ui
                            .add_enabled(
                                dirty,
                                egui::Button::new(theme::icon_label(
                                    "save",
                                    save_label,
                                    12.0,
                                    palette.text,
                                )),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                    });
                });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(self.tr("环境变量", "Environment variables"))
                        .strong()
                        .size(14.0),
                );
                ui.add_space(4.0);
                variable_error = variable_table(
                    ui,
                    &mut self.environments[self.selected_environment],
                    Arc::clone(&self.secrets),
                    self.language,
                    palette,
                );
            });
        if let Some(error) = variable_error {
            self.toast(ToastKind::Error, error);
        }
        if save {
            self.save_current_environment();
        }
        if delete {
            self.confirmation = Some(Confirmation::DeleteEnvironment(environment_id));
        }
    }

    fn settings_window(&mut self, context: &egui::Context) {
        let mut open = self.show_settings;
        let mut theme_changed = false;
        let mut language_changed = false;
        let dark_label = self.tr("深色", "Dark");
        let light_label = self.tr("浅色", "Light");
        egui::Window::new(self.tr("设置", "Settings"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(360.0)
            .show(context, |ui| {
                ui.label(RichText::new(self.tr("外观", "Appearance")).strong());
                ui.horizontal(|ui| {
                    theme_changed |= ui
                        .selectable_value(&mut self.theme, ThemeMode::Dark, dark_label)
                        .changed();
                    theme_changed |= ui
                        .selectable_value(&mut self.theme, ThemeMode::Light, light_label)
                        .changed();
                });
                ui.add_space(8.0);
                ui.label(RichText::new(self.tr("语言", "Language")).strong());
                ui.horizontal(|ui| {
                    language_changed |= ui
                        .selectable_value(&mut self.language, Language::Chinese, "简体中文")
                        .changed();
                    language_changed |= ui
                        .selectable_value(&mut self.language, Language::English, "English")
                        .changed();
                });
            });
        self.show_settings = open;
        if theme_changed {
            theme::apply(context, self.theme);
            let theme = self.theme;
            self.persist_setting(THEME_SETTING, &theme);
        }
        if language_changed {
            let language = self.language;
            self.persist_setting(LANGUAGE_SETTING, &language);
        }
    }

    fn curl_import_window(&mut self, context: &egui::Context) {
        let mut open = self.show_curl_import;
        let mut import = false;
        let mut cancel = false;
        let title = self.tr("粘贴 cURL", "Paste cURL");
        let hint = self
            .tr(
                "粘贴完整的 curl 命令；命令只会被解析，不会执行",
                "Paste a complete curl command; it is parsed and never executed",
            )
            .to_owned();
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .default_size([720.0, 420.0])
            .show(context, |ui| {
                ui.label(RichText::new(&hint).color(Palette::for_mode(self.theme).muted));
                ui.add_sized(
                    ui.available_size() - egui::vec2(0.0, 44.0),
                    egui::TextEdit::multiline(&mut self.curl_import_source)
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .hint_text("curl --request GET 'https://api.example.com/users'"),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !self.curl_import_source.trim().is_empty(),
                            egui::Button::new(self.tr("导入", "Import")),
                        )
                        .clicked()
                    {
                        import = true;
                    }
                    if ui.button(self.tr("取消", "Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            open = false;
        }
        self.show_curl_import = open;
        if import {
            let source = self.curl_import_source.clone();
            match self.import_source(InteropAction::ImportCurl, &source) {
                Ok(count) => {
                    self.curl_import_source.clear();
                    self.show_curl_import = false;
                    self.toast(
                        ToastKind::Success,
                        match self.language {
                            Language::Chinese => format!("已导入 {count} 个请求"),
                            Language::English => format!("Imported {count} requests"),
                        },
                    );
                }
                Err(error) => self.toast(ToastKind::Error, error),
            }
        }
    }

    fn openapi_preview_window(&mut self, context: &egui::Context) {
        let mut open = self.show_openapi_preview;
        let mut refresh = false;
        let mut validate = false;
        let mut export_yaml = false;
        let mut export_html = false;
        let palette = Palette::for_mode(self.theme);
        egui::Window::new(self.tr("OpenAPI 设计与预览", "OpenAPI design & preview"))
            .open(&mut open)
            .collapsible(false)
            .default_size([900.0, 650.0])
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    for (tab, chinese, english) in [
                        (OpenApiPreviewTab::Yaml, "YAML", "YAML"),
                        (OpenApiPreviewTab::Validation, "校验结果", "Validation"),
                        (
                            OpenApiPreviewTab::Documentation,
                            "HTML 文档",
                            "HTML documentation",
                        ),
                    ] {
                        ui.selectable_value(
                            &mut self.openapi_preview_tab,
                            tab,
                            match self.language {
                                Language::Chinese => chinese,
                                Language::English => english,
                            },
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(self.tr("刷新", "Refresh")).clicked() {
                            refresh = true;
                        }
                        if ui.button(self.tr("校验", "Validate")).clicked() {
                            validate = true;
                        }
                        if ui.button(self.tr("导出 HTML", "Export HTML")).clicked() {
                            export_html = true;
                        }
                        if ui.button(self.tr("导出 YAML", "Export YAML")).clicked() {
                            export_yaml = true;
                        }
                    });
                });
                ui.separator();
                match self.openapi_preview_tab {
                    OpenApiPreviewTab::Yaml => {
                        ui.add_sized(
                            ui.available_size(),
                            egui::TextEdit::multiline(&mut self.openapi_source)
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    }
                    OpenApiPreviewTab::Validation => {
                        if self.openapi_issues.is_empty() {
                            ui.colored_label(
                                palette.success,
                                self.tr("未发现问题", "No issues found"),
                            );
                        }
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for issue in &self.openapi_issues {
                                let (label, color) = match issue.level {
                                    OpenApiIssueLevel::Error => {
                                        (self.tr("错误", "Error"), palette.danger)
                                    }
                                    OpenApiIssueLevel::Warning => {
                                        (self.tr("警告", "Warning"), palette.warning)
                                    }
                                };
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(label).strong().color(color));
                                    ui.label(RichText::new(&issue.path).monospace());
                                    ui.label(&issue.message);
                                });
                                ui.separator();
                            }
                        });
                    }
                    OpenApiPreviewTab::Documentation => {
                        ui.label(
                            RichText::new(self.tr(
                                "以下为可直接发布的静态 HTML 源码预览",
                                "Preview of the publish-ready static HTML source",
                            ))
                            .color(palette.muted),
                        );
                        egui::ScrollArea::both().show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&self.openapi_html)
                                        .monospace()
                                        .color(palette.text),
                                )
                                .selectable(true)
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        });
                    }
                }
            });
        self.show_openapi_preview = open;
        if refresh && let Err(error) = self.refresh_openapi_preview() {
            self.toast(ToastKind::Error, error);
        }
        if validate {
            match validate_openapi(&self.openapi_source) {
                Ok(issues) => {
                    self.openapi_issues = issues;
                    self.openapi_preview_tab = OpenApiPreviewTab::Validation;
                }
                Err(error) => self.toast(ToastKind::Error, error.to_string()),
            }
        }
        if export_yaml {
            let source = self.openapi_source.clone();
            self.export_text("openapi.yaml", "OpenAPI", &source);
        }
        if export_html {
            let html = self.openapi_html.clone();
            self.export_text("api-documentation.html", "HTML", &html);
        }
    }

    fn confirmation_window(&mut self, context: &egui::Context) {
        let Some(confirmation) = self.confirmation else {
            return;
        };
        let title = match confirmation {
            Confirmation::Unsaved(_) => self.tr("未保存的更改", "Unsaved changes"),
            Confirmation::DeleteRequest(_) => self.tr("删除请求", "Delete request"),
            Confirmation::DeleteEnvironment(_) => self.tr("删除环境", "Delete environment"),
        };
        let message = match confirmation {
            Confirmation::Unsaved(_) => self.tr(
                "当前内容尚未保存。",
                "The current changes have not been saved.",
            ),
            Confirmation::DeleteRequest(_) => self.tr(
                "该请求及其本地密钥将被删除。",
                "This request and its local secrets will be deleted.",
            ),
            Confirmation::DeleteEnvironment(_) => self.tr(
                "该环境及其本地密钥将被删除。",
                "This environment and its local secrets will be deleted.",
            ),
        };
        let mut save = false;
        let mut discard = false;
        let mut confirm_delete = false;
        let mut cancel = false;
        egui::Window::new(title)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .resizable(false)
            .collapsible(false)
            .show(context, |ui| {
                ui.set_min_width(340.0);
                ui.label(message);
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match confirmation {
                        Confirmation::Unsaved(_) => {
                            if ui.button(self.tr("保存", "Save")).clicked() {
                                save = true;
                            }
                            if ui.button(self.tr("放弃", "Discard")).clicked() {
                                discard = true;
                            }
                        }
                        Confirmation::DeleteRequest(_) | Confirmation::DeleteEnvironment(_) => {
                            if ui
                                .button(
                                    RichText::new(self.tr("删除", "Delete"))
                                        .color(Palette::for_mode(self.theme).danger),
                                )
                                .clicked()
                            {
                                confirm_delete = true;
                            }
                        }
                    }
                    if ui.button(self.tr("取消", "Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            self.confirmation = None;
        } else if save {
            if let Confirmation::Unsaved(action) = confirmation {
                let saved = if action.leaves_workspace() {
                    self.save_workspace_and_wait()
                } else {
                    self.save_current_and_wait()
                };
                if !saved {
                    return;
                }
                self.confirmation = None;
                self.perform_action(action);
            }
        } else if discard {
            if let Confirmation::Unsaved(action) = confirmation {
                let discarded = if action.leaves_workspace() {
                    self.discard_workspace()
                } else {
                    self.discard_current()
                };
                if !discarded {
                    return;
                }
                self.confirmation = None;
                self.perform_action(action);
            }
        } else if confirm_delete {
            self.confirmation = None;
            match confirmation {
                Confirmation::DeleteRequest(id) => self.delete_request(id),
                Confirmation::DeleteEnvironment(id) => self.delete_environment(id),
                Confirmation::Unsaved(_) => {}
            }
        }
    }

    fn toast_area(&mut self, context: &egui::Context) {
        let Some(toast) = &self.toast else {
            return;
        };
        let lifetime = if toast.kind == ToastKind::Error {
            Duration::from_secs(8)
        } else {
            Duration::from_secs(4)
        };
        if toast.created_at.elapsed() >= lifetime {
            self.toast = None;
            return;
        }
        context.request_repaint_after(Duration::from_millis(250));
        let palette = Palette::for_mode(self.theme);
        let (icon, color) = match toast.kind {
            ToastKind::Success => ("circle-check", palette.success),
            ToastKind::Info => ("info", palette.info),
            ToastKind::Error => ("circle-alert", palette.danger),
        };
        let message = toast.message.clone();
        let mut close = false;
        egui::Area::new("toast".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, [-18.0, -18.0])
            .order(egui::Order::Foreground)
            .show(context, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, color))
                    .corner_radius(CornerRadius::same(6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(theme::icon(icon, 15.0).color(color));
                            ui.label(message);
                            if ui
                                .add_sized([24.0, 24.0], egui::Button::new(theme::icon("x", 13.0)))
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    });
            });
        if close {
            self.toast = None;
        }
    }
}

impl eframe::App for ApiTestApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_runtime();
        self.drain_storage();
        self.keyboard_shortcuts(context);
        self.schedule_request_autosaves(context);
        self.schedule_environment_autosaves(context);
        if context.input(|input| input.viewport().close_requested())
            && !self.allow_close
            && self.workspace_dirty()
        {
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.confirmation.is_none() {
                self.confirmation = Some(Confirmation::Unsaved(PendingAction::Close));
            }
        }
        if self.allow_close {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let palette = Palette::for_mode(self.theme);
        egui::Panel::top("top_bar")
            .exact_size(48.0)
            .frame(
                egui::Frame::new()
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, palette.divider)),
            )
            .show(ui, |ui| self.top_bar(ui, palette));
        egui::Panel::left("activity")
            .exact_size(60.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette.rail_bg)
                    .stroke(Stroke::new(1.0, palette.divider)),
            )
            .show(ui, |ui| self.activity_bar(ui, palette));
        egui::Panel::left("sidebar")
            .default_size(252.0)
            .min_size(220.0)
            .max_size(320.0)
            .frame(
                egui::Frame::new()
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, palette.divider)),
            )
            .show(ui, |ui| match self.navigation {
                Navigation::Api => self.api_sidebar(ui, palette),
                Navigation::Scenario => self.scenario_sidebar(ui, palette),
                Navigation::Mock => self.mock_sidebar(ui, palette),
                Navigation::History => self.history_sidebar(ui, palette),
                Navigation::Environment => self.environment_sidebar(ui, palette),
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.surface))
            .show(ui, |ui| {
                self.document_tab_bar(ui, palette);
                match self.navigation {
                    Navigation::Api => self.request_workspace(ui, palette),
                    Navigation::Scenario => self.scenario_workspace(ui, palette),
                    Navigation::Mock => self.mock_workspace(ui, palette),
                    Navigation::History => self.history_workspace(ui, palette),
                    Navigation::Environment => self.environment_workspace(ui, palette),
                }
            });

        if self.show_settings {
            self.settings_window(ui.ctx());
        }
        if self.show_curl_import {
            self.curl_import_window(ui.ctx());
        }
        if self.show_openapi_preview {
            self.openapi_preview_window(ui.ctx());
        }
        self.confirmation_window(ui.ctx());
        self.toast_area(ui.ctx());
    }
}

fn open_database() -> Result<Database, String> {
    let directory = directories::ProjectDirs::from("io.github", "zzhtl", "ApiTest")
        .ok_or_else(|| "failed to resolve the application data directory".to_owned())?;
    let startup =
        open_workspace_database(directory.data_local_dir()).map_err(|error| error.to_string())?;
    if let Some(path) = startup.legacy_archive {
        tracing::info!(archive = %path.display(), "archived ApiTest v1 database");
    }
    Ok(startup.database)
}

fn load_setting<T>(database: Option<&Database>, key: &str, default: T) -> T
where
    T: serde::de::DeserializeOwned,
{
    database
        .and_then(|database| database.get_setting(key).ok().flatten())
        .unwrap_or(default)
}

type WorkspaceLoad = (
    Vec<Project>,
    Project,
    Vec<WorkspaceRequest>,
    Vec<EnvironmentDraft>,
    HashMap<Option<EntityId>, ResourcePage>,
    Vec<String>,
);

type ProjectContent = (
    Vec<WorkspaceRequest>,
    Vec<EnvironmentDraft>,
    HashMap<Option<EntityId>, ResourcePage>,
    Vec<String>,
);

fn load_workspace(database: Option<&Database>, language: Language) -> WorkspaceLoad {
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

fn load_project_content(
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

fn load_automation(
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

fn document_snapshot(value: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec(value).expect("workspace document should serialize")
}

fn validate_scenario_nodes(
    nodes: &[ScenarioNode],
    requests: &[WorkspaceRequest],
) -> Result<(), String> {
    for node in nodes {
        match node {
            ScenarioNode::Request { case_id, .. } => {
                if !requests
                    .iter()
                    .any(|request| request.request_case.id == *case_id)
                {
                    return Err(format!(
                        "scenario references missing request case {case_id}"
                    ));
                }
            }
            ScenarioNode::Group { name, nodes } => {
                if name.trim().is_empty() {
                    return Err("scenario group name cannot be empty".into());
                }
                validate_scenario_nodes(nodes, requests)?;
            }
            ScenarioNode::If {
                expression,
                then_nodes,
                else_nodes,
            } => {
                if expression.trim().is_empty() {
                    return Err("scenario condition cannot be empty".into());
                }
                validate_scenario_nodes(then_nodes, requests)?;
                validate_scenario_nodes(else_nodes, requests)?;
            }
            ScenarioNode::Loop {
                source,
                item_name,
                max_iterations,
                nodes,
            } => {
                if source.trim().is_empty() || item_name.trim().is_empty() {
                    return Err("scenario loop source and item name cannot be empty".into());
                }
                if *max_iterations == 0 {
                    return Err("scenario loop iteration limit must be positive".into());
                }
                validate_scenario_nodes(nodes, requests)?;
            }
            ScenarioNode::Delay { .. } => {}
        }
    }
    Ok(())
}

fn scenario_case_ids(nodes: &[ScenarioNode]) -> HashSet<EntityId> {
    fn collect(nodes: &[ScenarioNode], ids: &mut HashSet<EntityId>) {
        for node in nodes {
            match node {
                ScenarioNode::Request { case_id, .. } => {
                    ids.insert(*case_id);
                }
                ScenarioNode::Group { nodes, .. } | ScenarioNode::Loop { nodes, .. } => {
                    collect(nodes, ids);
                }
                ScenarioNode::If {
                    then_nodes,
                    else_nodes,
                    ..
                } => {
                    collect(then_nodes, ids);
                    collect(else_nodes, ids);
                }
                ScenarioNode::Delay { .. } => {}
            }
        }
    }

    let mut ids = HashSet::new();
    collect(nodes, &mut ids);
    ids
}

fn load_scenario_datasets(path: Option<&str>) -> Result<Vec<BTreeMap<String, String>>, String> {
    let Some(path) = path else {
        return Ok(vec![BTreeMap::new()]);
    };
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let datasets = match extension.as_str() {
        "json" => {
            let source = std::fs::read_to_string(path)
                .map_err(|error| format!("failed to read scenario dataset: {error}"))?;
            let value = serde_json::from_str::<serde_json::Value>(&source)
                .map_err(|error| format!("invalid JSON scenario dataset: {error}"))?;
            match value {
                serde_json::Value::Array(rows) => rows
                    .into_iter()
                    .map(json_dataset_row)
                    .collect::<Result<Vec<_>, _>>()?,
                serde_json::Value::Object(row) => {
                    vec![json_dataset_row(serde_json::Value::Object(row))?]
                }
                _ => {
                    return Err(
                        "JSON scenario dataset must be an object or an array of objects".into(),
                    );
                }
            }
        }
        "csv" => {
            let mut reader = csv::Reader::from_path(path)
                .map_err(|error| format!("failed to read CSV scenario dataset: {error}"))?;
            let headers = reader
                .headers()
                .map_err(|error| format!("invalid CSV scenario dataset: {error}"))?
                .clone();
            let mut rows = Vec::new();
            for record in reader.records() {
                let record =
                    record.map_err(|error| format!("invalid CSV scenario dataset: {error}"))?;
                rows.push(
                    headers
                        .iter()
                        .zip(record.iter())
                        .map(|(name, value)| (name.to_owned(), value.to_owned()))
                        .collect(),
                );
            }
            rows
        }
        _ => return Err("scenario dataset must be a JSON or CSV file".into()),
    };
    if datasets.is_empty() {
        return Err("scenario dataset contains no rows".into());
    }
    Ok(datasets)
}

fn json_dataset_row(value: serde_json::Value) -> Result<BTreeMap<String, String>, String> {
    let serde_json::Value::Object(row) = value else {
        return Err("every JSON scenario dataset row must be an object".into());
    };
    Ok(row
        .into_iter()
        .map(|(name, value)| {
            let value = value.as_str().map(str::to_owned).unwrap_or_else(|| {
                serde_json::to_string(&value).expect("JSON dataset value should serialize")
            });
            (name, value)
        })
        .collect())
}

fn sensitive_name(name: &str) -> bool {
    let normalized = name
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    [
        "authorization",
        "credential",
        "password",
        "passwd",
        "secret",
        "token",
        "apikey",
        "cookie",
    ]
    .iter()
    .any(|candidate| normalized.contains(candidate))
}

fn collect_sensitive_json_values(value: &serde_json::Value, values: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            for (name, value) in object {
                if sensitive_name(name) {
                    collect_json_strings(value, values);
                } else {
                    collect_sensitive_json_values(value, values);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_sensitive_json_values(item, values);
            }
        }
        _ => {}
    }
}

fn collect_json_strings(value: &serde_json::Value, values: &mut Vec<String>) {
    match value {
        serde_json::Value::String(value) => values.push(value.clone()),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_strings(item, values);
            }
        }
        serde_json::Value::Object(object) => {
            for value in object.values() {
                collect_json_strings(value, values);
            }
        }
        _ => {}
    }
}

fn redact_text(value: &str, redactions: &[String]) -> String {
    redactions.iter().fold(value.to_owned(), |value, secret| {
        value.replace(secret, "[REDACTED]")
    })
}

fn validate_mock_profile(profile: &MockProfile) -> Result<(), String> {
    if profile.name.trim().is_empty() {
        return Err("mock profile name cannot be empty".into());
    }
    profile
        .bind_address
        .parse::<std::net::IpAddr>()
        .map_err(|error| format!("invalid mock bind address: {error}"))?;
    for rule in &profile.rules {
        if rule.name.trim().is_empty() {
            return Err("mock rule name cannot be empty".into());
        }
        if !rule.path.starts_with('/') {
            return Err(format!(
                "mock rule `{}` path must start with `/`",
                rule.name
            ));
        }
        if !(100..=599).contains(&rule.response.status) {
            return Err(format!("mock rule `{}` has an invalid status", rule.name));
        }
    }
    Ok(())
}

fn active_environment_setting(project_id: EntityId) -> String {
    format!("{ACTIVE_ENVIRONMENT_SETTING}.{project_id}")
}

fn document_tabs_setting(project_id: EntityId) -> String {
    format!("{DOCUMENT_TABS_SETTING}.{project_id}")
}

fn load_document_tabs(
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

fn collect_resource_rows(
    parent_id: Option<EntityId>,
    depth: usize,
    pages: &HashMap<Option<EntityId>, ResourcePage>,
    expanded: &HashSet<EntityId>,
    visiting: &mut HashSet<EntityId>,
    rows: &mut Vec<ResourceRow>,
) {
    let Some(page) = pages.get(&parent_id) else {
        return;
    };
    for node in &page.items {
        rows.push(ResourceRow::Node {
            node: node.clone(),
            depth,
        });
        if node.kind == ProjectNodeKind::Folder
            && expanded.contains(&node.id)
            && visiting.insert(node.id)
        {
            collect_resource_rows(
                Some(node.id),
                depth.saturating_add(1),
                pages,
                expanded,
                visiting,
                rows,
            );
            visiting.remove(&node.id);
        }
    }
    if page.items.len() < page.total {
        rows.push(ResourceRow::More { parent_id, depth });
    }
}

fn method_combo(ui: &mut egui::Ui, method: &mut HttpMethod, palette: Palette) {
    egui::ComboBox::from_id_salt("http_method")
        .selected_text(
            RichText::new(method.to_string())
                .strong()
                .color(method_color(*method, palette)),
        )
        .width(82.0)
        .show_ui(ui, |ui| {
            for value in [
                HttpMethod::Get,
                HttpMethod::Post,
                HttpMethod::Put,
                HttpMethod::Patch,
                HttpMethod::Delete,
                HttpMethod::Head,
                HttpMethod::Options,
                HttpMethod::Trace,
                HttpMethod::Connect,
            ] {
                ui.selectable_value(method, value, value.to_string());
            }
        });
}

fn protocol_creation_menu(
    ui: &mut egui::Ui,
    language: Language,
    selected: &mut Option<ProtocolKind>,
) {
    ui.set_min_width(180.0);
    ui.label(match language {
        Language::Chinese => "新建接口",
        Language::English => "New API",
    });
    ui.separator();
    for (kind, label) in [
        (ProtocolKind::Http, "HTTP"),
        (ProtocolKind::GraphQl, "GraphQL"),
        (ProtocolKind::Sse, "SSE"),
        (ProtocolKind::WebSocket, "WebSocket"),
        (ProtocolKind::Grpc, "gRPC"),
    ] {
        if ui.button(label).clicked() {
            *selected = Some(kind);
            ui.close();
        }
    }
}

fn method_color(method: HttpMethod, palette: Palette) -> Color32 {
    match method {
        HttpMethod::Get | HttpMethod::Head => palette.success,
        HttpMethod::Post => palette.accent_text,
        HttpMethod::Put | HttpMethod::Patch => palette.warning,
        HttpMethod::Delete => palette.danger,
        HttpMethod::Options | HttpMethod::Trace | HttpMethod::Connect => palette.info,
    }
}

fn request_row_text(request: &WorkspaceRequest, palette: Palette) -> LayoutJob {
    let mut job = LayoutJob::default();
    let (prefix, color) = match request.alternate_protocol.as_ref() {
        None | Some(ProtocolSpec::Http(_)) => (
            request.draft.method.to_string(),
            method_color(request.draft.method, palette),
        ),
        Some(protocol) => (
            protocol_label(protocol.kind()).to_owned(),
            protocol_color(protocol.kind(), palette),
        ),
    };
    job.append(
        &prefix,
        0.0,
        TextFormat {
            font_id: egui::FontId::new(11.0, egui::FontFamily::Proportional),
            color,
            ..Default::default()
        },
    );
    job.append(
        &format!(
            "  {}{}",
            request.name,
            if request.is_dirty() { "  •" } else { "" }
        ),
        0.0,
        TextFormat {
            font_id: egui::FontId::new(13.0, egui::FontFamily::Proportional),
            color: palette.text,
            ..Default::default()
        },
    );
    job
}

fn scenario_report_view(
    ui: &mut egui::Ui,
    report: &ScenarioReport,
    language: Language,
    palette: Palette,
) {
    let result_color = if report.passed {
        palette.success
    } else {
        palette.danger
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(match (language, report.passed) {
                (Language::Chinese, true) => "执行通过",
                (Language::Chinese, false) => "执行失败",
                (Language::English, true) => "Passed",
                (Language::English, false) => "Failed",
            })
            .strong()
            .color(result_color),
        );
        ui.label(
            RichText::new(format!("{} ms", report.elapsed_ms))
                .small()
                .color(palette.muted),
        );
        ui.label(
            RichText::new(format!(
                "{} {}",
                report.steps.len(),
                match language {
                    Language::Chinese => "个步骤",
                    Language::English => "steps",
                }
            ))
            .small()
            .color(palette.muted),
        );
    });
    for (index, step) in report.steps.iter().enumerate() {
        ui.push_id(("scenario_report_step", index), |ui| {
            let status = step
                .status
                .map(|status| status.to_string())
                .unwrap_or_else(|| "—".into());
            let elapsed = step
                .metrics
                .map(|metrics| format!("{} ms", metrics.elapsed_ms))
                .unwrap_or_else(|| "—".into());
            let title = format!(
                "{}  {}  ·  {}  ·  {}",
                if step.passed { "✓" } else { "✕" },
                step.name,
                status,
                elapsed
            );
            egui::CollapsingHeader::new(RichText::new(title).color(if step.passed {
                palette.success
            } else {
                palette.danger
            }))
            .show(ui, |ui| {
                if let Some(error) = &step.error {
                    ui.colored_label(palette.danger, error);
                }
                for assertion in &step.assertions {
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            if assertion.passed {
                                palette.success
                            } else {
                                palette.danger
                            },
                            if assertion.passed { "✓" } else { "✕" },
                        );
                        ui.label(&assertion.name);
                        if let Some(error) = &assertion.error {
                            ui.label(RichText::new(error).small().color(palette.muted));
                        }
                    });
                }
            });
        });
    }
}

fn scenario_nodes_editor(
    ui: &mut egui::Ui,
    nodes: &mut Vec<ScenarioNode>,
    request_options: &[(EntityId, String)],
    language: Language,
    palette: Palette,
    depth: usize,
) {
    let mut remove = None;
    for (index, node) in nodes.iter_mut().enumerate() {
        ui.push_id(("scenario_node", depth, index), |ui| {
            egui::Frame::new()
                .fill(palette.control)
                .stroke(Stroke::new(1.0, palette.divider))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let label = match node {
                            ScenarioNode::Request { .. } => match language {
                                Language::Chinese => "请求",
                                Language::English => "Request",
                            },
                            ScenarioNode::Group { .. } => match language {
                                Language::Chinese => "分组",
                                Language::English => "Group",
                            },
                            ScenarioNode::If { .. } => "If",
                            ScenarioNode::Loop { .. } => match language {
                                Language::Chinese => "循环",
                                Language::English => "Loop",
                            },
                            ScenarioNode::Delay { .. } => match language {
                                Language::Chinese => "等待",
                                Language::English => "Delay",
                            },
                        };
                        ui.label(RichText::new(label).strong().color(palette.accent_text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_sized(
                                    [26.0, 26.0],
                                    egui::Button::new(theme::icon("x", 12.0)).frame(false),
                                )
                                .on_hover_text(match language {
                                    Language::Chinese => "删除节点",
                                    Language::English => "Delete node",
                                })
                                .clicked()
                            {
                                remove = Some(index);
                            }
                        });
                    });
                    ui.separator();
                    match node {
                        ScenarioNode::Request {
                            case_id,
                            assertions,
                            extractors,
                        } => {
                            let selected = request_options
                                .iter()
                                .find(|(id, _)| id == case_id)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or(match language {
                                    Language::Chinese => "请求已不存在",
                                    Language::English => "Missing request",
                                });
                            ui.horizontal(|ui| {
                                ui.label(match language {
                                    Language::Chinese => "执行请求",
                                    Language::English => "Run request",
                                });
                                egui::ComboBox::from_id_salt("request_case")
                                    .selected_text(selected)
                                    .width(260.0)
                                    .show_ui(ui, |ui| {
                                        for (id, name) in request_options {
                                            ui.selectable_value(case_id, *id, name);
                                        }
                                    });
                            });
                            ui.collapsing(
                                format!(
                                    "{} ({})",
                                    match language {
                                        Language::Chinese => "断言",
                                        Language::English => "Assertions",
                                    },
                                    assertions.len()
                                ),
                                |ui| assertion_rules_editor(ui, assertions, language, palette),
                            );
                            ui.collapsing(
                                format!(
                                    "{} ({})",
                                    match language {
                                        Language::Chinese => "提取变量",
                                        Language::English => "Extract variables",
                                    },
                                    extractors.len()
                                ),
                                |ui| extractor_rules_editor(ui, extractors, language, palette),
                            );
                        }
                        ScenarioNode::Group { name, nodes } => {
                            ui.horizontal(|ui| {
                                ui.label(match language {
                                    Language::Chinese => "分组名称",
                                    Language::English => "Group name",
                                });
                                ui.add(
                                    egui::TextEdit::singleline(name)
                                        .desired_width(ui.available_width()),
                                );
                            });
                            ui.indent("group_nodes", |ui| {
                                scenario_nodes_editor(
                                    ui,
                                    nodes,
                                    request_options,
                                    language,
                                    palette,
                                    depth + 1,
                                );
                            });
                        }
                        ScenarioNode::If {
                            expression,
                            then_nodes,
                            else_nodes,
                        } => {
                            ui.horizontal(|ui| {
                                ui.label(match language {
                                    Language::Chinese => "条件表达式",
                                    Language::English => "Expression",
                                });
                                ui.add(
                                    egui::TextEdit::singleline(expression)
                                        .code_editor()
                                        .desired_width(ui.available_width()),
                                );
                            });
                            ui.label(RichText::new("Then").strong().color(palette.success));
                            ui.indent("then_nodes", |ui| {
                                scenario_nodes_editor(
                                    ui,
                                    then_nodes,
                                    request_options,
                                    language,
                                    palette,
                                    depth + 1,
                                );
                            });
                            ui.label(RichText::new("Else").strong().color(palette.warning));
                            ui.indent("else_nodes", |ui| {
                                scenario_nodes_editor(
                                    ui,
                                    else_nodes,
                                    request_options,
                                    language,
                                    palette,
                                    depth + 1,
                                );
                            });
                        }
                        ScenarioNode::Loop {
                            source,
                            item_name,
                            max_iterations,
                            nodes,
                        } => {
                            ui.horizontal(|ui| {
                                ui.label(match language {
                                    Language::Chinese => "数据源",
                                    Language::English => "Source",
                                });
                                ui.add(
                                    egui::TextEdit::singleline(source)
                                        .code_editor()
                                        .desired_width(220.0),
                                );
                                ui.label(match language {
                                    Language::Chinese => "变量名",
                                    Language::English => "Item",
                                });
                                ui.add(egui::TextEdit::singleline(item_name).desired_width(120.0));
                                ui.label(match language {
                                    Language::Chinese => "上限",
                                    Language::English => "Limit",
                                });
                                ui.add(egui::DragValue::new(max_iterations).range(1..=100_000));
                            });
                            ui.indent("loop_nodes", |ui| {
                                scenario_nodes_editor(
                                    ui,
                                    nodes,
                                    request_options,
                                    language,
                                    palette,
                                    depth + 1,
                                );
                            });
                        }
                        ScenarioNode::Delay { millis } => {
                            ui.horizontal(|ui| {
                                ui.label(match language {
                                    Language::Chinese => "等待时长",
                                    Language::English => "Duration",
                                });
                                ui.add(egui::DragValue::new(millis).range(0..=3_600_000));
                                ui.label("ms");
                            });
                        }
                    }
                });
        });
        ui.add_space(8.0);
    }
    if let Some(index) = remove {
        nodes.remove(index);
    }

    ui.menu_button(
        theme::icon_label(
            "plus",
            match language {
                Language::Chinese => "添加节点",
                Language::English => "Add node",
            },
            12.0,
            palette.text,
        ),
        |ui| {
            let request = ui.add_enabled(
                !request_options.is_empty(),
                egui::Button::new(match language {
                    Language::Chinese => "请求",
                    Language::English => "Request",
                }),
            );
            if request.clicked() {
                nodes.push(ScenarioNode::Request {
                    case_id: request_options[0].0,
                    assertions: Vec::new(),
                    extractors: Vec::new(),
                });
                ui.close();
            }
            if ui
                .button(match language {
                    Language::Chinese => "分组",
                    Language::English => "Group",
                })
                .clicked()
            {
                nodes.push(ScenarioNode::Group {
                    name: match language {
                        Language::Chinese => "新分组",
                        Language::English => "New group",
                    }
                    .into(),
                    nodes: Vec::new(),
                });
                ui.close();
            }
            if ui.button("If").clicked() {
                nodes.push(ScenarioNode::If {
                    expression: String::new(),
                    then_nodes: Vec::new(),
                    else_nodes: Vec::new(),
                });
                ui.close();
            }
            if ui
                .button(match language {
                    Language::Chinese => "循环",
                    Language::English => "Loop",
                })
                .clicked()
            {
                nodes.push(ScenarioNode::Loop {
                    source: String::new(),
                    item_name: "item".into(),
                    max_iterations: 100,
                    nodes: Vec::new(),
                });
                ui.close();
            }
            if ui
                .button(match language {
                    Language::Chinese => "等待",
                    Language::English => "Delay",
                })
                .clicked()
            {
                nodes.push(ScenarioNode::Delay { millis: 100 });
                ui.close();
            }
        },
    );
}

fn assertion_rules_editor(
    ui: &mut egui::Ui,
    rules: &mut Vec<AssertionRule>,
    language: Language,
    palette: Palette,
) {
    let mut remove = None;
    for (index, rule) in rules.iter_mut().enumerate() {
        ui.push_id(("assertion", index), |ui| {
            let current_kind = match rule {
                AssertionRule::Status { .. } => 0,
                AssertionRule::HeaderEquals { .. } => 1,
                AssertionRule::JsonPathEquals { .. } => 2,
                AssertionRule::Schema => 3,
                AssertionRule::DurationBelow { .. } => 4,
                AssertionRule::Script { .. } => 5,
            };
            let mut selected_kind = current_kind;
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("kind")
                    .selected_text(assertion_kind_label(current_kind, language))
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for kind in 0..6 {
                            ui.selectable_value(
                                &mut selected_kind,
                                kind,
                                assertion_kind_label(kind, language),
                            );
                        }
                    });
                if selected_kind == current_kind {
                    match rule {
                        AssertionRule::Status { expected } => {
                            ui.label(match language {
                                Language::Chinese => "期望状态码",
                                Language::English => "Expected status",
                            });
                            ui.add(egui::DragValue::new(expected).range(100..=599));
                        }
                        AssertionRule::HeaderEquals { name, expected } => {
                            ui.add(
                                egui::TextEdit::singleline(name)
                                    .hint_text("Content-Type")
                                    .desired_width(150.0),
                            );
                            ui.add(
                                egui::TextEdit::singleline(expected)
                                    .hint_text(match language {
                                        Language::Chinese => "期望值",
                                        Language::English => "Expected value",
                                    })
                                    .desired_width(220.0),
                            );
                        }
                        AssertionRule::JsonPathEquals { path, expected } => {
                            ui.add(
                                egui::TextEdit::singleline(path)
                                    .hint_text("$.data.id")
                                    .code_editor()
                                    .desired_width(180.0),
                            );
                            ui.add(
                                egui::TextEdit::singleline(expected)
                                    .hint_text(match language {
                                        Language::Chinese => "期望值",
                                        Language::English => "Expected value",
                                    })
                                    .desired_width(220.0),
                            );
                        }
                        AssertionRule::Schema => {
                            ui.label(
                                RichText::new(match language {
                                    Language::Chinese => "校验响应是否符合契约 Schema",
                                    Language::English => "Validate the response against its schema",
                                })
                                .color(palette.muted),
                            );
                        }
                        AssertionRule::DurationBelow { milliseconds } => {
                            ui.label("<");
                            ui.add(egui::DragValue::new(milliseconds).range(0..=3_600_000));
                            ui.label("ms");
                        }
                        AssertionRule::Script { expression } => {
                            ui.add(
                                egui::TextEdit::singleline(expression)
                                    .hint_text("response.status === 200")
                                    .code_editor()
                                    .desired_width(360.0),
                            );
                        }
                    }
                }
                if ui
                    .add_sized(
                        [26.0, 26.0],
                        egui::Button::new(theme::icon("x", 12.0)).frame(false),
                    )
                    .on_hover_text(match language {
                        Language::Chinese => "删除断言",
                        Language::English => "Delete assertion",
                    })
                    .clicked()
                {
                    remove = Some(index);
                }
            });
            if selected_kind != current_kind {
                *rule = new_assertion_rule(selected_kind);
            }
        });
    }
    if let Some(index) = remove {
        rules.remove(index);
    }
    ui.menu_button(
        match language {
            Language::Chinese => "+ 添加断言",
            Language::English => "+ Add assertion",
        },
        |ui| {
            for kind in 0..6 {
                if ui.button(assertion_kind_label(kind, language)).clicked() {
                    rules.push(new_assertion_rule(kind));
                    ui.close();
                }
            }
        },
    );
}

fn assertion_kind_label(kind: usize, language: Language) -> &'static str {
    match (kind, language) {
        (0, Language::Chinese) => "状态码",
        (0, Language::English) => "Status",
        (1, Language::Chinese) => "响应头等于",
        (1, Language::English) => "Header equals",
        (2, Language::Chinese) => "JSONPath 等于",
        (2, Language::English) => "JSONPath equals",
        (3, Language::Chinese) => "Schema 校验",
        (3, Language::English) => "Schema",
        (4, Language::Chinese) => "响应时间",
        (4, Language::English) => "Duration below",
        (5, Language::Chinese) => "脚本表达式",
        (5, Language::English) => "Script expression",
        _ => "Unknown",
    }
}

fn new_assertion_rule(kind: usize) -> AssertionRule {
    match kind {
        0 => AssertionRule::Status { expected: 200 },
        1 => AssertionRule::HeaderEquals {
            name: String::new(),
            expected: String::new(),
        },
        2 => AssertionRule::JsonPathEquals {
            path: "$.".into(),
            expected: String::new(),
        },
        3 => AssertionRule::Schema,
        4 => AssertionRule::DurationBelow {
            milliseconds: 1_000,
        },
        5 => AssertionRule::Script {
            expression: String::new(),
        },
        _ => AssertionRule::Status { expected: 200 },
    }
}

fn extractor_rules_editor(
    ui: &mut egui::Ui,
    rules: &mut Vec<ExtractorRule>,
    language: Language,
    palette: Palette,
) {
    let mut remove = None;
    for (index, rule) in rules.iter_mut().enumerate() {
        ui.push_id(("extractor", index), |ui| {
            let current_kind = match &rule.source {
                VariableSource::JsonPath(_) => 0,
                VariableSource::Header(_) => 1,
                VariableSource::Cookie(_) => 2,
                VariableSource::Regex(_) => 3,
            };
            let mut selected_kind = current_kind;
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut rule.name)
                        .hint_text(match language {
                            Language::Chinese => "变量名",
                            Language::English => "Variable name",
                        })
                        .desired_width(150.0),
                );
                egui::ComboBox::from_id_salt("source_kind")
                    .selected_text(extractor_kind_label(current_kind, language))
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for kind in 0..4 {
                            ui.selectable_value(
                                &mut selected_kind,
                                kind,
                                extractor_kind_label(kind, language),
                            );
                        }
                    });
                if selected_kind == current_kind {
                    let (value, hint) = match &mut rule.source {
                        VariableSource::JsonPath(value) => (value, "$.data.token"),
                        VariableSource::Header(value) => (value, "X-Request-Id"),
                        VariableSource::Cookie(value) => (value, "session_id"),
                        VariableSource::Regex(value) => (value, "token=(.+)"),
                    };
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .hint_text(hint)
                            .code_editor()
                            .desired_width(260.0),
                    );
                }
                if ui
                    .add_sized(
                        [26.0, 26.0],
                        egui::Button::new(theme::icon("x", 12.0)).frame(false),
                    )
                    .on_hover_text(match language {
                        Language::Chinese => "删除提取器",
                        Language::English => "Delete extractor",
                    })
                    .clicked()
                {
                    remove = Some(index);
                }
            });
            if selected_kind != current_kind {
                rule.source = new_variable_source(selected_kind);
            }
        });
    }
    if let Some(index) = remove {
        rules.remove(index);
    }
    ui.menu_button(
        match language {
            Language::Chinese => "+ 添加提取器",
            Language::English => "+ Add extractor",
        },
        |ui| {
            for kind in 0..4 {
                if ui.button(extractor_kind_label(kind, language)).clicked() {
                    rules.push(ExtractorRule {
                        name: String::new(),
                        source: new_variable_source(kind),
                    });
                    ui.close();
                }
            }
        },
    );
    if rules.is_empty() {
        ui.label(
            RichText::new(match language {
                Language::Chinese => "可将响应字段写入后续步骤变量",
                Language::English => "Extract response values for later steps",
            })
            .small()
            .color(palette.muted),
        );
    }
}

fn extractor_kind_label(kind: usize, language: Language) -> &'static str {
    match (kind, language) {
        (0, _) => "JSONPath",
        (1, Language::Chinese) => "响应头",
        (1, Language::English) => "Header",
        (2, Language::Chinese) => "Cookie",
        (2, Language::English) => "Cookie",
        (3, Language::Chinese) => "正则表达式",
        (3, Language::English) => "Regex",
        _ => "Unknown",
    }
}

fn new_variable_source(kind: usize) -> VariableSource {
    match kind {
        0 => VariableSource::JsonPath("$.".into()),
        1 => VariableSource::Header(String::new()),
        2 => VariableSource::Cookie(String::new()),
        3 => VariableSource::Regex(String::new()),
        _ => VariableSource::JsonPath("$.".into()),
    }
}

fn mock_rules_editor(
    ui: &mut egui::Ui,
    rules: &mut Vec<MockRule>,
    language: Language,
    palette: Palette,
    running: bool,
) {
    if running {
        ui.label(
            RichText::new(match language {
                Language::Chinese => "服务运行期间配置已锁定，停止服务后可编辑",
                Language::English => "Configuration is locked while the server is running",
            })
            .color(palette.warning),
        );
    }
    ui.add_enabled_ui(!running, |ui| {
        let mut remove = None;
        for (index, rule) in rules.iter_mut().enumerate() {
            ui.push_id(("mock_rule", rule.id), |ui| {
                let title = format!("{}  {}  {}", rule.method, rule.path, rule.name);
                egui::CollapsingHeader::new(title)
                    .default_open(index == 0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(
                                &mut rule.enabled,
                                match language {
                                    Language::Chinese => "启用",
                                    Language::English => "Enabled",
                                },
                            );
                            ui.label(match language {
                                Language::Chinese => "名称",
                                Language::English => "Name",
                            });
                            ui.add(egui::TextEdit::singleline(&mut rule.name).desired_width(220.0));
                            ui.label(match language {
                                Language::Chinese => "优先级",
                                Language::English => "Priority",
                            });
                            ui.add(egui::DragValue::new(&mut rule.priority));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [26.0, 26.0],
                                            egui::Button::new(theme::icon("x", 12.0)).frame(false),
                                        )
                                        .on_hover_text(match language {
                                            Language::Chinese => "删除规则",
                                            Language::English => "Delete rule",
                                        })
                                        .clicked()
                                    {
                                        remove = Some(index);
                                    }
                                },
                            );
                        });
                        ui.horizontal(|ui| {
                            method_combo(ui, &mut rule.method, palette);
                            ui.add(
                                egui::TextEdit::singleline(&mut rule.path)
                                    .hint_text("/users/{id}")
                                    .desired_width(ui.available_width()),
                            );
                        });
                        ui.collapsing(
                            match language {
                                Language::Chinese => "请求匹配条件",
                                Language::English => "Request matching",
                            },
                            |ui| {
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "查询参数",
                                        Language::English => "Query parameters",
                                    })
                                    .strong(),
                                );
                                mock_key_value_editor(ui, &mut rule.query, language, palette);
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "请求头",
                                        Language::English => "Request headers",
                                    })
                                    .strong(),
                                );
                                mock_key_value_editor(ui, &mut rule.headers, language, palette);
                                let mut path_variables = rule
                                    .path_variables
                                    .iter()
                                    .map(|(name, value)| (name.clone(), value.clone()))
                                    .collect::<Vec<_>>();
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "路径变量",
                                        Language::English => "Path variables",
                                    })
                                    .strong(),
                                );
                                string_pair_editor(ui, &mut path_variables, language, palette);
                                rule.path_variables = path_variables.into_iter().collect();
                                let mut body_contains =
                                    rule.body_contains.clone().unwrap_or_default();
                                ui.horizontal(|ui| {
                                    ui.label(match language {
                                        Language::Chinese => "正文包含",
                                        Language::English => "Body contains",
                                    });
                                    ui.add(
                                        egui::TextEdit::singleline(&mut body_contains)
                                            .desired_width(ui.available_width()),
                                    );
                                });
                                rule.body_contains =
                                    (!body_contains.is_empty()).then_some(body_contains);
                            },
                        );
                        ui.collapsing(
                            match language {
                                Language::Chinese => "响应配置",
                                Language::English => "Response",
                            },
                            |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(match language {
                                        Language::Chinese => "状态码",
                                        Language::English => "Status",
                                    });
                                    ui.add(
                                        egui::DragValue::new(&mut rule.response.status)
                                            .range(100..=599),
                                    );
                                    ui.label(match language {
                                        Language::Chinese => "延迟",
                                        Language::English => "Delay",
                                    });
                                    ui.add(
                                        egui::DragValue::new(&mut rule.response.delay_ms)
                                            .range(0..=3_600_000),
                                    );
                                    ui.label("ms");
                                });
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "响应头",
                                        Language::English => "Response headers",
                                    })
                                    .strong(),
                                );
                                mock_key_value_editor(
                                    ui,
                                    &mut rule.response.headers,
                                    language,
                                    palette,
                                );
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "响应正文",
                                        Language::English => "Response body",
                                    })
                                    .strong(),
                                );
                                ui.add(
                                    egui::TextEdit::multiline(&mut rule.response.body)
                                        .code_editor()
                                        .desired_rows(8)
                                        .desired_width(f32::INFINITY),
                                );
                                ui.label(
                                    RichText::new(match language {
                                        Language::Chinese => "动态脚本",
                                        Language::English => "Dynamic script",
                                    })
                                    .strong(),
                                );
                                ui.add(
                                    egui::TextEdit::multiline(&mut rule.response.script)
                                        .code_editor()
                                        .desired_rows(5)
                                        .hint_text(match language {
                                            Language::Chinese => "可选响应脚本",
                                            Language::English => "Optional response script",
                                        })
                                        .desired_width(f32::INFINITY),
                                );
                            },
                        );
                    });
            });
            ui.separator();
        }
        if let Some(index) = remove {
            rules.remove(index);
        }
        if ui
            .button(theme::icon_label(
                "plus",
                match language {
                    Language::Chinese => "添加 Mock 规则",
                    Language::English => "Add mock rule",
                },
                12.0,
                palette.text,
            ))
            .clicked()
        {
            let rule = MockRule {
                name: match language {
                    Language::Chinese => format!("Mock 规则 {}", rules.len() + 1),
                    Language::English => format!("Mock rule {}", rules.len() + 1),
                },
                ..MockRule::default()
            };
            rules.push(rule);
        }
    });
}

fn mock_key_value_editor(
    ui: &mut egui::Ui,
    values: &mut Vec<KeyValue>,
    language: Language,
    palette: Palette,
) {
    let mut remove = None;
    for (index, value) in values.iter_mut().enumerate() {
        ui.push_id(("mock_pair", index), |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut value.enabled, "");
                ui.add(
                    egui::TextEdit::singleline(&mut value.name)
                        .hint_text(match language {
                            Language::Chinese => "名称",
                            Language::English => "Name",
                        })
                        .desired_width(180.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut value.value)
                        .hint_text(match language {
                            Language::Chinese => "值",
                            Language::English => "Value",
                        })
                        .desired_width(260.0),
                );
                if ui
                    .add_sized(
                        [26.0, 26.0],
                        egui::Button::new(theme::icon("x", 12.0)).frame(false),
                    )
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        values.remove(index);
    }
    if ui
        .small_button(match language {
            Language::Chinese => "+ 添加",
            Language::English => "+ Add",
        })
        .clicked()
    {
        values.push(KeyValue::enabled("", ""));
    }
    if values.is_empty() {
        ui.label(RichText::new("—").color(palette.muted));
    }
}

fn string_pair_editor(
    ui: &mut egui::Ui,
    values: &mut Vec<(String, String)>,
    language: Language,
    palette: Palette,
) {
    let mut remove = None;
    for (index, (name, value)) in values.iter_mut().enumerate() {
        ui.push_id(("string_pair", index), |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(name)
                        .hint_text(match language {
                            Language::Chinese => "变量名",
                            Language::English => "Variable",
                        })
                        .desired_width(180.0),
                );
                ui.add(
                    egui::TextEdit::singleline(value)
                        .hint_text(match language {
                            Language::Chinese => "匹配值",
                            Language::English => "Expected value",
                        })
                        .desired_width(260.0),
                );
                if ui
                    .add_sized(
                        [26.0, 26.0],
                        egui::Button::new(theme::icon("x", 12.0)).frame(false),
                    )
                    .clicked()
                {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        values.remove(index);
    }
    if ui
        .small_button(match language {
            Language::Chinese => "+ 添加",
            Language::English => "+ Add",
        })
        .clicked()
    {
        values.push((String::new(), String::new()));
    }
    if values.is_empty() {
        ui.label(RichText::new("—").color(palette.muted));
    }
}

fn protocol_editor(
    ui: &mut egui::Ui,
    protocol: &mut ProtocolSpec,
    language: Language,
    palette: Palette,
) {
    match protocol {
        ProtocolSpec::GraphQl(spec) => {
            form_field(ui, language, "地址", "Endpoint", |ui| {
                ui.add_sized(
                    [ui.available_width(), 32.0],
                    egui::TextEdit::singleline(&mut spec.endpoint)
                        .hint_text("https://api.example.com/graphql"),
                );
            });
            let mut operation_name = spec.operation_name.clone().unwrap_or_default();
            form_field(ui, language, "操作名称", "Operation", |ui| {
                ui.add_sized(
                    [360.0, 32.0],
                    egui::TextEdit::singleline(&mut operation_name),
                );
            });
            spec.operation_name = (!operation_name.trim().is_empty()).then_some(operation_name);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Query").strong());
                ui.separator();
                ui.label(RichText::new("Variables (JSON)").strong());
            });
            ui.columns(2, |columns| {
                columns[0].add_sized(
                    columns[0].available_size(),
                    egui::TextEdit::multiline(&mut spec.query)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
                columns[1].add_sized(
                    columns[1].available_size(),
                    egui::TextEdit::multiline(&mut spec.variables)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
        }
        ProtocolSpec::Sse(spec) => {
            ui.horizontal(|ui| {
                method_combo(ui, &mut spec.request.method, palette);
                ui.add_sized(
                    [ui.available_width() - 150.0, 34.0],
                    egui::TextEdit::singleline(&mut spec.request.url)
                        .hint_text("https://api.example.com/events"),
                );
                ui.checkbox(
                    &mut spec.reconnect,
                    match language {
                        Language::Chinese => "自动重连",
                        Language::English => "Reconnect",
                    },
                );
            });
            ui.label(
                RichText::new(match language {
                    Language::Chinese => "请求头",
                    Language::English => "Headers",
                })
                .strong(),
            );
            key_value_editor(ui, &mut spec.request.headers, language, palette);
        }
        ProtocolSpec::WebSocket(spec) => {
            form_field(ui, language, "地址", "URL", |ui| {
                ui.add_sized(
                    [ui.available_width(), 32.0],
                    egui::TextEdit::singleline(&mut spec.url)
                        .hint_text("wss://api.example.com/socket"),
                );
            });
            let mut subprotocols = spec.subprotocols.join(", ");
            form_field(ui, language, "子协议", "Subprotocols", |ui| {
                ui.add_sized(
                    [ui.available_width(), 32.0],
                    egui::TextEdit::singleline(&mut subprotocols)
                        .hint_text("graphql-transport-ws, chat"),
                );
            });
            spec.subprotocols = subprotocols
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect();
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut spec.validate_tls,
                    match language {
                        Language::Chinese => "校验 TLS 证书",
                        Language::English => "Validate TLS",
                    },
                );
                ui.label(match language {
                    Language::Chinese => "连接超时",
                    Language::English => "Connect timeout",
                });
                ui.add(
                    egui::DragValue::new(&mut spec.connect_timeout_ms)
                        .range(1..=600_000)
                        .suffix(" ms"),
                );
            });
            ui.label(
                RichText::new(match language {
                    Language::Chinese => "请求头",
                    Language::English => "Headers",
                })
                .strong(),
            );
            key_value_editor(ui, &mut spec.headers, language, palette);
        }
        ProtocolSpec::Grpc(spec) => {
            form_field(ui, language, "地址", "Endpoint", |ui| {
                ui.add_sized(
                    [ui.available_width(), 32.0],
                    egui::TextEdit::singleline(&mut spec.endpoint)
                        .hint_text("http://127.0.0.1:50051"),
                );
            });
            ui.horizontal(|ui| {
                ui.label(match language {
                    Language::Chinese => "服务",
                    Language::English => "Service",
                });
                ui.add(
                    egui::TextEdit::singleline(&mut spec.service)
                        .hint_text("package.Greeter")
                        .desired_width(240.0),
                );
                ui.label(match language {
                    Language::Chinese => "方法",
                    Language::English => "Method",
                });
                ui.add(
                    egui::TextEdit::singleline(&mut spec.method)
                        .hint_text("SayHello")
                        .desired_width(180.0),
                );
                egui::ComboBox::from_id_salt("grpc_call_kind")
                    .selected_text(grpc_call_kind_label(spec.call_kind))
                    .show_ui(ui, |ui| {
                        for kind in [
                            GrpcCallKind::Unary,
                            GrpcCallKind::ServerStreaming,
                            GrpcCallKind::ClientStreaming,
                            GrpcCallKind::BidirectionalStreaming,
                        ] {
                            ui.selectable_value(
                                &mut spec.call_kind,
                                kind,
                                grpc_call_kind_label(kind),
                            );
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut spec.use_reflection,
                    match language {
                        Language::Chinese => "服务端反射",
                        Language::English => "Server reflection",
                    },
                );
                if ui
                    .button(match language {
                        Language::Chinese => "选择 proto",
                        Language::English => "Choose proto",
                    })
                    .clicked()
                    && let Some(files) = rfd::FileDialog::new()
                        .add_filter("Protocol Buffers", &["proto"])
                        .pick_files()
                {
                    spec.proto_files = files;
                    spec.use_reflection = false;
                }
                if ui
                    .button(match language {
                        Language::Chinese => "选择 descriptor",
                        Language::English => "Choose descriptor",
                    })
                    .clicked()
                    && let Some(file) = rfd::FileDialog::new().pick_file()
                {
                    spec.descriptor_set = Some(file);
                    spec.use_reflection = false;
                }
                let source = if spec.use_reflection {
                    match language {
                        Language::Chinese => "使用反射发现服务".into(),
                        Language::English => "Discovering through reflection".into(),
                    }
                } else if let Some(path) = &spec.descriptor_set {
                    path.display().to_string()
                } else {
                    spec.proto_files
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                ui.label(RichText::new(source).color(palette.muted));
            });
            ui.label(
                RichText::new(match language {
                    Language::Chinese => "请求消息 JSON",
                    Language::English => "Request message JSON",
                })
                .strong(),
            );
            ui.add_sized(
                ui.available_size(),
                egui::TextEdit::multiline(&mut spec.message_json)
                    .code_editor()
                    .desired_width(f32::INFINITY),
            );
        }
        ProtocolSpec::Http(_) => {}
    }
}

fn key_value_editor(
    ui: &mut egui::Ui,
    values: &mut Vec<KeyValue>,
    language: Language,
    palette: Palette,
) {
    if values.last().is_none_or(|value| {
        !value.name.is_empty() || !value.value.is_empty() || !value.description.is_empty()
    }) {
        values.push(KeyValue {
            name: String::new(),
            value: String::new(),
            enabled: false,
            description: String::new(),
        });
    }
    let mut remove = None;
    TableBuilder::new(ui)
        .column(Column::exact(28.0))
        .column(Column::initial(180.0).resizable(true))
        .column(Column::remainder())
        .column(Column::exact(32.0))
        .header(26.0, |mut header| {
            header.col(|_| {});
            header.col(|ui| {
                ui.label(
                    RichText::new(match language {
                        Language::Chinese => "名称",
                        Language::English => "Name",
                    })
                    .color(palette.muted),
                );
            });
            header.col(|ui| {
                ui.label(
                    RichText::new(match language {
                        Language::Chinese => "值",
                        Language::English => "Value",
                    })
                    .color(palette.muted),
                );
            });
            header.col(|_| {});
        })
        .body(|mut body| {
            for (index, value) in values.iter_mut().enumerate() {
                body.row(34.0, |mut row| {
                    row.col(|ui| {
                        ui.checkbox(&mut value.enabled, "");
                    });
                    row.col(|ui| {
                        if ui.text_edit_singleline(&mut value.name).changed()
                            && !value.name.is_empty()
                        {
                            value.enabled = true;
                        }
                    });
                    row.col(|ui| {
                        if ui.text_edit_singleline(&mut value.value).changed()
                            && !value.value.is_empty()
                        {
                            value.enabled = true;
                        }
                    });
                    row.col(|ui| {
                        if (!value.name.is_empty() || !value.value.is_empty())
                            && ui
                                .add_sized(
                                    [26.0, 26.0],
                                    egui::Button::new(theme::icon("x", 12.0)).frame(false),
                                )
                                .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                });
            }
        });
    if let Some(index) = remove {
        values.remove(index);
    }
}

fn grpc_call_kind_label(kind: GrpcCallKind) -> &'static str {
    match kind {
        GrpcCallKind::Unary => "Unary",
        GrpcCallKind::ServerStreaming => "Server stream",
        GrpcCallKind::ClientStreaming => "Client stream",
        GrpcCallKind::BidirectionalStreaming => "Bidirectional",
    }
}

fn protocol_label(kind: ProtocolKind) -> &'static str {
    match kind {
        ProtocolKind::Http => "HTTP",
        ProtocolKind::GraphQl => "GQL",
        ProtocolKind::Sse => "SSE",
        ProtocolKind::WebSocket => "WS",
        ProtocolKind::Grpc => "gRPC",
    }
}

fn protocol_color(kind: ProtocolKind, palette: Palette) -> Color32 {
    match kind {
        ProtocolKind::Http => palette.info,
        ProtocolKind::GraphQl => Color32::from_rgb(225, 55, 143),
        ProtocolKind::Sse => palette.success,
        ProtocolKind::WebSocket => palette.warning,
        ProtocolKind::Grpc => palette.accent_text,
    }
}

fn editor_tabs(
    ui: &mut egui::Ui,
    selected: &mut EditorTab,
    language: Language,
    draft: &RequestDraft,
    palette: Palette,
) {
    ui.horizontal(|ui| {
        let params = draft.query.iter().filter(|pair| !pair.is_empty()).count();
        let headers = draft.headers.iter().filter(|pair| !pair.is_empty()).count();
        let cookies = draft.cookies.iter().filter(|pair| !pair.is_empty()).count();
        for (tab, chinese, english, count) in [
            (EditorTab::Params, "参数", "Params", Some(params)),
            (EditorTab::Headers, "请求头", "Headers", Some(headers)),
            (EditorTab::Cookies, "Cookie", "Cookies", Some(cookies)),
            (EditorTab::Body, "请求体", "Body", None),
            (EditorTab::Auth, "认证", "Auth", None),
        ] {
            let label = match language {
                Language::Chinese => chinese,
                Language::English => english,
            };
            let label = count
                .filter(|count| *count > 0)
                .map(|count| format!("{label} {count}"))
                .unwrap_or_else(|| label.to_owned());
            let response = ui.add(
                egui::Button::new(RichText::new(label).color(if *selected == tab {
                    palette.accent_text
                } else {
                    palette.muted
                }))
                .frame(false),
            );
            if *selected == tab {
                ui.painter().line_segment(
                    [response.rect.left_bottom(), response.rect.right_bottom()],
                    Stroke::new(2.0, palette.primary),
                );
            }
            if response.clicked() {
                *selected = tab;
            }
        }
    });
}

fn tab_button(ui: &mut egui::Ui, selected: bool, label: &str, palette: Palette) -> egui::Response {
    let response = ui.add(
        egui::Button::new(RichText::new(label).color(if selected {
            palette.accent_text
        } else {
            palette.muted
        }))
        .frame(false),
    );
    if selected {
        ui.painter().line_segment(
            [response.rect.left_bottom(), response.rect.right_bottom()],
            Stroke::new(2.0, palette.primary),
        );
    }
    response
}

fn editable_pairs(
    ui: &mut egui::Ui,
    pairs: &mut Vec<EditablePair>,
    language: Language,
    palette: Palette,
    description: bool,
) {
    let labels = match language {
        Language::Chinese => ("名称", "值", "说明"),
        Language::English => ("Key", "Value", "Description"),
    };
    let mut remove = None;
    let mut table = TableBuilder::new(ui)
        .striped(false)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(28.0))
        .column(Column::initial(170.0).at_least(140.0).resizable(true))
        .column(Column::initial(230.0).at_least(180.0).resizable(true));
    if description {
        table = table.column(Column::remainder().at_least(120.0));
    }
    table
        .column(Column::exact(32.0))
        .header(28.0, |mut header| {
            header.col(|_| {});
            header.col(|ui| {
                ui.label(RichText::new(labels.0).color(palette.muted));
            });
            header.col(|ui| {
                ui.label(RichText::new(labels.1).color(palette.muted));
            });
            if description {
                header.col(|ui| {
                    ui.label(RichText::new(labels.2).color(palette.muted));
                });
            }
            header.col(|_| {});
        })
        .body(|mut body| {
            for (index, pair) in pairs.iter_mut().enumerate() {
                body.row(36.0, |mut row| {
                    row.col(|ui| {
                        ui.checkbox(&mut pair.enabled, "");
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut pair.name).desired_width(f32::INFINITY),
                        );
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut pair.value)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    if description {
                        row.col(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut pair.description)
                                    .desired_width(f32::INFINITY),
                            );
                        });
                    }
                    row.col(|ui| {
                        if !pair.is_empty()
                            && ui
                                .add_sized(
                                    [28.0, 28.0],
                                    egui::Button::new(theme::icon("x", 13.0)).frame(false),
                                )
                                .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                });
            }
        });
    if let Some(index) = remove {
        pairs.remove(index);
    }
}

fn body_editor(
    ui: &mut egui::Ui,
    draft: &mut RequestDraft,
    language: Language,
    palette: Palette,
) -> Option<String> {
    let mut error = None;
    ui.horizontal_wrapped(|ui| {
        for (mode, label) in [
            (BodyMode::None, "none"),
            (BodyMode::Json, "JSON"),
            (BodyMode::Text, "Text"),
            (BodyMode::Xml, "XML"),
            (BodyMode::FormUrlEncoded, "x-www-form-urlencoded"),
            (BodyMode::Multipart, "multipart"),
            (BodyMode::Binary, "binary"),
        ] {
            ui.selectable_value(&mut draft.body_mode, mode, label);
        }
        if draft.body_mode == BodyMode::Json
            && ui
                .button(theme::icon_label(
                    "braces",
                    match language {
                        Language::Chinese => "格式化",
                        Language::English => "Format",
                    },
                    12.0,
                    palette.text,
                ))
                .clicked()
        {
            match serde_json::from_str::<serde_json::Value>(&draft.body)
                .and_then(|value| serde_json::to_string_pretty(&value))
            {
                Ok(pretty) => draft.body = pretty,
                Err(value) => {
                    error = Some(match language {
                        Language::Chinese => format!("JSON 格式错误：{value}"),
                        Language::English => format!("Invalid JSON: {value}"),
                    })
                }
            }
        }
    });
    ui.add_space(4.0);
    match draft.body_mode {
        BodyMode::None => empty_state(
            ui,
            match language {
                Language::Chinese => "无请求体",
                Language::English => "No body",
            },
            "",
            palette,
        ),
        BodyMode::Json | BodyMode::Text | BodyMode::Xml => {
            egui::ScrollArea::both().show(ui, |ui| {
                ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::multiline(&mut draft.body)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
        }
        BodyMode::FormUrlEncoded => {
            editable_pairs(ui, &mut draft.form_fields, language, palette, false)
        }
        BodyMode::Multipart => multipart_table(ui, &mut draft.multipart_parts, language, palette),
        BodyMode::Binary => {
            ui.horizontal(|ui| {
                let path = draft
                    .binary_file
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| match language {
                        Language::Chinese => "未选择文件".into(),
                        Language::English => "No file selected".into(),
                    });
                ui.add_sized(
                    [ui.available_width() - 106.0, 32.0],
                    egui::Label::new(RichText::new(path).monospace().color(palette.muted))
                        .truncate(),
                );
                if ui
                    .button(theme::icon_label(
                        "folder-open",
                        match language {
                            Language::Chinese => "选择",
                            Language::English => "Choose",
                        },
                        12.0,
                        palette.text,
                    ))
                    .clicked()
                    && let Some(path) = rfd::FileDialog::new().pick_file()
                {
                    draft.binary_file = Some(path);
                }
            });
        }
    }
    error
}

fn multipart_table(
    ui: &mut egui::Ui,
    parts: &mut Vec<EditableMultipartPart>,
    language: Language,
    palette: Palette,
) {
    let labels = match language {
        Language::Chinese => ("名称", "类型", "值 / 文件", "Content-Type", "选择"),
        Language::English => ("Key", "Type", "Value / File", "Content-Type", "Choose"),
    };
    let mut remove = None;
    TableBuilder::new(ui)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(28.0))
        .column(Column::initial(150.0).at_least(120.0).resizable(true))
        .column(Column::exact(82.0))
        .column(Column::remainder().at_least(170.0))
        .column(Column::initial(130.0).at_least(110.0).resizable(true))
        .column(Column::exact(32.0))
        .header(28.0, |mut header| {
            header.col(|_| {});
            for label in [labels.0, labels.1, labels.2, labels.3] {
                header.col(|ui| {
                    ui.label(RichText::new(label).color(palette.muted));
                });
            }
            header.col(|_| {});
        })
        .body(|mut body| {
            for (index, part) in parts.iter_mut().enumerate() {
                body.row(36.0, |mut row| {
                    row.col(|ui| {
                        ui.checkbox(&mut part.enabled, "");
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut part.name).desired_width(f32::INFINITY),
                        );
                    });
                    row.col(|ui| {
                        egui::ComboBox::from_id_salt(("multipart_mode", index))
                            .selected_text(match part.mode {
                                MultipartValueMode::Text => "Text",
                                MultipartValueMode::File => "File",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut part.mode,
                                    MultipartValueMode::Text,
                                    "Text",
                                );
                                ui.selectable_value(
                                    &mut part.mode,
                                    MultipartValueMode::File,
                                    "File",
                                );
                            });
                    });
                    row.col(|ui| match part.mode {
                        MultipartValueMode::Text => {
                            ui.add(
                                egui::TextEdit::singleline(&mut part.value)
                                    .desired_width(f32::INFINITY),
                            );
                        }
                        MultipartValueMode::File => {
                            let text = part
                                .file
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| labels.4.into());
                            if ui.button(text).clicked()
                                && let Some(path) = rfd::FileDialog::new().pick_file()
                            {
                                part.file = Some(path);
                            }
                        }
                    });
                    row.col(|ui| {
                        ui.add_enabled(
                            part.mode == MultipartValueMode::File,
                            egui::TextEdit::singleline(&mut part.content_type)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    row.col(|ui| {
                        if !part.is_empty()
                            && ui
                                .add_sized(
                                    [28.0, 28.0],
                                    egui::Button::new(theme::icon("x", 13.0)).frame(false),
                                )
                                .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                });
            }
        });
    if let Some(index) = remove {
        parts.remove(index);
    }
}

fn auth_editor(
    ui: &mut egui::Ui,
    auth: &mut AuthDraft,
    request_id: EntityId,
    secrets: Arc<dyn SecretStore>,
    language: Language,
    palette: Palette,
) -> Option<String> {
    let mut mode = auth.mode();
    let previous = mode;
    ui.horizontal_wrapped(|ui| {
        for (value, chinese, english) in [
            (AuthMode::None, "无认证", "None"),
            (AuthMode::Basic, "Basic", "Basic"),
            (AuthMode::Bearer, "Bearer", "Bearer"),
            (AuthMode::ApiKey, "API Key", "API Key"),
            (AuthMode::OAuth2, "OAuth 2.0", "OAuth 2.0"),
            (AuthMode::Digest, "Digest", "Digest"),
            (AuthMode::AwsSigV4, "AWS SigV4", "AWS SigV4"),
        ] {
            ui.selectable_value(
                &mut mode,
                value,
                match language {
                    Language::Chinese => chinese,
                    Language::English => english,
                },
            );
        }
    });
    let mut error = None;
    if mode != previous {
        *auth = AuthDraft::for_mode(mode, request_id);
        for secret in auth.secrets_mut() {
            match secrets.get(&secret.reference) {
                Ok(Some(_)) => secret.configured = true,
                Ok(None) => {}
                Err(value) => error = Some(value.to_string()),
            }
        }
    }
    ui.add_space(8.0);
    match auth {
        AuthDraft::None => empty_state(
            ui,
            match language {
                Language::Chinese => "无认证",
                Language::English => "No authentication",
            },
            "",
            palette,
        ),
        AuthDraft::Basic { username, password } => {
            form_field(ui, language, "用户名", "Username", |ui| {
                ui.add_sized([360.0, 32.0], egui::TextEdit::singleline(username));
            });
            form_field(ui, language, "密码", "Password", |ui| {
                ui.add_sized(
                    [360.0, 32.0],
                    egui::TextEdit::singleline(&mut password.replacement)
                        .password(true)
                        .hint_text(if password.configured {
                            "••••••••"
                        } else {
                            ""
                        }),
                );
            });
        }
        AuthDraft::Bearer { token } => {
            form_field(ui, language, "Token", "Token", |ui| {
                ui.add_sized(
                    [520.0, 32.0],
                    egui::TextEdit::singleline(&mut token.replacement)
                        .password(true)
                        .hint_text(if token.configured {
                            "••••••••"
                        } else {
                            ""
                        }),
                );
            });
        }
        AuthDraft::ApiKey {
            name,
            value,
            location,
        } => {
            form_field(ui, language, "名称", "Name", |ui| {
                ui.add_sized([360.0, 32.0], egui::TextEdit::singleline(name));
            });
            form_field(ui, language, "值", "Value", |ui| {
                ui.add_sized(
                    [360.0, 32.0],
                    egui::TextEdit::singleline(&mut value.replacement)
                        .password(true)
                        .hint_text(if value.configured {
                            "••••••••"
                        } else {
                            ""
                        }),
                );
            });
            form_field(ui, language, "位置", "Location", |ui| {
                ui.selectable_value(location, ApiKeyLocation::Header, "Header");
                ui.selectable_value(location, ApiKeyLocation::Query, "Query");
            });
        }
        AuthDraft::OAuth2 {
            grant,
            authorization_url,
            token_url,
            client_id,
            client_secret,
            scopes,
            username,
            password,
            access_token,
        } => {
            form_field(ui, language, "授权类型", "Grant type", |ui| {
                egui::ComboBox::from_id_salt("oauth_grant")
                    .selected_text(match grant {
                        OAuth2Grant::ClientCredentials => "Client Credentials",
                        OAuth2Grant::Password => "Password",
                        OAuth2Grant::AuthorizationCodePkce => "Authorization Code + PKCE",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            grant,
                            OAuth2Grant::ClientCredentials,
                            "Client Credentials",
                        );
                        ui.selectable_value(grant, OAuth2Grant::Password, "Password");
                        ui.selectable_value(
                            grant,
                            OAuth2Grant::AuthorizationCodePkce,
                            "Authorization Code + PKCE",
                        );
                    });
            });
            if *grant == OAuth2Grant::AuthorizationCodePkce {
                form_field(ui, language, "授权地址", "Authorization URL", |ui| {
                    ui.add_sized(
                        [520.0, 32.0],
                        egui::TextEdit::singleline(authorization_url)
                            .hint_text("https://identity.example.com/authorize"),
                    );
                });
            }
            form_field(ui, language, "Token 地址", "Token URL", |ui| {
                ui.add_sized(
                    [520.0, 32.0],
                    egui::TextEdit::singleline(token_url)
                        .hint_text("https://identity.example.com/oauth/token"),
                );
            });
            form_field(ui, language, "Client ID", "Client ID", |ui| {
                ui.add_sized([360.0, 32.0], egui::TextEdit::singleline(client_id));
            });
            secret_form_field(
                ui,
                language,
                "Client 密钥",
                "Client secret",
                client_secret,
                360.0,
            );
            form_field(ui, language, "权限范围", "Scopes", |ui| {
                ui.add_sized(
                    [520.0, 32.0],
                    egui::TextEdit::singleline(scopes).hint_text("read write"),
                );
            });
            if *grant == OAuth2Grant::Password {
                form_field(ui, language, "用户名", "Username", |ui| {
                    ui.add_sized([360.0, 32.0], egui::TextEdit::singleline(username));
                });
                secret_form_field(ui, language, "密码", "Password", password, 360.0);
            }
            secret_form_field(
                ui,
                language,
                "访问令牌",
                "Access token",
                access_token,
                520.0,
            );
            ui.label(
                RichText::new(match language {
                    Language::Chinese => "访问令牌可选；配置后将跳过 Token 请求",
                    Language::English => {
                        "An access token is optional and bypasses the token request"
                    }
                })
                .small()
                .color(palette.muted),
            );
        }
        AuthDraft::Digest { username, password } => {
            form_field(ui, language, "用户名", "Username", |ui| {
                ui.add_sized([360.0, 32.0], egui::TextEdit::singleline(username));
            });
            secret_form_field(ui, language, "密码", "Password", password, 360.0);
        }
        AuthDraft::AwsSigV4 {
            access_key,
            secret_key,
            session_token,
            region,
            service,
        } => {
            secret_form_field(ui, language, "Access Key", "Access key", access_key, 360.0);
            secret_form_field(ui, language, "Secret Key", "Secret key", secret_key, 360.0);
            secret_form_field(
                ui,
                language,
                "会话令牌",
                "Session token",
                session_token,
                520.0,
            );
            form_field(ui, language, "区域", "Region", |ui| {
                ui.add_sized(
                    [240.0, 32.0],
                    egui::TextEdit::singleline(region).hint_text("us-east-1"),
                );
            });
            form_field(ui, language, "服务", "Service", |ui| {
                ui.add_sized(
                    [240.0, 32.0],
                    egui::TextEdit::singleline(service).hint_text("execute-api"),
                );
            });
        }
        AuthDraft::Unsupported(_) => {
            ui.colored_label(
                palette.warning,
                match language {
                    Language::Chinese => "该认证类型保持原配置，但当前不可编辑",
                    Language::English => "This authentication type is preserved but not editable",
                },
            );
        }
    }
    error
}

fn secret_form_field(
    ui: &mut egui::Ui,
    language: Language,
    chinese: &str,
    english: &str,
    secret: &mut crate::draft::SecretDraft,
    width: f32,
) {
    form_field(ui, language, chinese, english, |ui| {
        ui.add_sized(
            [width, 32.0],
            egui::TextEdit::singleline(&mut secret.replacement)
                .password(true)
                .hint_text(if secret.configured {
                    "••••••••"
                } else {
                    ""
                }),
        );
    });
}

fn form_field(
    ui: &mut egui::Ui,
    language: Language,
    chinese: &str,
    english: &str,
    add: impl FnOnce(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [88.0, 32.0],
            egui::Label::new(match language {
                Language::Chinese => chinese,
                Language::English => english,
            }),
        );
        add(ui);
    });
}

fn variable_table(
    ui: &mut egui::Ui,
    environment: &mut EnvironmentDraft,
    secrets: Arc<dyn SecretStore>,
    language: Language,
    palette: Palette,
) -> Option<String> {
    let labels = match language {
        Language::Chinese => ("名称", "值", "类型", "普通", "密钥"),
        Language::English => ("Name", "Value", "Type", "Plain", "Secret"),
    };
    let environment_id = environment.id();
    let mut remove = None;
    let mut error = None;
    TableBuilder::new(ui)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(28.0))
        .column(Column::initial(190.0).at_least(150.0).resizable(true))
        .column(Column::remainder().at_least(220.0))
        .column(Column::exact(104.0))
        .column(Column::exact(32.0))
        .header(28.0, |mut header| {
            header.col(|_| {});
            for label in [labels.0, labels.1, labels.2] {
                header.col(|ui| {
                    ui.label(RichText::new(label).color(palette.muted));
                });
            }
            header.col(|_| {});
        })
        .body(|mut body| {
            for (index, variable) in environment.variables.iter_mut().enumerate() {
                body.row(36.0, |mut row| {
                    row.col(|ui| {
                        ui.checkbox(&mut variable.enabled, "");
                    });
                    row.col(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut variable.name)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    row.col(|ui| {
                        let mut edit = egui::TextEdit::singleline(&mut variable.value)
                            .desired_width(f32::INFINITY);
                        if variable.secret {
                            edit = edit.password(true).hint_text(if variable.configured {
                                "••••••••"
                            } else {
                                ""
                            });
                        }
                        ui.add(edit);
                    });
                    row.col(|ui| {
                        let previous = variable.secret;
                        egui::ComboBox::from_id_salt(("variable_type", variable.id.to_string()))
                            .selected_text(if variable.secret { labels.4 } else { labels.3 })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut variable.secret, false, labels.3);
                                ui.selectable_value(&mut variable.secret, true, labels.4);
                            });
                        if variable.secret && !previous {
                            let reference = variable.ensure_reference(environment_id).clone();
                            match secrets.get(&reference) {
                                Ok(Some(_)) => variable.configured = true,
                                Ok(None) => variable.configured = false,
                                Err(value) => error = Some(value.to_string()),
                            }
                        }
                    });
                    row.col(|ui| {
                        if !variable.is_empty()
                            && ui
                                .add_sized(
                                    [28.0, 28.0],
                                    egui::Button::new(theme::icon("x", 13.0)).frame(false),
                                )
                                .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                });
            }
        });
    if let Some(index) = remove {
        environment.variables.remove(index);
    }
    environment.ensure_empty_row();
    error
}

fn response_headers(
    ui: &mut egui::Ui,
    headers: &[(String, String)],
    language: Language,
    palette: Palette,
) {
    if headers.is_empty() {
        empty_state(
            ui,
            match language {
                Language::Chinese => "暂无响应头",
                Language::English => "No response headers",
            },
            "",
            palette,
        );
        return;
    }
    TableBuilder::new(ui)
        .striped(true)
        .column(Column::initial(220.0).at_least(140.0).resizable(true))
        .column(Column::remainder().at_least(240.0))
        .body(|mut body| {
            for (name, value) in headers {
                body.row(30.0, |mut row| {
                    row.col(|ui| {
                        ui.label(RichText::new(name).strong());
                    });
                    row.col(|ui| {
                        ui.label(value);
                    });
                });
            }
        });
}

fn response_timeline(
    ui: &mut egui::Ui,
    timeline: &[TimelineEntry],
    language: Language,
    palette: Palette,
) {
    if timeline.is_empty() {
        empty_state(
            ui,
            match language {
                Language::Chinese => "暂无时间线",
                Language::English => "No timeline",
            },
            "",
            palette,
        );
        return;
    }
    for entry in timeline {
        let (label, color) = match entry.phase {
            TimelinePhase::Started => (("开始", "Started"), palette.info),
            TimelinePhase::ResponseHead => (("收到响应头", "Response head"), palette.info),
            TimelinePhase::FirstByte => (("收到首字节", "First byte"), palette.accent_text),
            TimelinePhase::Completed => (("完成", "Completed"), palette.success),
            TimelinePhase::Cancelled => (("已取消", "Cancelled"), palette.warning),
            TimelinePhase::Failed => (("失败", "Failed"), palette.danger),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("●").color(color).size(9.0));
            ui.add_sized(
                [150.0, 24.0],
                egui::Label::new(match language {
                    Language::Chinese => label.0,
                    Language::English => label.1,
                }),
            );
            ui.label(RichText::new(format!("{} ms", entry.elapsed_ms)).color(palette.muted));
        });
    }
}

fn history_state_label(state: HistoryRunState, language: Language) -> &'static str {
    match (state, language) {
        (HistoryRunState::Running, Language::Chinese) => "运行中",
        (HistoryRunState::Running, Language::English) => "Running",
        (HistoryRunState::Passed, Language::Chinese) => "通过",
        (HistoryRunState::Passed, Language::English) => "Passed",
        (HistoryRunState::Failed, Language::Chinese) => "失败",
        (HistoryRunState::Failed, Language::English) => "Failed",
        (HistoryRunState::Cancelled, Language::Chinese) => "已取消",
        (HistoryRunState::Cancelled, Language::English) => "Cancelled",
    }
}

fn history_state_color(state: HistoryRunState, palette: Palette) -> Color32 {
    match state {
        HistoryRunState::Running => palette.info,
        HistoryRunState::Passed => palette.success,
        HistoryRunState::Failed => palette.danger,
        HistoryRunState::Cancelled => palette.warning,
    }
}

fn history_metric(ui: &mut egui::Ui, label: &str, value: String, palette: Palette) {
    ui.group(|ui| {
        ui.label(RichText::new(label).small().color(palette.muted));
        ui.label(RichText::new(value).strong());
    });
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.2} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn empty_state(ui: &mut egui::Ui, title: &str, description: &str, palette: Palette) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.label(RichText::new(title).strong().size(14.0));
        if !description.is_empty() {
            ui.label(RichText::new(description).color(palette.muted));
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        io::{Read, Write},
        net::{SocketAddr, TcpStream},
        sync::{Arc, mpsc},
        time::{Duration, Instant},
    };

    use apitest_core::{
        ApiContract, AssertionRule, EntityId, Environment, ExecutionCommand, ExecutionEvent,
        ExecutionMetrics, ExecutionRequest, ExecutionStream, ExtractorRule, GrpcCallKind,
        HttpMethod, KeyValue, MockRule, ProtocolExecutor, ProtocolKind, ProtocolSpec, ResponseHead,
        RunState as HistoryRunState, ScenarioNode, SecretRef, TestScenario, Variable,
        VariableSource,
    };
    use apitest_storage::{BodyStore, Database, MemorySecretStore, PageRequest, SecretStore};
    use chrono::Utc;
    use egui::accesskit::Role;
    use egui_kittest::{
        Harness,
        kittest::{NodeT as _, Queryable as _},
    };
    use futures::stream;
    use indexmap::IndexMap;
    use tokio_util::sync::CancellationToken;

    use super::{
        ACTIVE_PROJECT_SETTING, ApiTestApp, DocumentId, DocumentKind, DocumentTabs, EditorTab,
        EnvironmentDraft, ExecutorRegistry, InteropAction, Language, MAX_RESPONSE_BYTES,
        Navigation, OpenApiPreviewTab, PendingAction, ResourcePage, ResponseBodyMode, ResponseTab,
        ResponseView, StorageEvent, StorageWorker, ThemeMode, ToastKind, WorkspaceRequest,
        document_tabs_setting, load_automation, load_document_tabs, load_scenario_datasets,
    };
    use crate::{
        draft::{AuthDraft, AuthMode, EditablePair, ProxyDraft},
        theme,
    };

    struct FakeExecutor;

    struct HistoryExecutor;

    impl ProtocolExecutor for HistoryExecutor {
        fn kind(&self) -> ProtocolKind {
            ProtocolKind::Http
        }

        fn execute(
            &self,
            request: ExecutionRequest,
            _cancellation: CancellationToken,
        ) -> ExecutionStream {
            Box::pin(stream::iter([
                Ok(ExecutionEvent::Started {
                    id: request.id,
                    at: Utc::now(),
                }),
                Ok(ExecutionEvent::ResponseHead(ResponseHead {
                    status: Some(200),
                    version: Some("HTTP/1.1".into()),
                    headers: IndexMap::new(),
                })),
                Ok(ExecutionEvent::Data(b"token=token-".to_vec().into())),
                Ok(ExecutionEvent::Data(b"secret".to_vec().into())),
                Ok(ExecutionEvent::Completed(ExecutionMetrics {
                    elapsed_ms: 7,
                    received_bytes: 18,
                    sent_bytes: 0,
                })),
            ]))
        }
    }

    fn mock_get(address: SocketAddr, target: &str, headers: &[(&str, &str)]) -> String {
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1))
            .expect("mock server should accept connections");
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("read timeout should configure");
        let mut request =
            format!("GET {target} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
        for (name, value) in headers {
            request.push_str(name);
            request.push_str(": ");
            request.push_str(value);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        stream
            .write_all(request.as_bytes())
            .expect("mock request should write");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("mock response should read");
        response
    }

    impl ProtocolExecutor for FakeExecutor {
        fn kind(&self) -> ProtocolKind {
            ProtocolKind::Http
        }

        fn execute(
            &self,
            request: ExecutionRequest,
            _cancellation: CancellationToken,
        ) -> ExecutionStream {
            let bytes = "中文".as_bytes();
            let metrics = ExecutionMetrics {
                elapsed_ms: 12,
                received_bytes: bytes.len() as u64,
                sent_bytes: 0,
            };
            Box::pin(stream::iter([
                Ok(ExecutionEvent::Started {
                    id: request.id,
                    at: Utc::now(),
                }),
                Ok(ExecutionEvent::ResponseHead(ResponseHead {
                    status: Some(200),
                    version: Some("HTTP/1.1".into()),
                    headers: IndexMap::new(),
                })),
                Ok(ExecutionEvent::Data(bytes[..2].to_vec().into())),
                Ok(ExecutionEvent::Data(bytes[2..].to_vec().into())),
                Ok(ExecutionEvent::Completed(metrics)),
            ]))
        }
    }

    fn test_app(context: &mut eframe::CreationContext<'_>) -> ApiTestApp {
        theme::install_fonts(&context.egui_ctx);
        theme::apply(&context.egui_ctx, ThemeMode::Dark);
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("test runtime should initialize"),
        );
        let database = Arc::new(Database::open_in_memory().expect("test database should open"));
        let project = apitest_core::Project::new("Test Workspace");
        database
            .save_project(&project)
            .expect("test project should save");
        let mut request = WorkspaceRequest::new("Health check");
        request.draft.url = "https://example.test/health".into();
        request.sync_edit_revision(Instant::now());
        let revision = request.autosave.current_revision();
        let (definition, request_case) = request.save_snapshot();
        request.mark_saved(definition, request_case, revision);
        database
            .save_definition_and_case(project.id, &request.definition, &request.request_case)
            .expect("test request should save");
        let environment = Environment::new("Local");
        let environment_id = environment.id;
        database
            .save_environment(project.id, &environment)
            .expect("test environment should save");
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecretStore::default());
        let fake: Arc<dyn ProtocolExecutor> = Arc::new(FakeExecutor);
        let mut executors = ExecutorRegistry::new();
        for kind in [
            ProtocolKind::Http,
            ProtocolKind::GraphQl,
            ProtocolKind::Sse,
            ProtocolKind::WebSocket,
            ProtocolKind::Grpc,
        ] {
            executors.register(kind, Arc::clone(&fake));
        }
        let (sender, receiver) = mpsc::channel();
        let storage_worker = StorageWorker::new(Arc::clone(&database));
        let mut document_tabs = DocumentTabs::default();
        document_tabs.open(
            DocumentId {
                kind: DocumentKind::Api,
                entity_id: request.id(),
            },
            request.name.clone(),
        );

        ApiTestApp {
            runtime,
            executors: Arc::new(executors),
            secrets,
            database: Some(database),
            body_store: None,
            history_body: None,
            history_record: None,
            history_redactions: Vec::new(),
            run_records: Vec::new(),
            selected_history: 0,
            history_body_preview: String::new(),
            history_body_truncated: false,
            storage_worker: Some(storage_worker),
            projects: vec![project.clone()],
            project,
            requests: vec![request],
            resource_pages: HashMap::from([(None, ResourcePage::default())]),
            expanded_resources: HashSet::new(),
            environments: vec![EnvironmentDraft::from_environment(environment)],
            scenarios: Vec::new(),
            selected_scenario: 0,
            scenario_snapshots: HashMap::new(),
            scenario_report: None,
            scenario_cancellation: None,
            scenario_run_id: 0,
            mock_profiles: Vec::new(),
            selected_mock: 0,
            mock_snapshots: HashMap::new(),
            mock_server: None,
            mock_run_id: 0,
            selected: 0,
            selected_environment: 0,
            active_environment: environment_id,
            navigation: Navigation::Api,
            editor_tab: EditorTab::Params,
            response_tab: ResponseTab::Body,
            response_body_mode: ResponseBodyMode::Pretty,
            response: ResponseView::default(),
            sender,
            receiver,
            cancellation: None,
            execution_commands: None,
            websocket_message: String::new(),
            run_id: 0,
            theme: ThemeMode::Dark,
            language: Language::Chinese,
            search: String::new(),
            toast: None,
            confirmation: None,
            document_tabs,
            show_settings: false,
            show_curl_import: false,
            curl_import_source: String::new(),
            show_openapi_preview: false,
            openapi_preview_tab: OpenApiPreviewTab::Yaml,
            openapi_source: String::new(),
            openapi_html: String::new(),
            openapi_issues: Vec::new(),
            allow_close: false,
        }
    }

    #[test]
    fn response_decoder_preserves_utf8_split_across_chunks() {
        let mut response = ResponseView::running();
        let bytes = "中文".as_bytes();
        response.append_body(&bytes[..2]);
        response.append_body(&bytes[2..4]);
        response.append_body(&bytes[4..]);
        response.finish_body();

        assert_eq!(response.body, "中文");
    }

    #[test]
    fn response_preview_stops_at_the_byte_limit() {
        let mut response = ResponseView::running();
        response.append_body(&vec![b'a'; MAX_RESPONSE_BYTES + 16]);
        response.finish_body();

        assert_eq!(response.preview_bytes, MAX_RESPONSE_BYTES);
        assert_eq!(response.body.len(), MAX_RESPONSE_BYTES);
        assert!(response.truncated);
    }

    #[test]
    fn execution_history_externalizes_and_redacts_streamed_response_bodies() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let temp = tempfile::tempdir().expect("history directory should create");
        harness.state_mut().body_store = Some(
            BodyStore::new(temp.path().join("bodies"))
                .expect("history body store should initialize"),
        );
        harness.state_mut().requests[0]
            .draft
            .headers
            .push(EditablePair::new("Authorization", "Bearer token-secret"));
        let mut executors = ExecutorRegistry::new();
        let executor: Arc<dyn ProtocolExecutor> = Arc::new(HistoryExecutor);
        executors.register(ProtocolKind::Http, executor);
        harness.state_mut().executors = Arc::new(executors);

        let context = harness.ctx.clone();
        harness.state_mut().send_current(&context);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            harness.state_mut().drain_runtime();
            if harness.state().history_record.is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "history should finish within one second"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        let history = harness
            .state()
            .database
            .as_ref()
            .expect("database should exist")
            .list_run_records(harness.state().project.id, PageRequest::new(0, 10))
            .expect("history should load");
        assert_eq!(history.total, 1);
        assert_eq!(history.items[0].state, HistoryRunState::Passed);
        assert_eq!(history.items[0].status_code, Some(200));
        assert_eq!(history.items[0].response_bytes, 18);
        let body_path = history.items[0]
            .body_path
            .as_ref()
            .expect("history body should be externalized");
        let body = std::fs::read(body_path).expect("history body should read");
        assert_eq!(body, b"token=[REDACTED]");
        assert!(!String::from_utf8_lossy(&body).contains("token-secret"));

        harness
            .state_mut()
            .perform_action(PendingAction::Navigate(Navigation::History));
        harness.step();
        assert!(harness.query_by_label_contains("运行历史").is_some());
        assert!(
            harness
                .query_by_label_contains("token=[REDACTED]")
                .is_some()
        );
        assert!(harness.query_by_label_contains("token-secret").is_none());
    }

    #[test]
    fn minimum_window_keeps_request_inputs_visible() {
        let harness = Harness::builder()
            .with_size(egui::vec2(980.0, 640.0))
            .build_eframe(test_app);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(980.0, 640.0));
        let inputs = harness
            .query_all_by_role(Role::TextInput)
            .collect::<Vec<_>>();

        assert!(
            inputs.len() >= 6,
            "expected the search, name, URL and pair inputs"
        );
        assert!(
            inputs
                .iter()
                .all(|input| screen.contains_rect(input.rect()))
        );
        let wide_inputs = inputs
            .iter()
            .filter(|input| input.rect().width() >= 120.0)
            .count();
        assert!(
            wide_inputs >= 5,
            "request fields should not collapse at minimum size"
        );
    }

    #[test]
    fn light_english_layout_keeps_primary_actions_visible() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(980.0, 640.0))
            .build_eframe(test_app);
        harness.state_mut().theme = ThemeMode::Light;
        harness.state_mut().language = Language::English;
        theme::apply(&harness.ctx, ThemeMode::Light);
        harness.step();

        assert!(harness.query_by_label("Params").is_some());
        assert!(
            harness
                .query_all_by_label_contains("Send")
                .any(|node| node.accesskit_node().role() == Role::Button)
        );
        assert!(
            harness
                .query_all_by_role(Role::TextInput)
                .all(|input| input.rect().max.x <= 980.0 && input.rect().max.y <= 640.0)
        );
    }

    #[test]
    fn advanced_authentication_modes_expose_editable_fields() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let request_id = harness.state().requests[0].id();
        harness.state_mut().editor_tab = EditorTab::Auth;

        for (mode, field) in [
            (AuthMode::OAuth2, "Token 地址"),
            (AuthMode::Digest, "用户名"),
            (AuthMode::AwsSigV4, "Access Key"),
        ] {
            harness.state_mut().requests[0].draft.auth = AuthDraft::for_mode(mode, request_id);
            harness.step();
            assert!(
                harness.query_by_label_contains(field).is_some(),
                "{mode:?} should expose {field}"
            );
        }
    }

    #[test]
    fn desktop_interop_imports_curl_and_previews_openapi() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let initial_count = harness.state().requests.len();
        let imported = harness
            .state_mut()
            .import_source(
                InteropAction::ImportCurl,
                "curl --request POST 'https://example.test/users' --data '{\"name\":\"Ada\"}'",
            )
            .expect("cURL should import");
        assert_eq!(imported, 1);
        assert_eq!(harness.state().requests.len(), initial_count + 1);
        assert!(matches!(
            harness.state().requests.last().map(WorkspaceRequest::edited_protocol),
            Some(ProtocolSpec::Http(spec))
                if spec.url == "https://example.test/users"
                    && spec.method == apitest_core::HttpMethod::Post
        ));
        assert!(matches!(
            harness
                .state()
                .storage_worker
                .as_ref()
                .expect("storage worker should exist")
                .recv_timeout(Duration::from_secs(1)),
            Some(StorageEvent::RequestSaved { notify: false, .. })
        ));

        let openapi = r#"
openapi: 3.1.0
info: { title: Catalog, version: 1.0.0 }
paths:
  /catalog:
    get:
      operationId: listCatalog
      summary: List catalog
      responses:
        '200': { description: OK }
components:
  schemas:
    CatalogItem: { type: object }
"#;
        assert_eq!(
            harness
                .state_mut()
                .import_source(InteropAction::ImportOpenApi, openapi)
                .expect("OpenAPI should import"),
            1
        );
        assert!(
            harness
                .state()
                .project
                .components
                .schemas
                .contains_key("CatalogItem")
        );
        assert!(matches!(
            harness
                .state()
                .storage_worker
                .as_ref()
                .expect("storage worker should exist")
                .recv_timeout(Duration::from_secs(1)),
            Some(StorageEvent::RequestSaved { notify: false, .. })
        ));

        harness
            .state_mut()
            .refresh_openapi_preview()
            .expect("preview should render");
        assert!(harness.state().openapi_source.contains("openapi: 3.1.0"));
        assert!(harness.state().openapi_source.contains("listCatalog"));
        assert!(harness.state().openapi_html.contains("<!doctype html>"));
        harness.state_mut().show_openapi_preview = true;
        harness.step();
        assert!(
            harness
                .query_by_label_contains("OpenAPI 设计与预览")
                .is_some()
        );
    }

    #[test]
    fn securing_openapi_examples_preserves_the_full_contract() {
        let source = r#"
openapi: 3.1.0
info: { title: Secure, version: 1.0.0 }
servers: [{ url: https://example.test }]
paths:
  /users:
    get:
      operationId: listUsers
      security: [{ apiKey: [] }]
      parameters:
        - name: x-api-key
          in: header
          schema: { type: string }
          example: contract-secret
      requestBody:
        content:
          application/json:
            schema: { type: object }
            examples:
              primary: { value: { password: body-secret-one } }
              secondary: { value: { token: body-secret-two } }
      responses:
        '201':
          description: Users
          content:
            application/json:
              schema: { type: array, items: { type: string } }
components:
  securitySchemes:
    apiKey: { type: apiKey, in: header, name: x-api-key }
"#;
        let imported = super::import_openapi_document(source).expect("OpenAPI should import");
        let definition = imported
            .definitions
            .into_iter()
            .next()
            .expect("one operation");
        let mut request = WorkspaceRequest::from_definition(definition, None);
        let secrets = MemorySecretStore::default();

        super::secure_imported_request(&mut request, &secrets)
            .expect("sensitive examples should be secured");

        let ApiContract::Http(contract) = &request.definition.contract else {
            panic!("OpenAPI operation should remain HTTP");
        };
        assert_eq!(contract.responses[0].status, "201");
        assert_eq!(contract.security[0].scheme, "apiKey");
        assert_eq!(contract.extensions["operationId"], "listUsers");
        assert_eq!(contract.servers[0].url, "https://example.test");
        assert_eq!(
            contract.parameters[0].example,
            Some(serde_json::json!("{{__import_secret_0}}"))
        );
        let stored = serde_json::to_string(&request.definition.contract)
            .expect("secured contract should serialize");
        assert!(!stored.contains("contract-secret"));
        assert!(!stored.contains("body-secret-one"));
        assert!(!stored.contains("body-secret-two"));
        assert_eq!(
            contract
                .request_body
                .as_ref()
                .expect("request body should remain")
                .examples
                .len(),
            2
        );
    }

    #[test]
    fn importing_openapi_merges_existing_project_components() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        harness
            .state_mut()
            .project
            .components
            .schemas
            .insert("Existing".into(), apitest_core::ApiSchema::string());
        let source = r#"
openapi: 3.1.0
info: { title: New, version: 1.0.0 }
paths:
  /new:
    get:
      responses:
        '200': { description: OK }
components:
  schemas:
    Imported: { type: object }
"#;

        harness
            .state_mut()
            .import_source(InteropAction::ImportOpenApi, source)
            .expect("OpenAPI should import");

        assert!(
            harness
                .state()
                .project
                .components
                .schemas
                .contains_key("Existing")
        );
        assert!(
            harness
                .state()
                .project
                .components
                .schemas
                .contains_key("Imported")
        );
    }

    #[test]
    fn history_redaction_includes_request_local_secret_references() {
        let harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let reference = SecretRef::new("keyring://request-local-api-key");
        harness
            .state()
            .secrets
            .set(&reference, "local-secret")
            .expect("local secret should save");
        let mut harness = harness;
        harness.state_mut().requests[0]
            .request_case
            .local_variables
            .push(Variable::secret("api_key", reference.0));

        let values = harness.state().history_redaction_values(0, 0);

        assert!(values.iter().any(|value| value == "local-secret"));
    }

    #[test]
    fn failed_database_delete_does_not_remove_request_secrets() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let request_id = harness.state().requests[0].id();
        let reference = SecretRef::new(format!("keyring://request-{request_id}-local"));
        harness
            .state()
            .secrets
            .set(&reference, "keep-me")
            .expect("request secret should save");
        harness.state_mut().requests[0]
            .request_case
            .local_variables
            .push(Variable::secret("token", reference.0.clone()));
        harness.state_mut().database = None;

        harness.state_mut().delete_request(request_id);

        assert_eq!(harness.state().requests.len(), 1);
        assert_eq!(
            harness
                .state()
                .secrets
                .get(&reference)
                .expect("secret lookup should succeed")
                .as_deref(),
            Some("keep-me")
        );
    }

    #[test]
    fn deleting_a_new_request_cannot_be_undone_by_a_queued_save() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        harness
            .state_mut()
            .perform_action(PendingAction::NewRequest(ProtocolKind::Http));
        let index = harness.state().selected;
        harness.state_mut().requests[index].draft.url = "https://example.test/new".into();
        let request_id = harness.state().requests[index].id();
        let database = Arc::clone(
            harness
                .state()
                .database
                .as_ref()
                .expect("database should exist"),
        );

        assert!(harness.state_mut().queue_request_save(index, true, false));
        harness.state_mut().delete_request(request_id);

        assert!(
            database
                .get_definition(request_id)
                .expect("definition lookup should succeed")
                .is_none()
        );
    }

    #[test]
    fn discarding_an_edit_restores_the_database_after_a_queued_save() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let request_id = harness.state().requests[0].id();
        harness.state_mut().requests[0].name = "Queued edit".into();

        assert!(harness.state_mut().queue_request_save(0, true, false));
        harness.state_mut().discard_current();

        let stored = harness
            .state()
            .database
            .as_ref()
            .expect("database should exist")
            .get_definition(request_id)
            .expect("definition lookup should succeed")
            .expect("definition should exist");
        assert_eq!(stored.name, "Health check");
    }

    #[test]
    fn leaving_the_workspace_checks_dirty_background_documents() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        harness.state_mut().scenarios.push(TestScenario {
            name: "Unsaved background scenario".into(),
            ..TestScenario::default()
        });

        harness.state_mut().queue_action(PendingAction::Close);

        assert!(matches!(
            harness.state().confirmation,
            Some(super::Confirmation::Unsaved(PendingAction::Close))
        ));
        assert!(!harness.state().allow_close);
    }

    #[test]
    fn deleting_a_new_environment_cannot_be_undone_by_a_queued_save() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        harness
            .state_mut()
            .perform_action(PendingAction::NewEnvironment);
        let index = harness.state().selected_environment;
        let environment_id = harness.state().environments[index].id();
        let database = Arc::clone(
            harness
                .state()
                .database
                .as_ref()
                .expect("database should exist"),
        );

        assert!(
            harness
                .state_mut()
                .queue_environment_save(index, true, false)
        );
        harness.state_mut().delete_environment(environment_id);

        assert!(
            database
                .list_environments(harness.state().project.id)
                .expect("environments should load")
                .iter()
                .all(|environment| environment.id != environment_id)
        );
    }

    #[test]
    fn stale_environment_receipts_keep_secrets_used_by_the_current_draft() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let environment_id = harness.state().environments[0].id();
        let reference = SecretRef::new(format!(
            "keyring://environment-{environment_id}-variable-shared"
        ));
        harness
            .state()
            .secrets
            .set(&reference, "keep-current")
            .expect("environment secret should save");
        let mut saved = harness.state().environments[0].saved.clone();
        saved.variables = vec![Variable::secret("token", reference.0.clone())];
        harness.state_mut().environments[0] = EnvironmentDraft::from_environment(saved);
        {
            let variable = &mut harness.state_mut().environments[0].variables[0];
            variable.secret = false;
            variable.value = "plain".into();
            variable.reference = None;
            variable.configured = false;
        }
        assert!(harness.state_mut().queue_environment_save(0, true, false));
        {
            let variable = &mut harness.state_mut().environments[0].variables[0];
            variable.secret = true;
            variable.value.clear();
            variable.reference = Some(reference.clone());
            variable.configured = true;
        }

        assert!(harness.state_mut().settle_storage());

        assert_eq!(
            harness
                .state()
                .secrets
                .get(&reference)
                .expect("secret lookup should succeed")
                .as_deref(),
            Some("keep-current")
        );
    }

    #[test]
    fn discarding_a_queued_environment_edit_preserves_restored_secrets() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let environment_id = harness.state().environments[0].id();
        let reference = SecretRef::new(format!(
            "keyring://environment-{environment_id}-variable-restored"
        ));
        harness
            .state()
            .secrets
            .set(&reference, "restore-me")
            .expect("environment secret should save");
        let mut saved = harness.state().environments[0].saved.clone();
        saved.variables = vec![Variable::secret("token", reference.0.clone())];
        harness.state_mut().environments[0] = EnvironmentDraft::from_environment(saved);
        {
            let variable = &mut harness.state_mut().environments[0].variables[0];
            variable.secret = false;
            variable.value = "plain".into();
            variable.reference = None;
            variable.configured = false;
        }
        assert!(harness.state_mut().queue_environment_save(0, true, false));

        assert!(harness.state_mut().discard_environment(0));

        assert_eq!(
            harness
                .state()
                .secrets
                .get(&reference)
                .expect("secret lookup should succeed")
                .as_deref(),
            Some("restore-me")
        );
        assert_eq!(
            harness.state().environments[0].saved.variables[0]
                .secret_ref
                .as_ref(),
            Some(&reference)
        );
    }

    #[test]
    fn saving_before_leaving_waits_for_all_dirty_documents() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let request_id = harness.state().requests[0].id();
        harness.state_mut().requests[0].name = "Persisted before leave".into();
        let scenario = TestScenario {
            name: "Background scenario".into(),
            ..TestScenario::default()
        };
        let scenario_id = scenario.id;
        harness.state_mut().scenarios.push(scenario);

        assert!(harness.state_mut().save_workspace_and_wait());

        let database = harness
            .state()
            .database
            .as_ref()
            .expect("database should exist");
        assert_eq!(
            database
                .get_definition(request_id)
                .expect("definition should load")
                .expect("definition should exist")
                .name,
            "Persisted before leave"
        );
        assert!(
            database
                .list_scenarios(harness.state().project.id)
                .expect("scenarios should load")
                .iter()
                .any(|scenario| scenario.id == scenario_id)
        );
        assert!(!harness.state().workspace_dirty());
    }

    #[test]
    fn imported_authentication_values_are_moved_to_the_secret_store() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let initial_count = harness.state().requests.len();
        let postman = r#"
        {
          "info": {
            "name": "Secure import",
            "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
          },
          "item": [
            {
              "name": "Bearer request",
              "request": {
                "method": "GET",
                "url": "https://example.test/bearer",
                "auth": {
                  "type": "bearer",
                  "bearer": [{"key":"token","value":"bearer-secret"}]
                }
              }
            },
            {
              "name": "Basic request",
              "request": {
                "method": "GET",
                "url": "https://example.test/basic",
                "auth": {
                  "type": "basic",
                  "basic": [
                    {"key":"username","value":"alice"},
                    {"key":"password","value":"basic-secret"}
                  ]
                }
              }
            },
            {
              "name": "Custom API key",
              "request": {
                "method": "GET",
                "url": "https://example.test/key",
                "auth": {
                  "type": "apikey",
                  "apikey": [
                    {"key":"key","value":"X-Client-Credential"},
                    {"key":"value","value":"custom-secret"},
                    {"key":"in","value":"header"}
                  ]
                }
              }
            }
          ]
        }
        "#;

        assert_eq!(
            harness
                .state_mut()
                .import_source(InteropAction::ImportPostman, postman)
                .expect("Postman authentication should import"),
            3
        );
        for _ in 0..3 {
            assert!(matches!(
                harness
                    .state()
                    .storage_worker
                    .as_ref()
                    .expect("storage worker should exist")
                    .recv_timeout(Duration::from_secs(1)),
                Some(StorageEvent::RequestSaved { notify: false, .. })
            ));
        }

        let imported = &harness.state().requests[initial_count..];
        let AuthDraft::Bearer { token } = &imported[0].draft.auth else {
            panic!("bearer authentication should remain editable");
        };
        assert!(token.configured);
        assert!(token.replacement.is_empty());
        assert_eq!(
            harness
                .state()
                .secrets
                .get(&token.reference)
                .expect("bearer secret should load")
                .as_deref(),
            Some("bearer-secret")
        );
        let AuthDraft::Basic { username, password } = &imported[1].draft.auth else {
            panic!("basic authentication should remain editable");
        };
        assert_eq!(username, "alice");
        assert_eq!(
            harness
                .state()
                .secrets
                .get(&password.reference)
                .expect("basic secret should load")
                .as_deref(),
            Some("basic-secret")
        );
        assert_eq!(imported[2].draft.headers[0].value, "{{__import_secret_0}}");
        assert!(imported[2].draft.headers[0].description.is_empty());
        let api_key_reference = imported[2].request_case.local_variables[0]
            .secret_ref
            .as_ref()
            .expect("API key should use a secret reference");
        assert_eq!(
            harness
                .state()
                .secrets
                .get(api_key_reference)
                .expect("API key secret should load")
                .as_deref(),
            Some("custom-secret")
        );

        let database = harness
            .state()
            .database
            .as_ref()
            .expect("database should exist");
        for request in imported {
            let definition = database
                .get_definition(request.id())
                .expect("definition should load")
                .expect("definition should exist");
            let cases = database
                .list_request_cases(
                    harness.state().project.id,
                    request.id(),
                    PageRequest::new(0, 10),
                )
                .expect("request case should load");
            let stored = serde_json::to_string(&(definition, cases.items))
                .expect("stored documents should serialize");
            for secret in ["bearer-secret", "basic-secret", "custom-secret"] {
                assert!(!stored.contains(secret), "stored document leaked {secret}");
            }
        }
    }

    #[test]
    fn send_flow_reaches_a_completed_response() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);

        harness
            .query_all_by_label_contains("发送")
            .find(|node| node.accesskit_node().role() == Role::Button)
            .expect("send button should exist")
            .click();
        harness.run_steps(8);

        assert!(harness.query_by_label("200").is_some());
        assert!(harness.query_by_label_contains("中文").is_some());
        assert_eq!(harness.state().response.body, "中文");
        assert_eq!(harness.state().response.state, super::RunState::Completed);
    }

    #[test]
    fn websocket_console_forwards_messages_and_graceful_close() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let (commands, mut received) = tokio::sync::mpsc::channel(4);
        {
            let state = harness.state_mut();
            state.requests[0].alternate_protocol =
                Some(super::default_protocol(ProtocolKind::WebSocket));
            state.execution_commands = Some(commands);
            state.websocket_message = "hello socket".into();
        }
        harness.step();

        assert!(harness.query_by_label_contains("发送消息").is_some());
        assert!(harness.query_by_label_contains("关闭连接").is_some());

        harness.state_mut().send_websocket_message();
        let command = received
            .try_recv()
            .expect("message command should be forwarded");
        assert!(matches!(
            command,
            ExecutionCommand::SendMessage { data, .. } if data.as_ref() == b"hello socket"
        ));
        assert!(harness.state().websocket_message.is_empty());

        harness.state_mut().close_websocket_input();
        assert_eq!(
            received
                .try_recv()
                .expect("close command should be forwarded"),
            ExecutionCommand::CompleteInput
        );
        assert!(harness.state().execution_commands.is_none());
    }

    #[test]
    fn environment_navigation_exposes_real_variable_editor() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);

        assert!(harness.query_by_label_contains("RUN").is_some());
        assert!(harness.query_by_label_contains("MOCK").is_some());
        assert!(harness.query_by_label_contains("HIST").is_some());
        harness.get_by_label_contains("ENV").click();
        harness.run();

        assert!(harness.query_by_label("环境变量").is_some());
        assert_eq!(harness.state().navigation, Navigation::Environment);
    }

    #[test]
    fn nested_scenario_saves_reloads_and_restores_its_document_tab() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let request_id = harness.state().requests[0].request_case.id;

        harness
            .state_mut()
            .perform_action(PendingAction::NewScenario);
        let scenario = &mut harness.state_mut().scenarios[0];
        scenario.name = "Release smoke".into();
        scenario.nodes = vec![ScenarioNode::Group {
            name: "Authentication".into(),
            nodes: vec![ScenarioNode::If {
                expression: "env.enabled".into(),
                then_nodes: vec![ScenarioNode::Request {
                    case_id: request_id,
                    assertions: vec![
                        AssertionRule::Status { expected: 200 },
                        AssertionRule::HeaderEquals {
                            name: "content-type".into(),
                            expected: "application/json".into(),
                        },
                    ],
                    extractors: vec![ExtractorRule {
                        name: "request_id".into(),
                        source: VariableSource::Header("x-request-id".into()),
                    }],
                }],
                else_nodes: vec![ScenarioNode::Delay { millis: 50 }],
            }],
        }];
        let expected = scenario.clone();

        assert!(harness.state().current_dirty());
        assert!(harness.state_mut().save_current_scenario());
        assert!(!harness.state().current_dirty());

        let state = harness.state();
        let database = state.database.as_deref().expect("database should exist");
        let (scenarios, _, errors) = load_automation(Some(database), state.project.id);
        assert!(
            errors.is_empty(),
            "reload should not report errors: {errors:?}"
        );
        assert_eq!(scenarios, vec![expected.clone()]);
        let restored_tabs = load_document_tabs(
            Some(database),
            state.project.id,
            &state.requests,
            &state.environments,
            &scenarios,
            &state.mock_profiles,
        );
        assert_eq!(
            restored_tabs.active(),
            Some(DocumentId {
                kind: DocumentKind::Scenario,
                entity_id: expected.id,
            })
        );
        assert_eq!(
            restored_tabs
                .items()
                .iter()
                .find(|tab| tab.id.entity_id == expected.id)
                .map(|tab| tab.title.as_str()),
            Some("Release smoke")
        );
    }

    #[test]
    fn scenario_runs_from_the_workbench_and_exposes_a_step_report() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let request_id = harness.state().requests[0].request_case.id;
        harness
            .state_mut()
            .perform_action(PendingAction::NewScenario);
        harness.state_mut().scenarios[0].nodes = vec![ScenarioNode::Request {
            case_id: request_id,
            assertions: vec![AssertionRule::Status { expected: 200 }],
            extractors: Vec::new(),
        }];

        let context = harness.ctx.clone();
        harness.state_mut().run_current_scenario(&context);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            harness.state_mut().drain_runtime();
            if harness.state().scenario_cancellation.is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "scenario should finish within one second"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        let report = harness
            .state()
            .scenario_report
            .as_ref()
            .expect("scenario report should be retained");
        assert!(report.passed);
        assert_eq!(report.steps.len(), 1);
        assert_eq!(report.steps[0].name, "Health check");
        assert_eq!(report.steps[0].status, Some(200));
        assert_eq!(report.steps[0].assertions[0].name, "Status = 200");
    }

    #[test]
    fn scenario_datasets_load_json_and_csv_rows_and_reject_invalid_shapes() {
        let mut json = tempfile::Builder::new()
            .suffix(".json")
            .tempfile()
            .expect("JSON dataset should create");
        json.write_all(br#"[{"tenant":"alpha","limit":2},{"tenant":"beta","limit":3}]"#)
            .expect("JSON dataset should write");
        let json_rows = load_scenario_datasets(Some(
            json.path()
                .to_str()
                .expect("temporary path should be UTF-8"),
        ))
        .expect("JSON dataset should load");
        assert_eq!(json_rows.len(), 2);
        assert_eq!(
            json_rows[0].get("tenant").map(String::as_str),
            Some("alpha")
        );
        assert_eq!(json_rows[1].get("limit").map(String::as_str), Some("3"));

        let mut csv = tempfile::Builder::new()
            .suffix(".csv")
            .tempfile()
            .expect("CSV dataset should create");
        csv.write_all(b"tenant,enabled\nalpha,true\nbeta,false\n")
            .expect("CSV dataset should write");
        let csv_rows = load_scenario_datasets(Some(
            csv.path().to_str().expect("temporary path should be UTF-8"),
        ))
        .expect("CSV dataset should load");
        assert_eq!(csv_rows.len(), 2);
        assert_eq!(csv_rows[1].get("tenant").map(String::as_str), Some("beta"));

        let mut invalid = tempfile::Builder::new()
            .suffix(".json")
            .tempfile()
            .expect("invalid dataset should create");
        invalid
            .write_all(br#"["not an object"]"#)
            .expect("invalid dataset should write");
        let error = load_scenario_datasets(Some(
            invalid
                .path()
                .to_str()
                .expect("temporary path should be UTF-8"),
        ))
        .expect_err("non-object rows should fail");
        assert!(error.contains("row must be an object"));
    }

    #[test]
    fn smart_mock_generation_saves_reloads_and_restores_its_document_tab() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);

        harness.state_mut().perform_action(PendingAction::NewMock);
        harness.state_mut().mock_profiles[0].name = "Contract mock".into();
        harness.state_mut().generate_current_mock_rules();

        let profile = &mut harness.state_mut().mock_profiles[0];
        assert_eq!(profile.rules.len(), 1);
        assert_eq!(profile.rules[0].method, HttpMethod::Get);
        assert_eq!(profile.rules[0].path, "/health");
        profile.rules[0].response.status = 202;
        profile.rules[0].response.body = r#"{"accepted":true}"#.into();
        let expected = profile.clone();

        assert!(harness.state_mut().save_current_mock());
        assert!(!harness.state().current_dirty());

        let state = harness.state();
        let database = state.database.as_deref().expect("database should exist");
        let (_, profiles, errors) = load_automation(Some(database), state.project.id);
        assert!(
            errors.is_empty(),
            "reload should not report errors: {errors:?}"
        );
        assert_eq!(profiles, vec![expected.clone()]);
        let restored_tabs = load_document_tabs(
            Some(database),
            state.project.id,
            &state.requests,
            &state.environments,
            &state.scenarios,
            &profiles,
        );
        assert_eq!(
            restored_tabs.active(),
            Some(DocumentId {
                kind: DocumentKind::Mock,
                entity_id: expected.id,
            })
        );
    }

    #[test]
    fn desktop_mock_configuration_starts_a_server_and_matches_requests() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        harness.state_mut().perform_action(PendingAction::NewMock);
        let mut rule = MockRule {
            name: "Admin user".into(),
            method: HttpMethod::Get,
            path: "/users/{id}".into(),
            query: vec![KeyValue::enabled("role", "admin")],
            headers: vec![KeyValue::enabled("x-token", "desktop")],
            path_variables: IndexMap::from([("id".into(), "42".into())]),
            ..MockRule::default()
        };
        rule.response.status = 201;
        rule.response.headers = vec![KeyValue::enabled("content-type", "application/json")];
        rule.response.body = r#"{"id":42}"#.into();
        harness.state_mut().mock_profiles[0].name = "Desktop server".into();
        harness.state_mut().mock_profiles[0].rules = vec![rule];

        let context = harness.ctx.clone();
        harness.state_mut().start_current_mock(&context);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            harness.state_mut().drain_runtime();
            if harness.state().mock_server.is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "mock server should start within one second"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let address = harness
            .state()
            .mock_server
            .as_ref()
            .expect("mock server should be running")
            .address();

        let matched = mock_get(address, "/users/42?role=admin", &[("x-token", "desktop")]);
        assert!(
            matched.starts_with("HTTP/1.1 201"),
            "response was {matched:?}"
        );
        assert!(matched.contains("content-type: application/json"));
        assert!(matched.ends_with(r#"{"id":42}"#));

        let unmatched = mock_get(address, "/users/7?role=admin", &[("x-token", "desktop")]);
        assert!(
            unmatched.starts_with("HTTP/1.1 404"),
            "response was {unmatched:?}"
        );
        assert!(unmatched.ends_with("Mock route not found"));

        harness.state_mut().stop_current_mock();
        assert!(harness.state().mock_server.is_none());
    }

    #[test]
    fn toast_error_state_remains_distinct() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(980.0, 640.0))
            .build_eframe(test_app);
        harness
            .state_mut()
            .toast(ToastKind::Error, "validation failed");
        harness.step();

        assert!(harness.query_by_label("validation failed").is_some());
    }

    #[test]
    fn saving_an_edit_preserves_definition_metadata() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let request_id = harness.state().requests[0].id();
        harness.state_mut().requests[0]
            .definition
            .description_markdown = "kept".into();
        harness.state_mut().requests[0].definition.tags = vec!["critical".into()];
        harness.state_mut().requests[0].name = "Updated health check".into();

        assert!(harness.state_mut().save_current_request());
        harness
            .state()
            .storage_worker
            .as_ref()
            .expect("storage worker should exist")
            .recv_timeout(Duration::from_secs(1))
            .expect("save should complete");
        let stored = harness
            .state()
            .database
            .as_ref()
            .expect("database should exist")
            .get_definition(request_id)
            .expect("definition should load")
            .expect("definition should exist");

        assert_eq!(stored.name, "Updated health check");
        assert_eq!(stored.description_markdown, "kept");
        assert_eq!(stored.tags, ["critical"]);
    }

    #[test]
    fn auth_and_proxy_plaintext_is_moved_to_the_secret_store_before_save() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let request_id = harness.state().requests[0].id();
        let references = {
            let request = &mut harness.state_mut().requests[0];
            request.draft.auth = AuthDraft::for_mode(AuthMode::AwsSigV4, request_id);
            let AuthDraft::AwsSigV4 {
                access_key,
                secret_key,
                session_token,
                region,
                service,
            } = &mut request.draft.auth
            else {
                panic!("AWS authentication draft should be created");
            };
            access_key.replacement = "AKIA-LOCAL".into();
            secret_key.replacement = "aws-secret-local".into();
            session_token.replacement = "aws-session-local".into();
            *region = "us-east-1".into();
            *service = "execute-api".into();
            let mut proxy = ProxyDraft::new(request_id);
            proxy.url = "http://127.0.0.1:8080".into();
            proxy.username = "proxy-user".into();
            proxy.password.replacement = "proxy-secret-local".into();
            let references = vec![
                (access_key.reference.clone(), "AKIA-LOCAL"),
                (secret_key.reference.clone(), "aws-secret-local"),
                (session_token.reference.clone(), "aws-session-local"),
                (proxy.password.reference.clone(), "proxy-secret-local"),
            ];
            request.draft.proxy = Some(proxy);
            references
        };

        assert!(harness.state_mut().save_current_request());
        let event = harness
            .state()
            .storage_worker
            .as_ref()
            .expect("storage worker should exist")
            .recv_timeout(Duration::from_secs(1))
            .expect("save should complete");
        let StorageEvent::RequestSaved { request_case, .. } = event else {
            panic!("request save should be acknowledged");
        };
        let document =
            serde_json::to_string(&request_case).expect("saved request case should serialize");

        for (reference, plaintext) in references {
            assert_eq!(
                harness
                    .state()
                    .secrets
                    .get(&reference)
                    .expect("secret store should be readable")
                    .as_deref(),
                Some(plaintext)
            );
            assert!(!document.contains(plaintext));
        }
        assert!(!harness.state().requests[0].draft.has_pending_secret());
    }

    #[test]
    fn non_http_requests_autosave_and_reload_without_protocol_loss() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let mut expected = Vec::new();

        for (kind, name) in [
            (ProtocolKind::GraphQl, "GraphQL users"),
            (ProtocolKind::Sse, "SSE events"),
            (ProtocolKind::WebSocket, "WebSocket chat"),
            (ProtocolKind::Grpc, "gRPC greeter"),
        ] {
            let state = harness.state_mut();
            state.perform_action(PendingAction::NewRequest(kind));
            let index = state.selected;
            let mut protocol = super::default_protocol(kind);
            match &mut protocol {
                ProtocolSpec::GraphQl(spec) => {
                    spec.endpoint = "https://api.example.test/graphql".into();
                    spec.query = "query Users { users { id name } }".into();
                    spec.variables = r#"{"limit":25}"#.into();
                    spec.operation_name = Some("Users".into());
                    spec.headers.push(KeyValue::enabled("x-tenant", "dev"));
                    spec.timeout_ms = 12_345;
                }
                ProtocolSpec::Sse(spec) => {
                    spec.request.url = "https://api.example.test/events".into();
                    spec.request
                        .headers
                        .push(KeyValue::enabled("last-event-id", "42"));
                    spec.request.timeout_ms = 45_000;
                    spec.reconnect = false;
                }
                ProtocolSpec::WebSocket(spec) => {
                    spec.url = "wss://api.example.test/chat".into();
                    spec.query.push(KeyValue::enabled("room", "rust"));
                    spec.headers.push(KeyValue::enabled("x-client", "desktop"));
                    spec.subprotocols = vec!["chat.v2".into(), "json".into()];
                    spec.validate_tls = false;
                    spec.connect_timeout_ms = 9_876;
                }
                ProtocolSpec::Grpc(spec) => {
                    spec.endpoint = "https://grpc.example.test:443".into();
                    spec.service = "demo.v1.Greeter".into();
                    spec.method = "WatchHello".into();
                    spec.call_kind = GrpcCallKind::ServerStreaming;
                    spec.proto_files.push("proto/greeter.proto".into());
                    spec.import_paths.push("proto".into());
                    spec.use_reflection = false;
                    spec.metadata.push(KeyValue::enabled("x-tenant", "dev"));
                    spec.message_json = r#"{"name":"ApiTest"}"#.into();
                    spec.validate_tls = false;
                    spec.timeout_ms = 54_321;
                }
                ProtocolSpec::Http(_) => panic!("test only configures non-HTTP protocols"),
            }
            state.requests[index].name = name.into();
            state.requests[index].alternate_protocol = Some(protocol.clone());
            state.requests[index].sync_edit_revision(Instant::now());
            state.requests[index]
                .autosave
                .mark_changed(Instant::now() - Duration::from_secs(1));
            expected.push((name, protocol));
        }

        let context = harness.ctx.clone();
        harness.state_mut().schedule_request_autosaves(&context);
        for _ in 0..expected.len() {
            let event = harness
                .state()
                .storage_worker
                .as_ref()
                .expect("storage worker should exist")
                .recv_timeout(Duration::from_secs(1))
                .expect("autosave should complete");
            assert!(matches!(
                event,
                StorageEvent::RequestSaved { notify: false, .. }
            ));
        }

        let database = Arc::clone(
            harness
                .state()
                .database
                .as_ref()
                .expect("database should exist"),
        );
        let project = harness.state().project.clone();
        let (reloaded, _, _, errors) =
            super::load_project_content(Some(database.as_ref()), &project, Language::Chinese);
        assert!(
            errors.is_empty(),
            "reload should not report errors: {errors:?}"
        );
        for (name, protocol) in expected {
            let request = reloaded
                .iter()
                .find(|request| request.name == name)
                .expect("saved protocol request should reload");
            assert_eq!(request.edited_protocol(), protocol);
            let definition = database
                .get_definition(request.id())
                .expect("definition should load")
                .expect("definition should exist");
            assert_eq!(definition.contract.example_protocol(), protocol);
        }
    }

    #[test]
    fn project_switching_creates_an_isolated_workspace_and_remembers_it() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let original_project = harness.state().project.id;

        harness.state_mut().create_project();

        assert_eq!(harness.state().projects.len(), 2);
        assert_ne!(harness.state().project.id, original_project);
        assert_eq!(harness.state().requests.len(), 1);
        assert_eq!(harness.state().environments.len(), 1);
        let stored: EntityId = harness
            .state()
            .database
            .as_ref()
            .expect("database should exist")
            .get_setting(ACTIVE_PROJECT_SETTING)
            .expect("setting should load")
            .expect("active project should be stored");
        assert_eq!(stored, harness.state().project.id);

        harness.state_mut().switch_project(original_project);
        assert_eq!(harness.state().project.id, original_project);
        assert_eq!(harness.state().requests[0].name, "Health check");
    }

    #[test]
    fn opened_documents_are_persisted_per_project() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let environment_id = harness.state().environments[0].id();

        harness
            .state_mut()
            .perform_action(PendingAction::SelectEnvironment(environment_id));

        let key = document_tabs_setting(harness.state().project.id);
        let stored: DocumentTabs = harness
            .state()
            .database
            .as_ref()
            .expect("database should exist")
            .get_setting(&key)
            .expect("setting should load")
            .expect("tabs should be stored");
        assert_eq!(stored.items().len(), 2);
        assert_eq!(
            stored.active(),
            Some(DocumentId {
                kind: DocumentKind::Environment,
                entity_id: environment_id,
            })
        );
    }

    #[test]
    fn environment_save_is_acknowledged_by_the_background_worker() {
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_eframe(test_app);
        let environment_id = harness.state().environments[0].id();
        harness.state_mut().environments[0].name = "Staging".into();

        assert!(harness.state_mut().save_current_environment());
        let event = harness
            .state()
            .storage_worker
            .as_ref()
            .expect("storage worker should exist")
            .recv_timeout(Duration::from_secs(1))
            .expect("save should complete");
        assert!(matches!(
            event,
            StorageEvent::EnvironmentSaved {
                environment,
                notify: true,
                ..
            } if environment.id == environment_id && environment.name == "Staging"
        ));
        assert_eq!(
            harness
                .state()
                .database
                .as_ref()
                .expect("database should exist")
                .list_environments(harness.state().project.id)
                .expect("environments should load")[0]
                .name,
            "Staging"
        );
    }
}
