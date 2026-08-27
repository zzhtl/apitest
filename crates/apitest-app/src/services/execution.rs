use apitest_core::{
    ExecutionCommand, ExecutionError, ExecutionEvent, ExecutionRequest, ProtocolSpec, ResponseHead,
    RunState as HistoryRunState,
};
use eframe::egui::{self};
use futures::StreamExt;

use crate::app::ApiTestApp;
use crate::draft::{BodyMode, MultipartValueMode};
use crate::i18n::Language;
use crate::state::action::{RuntimeMessage, ToastKind};
use crate::state::response::{
    ResponseBodyMode, ResponseTab, ResponseView, RunState, TimelinePhase,
};
use crate::workbench::DocumentId;

impl ApiTestApp {
    pub(crate) fn validate_send(&self) -> Result<(), String> {
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
                    serde_json::from_str::<serde_json::Value>(&spec.variables).map_err(
                        |error| match self.language {
                            Language::Chinese => format!("GraphQL 变量无效：{error}"),
                            Language::English => format!("GraphQL variables: {error}"),
                        },
                    )?;
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
                        .map_err(|error| match self.language {
                            Language::Chinese => format!("gRPC 消息 JSON 无效：{error}"),
                            Language::English => format!("gRPC JSON: {error}"),
                        })
                }
            }
        }
    }

    pub(crate) fn send_current(&mut self, context: &egui::Context) {
        let Some(document) = self.active_api_document() else {
            return;
        };
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

        // Re-sending in the same tab supersedes whatever it was streaming.
        if self.sessions.entry(document).history_record.is_some() {
            let metrics = self.sessions.entry(document).response.metrics;
            self.finish_run_history(
                document,
                HistoryRunState::Cancelled,
                metrics,
                Some(
                    self.tr("已被新的执行取代", "superseded by a new execution")
                        .into(),
                ),
            );
        }
        let run = self.sessions.next_run();
        {
            let session = self.sessions.entry(document);
            session.cancel();
            session.run = run;
            session.response = ResponseView::running();
            session.response_tab = ResponseTab::Body;
            session.response_body_mode = ResponseBodyMode::Pretty;
            session.verification = None;
        }

        let script_variables = match self.run_pre_request_script(self.selected) {
            Ok(variables) => variables,
            Err(error) => {
                let session = self.sessions.entry(document);
                session.response = ResponseView::default();
                session.run = 0;
                self.toast(ToastKind::Error, error);
                return;
            }
        };
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
        request.local_variables.extend(script_variables);
        let handle = match self.executors.start(request) {
            Ok(handle) => handle,
            Err(error) => {
                let session = self.sessions.entry(document);
                session.response = ResponseView::default();
                session.run = 0;
                self.toast(ToastKind::Error, error.to_string());
                return;
            }
        };
        self.begin_run_history(document, request_case_id, redactions);
        let session = self.sessions.entry(document);
        session.execution_commands = handle.command_sender();
        session.cancellation = Some(handle.cancellation_token());
        let mut stream = handle.events;
        let sender = self.sender.clone();
        let context = context.clone();
        self.runtime.spawn(async move {
            while let Some(event) = stream.next().await {
                if sender.send(RuntimeMessage::Event(run, event)).is_err() {
                    return;
                }
                context.request_repaint();
            }
            let _ = sender.send(RuntimeMessage::Closed(run));
            context.request_repaint();
        });
    }

    pub(crate) fn stop(&mut self) {
        let session = self.session_mut();
        if let Some(commands) = &session.execution_commands {
            let _ = commands.try_send(ExecutionCommand::Cancel);
        }
        if session.cancellation.is_some() {
            session.cancel();
            session.response.state = RunState::Cancelling;
        }
    }

    /// Abandon the scenario run. Request tabs keep streaming: their runs belong
    /// to the tab, not to whatever the user is currently looking at.
    pub(crate) fn invalidate_scenario_run(&mut self) {
        if let Some(cancellation) = self.scenario_cancellation.take() {
            cancellation.cancel();
        }
        self.scenario_run_id = self.scenario_run_id.wrapping_add(1);
    }

    pub(crate) fn send_websocket_message(&mut self) {
        if self.session().websocket_message.trim().is_empty() {
            return;
        }
        let message = self.session().websocket_message.clone();
        match self.try_execution_command(ExecutionCommand::SendMessage {
            media_type: Some("text/plain; charset=utf-8".to_owned()),
            data: message.into(),
        }) {
            Ok(()) => self.session_mut().websocket_message.clear(),
            Err(error) => self.toast(ToastKind::Error, error),
        }
    }

    pub(crate) fn close_websocket_input(&mut self) {
        match self.try_execution_command(ExecutionCommand::CompleteInput) {
            Ok(()) => self.session_mut().execution_commands = None,
            Err(error) => self.toast(ToastKind::Error, error),
        }
    }

    pub(crate) fn try_execution_command(
        &mut self,
        command: ExecutionCommand,
    ) -> Result<(), String> {
        let Some(commands) = self.session().execution_commands.clone() else {
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
                self.session_mut().execution_commands = None;
                Err(self
                    .tr("连接已经关闭", "The connection is already closed")
                    .into())
            }
        }
    }

    pub(crate) fn drain_runtime(&mut self, context: &egui::Context) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                RuntimeMessage::Event(run, event) => {
                    if let Some(document) = self.sessions.owner(run) {
                        self.apply_runtime_event(document, event, context);
                    }
                }
                RuntimeMessage::Verified(run, outcome) => self.apply_verification(run, *outcome),
                RuntimeMessage::Closed(run) => {
                    let Some(document) = self.sessions.owner(run) else {
                        continue;
                    };
                    let stream_ended_early = self.tr(
                        "请求流意外结束，未收到完成事件",
                        "The request stream ended without a completion event",
                    );
                    let session = self.sessions.entry(document);
                    if session.response.state == RunState::Cancelling {
                        session.response.state = RunState::Cancelled;
                        session.response.push_timeline(TimelinePhase::Cancelled);
                        session.response.finish_body();
                    } else if session.response.state == RunState::Running {
                        session.response.state = RunState::Failed;
                        session.response.error = Some(stream_ended_early.to_owned());
                        session.response.push_timeline(TimelinePhase::Failed);
                        session.response.finish_body();
                    }
                    session.cancellation = None;
                    session.execution_commands = None;
                    if session.history_record.is_some() {
                        let state = if session.response.state == RunState::Cancelled {
                            HistoryRunState::Cancelled
                        } else {
                            HistoryRunState::Failed
                        };
                        let metrics = session.response.metrics;
                        let error = session.response.error.clone();
                        self.finish_run_history(document, state, metrics, error);
                    }
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
                RuntimeMessage::ScenarioFinished(_, _) | RuntimeMessage::MockStarted(_, _) => {}
            }
        }
    }

    pub(crate) fn apply_runtime_event(
        &mut self,
        document: DocumentId,
        event: Result<ExecutionEvent, ExecutionError>,
        context: &egui::Context,
    ) {
        match event {
            Ok(ExecutionEvent::Started { .. }) => {}
            Ok(ExecutionEvent::ResponseHead(head)) => {
                let session = self.sessions.entry(document);
                apply_response_head(&mut session.response, head);
                session.response.push_timeline(TimelinePhase::ResponseHead);
            }
            Ok(ExecutionEvent::Data(data)) => {
                self.write_run_history_body(document, &data);
                self.sessions.entry(document).response.append_body(&data);
            }
            Ok(ExecutionEvent::Message { data, .. }) => {
                self.write_run_history_body(document, &data);
                self.write_run_history_body(document, b"\n");
                let session = self.sessions.entry(document);
                session.response.append_body(&data);
                session.response.append_body(b"\n");
            }
            Ok(ExecutionEvent::Metrics(metrics)) => {
                self.sessions.entry(document).response.metrics = Some(metrics);
            }
            Ok(ExecutionEvent::Completed(metrics)) => {
                let session = self.sessions.entry(document);
                session.response.state = RunState::Completed;
                session.response.metrics = Some(metrics);
                session.response.finish_body();
                session.response.push_timeline(TimelinePhase::Completed);
                session.cancellation = None;
                session.execution_commands = None;
                self.finish_run_history(document, HistoryRunState::Passed, Some(metrics), None);
                let run = self.sessions.entry(document).run;
                self.spawn_verification(document, run, context);
            }
            Err(ExecutionError::Cancelled) => {
                let session = self.sessions.entry(document);
                session.response.state = RunState::Cancelled;
                session.response.finish_body();
                session.response.push_timeline(TimelinePhase::Cancelled);
                session.cancellation = None;
                session.execution_commands = None;
                let metrics = session.response.metrics;
                self.finish_run_history(document, HistoryRunState::Cancelled, metrics, None);
            }
            Err(error) => {
                let error = error.to_string();
                let session = self.sessions.entry(document);
                session.response.state = RunState::Failed;
                session.response.error = Some(error.clone());
                session.response.finish_body();
                session.response.push_timeline(TimelinePhase::Failed);
                session.cancellation = None;
                session.execution_commands = None;
                let metrics = session.response.metrics;
                self.finish_run_history(document, HistoryRunState::Failed, metrics, Some(error));
            }
        }
    }
}

fn apply_response_head(response: &mut ResponseView, head: ResponseHead) {
    response.status = head.status;
    response.version = head.version;
    response.headers = head
        .headers
        .into_iter()
        .flat_map(|(name, values)| values.into_iter().map(move |value| (name.clone(), value)))
        .collect();
}
