use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, mpsc},
    time::Instant,
};

use apitest_core::{
    EntityId, MockProfile, Project, ProtocolExecutor, ProtocolKind, RunRecord, TestScenario,
};
use apitest_interop::{CodeLanguage, OpenApiIssue};
use apitest_runtime::{
    ExecutorRegistry, GrpcExecutor, HttpExecutor, MockServer, ScenarioReport, ScriptEngine,
    WebSocketExecutor,
};
use apitest_storage::{BodyStore, Database, PageRequest, SecretStore, SystemSecretStore};
use eframe::egui::{self, Stroke};
use tokio_util::sync::CancellationToken;

use crate::environment::EnvironmentDraft;
use crate::i18n::{self, Language};
use crate::persistence::StorageWorker;
use crate::services::document::{SearchCache, document_snapshot};
use crate::services::history::HISTORY_MAX_RECORDS;
use crate::services::loader::{
    active_environment_setting, load_automation, load_document_tabs, load_setting, load_workspace,
    open_database,
};
use crate::state::action::{
    Confirmation, OpenApiPreviewTab, PendingAction, RuntimeMessage, ToastKind, Toasts,
};
use crate::state::session::{DocumentSession, Sessions};
use crate::state::workspace::{Navigation, ResourcePage, WorkspaceRequest};
use crate::theme::tokens::size;
use crate::theme::{self, ThemeMode, UiExt};
use crate::workbench::{DocumentKind, DocumentTabs};

pub(crate) const THEME_SETTING: &str = "ui.theme";

pub(crate) const LANGUAGE_SETTING: &str = "ui.language";

pub(crate) const ACTIVE_ENVIRONMENT_SETTING: &str = "ui.active_environment";

pub(crate) const ACTIVE_PROJECT_SETTING: &str = "ui.active_project";

pub(crate) const DOCUMENT_TABS_SETTING: &str = "ui.document_tabs";

pub struct ApiTestApp {
    pub(crate) runtime: Arc<tokio::runtime::Runtime>,
    pub(crate) executors: Arc<ExecutorRegistry>,
    pub(crate) scripts: ScriptEngine,
    pub(crate) secrets: Arc<dyn SecretStore>,
    pub(crate) database: Option<Arc<Database>>,
    pub(crate) body_store: Option<BodyStore>,
    pub(crate) run_records: Vec<RunRecord>,
    pub(crate) selected_history: usize,
    pub(crate) history_body_preview: String,
    pub(crate) history_body_truncated: bool,
    /// Display rows of `history_body_preview` for the virtualized viewer,
    /// rebuilt whenever the preview is loaded.
    pub(crate) history_preview_rows: Vec<std::ops::Range<usize>>,
    pub(crate) storage_worker: Option<StorageWorker>,
    pub(crate) projects: Vec<Project>,
    pub(crate) project: Project,
    pub(crate) requests: Vec<WorkspaceRequest>,
    pub(crate) resource_pages: HashMap<Option<EntityId>, ResourcePage>,
    pub(crate) expanded_resources: HashSet<EntityId>,
    pub(crate) environments: Vec<EnvironmentDraft>,
    pub(crate) scenarios: Vec<TestScenario>,
    pub(crate) selected_scenario: usize,
    pub(crate) scenario_snapshots: HashMap<EntityId, Vec<u8>>,
    pub(crate) scenario_report: Option<ScenarioReport>,
    pub(crate) scenario_cancellation: Option<CancellationToken>,
    pub(crate) scenario_run_id: u64,
    pub(crate) mock_profiles: Vec<MockProfile>,
    pub(crate) selected_mock: usize,
    pub(crate) mock_snapshots: HashMap<EntityId, Vec<u8>>,
    pub(crate) mock_server: Option<MockServer>,
    pub(crate) mock_run_id: u64,
    pub(crate) selected: usize,
    pub(crate) selected_environment: usize,
    pub(crate) active_environment: EntityId,
    pub(crate) navigation: Navigation,
    /// Per-tab editor selection, response and in-flight run.
    pub(crate) sessions: Sessions,
    /// Scratch session used while no request is selected.
    pub(crate) idle_session: DocumentSession,
    pub(crate) sender: mpsc::Sender<RuntimeMessage>,
    pub(crate) receiver: mpsc::Receiver<RuntimeMessage>,
    pub(crate) theme: ThemeMode,
    pub(crate) language: Language,
    pub(crate) search: String,
    pub(crate) toasts: Toasts,
    pub(crate) confirmation: Option<Confirmation>,
    pub(crate) document_tabs: DocumentTabs,
    pub(crate) show_settings: bool,
    pub(crate) show_curl_import: bool,
    pub(crate) curl_import_source: String,
    pub(crate) show_openapi_preview: bool,
    pub(crate) show_snippet: bool,
    pub(crate) show_palette: bool,
    pub(crate) palette_query: String,
    /// Node being renamed inline in the resource tree, with its draft name.
    pub(crate) rename_target: Option<(EntityId, String)>,
    pub(crate) snippet_language: CodeLanguage,
    pub(crate) openapi_preview_tab: OpenApiPreviewTab,
    pub(crate) openapi_source: String,
    pub(crate) openapi_html: String,
    pub(crate) openapi_issues: Vec<OpenApiIssue>,
    pub(crate) allow_close: bool,
    /// When the edit-snapshot sweep last ran; see `sync_edit_snapshots`.
    pub(crate) last_edit_sweep: Option<Instant>,
    /// Cached sidebar/palette search results; see `cached_search_hits`.
    pub(crate) search_cache: SearchCache,
    /// Cached snippet output keyed by request, language and edit revision.
    pub(crate) snippet_cache: Option<SnippetCache>,
}

