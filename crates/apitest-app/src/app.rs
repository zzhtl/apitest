use std::sync::{Arc, mpsc};

use apitest_core::{
    ApiDefinition, EntityId, Environment, ExecutionError, ExecutionEvent, ExecutionMetrics,
    ExecutionRequest, HttpMethod, Project, ProtocolExecutor, ProtocolSpec, ResponseHead,
};
use apitest_runtime::HttpExecutor;
use apitest_storage::{Database, SystemSecretStore};
use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::{
    draft::{BodyMode, EditablePair, RequestDraft},
    theme::{self, Palette, ThemeMode},
};

const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    Chinese,
    English,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Navigation {
    Api,
    Environment,
    Mock,
    Runner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorTab {
    Params,
    Headers,
    Body,
    Auth,
    Scripts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseTab {
    Body,
    Headers,
    Timeline,
}

struct WorkspaceRequest {
    id: Option<EntityId>,
    name: String,
    draft: RequestDraft,
    dirty: bool,
}

#[derive(Default)]
struct ResponseView {
    running: bool,
    status: Option<u16>,
    version: Option<String>,
    headers: Vec<(String, String)>,
    body: String,
    metrics: Option<ExecutionMetrics>,
    error: Option<String>,
    truncated: bool,
}

enum RuntimeMessage {
    Event(u64, Result<ExecutionEvent, ExecutionError>),
}

pub struct ApiTestApp {
    runtime: Arc<tokio::runtime::Runtime>,
    executor: HttpExecutor,
    database: Option<Arc<Database>>,
    project: Project,
    environment: Environment,
    requests: Vec<WorkspaceRequest>,
    selected: usize,
    navigation: Navigation,
    editor_tab: EditorTab,
    response_tab: ResponseTab,
    response: ResponseView,
    sender: mpsc::Sender<RuntimeMessage>,
    receiver: mpsc::Receiver<RuntimeMessage>,
    cancellation: Option<CancellationToken>,
    run_id: u64,
    theme: ThemeMode,
    language: Language,
    search: String,
    toast: Option<String>,
    show_settings: bool,
}

impl ApiTestApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        theme::install_cjk_font(&context.egui_ctx);
        let theme = ThemeMode::Dark;
        theme::apply(&context.egui_ctx, theme);
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("apitest-runtime")
                .build()
                .expect("ApiTest async runtime should initialize"),
        );
        let executor = HttpExecutor::new(Arc::new(SystemSecretStore::new("ApiTest")));
        let database = open_database().map(Arc::new);
        let (project, requests, toast) = load_workspace(database.as_deref());
        let (sender, receiver) = mpsc::channel();

        Self {
            runtime,
            executor,
            database,
            project,
            environment: Environment::new("Local"),
            requests,
            selected: 0,
            navigation: Navigation::Api,
            editor_tab: EditorTab::Params,
            response_tab: ResponseTab::Body,
            response: ResponseView::default(),
            sender,
            receiver,
            cancellation: None,
            run_id: 0,
            theme,
            language: Language::Chinese,
            search: String::new(),
            toast,
            show_settings: false,
        }
    }

    fn tr<'a>(&self, chinese: &'a str, english: &'a str) -> &'a str {
        match self.language {
            Language::Chinese => chinese,
            Language::English => english,
        }
    }

    fn send_current(&mut self, context: &egui::Context) {
        let Some(item) = self.requests.get(self.selected) else {
            return;
        };
        if item.draft.url.trim().is_empty() {
            self.toast = Some(self.tr("请输入请求地址", "Enter a request URL").into());
            return;
        }
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        self.run_id = self.run_id.wrapping_add(1);
        let run_id = self.run_id;
        let cancellation = CancellationToken::new();
        self.cancellation = Some(cancellation.clone());
        self.response = ResponseView {
            running: true,
            ..Default::default()
        };

        let request = ExecutionRequest::new(
            ProtocolSpec::Http(item.draft.to_http_spec()),
            self.environment.clone(),
        );
        let executor = self.executor.clone();
        let sender = self.sender.clone();
        let context = context.clone();
        self.runtime.spawn(async move {
            let mut stream = executor.execute(request, cancellation);
            while let Some(event) = stream.next().await {
                if sender.send(RuntimeMessage::Event(run_id, event)).is_err() {
                    break;
                }
                context.request_repaint();
            }
        });
    }

    fn stop(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
    }

    fn drain_runtime(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                RuntimeMessage::Event(run_id, event) if run_id == self.run_id => {
                    self.apply_runtime_event(event)
                }
                RuntimeMessage::Event(_, _) => {}
            }
        }
    }

    fn apply_runtime_event(&mut self, event: Result<ExecutionEvent, ExecutionError>) {
        match event {
            Ok(ExecutionEvent::Started { .. }) => {}
            Ok(ExecutionEvent::ResponseHead(head)) => self.apply_response_head(head),
            Ok(ExecutionEvent::Data(data)) => self.append_body(&data),
            Ok(ExecutionEvent::Message { data, .. }) => {
                self.append_body(&data);
                self.response.body.push('\n');
            }
            Ok(ExecutionEvent::Metrics(metrics)) => self.response.metrics = Some(metrics),
            Ok(ExecutionEvent::Completed(metrics)) => {
                self.response.running = false;
                self.response.metrics = Some(metrics);
                self.cancellation = None;
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.response.body)
                    && let Ok(pretty) = serde_json::to_string_pretty(&value)
                {
                    self.response.body = pretty;
                }
            }
            Err(error) => {
                self.response.running = false;
                self.response.error = Some(error.to_string());
                self.cancellation = None;
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

    fn append_body(&mut self, bytes: &[u8]) {
        if self.response.body.len() >= MAX_RESPONSE_BYTES {
            self.response.truncated = true;
            return;
        }
        let available = MAX_RESPONSE_BYTES - self.response.body.len();
        let truncated = bytes.len() > available;
        let bytes = &bytes[..bytes.len().min(available)];
        self.response.body.push_str(&String::from_utf8_lossy(bytes));
        self.response.truncated |= truncated;
    }

    fn new_request(&mut self) {
        self.requests.push(WorkspaceRequest {
            id: None,
            name: self.tr("未命名请求", "Untitled request").into(),
            draft: RequestDraft::default(),
            dirty: true,
        });
        self.selected = self.requests.len() - 1;
        self.navigation = Navigation::Api;
        self.response = ResponseView::default();
    }

    fn save_current(&mut self) {
        let Some(database) = &self.database else {
            self.toast = Some(
                self.tr("本地数据库不可用", "Local database unavailable")
                    .into(),
            );
            return;
        };
        let Some(item) = self.requests.get_mut(self.selected) else {
            return;
        };
        let mut definition = ApiDefinition::new(
            item.name.clone(),
            ProtocolSpec::Http(item.draft.to_http_spec()),
        );
        if let Some(id) = item.id {
            definition.id = id;
        }
        match database.save_definition(self.project.id, &definition) {
            Ok(()) => {
                item.id = Some(definition.id);
                item.dirty = false;
                self.toast = Some(self.tr("已保存", "Saved").into());
            }
            Err(error) => self.toast = Some(error.to_string()),
        }
    }

    fn keyboard_shortcuts(&mut self, context: &egui::Context) {
        let (send, save, new_request) = context.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::Enter),
                input.modifiers.command && input.key_pressed(egui::Key::S),
                input.modifiers.command && input.key_pressed(egui::Key::N),
            )
        });
        if send {
            self.send_current(context);
        }
        if save {
            self.save_current();
        }
        if new_request {
            self.new_request();
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                let hint = self.tr("搜索接口  ⌘K", "Search APIs  ⌘K");
                let new_label = self.tr("＋ 新建", "＋ New");
                let theme_tip = self.tr("切换主题", "Toggle theme");
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("A")
                            .size(19.0)
                            .strong()
                            .color(Color32::WHITE)
                            .background_color(palette.accent),
                    );
                    ui.label(RichText::new("ApiTest").size(17.0).strong());
                    ui.separator();
                    ui.label(RichText::new(&self.project.name).color(palette.muted));
                    ui.add_space(12.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text(hint)
                            .desired_width(240.0),
                    );
                    ui.add_space((ui.available_width() - 280.0).max(0.0));
                    egui::ComboBox::from_id_salt("environment")
                        .selected_text(format!("◉ {}", self.environment.name))
                        .width(105.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.environment.name,
                                "Local".into(),
                                "Local",
                            );
                        });
                    if ui.button(new_label).clicked() {
                        self.new_request();
                    }
                    if ui.button("◐").on_hover_text(theme_tip).clicked() {
                        self.theme = match self.theme {
                            ThemeMode::Dark => ThemeMode::Light,
                            ThemeMode::Light => ThemeMode::Dark,
                        };
                        theme::apply(ui.ctx(), self.theme);
                    }
                    if ui.button("⚙").clicked() {
                        self.show_settings = true;
                    }
                });
            });
    }

    fn activity_bar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        egui::Frame::new()
            .fill(palette.canvas)
            .inner_margin(egui::Margin::symmetric(7, 12))
            .show(ui, |ui| {
                ui.set_min_width(52.0);
                ui.vertical_centered(|ui| {
                    nav_button(
                        ui,
                        &mut self.navigation,
                        Navigation::Api,
                        "API",
                        "接口",
                        palette,
                    );
                    nav_button(
                        ui,
                        &mut self.navigation,
                        Navigation::Environment,
                        "ENV",
                        "环境",
                        palette,
                    );
                    nav_button(
                        ui,
                        &mut self.navigation,
                        Navigation::Mock,
                        "M",
                        "Mock",
                        palette,
                    );
                    nav_button(
                        ui,
                        &mut self.navigation,
                        Navigation::Runner,
                        "RUN",
                        "测试",
                        palette,
                    );
                });
            });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(12, 12))
            .show(ui, |ui| {
                ui.set_min_width(224.0);
                match self.navigation {
                    Navigation::Api => self.api_sidebar(ui, palette),
                    Navigation::Environment => placeholder_sidebar(
                        ui,
                        self.tr("环境变量", "Environments"),
                        self.tr(
                            "集中管理普通变量与密钥引用",
                            "Manage variables and secret references",
                        ),
                        palette,
                    ),
                    Navigation::Mock => placeholder_sidebar(
                        ui,
                        "Mock",
                        self.tr(
                            "从接口示例快速启动本地 Mock",
                            "Start local mocks from examples",
                        ),
                        palette,
                    ),
                    Navigation::Runner => placeholder_sidebar(
                        ui,
                        self.tr("自动化测试", "Test runner"),
                        self.tr(
                            "串行执行场景并汇总断言",
                            "Run scenarios and aggregate assertions",
                        ),
                        palette,
                    ),
                }
            });
    }

    fn api_sidebar(&mut self, ui: &mut egui::Ui, palette: Palette) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(self.tr("接口管理", "APIs"))
                    .strong()
                    .size(15.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("＋").clicked() {
                    self.new_request();
                }
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("⌄").color(palette.muted));
            ui.label(RichText::new(&self.project.name).strong());
        });
        ui.add_space(5.0);
        let search = self.search.to_ascii_lowercase();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, request) in self.requests.iter().enumerate() {
                if !search.is_empty() && !request.name.to_ascii_lowercase().contains(&search) {
                    continue;
                }
                let selected = self.selected == index;
                let text = format!(
                    "{}  {}{}",
                    request.draft.method,
                    request.name,
                    if request.dirty { " •" } else { "" }
                );
                let response = ui.selectable_label(selected, RichText::new(text).size(13.0));
                if response.clicked() {
                    self.selected = index;
                    self.response = ResponseView::default();
                }
            }
        });
    }

    fn editor(&mut self, ui: &mut egui::Ui, palette: Palette) {
        let Some(_) = self.requests.get(self.selected) else {
            ui.centered_and_justified(|ui| {
                if ui.button(self.tr("新建请求", "Create request")).clicked() {
                    self.new_request();
                }
            });
            return;
        };
        let mut send = false;
        let mut stop = false;
        let mut save = false;
        egui::Frame::new()
            .fill(palette.panel)
            .inner_margin(egui::Margin::symmetric(18, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let item = &mut self.requests[self.selected];
                    ui.add(
                        egui::TextEdit::singleline(&mut item.name)
                            .font(egui::TextStyle::Heading)
                            .desired_width(260.0),
                    );
                    if ui.small_button(self.tr("保存", "Save")).clicked() {
                        save = true;
                    }
                });
                ui.add_space(5.0);
                ui.horizontal(|ui| {
                    let item = &mut self.requests[self.selected];
                    method_combo(ui, &mut item.draft.method, palette);
                    let url = ui.add_sized(
                        [ui.available_width() - 112.0, 38.0],
                        egui::TextEdit::singleline(&mut item.draft.url)
                            .hint_text("https://api.example.com/v1/users/{{id}}"),
                    );
                    item.dirty |= url.changed();
                    let button = if self.response.running {
                        egui::Button::new(self.tr("■ 停止", "■ Stop")).fill(palette.danger)
                    } else {
                        egui::Button::new(self.tr("▶ 发送", "▶ Send")).fill(palette.accent)
                    };
                    if ui.add_sized([96.0, 38.0], button).clicked() {
                        if self.response.running {
                            stop = true;
                        } else {
                            send = true;
                        }
                    }
                });
                ui.add_space(8.0);
                editor_tabs(ui, &mut self.editor_tab, self.language, palette);
                ui.separator();
                let available = (ui.available_height() * 0.46).max(150.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), available),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        match self.editor_tab {
                            EditorTab::Params => editable_pairs(
                                ui,
                                &mut self.requests[self.selected].draft.query,
                                palette,
                            ),
                            EditorTab::Headers => editable_pairs(
                                ui,
                                &mut self.requests[self.selected].draft.headers,
                                palette,
                            ),
                            EditorTab::Body => {
                                body_editor(ui, &mut self.requests[self.selected].draft, palette)
                            }
                            EditorTab::Auth => empty_state(
                                ui,
                                match self.language { Language::Chinese => "认证", Language::English => "Authentication" },
                                match self.language { Language::Chinese => "支持 Basic、Bearer、API Key；密钥仅保存到系统钥匙串。", Language::English => "Basic, Bearer and API Key secrets are stored in the system keychain." },
                                palette,
                            ),
                            EditorTab::Scripts => empty_state(
                                ui,
                                match self.language { Language::Chinese => "前置 / 后置脚本", Language::English => "Pre / post scripts" },
                                match self.language { Language::Chinese => "QuickJS 沙箱支持变量和响应断言。", Language::English => "The QuickJS sandbox supports variables and response assertions." },
                                palette,
                            ),
                        }
                        self.requests[self.selected].draft.ensure_empty_rows();
                    },
                );
                ui.separator();
                self.response_panel(ui, palette);
            });
        if send {
            self.send_current(ui.ctx());
        }
        if stop {
            self.stop();
        }
        if save {
            self.save_current();
        }
    }

    fn response_panel(&mut self, ui: &mut egui::Ui, palette: Palette) {
        ui.horizontal(|ui| {
            for (tab, chinese, english) in [
                (ResponseTab::Body, "响应体", "Body"),
                (ResponseTab::Headers, "响应头", "Headers"),
                (ResponseTab::Timeline, "时间线", "Timeline"),
            ] {
                if ui
                    .selectable_label(self.response_tab == tab, self.tr(chinese, english))
                    .clicked()
                {
                    self.response_tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(metrics) = self.response.metrics {
                    ui.label(
                        RichText::new(format!(
                            "{} ms  ·  {} B",
                            metrics.elapsed_ms, metrics.received_bytes
                        ))
                        .color(palette.muted),
                    );
                }
                if let Some(status) = self.response.status {
                    let color = if status < 400 {
                        palette.success
                    } else {
                        palette.danger
                    };
                    ui.label(RichText::new(status.to_string()).strong().color(color));
                }
                if self.response.running {
                    ui.spinner();
                }
            });
        });
        ui.add_space(4.0);
        if let Some(error) = &self.response.error {
            ui.colored_label(palette.danger, error);
        }
        match self.response_tab {
            ResponseTab::Body => {
                if self.response.body.is_empty() && !self.response.running {
                    empty_state(
                        ui,
                        self.tr("暂无响应", "No response yet"),
                        self.tr(
                            "发送请求后，响应会显示在这里。",
                            "Send a request to inspect its response.",
                        ),
                        palette,
                    );
                } else {
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.response.body)
                                .code_editor()
                                .interactive(false)
                                .desired_width(f32::INFINITY)
                                .desired_rows(10),
                        );
                        if self.response.truncated {
                            ui.colored_label(
                                palette.warning,
                                self.tr(
                                    "响应超过 10 MiB，已截断显示。",
                                    "Response display was truncated at 10 MiB.",
                                ),
                            );
                        }
                    });
                }
            }
            ResponseTab::Headers => {
                egui::Grid::new("response_headers")
                    .num_columns(2)
                    .striped(true)
                    .show(ui, |ui| {
                        for (name, value) in &self.response.headers {
                            ui.label(RichText::new(name).strong());
                            ui.label(value);
                            ui.end_row();
                        }
                    });
            }
            ResponseTab::Timeline => {
                ui.label(
                    RichText::new(self.tr(
                        "请求开始 → 响应头 → 流式数据 → 完成",
                        "Started → response head → streamed data → completed",
                    ))
                    .color(palette.muted),
                );
            }
        }
    }

    fn settings_window(&mut self, context: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new(self.tr("设置", "Settings"))
            .open(&mut open)
            .resizable(false)
            .default_width(380.0)
            .show(context, |ui| {
                ui.heading(self.tr("外观与语言", "Appearance & language"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(self.tr("语言", "Language"));
                    ui.selectable_value(&mut self.language, Language::Chinese, "简体中文");
                    ui.selectable_value(&mut self.language, Language::English, "English");
                });
                ui.horizontal(|ui| {
                    ui.label(self.tr("主题", "Theme"));
                    let dark_label = self.tr("深色", "Dark");
                    let light_label = self.tr("浅色", "Light");
                    if ui
                        .selectable_value(&mut self.theme, ThemeMode::Dark, dark_label)
                        .changed()
                        || ui
                            .selectable_value(&mut self.theme, ThemeMode::Light, light_label)
                            .changed()
                    {
                        theme::apply(context, self.theme);
                    }
                });
                ui.separator();
                ui.label(
                    RichText::new(self.tr(
                        "项目数据保存在本机 SQLite；敏感值保存在系统钥匙串。",
                        "Project data stays in local SQLite; secrets stay in the system keychain.",
                    ))
                    .color(Palette::for_mode(self.theme).muted),
                );
            });
        self.show_settings = open;
    }
}

