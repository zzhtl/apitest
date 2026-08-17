use apitest_core::{GrpcCallKind, ProtocolKind, ProtocolSpec};
use eframe::egui::{self, Color32, RichText};

use crate::i18n::{Language, tr};
use crate::theme::{Palette, UiExt};
use crate::ui::editors::auth::form_field;
use crate::ui::editors::key_value_editor;
use crate::ui::request::method_combo;

pub(crate) fn protocol_editor(ui: &mut egui::Ui, protocol: &mut ProtocolSpec, language: Language) {
    let palette = ui.palette();
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
                method_combo(ui, &mut spec.request.method);
                ui.add_sized(
                    [ui.available_width() - 150.0, 34.0],
                    egui::TextEdit::singleline(&mut spec.request.url)
                        .hint_text("https://api.example.com/events"),
                );
                ui.checkbox(&mut spec.reconnect, tr(language, "自动重连", "Reconnect"));
            });
            ui.label(RichText::new(tr(language, "请求头", "Headers")).strong());
            key_value_editor(ui, &mut spec.request.headers, language);
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
                    tr(language, "校验 TLS 证书", "Validate TLS"),
                );
                ui.label(tr(language, "连接超时", "Connect timeout"));
                ui.add(
                    egui::DragValue::new(&mut spec.connect_timeout_ms)
                        .range(1..=600_000)
                        .suffix(" ms"),
                );
            });
            ui.label(RichText::new(tr(language, "请求头", "Headers")).strong());
            key_value_editor(ui, &mut spec.headers, language);
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
                ui.label(tr(language, "服务", "Service"));
                ui.add(
                    egui::TextEdit::singleline(&mut spec.service)
                        .hint_text("package.Greeter")
                        .desired_width(240.0),
                );
                ui.label(tr(language, "方法", "Method"));
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
                    tr(language, "服务端反射", "Server reflection"),
                );
                if ui
                    .button(tr(language, "选择 proto", "Choose proto"))
                    .clicked()
                    && let Some(files) = rfd::FileDialog::new()
                        .add_filter("Protocol Buffers", &["proto"])
                        .pick_files()
                {
                    spec.proto_files = files;
                    spec.use_reflection = false;
                }
                if ui
                    .button(tr(language, "选择 descriptor", "Choose descriptor"))
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
            ui.label(RichText::new(tr(language, "请求消息 JSON", "Request message JSON")).strong());
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

pub(crate) fn grpc_call_kind_label(kind: GrpcCallKind) -> &'static str {
    match kind {
        GrpcCallKind::Unary => "Unary",
        GrpcCallKind::ServerStreaming => "Server stream",
        GrpcCallKind::ClientStreaming => "Client stream",
        GrpcCallKind::BidirectionalStreaming => "Bidirectional",
    }
}

pub(crate) fn protocol_label(kind: ProtocolKind) -> &'static str {
    match kind {
        ProtocolKind::Http => "HTTP",
        ProtocolKind::GraphQl => "GQL",
        ProtocolKind::Sse => "SSE",
        ProtocolKind::WebSocket => "WS",
        ProtocolKind::Grpc => "gRPC",
    }
}

pub(crate) fn protocol_color(kind: ProtocolKind, palette: Palette) -> Color32 {
    match kind {
        ProtocolKind::Http => palette.info,
        ProtocolKind::GraphQl => Color32::from_rgb(225, 55, 143),
        ProtocolKind::Sse => palette.success,
        ProtocolKind::WebSocket => palette.warning,
        ProtocolKind::Grpc => palette.accent_text,
    }
}
