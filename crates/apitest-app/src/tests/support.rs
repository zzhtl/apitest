use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use apitest_core::{
    Environment, ExecutionEvent, ExecutionMetrics, ExecutionRequest, ExecutionStream,
    ProtocolExecutor, ProtocolKind, ResponseHead,
};
use apitest_runtime::{ExecutorRegistry, ScriptEngine};
use apitest_storage::{Database, MemorySecretStore, SecretStore};
use chrono::Utc;
use futures::stream;
use indexmap::IndexMap;
use tokio_util::sync::CancellationToken;

use crate::app::ApiTestApp;
use crate::environment::EnvironmentDraft;
use crate::i18n::Language;
use crate::persistence::StorageWorker;
use crate::state::action::{OpenApiPreviewTab, Toasts};
use crate::state::session::{DocumentSession, Sessions};
use crate::state::workspace::{Navigation, ResourcePage, WorkspaceRequest};
use crate::theme;
use crate::theme::ThemeMode;
use crate::workbench::{DocumentId, DocumentKind, DocumentTabs};

pub(super) struct FakeExecutor;

pub(super) struct HistoryExecutor;

/// Streams a small JSON document so assertions and extractors have something
/// real to work on.
pub(super) struct JsonExecutor;

impl ProtocolExecutor for JsonExecutor {
    fn kind(&self) -> ProtocolKind {
        ProtocolKind::Http
    }

    fn execute(
        &self,
        request: ExecutionRequest,
        _cancellation: CancellationToken,
    ) -> ExecutionStream {
        let mut headers = IndexMap::new();
        headers.insert(
            "content-type".to_owned(),
            vec!["application/json".to_owned()],
        );
        Box::pin(stream::iter([
            Ok(ExecutionEvent::Started {
                id: request.id,
                at: Utc::now(),
            }),
            Ok(ExecutionEvent::ResponseHead(ResponseHead {
                status: Some(200),
                version: Some("HTTP/1.1".into()),
                headers,
            })),
            Ok(ExecutionEvent::Data(
                br#"{"token":"abc123","count":2}"#.to_vec().into(),
            )),
            Ok(ExecutionEvent::Completed(ExecutionMetrics {
                elapsed_ms: 5,
                received_bytes: 28,
                sent_bytes: 0,
            })),
        ]))
    }
}

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

pub(super) fn mock_get(address: SocketAddr, target: &str, headers: &[(&str, &str)]) -> String {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1))
        .expect("mock server should accept connections");
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("read timeout should configure");
    let mut request = format!("GET {target} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
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

pub(super) fn test_app(context: &mut eframe::CreationContext<'_>) -> ApiTestApp {
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
        scripts: ScriptEngine::default(),
        secrets,
        database: Some(database),
        body_store: None,
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
        sessions: Sessions::default(),
        idle_session: DocumentSession::default(),
        sender,
        receiver,
        theme: ThemeMode::Dark,
        language: Language::Chinese,
        search: String::new(),
        toasts: Toasts::default(),
        confirmation: None,
        document_tabs,
        show_settings: false,
        show_curl_import: false,
        curl_import_source: String::new(),
        show_openapi_preview: false,
        show_snippet: false,
        show_palette: false,
        palette_query: String::new(),
        rename_target: None,
        snippet_language: apitest_interop::CodeLanguage::Curl,
        openapi_preview_tab: OpenApiPreviewTab::Yaml,
        openapi_source: String::new(),
        openapi_html: String::new(),
        openapi_issues: Vec::new(),
        allow_close: false,
        last_edit_sweep: None,
        search_cache: Default::default(),
    }
}
