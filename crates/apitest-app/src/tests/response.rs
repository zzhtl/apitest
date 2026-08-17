use apitest_core::{ExecutionCommand, ProtocolKind};
use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use tokio_util::sync::CancellationToken;

use super::support::test_app;
use crate::state::action::PendingAction;
use crate::state::action::ToastKind;
use crate::state::response::{MAX_RESPONSE_BYTES, ResponseTab, ResponseView};

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
    assert_eq!(harness.state().session().response.body, "中文");
    assert_eq!(
        harness.state().session().response.state,
        crate::state::response::RunState::Completed
    );
}

#[test]
fn websocket_console_forwards_messages_and_graceful_close() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let (commands, mut received) = tokio::sync::mpsc::channel(4);
    {
        let state = harness.state_mut();
        state.requests[0].alternate_protocol = Some(crate::state::workspace::default_protocol(
            ProtocolKind::WebSocket,
        ));
        state.session_mut().execution_commands = Some(commands);
        state.session_mut().websocket_message = "hello socket".into();
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
    assert!(harness.state().session().websocket_message.is_empty());

    harness.state_mut().close_websocket_input();
    assert_eq!(
        received
            .try_recv()
            .expect("close command should be forwarded"),
        ExecutionCommand::CompleteInput
    );
    assert!(harness.state().session().execution_commands.is_none());
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
fn consecutive_errors_all_stay_visible() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(980.0, 640.0))
        .build_eframe(test_app);
    harness.state_mut().toast(ToastKind::Error, "first failure");
    harness
        .state_mut()
        .toast(ToastKind::Error, "second failure");
    harness.state_mut().toast(ToastKind::Error, "third failure");
    harness.step();

    assert_eq!(
        harness
            .state()
            .toasts
            .latest()
            .map(|toast| toast.message.as_str()),
        Some("third failure"),
    );
    for message in ["first failure", "second failure", "third failure"] {
        assert!(
            harness.query_by_label(message).is_some(),
            "{message} should still be on screen",
        );
    }
}

/// Each tab owns its response: sending in one must not overwrite what another
/// is showing, and switching tabs must not cancel a request in flight.
#[test]
fn each_tab_keeps_its_own_response() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);

    let first = harness
        .state()
        .active_api_document()
        .expect("the seeded request is selected");
    harness
        .state_mut()
        .perform_action(PendingAction::NewRequest(ProtocolKind::Http));
    let second = harness
        .state()
        .active_api_document()
        .expect("the new request is selected");
    assert_ne!(first, second);

    harness.state_mut().sessions.entry(first).response.body = "first body".into();
    harness.state_mut().sessions.entry(first).response_tab = ResponseTab::Headers;
    harness.state_mut().sessions.entry(second).response.body = "second body".into();
    harness.run_steps(2);

    assert_eq!(harness.state().session().response.body, "second body");
    assert_eq!(harness.state().session().response_tab, ResponseTab::Body);

    harness.state_mut().activate_document(first);
    harness.run_steps(2);

    assert_eq!(harness.state().session().response.body, "first body");
    assert_eq!(
        harness.state().session().response_tab,
        ResponseTab::Headers,
        "the editor selection belongs to the tab too",
    );
}

/// Closing a tab must abandon its run rather than leave it streaming.
#[test]
fn closing_a_tab_cancels_its_run() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let document = harness
        .state()
        .active_api_document()
        .expect("a request is selected");
    let cancellation = CancellationToken::new();
    harness.state_mut().sessions.entry(document).cancellation = Some(cancellation.clone());

    harness.state_mut().close_document(document);

    assert!(cancellation.is_cancelled());
    assert!(harness.state().sessions.get(document).is_none());
}