impl eframe::App for ApiTestApp {
    fn logic(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_runtime();
        self.keyboard_shortcuts(context);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let palette = Palette::for_mode(self.theme);
        egui::Panel::top("top_bar")
            .exact_size(52.0)
            .frame(egui::Frame::new().fill(palette.panel))
            .show(ui, |ui| self.top_bar(ui, palette));
        egui::Panel::left("activity")
            .exact_size(66.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette.canvas)
                    .stroke(Stroke::new(1.0, palette.border)),
            )
            .show(ui, |ui| self.activity_bar(ui, palette));
        egui::Panel::left("sidebar")
            .default_size(248.0)
            .min_size(200.0)
            .max_size(340.0)
            .frame(
                egui::Frame::new()
                    .fill(palette.panel)
                    .stroke(Stroke::new(1.0, palette.border)),
            )
            .show(ui, |ui| self.sidebar(ui, palette));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(palette.panel))
            .show(ui, |ui| self.editor(ui, palette));

        if self.show_settings {
            self.settings_window(ui.ctx());
        }
        if let Some(toast) = self.toast.clone() {
            egui::Area::new("toast".into())
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -24.0])
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(toast);
                            if ui.small_button("×").clicked() {
                                self.toast = None;
                            }
                        });
                    });
                });
        }
    }
}

