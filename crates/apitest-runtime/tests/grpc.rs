use std::path::PathBuf;

use apitest_core::GrpcCallKind;
use apitest_runtime::GrpcDescriptor;
use serde_json::json;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn compiles_proto_files_and_discovers_all_rpc_shapes() {
    let include = fixture("greeter.proto")
        .parent()
        .expect("fixture should have parent")
        .to_path_buf();
    let descriptor = GrpcDescriptor::from_proto_files(&[fixture("greeter.proto")], &[include])
        .expect("proto should compile");

    assert_eq!(
        descriptor
            .method("hello.Greeter", "SayHello")
            .expect("unary method should exist")
            .call_kind(),
        GrpcCallKind::Unary
    );
    assert_eq!(
        descriptor
            .method("hello.Greeter", "WatchHellos")
            .expect("server stream should exist")
            .call_kind(),
        GrpcCallKind::ServerStreaming
    );
    assert_eq!(
        descriptor
            .method("hello.Greeter", "UploadHellos")
            .expect("client stream should exist")
            .call_kind(),
        GrpcCallKind::ClientStreaming
    );
    assert_eq!(
        descriptor
            .method("hello.Greeter", "ChatHellos")
            .expect("bidi stream should exist")
            .call_kind(),
        GrpcCallKind::BidirectionalStreaming
    );
}

#[test]
fn converts_dynamic_messages_using_canonical_proto_json() {
    let include = fixture("greeter.proto")
        .parent()
        .expect("fixture should have parent")
        .to_path_buf();
    let descriptor = GrpcDescriptor::from_proto_files(&[fixture("greeter.proto")], &[include])
        .expect("proto should compile");
    let method = descriptor
        .method("hello.Greeter", "SayHello")
        .expect("method should exist");

    let message = method
        .input_from_json(r#"{"name":"Ada","sequence":"42"}"#)
        .expect("JSON should map to protobuf");
    let value = method
        .message_to_json(&message)
        .expect("protobuf should map to JSON");

    assert_eq!(value, json!({"name":"Ada","sequence":"42"}));
}