/// See `ApiTestApp::snippet_window`: regenerating the snippet every frame the
/// window stays open re-cloned the whole protocol per frame.
pub(crate) struct SnippetCache {
    pub(crate) request: EntityId,
    pub(crate) language: CodeLanguage,
    pub(crate) revision: u64,
    pub(crate) source: Result<String, String>,
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
        let font_report = theme::install_fonts(&context.egui_ctx);
        if font_report.is_missing() {
            startup_errors.push(
                i18n::tr(
                    language,
                    "未检测到中文字体，中文界面会显示为方块；请安装 Noto Sans CJK SC 或思源黑体",
                    "No Chinese font was found; Chinese labels will render as boxes. Install Noto Sans CJK SC or Source Han Sans.",
                )
                .to_owned(),
            );
        }
        tracing::info!(
            cjk_family = font_report.cjk_family.as_deref().unwrap_or("<none>"),
            scanned_fallback_paths = font_report.scanned_fallback_paths,
            "installed application fonts"
        );
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
            .map(|database| StorageWorker::new(Arc::clone(database), body_store.clone()));
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
            scripts: ScriptEngine::default(),
            secrets,
            database,
            body_store,
            run_records,
            selected_history: 0,
            history_body_preview: String::new(),
            history_body_truncated: false,
            history_preview_rows: Vec::new(),
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
            sessions: Sessions::default(),
            idle_session: DocumentSession::default(),
            sender,
            receiver,
            theme,
            language,
            search: String::new(),
            toasts: startup_errors
                .into_iter()
                .fold(Toasts::default(), |mut toasts, message| {
                    toasts.push(ToastKind::Error, message);
                    toasts
                }),
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
            snippet_language: CodeLanguage::Curl,
            openapi_preview_tab: OpenApiPreviewTab::Yaml,
            openapi_source: String::new(),
            openapi_html: String::new(),
            openapi_issues: Vec::new(),
            allow_close: false,
            last_edit_sweep: None,
            search_cache: SearchCache::default(),
            snippet_cache: None,
        }
    }
}

impl eframe::App for ApiTestApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_runtime(context);
        self.drain_storage();
        self.keyboard_shortcuts(context);
        self.sync_edit_snapshots(context);
        self.schedule_request_autosaves(context);
        self.schedule_environment_autosaves(context);
        if self.sessions.any_active() {
            // Keep the elapsed/byte readouts ticking while something streams.
            context.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if context.input(|input| input.viewport().close_requested())
            && !self.allow_close
            && self.workspace_dirty_strict()
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
        let palette = ui.palette();
        egui::Panel::top("top_bar")
            .exact_size(size::TOP_BAR)
            .frame(
                egui::Frame::new()
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, palette.divider)),
            )
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status_bar")
            .exact_size(size::STATUS_BAR)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette.rail_bg)
                    .stroke(Stroke::new(1.0, palette.divider)),
            )
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("activity")
            .exact_size(size::ACTIVITY_RAIL)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette.rail_bg)
                    .stroke(Stroke::new(1.0, palette.divider)),
            )
            .show(ui, |ui| self.activity_bar(ui));
        egui::Panel::left("sidebar")
            .default_size(size::SIDEBAR_DEFAULT)
            .min_size(size::SIDEBAR_MIN)
            .max_size(size::SIDEBAR_MAX)
            .frame(
                egui::Frame::new()
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, palette.divider)),
            )
            .show(ui, |ui| match self.navigation {
                Navigation::Api => self.api_sidebar(ui),
                Navigation::Scenario => self.scenario_sidebar(ui),
                Navigation::Mock => self.mock_sidebar(ui),
                Navigation::History => self.history_sidebar(ui),
                Navigation::Environment => self.environment_sidebar(ui),
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.surface))
            .show(ui, |ui| {
                self.document_tab_bar(ui);
                match self.navigation {
                    Navigation::Api => self.request_workspace(ui),
                    Navigation::Scenario => self.scenario_workspace(ui),
                    Navigation::Mock => self.mock_workspace(ui),
                    Navigation::History => self.history_workspace(ui),
                    Navigation::Environment => self.environment_workspace(ui),
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
        if self.show_snippet {
            self.snippet_window(ui.ctx());
        }
        self.command_palette(ui.ctx());
        self.rename_window(ui.ctx());
        self.confirmation_window(ui.ctx());
        self.toast_area(ui.ctx());
    }
}
