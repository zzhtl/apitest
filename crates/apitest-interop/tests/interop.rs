use apitest_core::{BodySpec, HttpMethod, HttpSpec, Project};
use apitest_interop::{
    CodeLanguage, PortableProject, export_project, generate_code, import_openapi, import_project,
};

#[test]
fn imports_openapi_operations_into_http_definitions() {
    let source = r#"
openapi: 3.1.0
info: { title: Users, version: 1.0.0 }
servers: [{ url: https://api.example.com }]
paths:
  /users/{id}:
    get:
      operationId: getUser
      summary: Get user
      parameters:
        - { in: path, name: id, required: true, example: 7 }
        - { in: query, name: verbose, example: true }
"#;

    let definitions = import_openapi(source).expect("OpenAPI should import");
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, "Get user");
    let apitest_core::ProtocolSpec::Http(spec) = &definitions[0].protocol else {
        panic!("operation should become HTTP");
    };
    assert_eq!(spec.method, HttpMethod::Get);
    assert_eq!(spec.url, "https://api.example.com/users/7");
    assert_eq!(spec.query[0].name, "verbose");
    assert_eq!(spec.query[0].value, "true");
}

#[test]
fn project_format_round_trips_without_losing_protocol_data() {
    let project = Project::new("Demo");
    let definition = apitest_core::ApiDefinition::new(
        "Create user",
        apitest_core::ProtocolSpec::Http(HttpSpec {
            body: BodySpec::Json(r#"{"name":"Ada"}"#.into()),
            ..HttpSpec::new(HttpMethod::Post, "https://example.com/users")
        }),
    );
    let bundle = PortableProject::new(project, vec![definition]);

    let json = export_project(&bundle).expect("project should export");
    let restored = import_project(&json).expect("project should import");

    assert_eq!(restored.schema_version, 1);
    assert_eq!(restored.definitions, bundle.definitions);
}

#[test]
fn generates_copy_ready_curl_and_rust_snippets() {
    let mut spec = HttpSpec::new(HttpMethod::Post, "https://example.com/users?q=rust");
    spec.headers
        .push(apitest_core::KeyValue::enabled("x-name", "Ada Lovelace"));
    spec.body = BodySpec::Json(r#"{"active":true}"#.into());

    let curl = generate_code(&spec, CodeLanguage::Curl);
    assert!(curl.contains("curl --request POST"));
    assert!(curl.contains("'x-name: Ada Lovelace'"));
    let rust = generate_code(&spec, CodeLanguage::RustReqwest);
    assert!(rust.contains("reqwest::Client::new()"));
    assert!(rust.contains(".json(&serde_json::json!"));
}