fn open_database() -> Option<Database> {
    let directory = directories::ProjectDirs::from("io.github", "zzhtl", "ApiTest")?;
    let path = directory.data_local_dir().join("apitest.sqlite3");
    match Database::open(path) {
        Ok(database) => Some(database),
        Err(error) => {
            tracing::error!(%error, "failed to open ApiTest database");
            None
        }
    }
}

fn load_workspace(database: Option<&Database>) -> (Project, Vec<WorkspaceRequest>, Option<String>) {
    let mut toast = None;
    let project = database
        .and_then(|database| database.list_projects().ok())
        .and_then(|projects| projects.into_iter().next())
        .unwrap_or_else(|| {
            let project = Project::new("My Workspace");
            if let Some(database) = database
                && let Err(error) = database.save_project(&project)
            {
                toast = Some(error.to_string());
            }
            project
        });
    let mut requests = database
        .and_then(|database| database.list_definitions(project.id).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|definition| match &definition.protocol {
            ProtocolSpec::Http(spec) => Some(WorkspaceRequest {
                id: Some(definition.id),
                name: definition.name,
                draft: RequestDraft::from(spec),
                dirty: false,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    if requests.is_empty() {
        requests.push(WorkspaceRequest {
            id: None,
            name: "Untitled request".into(),
            draft: RequestDraft {
                url: "https://httpbin.org/get".into(),
                ..Default::default()
            },
            dirty: true,
        });
    }
    (project, requests, toast)
}

fn nav_button(
    ui: &mut egui::Ui,
    current: &mut Navigation,
    target: Navigation,
    text: &str,
    tooltip: &str,
    palette: Palette,
) {
    let selected = *current == target;
    let button = egui::Button::new(RichText::new(text).size(11.0).strong())
        .fill(if selected {
            palette.accent
        } else {
            Color32::TRANSPARENT
        })
        .stroke(Stroke::NONE);
    if ui
        .add_sized([48.0, 42.0], button)
        .on_hover_text(tooltip)
        .clicked()
    {
        *current = target;
    }
}

fn placeholder_sidebar(ui: &mut egui::Ui, title: &str, description: &str, palette: Palette) {
    ui.label(RichText::new(title).strong().size(15.0));
    ui.add_space(8.0);
    ui.label(RichText::new(description).color(palette.muted));
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
            ] {
                ui.selectable_value(method, value, value.to_string());
            }
        });
}

fn method_color(method: HttpMethod, palette: Palette) -> Color32 {
    match method {
        HttpMethod::Get | HttpMethod::Head => palette.success,
        HttpMethod::Post => palette.accent_hover,
        HttpMethod::Put | HttpMethod::Patch => palette.warning,
        HttpMethod::Delete => palette.danger,
        _ => palette.muted,
    }
}

fn editor_tabs(ui: &mut egui::Ui, selected: &mut EditorTab, language: Language, palette: Palette) {
    ui.horizontal(|ui| {
        for (tab, chinese, english) in [
            (EditorTab::Params, "参数", "Params"),
            (EditorTab::Headers, "请求头", "Headers"),
            (EditorTab::Body, "请求体", "Body"),
            (EditorTab::Auth, "认证", "Auth"),
            (EditorTab::Scripts, "脚本", "Scripts"),
        ] {
            let label = match language {
                Language::Chinese => chinese,
                Language::English => english,
            };
            if ui.selectable_label(*selected == tab, label).clicked() {
                *selected = tab;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("⌘ ↵").color(palette.muted).size(12.0));
        });
    });
}

