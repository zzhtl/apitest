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

    pub(crate) fn send_current(&mut self, context: &egui::Context) {
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

    pub(crate) fn stop(&mut self) {
        if let Some(commands) = &self.execution_commands {
            let _ = commands.try_send(ExecutionCommand::Cancel);
        }
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
            self.response.state = RunState::Cancelling;
        }
        self.execution_commands = None;
    }

    pub(crate) fn invalidate_run(&mut self) {
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

    pub(crate) fn send_websocket_message(&mut self) {
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

    pub(crate) fn close_websocket_input(&mut self) {
        match self.try_execution_command(ExecutionCommand::CompleteInput) {
            Ok(()) => self.execution_commands = None,
            Err(error) => self.toast(ToastKind::Error, error),
        }
    }

    pub(crate) fn try_execution_command(
        &mut self,
        command: ExecutionCommand,
    ) -> Result<(), String> {
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

    pub(crate) fn drain_runtime(&mut self) {
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

    pub(crate) fn apply_runtime_event(&mut self, event: Result<ExecutionEvent, ExecutionError>) {
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

    pub(crate) fn apply_response_head(&mut self, head: ResponseHead) {
        self.response.status = head.status;
        self.response.version = head.version;
        self.response.headers = head
            .headers
            .into_iter()
            .flat_map(|(name, values)| values.into_iter().map(move |value| (name.clone(), value)))
            .collect();
    }
}