fn editable_pairs(ui: &mut egui::Ui, pairs: &mut Vec<EditablePair>, palette: Palette) {
    let mut remove = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new(ui.next_auto_id())
            .num_columns(5)
            .spacing([8.0, 7.0])
            .show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new("Key").color(palette.muted));
                ui.label(RichText::new("Value").color(palette.muted));
                ui.label(RichText::new("Description").color(palette.muted));
                ui.label("");
                ui.end_row();
                for (index, pair) in pairs.iter_mut().enumerate() {
                    ui.checkbox(&mut pair.enabled, "");
                    ui.add(egui::TextEdit::singleline(&mut pair.name).desired_width(170.0));
                    ui.add(egui::TextEdit::singleline(&mut pair.value).desired_width(230.0));
                    ui.add(egui::TextEdit::singleline(&mut pair.description).desired_width(170.0));
                    if !pair.is_empty() && ui.small_button("×").clicked() {
                        remove = Some(index);
                    }
                    ui.end_row();
                }
            });
    });
    if let Some(index) = remove {
        pairs.remove(index);
    }
}

fn body_editor(ui: &mut egui::Ui, draft: &mut RequestDraft, palette: Palette) {
    ui.horizontal(|ui| {
        for (mode, label) in [
            (BodyMode::None, "none"),
            (BodyMode::Json, "JSON"),
            (BodyMode::Text, "Text"),
            (BodyMode::Xml, "XML"),
        ] {
            ui.selectable_value(&mut draft.body_mode, mode, label);
        }
        if draft.body_mode == BodyMode::Json
            && ui.small_button("Format").clicked()
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(&draft.body)
            && let Ok(pretty) = serde_json::to_string_pretty(&value)
        {
            draft.body = pretty;
        }
    });
    if draft.body_mode == BodyMode::None {
        empty_state(
            ui,
            "No body",
            "Select JSON, Text or XML to add a request body.",
            palette,
        );
    } else {
        egui::ScrollArea::both().show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut draft.body)
                    .code_editor()
                    .desired_rows(10)
                    .desired_width(f32::INFINITY),
            );
        });
    }
}

fn empty_state(ui: &mut egui::Ui, title: &str, description: &str, palette: Palette) {
    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        ui.label(RichText::new(title).strong().size(15.0));
        ui.label(RichText::new(description).color(palette.muted));
    });
}
