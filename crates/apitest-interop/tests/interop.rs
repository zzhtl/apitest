use apitest_core::{
    ApiContract, ApiParameterLocation, ApiSchema, BodySpec, FormField, HttpMethod, HttpSpec,
    KeyValue, MultipartPart, Project, ProtocolSpec,
};
use apitest_interop::{
    CodeLanguage, OpenApiIssueLevel, PortableProject, export_har, export_openapi,
    export_openapi_documentation, export_postman, export_project, generate_code, import_curl,
    import_har, import_openapi, import_openapi_document, import_postman, import_project,
    validate_openapi,
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
    let apitest_core::ApiContract::Http(contract) = &definitions[0].contract else {
        panic!("operation should become HTTP");
    };
    let spec = contract.example_spec();
    assert_eq!(spec.method, HttpMethod::Get);
    assert_eq!(spec.url, "https://api.example.com/users/7");
    assert_eq!(spec.query[0].name, "verbose");
    assert_eq!(spec.query[0].value, "true");
}

#[test]
fn openapi_31_import_preserves_contracts_components_and_security() {
    let source = r#"
openapi: 3.1.0
info: { title: Users, version: 2.0.0 }
servers: [{ url: https://api.example.com }]
paths:
  /users/{id}:
    parameters:
      - name: id
        in: path
        required: true
        schema: { type: string }
        example: user-7
    get:
      operationId: getUser
      summary: Get user
      tags: [Users]
      security: [{ bearerAuth: [users:read] }]
      parameters:
        - name: verbose
          in: query
          schema: { type: boolean }
          example: true
      responses:
        '200':
          description: User found
          content:
            application/json:
              schema: { $ref: '#/components/schemas/User' }
              example: { id: user-7, name: Ada }
        '404': { description: User not found }
components:
  schemas:
    User:
      type: object
      required: [id, name]
      properties:
        id: { type: string }
        name: { type: string }
  securitySchemes:
    bearerAuth: { type: http, scheme: bearer, bearerFormat: JWT }
"#;

    let imported = import_openapi_document(source).expect("OpenAPI should import");

    assert_eq!(imported.title, "Users");
    assert_eq!(imported.version, "2.0.0");
    assert_eq!(imported.definitions.len(), 1);
    assert_eq!(
        imported.components.schemas["User"],
        ApiSchema::Inline(serde_json::json!({
            "type": "object",
            "required": ["id", "name"],
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" }
            }
        }))
    );
    assert!(
        imported
            .components
            .security_schemes
            .contains_key("bearerAuth")
    );
    let ApiContract::Http(contract) = &imported.definitions[0].contract else {
        panic!("operation should become HTTP");
    };
    assert_eq!(contract.path, "/users/{id}");
    assert_eq!(contract.parameters.len(), 2);
    assert_eq!(contract.parameters[0].location, ApiParameterLocation::Path);
    assert_eq!(contract.responses.len(), 2);
    assert_eq!(
        contract.responses[0].content[0].schema,
        ApiSchema::component("User")
    );
    assert_eq!(contract.security[0].scheme, "bearerAuth");
    assert_eq!(contract.security[0].scopes, ["users:read"]);
    assert_eq!(
        contract.extensions["operationId"],
        serde_json::Value::String("getUser".into())
    );
    assert_eq!(
        contract.example_spec().url,
        "https://api.example.com/users/user-7"
    );
}

#[test]
fn openapi_export_validates_and_renders_static_documentation() {
    let source = r#"
openapi: 3.1.0
info: { title: Users, version: 1.0.0 }
paths:
  /users:
    get:
      operationId: listUsers
      summary: List <users>
      responses:
        '200': { description: OK }
"#;
    let imported = import_openapi_document(source).expect("OpenAPI should import");
    let mut project = Project::new(&imported.title);
    project.components = imported.components;

    let exported = export_openapi(&project, &imported.definitions).expect("OpenAPI should export");
    let issues = validate_openapi(&exported).expect("exported OpenAPI should parse");
    assert!(
        issues
            .iter()
            .all(|issue| issue.level != OpenApiIssueLevel::Error),
        "exported contract should be valid: {issues:?}"
    );
    assert!(exported.contains("openapi: 3.1.0"));
    assert!(exported.contains("operationId: listUsers"));

    let html = export_openapi_documentation(&project, &imported.definitions);
    assert!(html.contains("<!doctype html>"));
    assert!(html.contains("List &lt;users&gt;"));
    assert!(!html.contains("List <users>"));
}

#[test]
fn openapi_validation_reports_actionable_contract_errors() {
    let invalid = r#"
openapi: 3.0.3
info: { title: Broken, version: 1.0.0 }
paths:
  users:
    get:
      operationId: duplicated
  /admins:
    get:
      operationId: duplicated
      responses: {}
"#;

    let issues = validate_openapi(invalid).expect("document should parse");

    assert!(issues.iter().any(|issue| {
        issue.level == OpenApiIssueLevel::Error && issue.message.contains("OpenAPI 3.1")
    }));
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("must start with `/`"))
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("duplicate operationId"))
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("at least one response"))
    );
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

    assert_eq!(restored.schema_version, 2);
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
    assert!(curl.contains("q=rust"));
    assert!(curl.contains("'x-name: Ada Lovelace'"));
    assert!(!curl.contains("\n+  --"));
    let rust = generate_code(&spec, CodeLanguage::RustReqwest);
    assert!(rust.contains("reqwest::Client::new()"));
    assert!(rust.contains(".json(&serde_json::json!"));
}

#[test]
fn imports_curl_without_executing_shell_syntax() {
    let definition = import_curl(
        r#"curl --request POST 'https://example.com/users?draft=true' \
          --header 'content-type: application/json' \
          --header 'x-name: Ada Lovelace' \
          --data-raw '{"active":true}'"#,
    )
    .expect("cURL should import");
    let ApiContract::Http(contract) = definition.contract else {
        panic!("cURL should become HTTP");
    };
    let spec = contract.example_spec();

    assert_eq!(spec.method, HttpMethod::Post);
    assert_eq!(spec.url, "https://example.com/users");
    assert_eq!(spec.query[0].name, "draft");
    assert_eq!(spec.headers[1].value, "Ada Lovelace");
    assert_eq!(spec.body, BodySpec::Json(r#"{"active":true}"#.into()));
}

#[test]
fn postman_and_har_round_trip_http_collections() {
    let postman = r#"
{
  "info": {
    "name": "Users",
    "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
  },
  "item": [{
    "name": "Create user",
    "request": {
      "method": "POST",
      "header": [{"key":"content-type","value":"application/json"}],
      "url": "https://api.example.com/users?notify=true",
      "body": {"mode":"raw","raw":"{\"name\":\"Ada\"}","options":{"raw":{"language":"json"}}}
    }
  }]
}
"#;
    let definitions = import_postman(postman).expect("Postman should import");
    assert_eq!(definitions.len(), 1);
    let project = Project::new("Users");
    let exported_postman = export_postman(&project, &definitions).expect("Postman should export");
    let restored = import_postman(&exported_postman).expect("Postman should re-import");
    assert_eq!(restored.len(), 1);
    let ApiContract::Http(restored_contract) = &restored[0].contract else {
        panic!("Postman should preserve HTTP");
    };
    assert_eq!(restored_contract.example_spec().method, HttpMethod::Post);
    assert_eq!(
        restored_contract.example_spec().body,
        BodySpec::Json(r#"{"name":"Ada"}"#.into())
    );

    let har = export_har(&definitions).expect("HAR should export");
    let restored_har = import_har(&har).expect("HAR should import");
    assert_eq!(restored_har.len(), 1);
    let ApiContract::Http(har_contract) = &restored_har[0].contract else {
        panic!("HAR should preserve HTTP");
    };
    let har_spec = har_contract.example_spec();
    assert_eq!(har_spec.url, "https://api.example.com/users");
    assert_eq!(har_spec.query[0].value, "true");
    assert_eq!(har_spec.body, BodySpec::Json(r#"{"name":"Ada"}"#.into()));
}

#[test]
fn openapi_validation_resolves_referenced_path_parameters() {
    let source = r#"
openapi: 3.1.0
info: { title: Users, version: 1.0.0 }
paths:
  /users/{id}:
    get:
      operationId: getUser
      parameters:
        - $ref: '#/components/parameters/UserId'
      responses:
        '200': { description: OK }
components:
  parameters:
    UserId:
      name: id
      in: path
      required: true
      schema: { type: string }
"#;

    let issues = validate_openapi(source).expect("OpenAPI should validate");

    assert!(
        issues
            .iter()
            .all(|issue| !issue.message.contains("must be declared and required")),
        "referenced path parameter should be accepted: {issues:?}"
    );
}

#[test]
fn postman_template_urls_extract_and_export_query_once() {
    let source = r#"
{
  "info": {
    "name": "Templates",
    "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
  },
  "item": [{
    "name": "Users",
    "request": {
      "method": "GET",
      "url": {
        "raw": "{{baseUrl}}/users?draft=true",
        "query": [{"key":"draft","value":"true"}]
      }
    }
  }]
}
"#;

    let definitions = import_postman(source).expect("Postman should import");
    let ApiContract::Http(contract) = &definitions[0].contract else {
        panic!("Postman request should be HTTP");
    };
    let spec = contract.example_spec();
    assert_eq!(spec.url, "{{baseUrl}}/users");
    assert_eq!(spec.query.len(), 1);

    let exported =
        export_postman(&Project::new("Templates"), &definitions).expect("Postman should export");
    assert_eq!(exported.matches("draft=true").count(), 1);
}

#[test]
fn har_round_trips_multipart_and_binary_bodies() {
    let mut multipart = HttpSpec::new(HttpMethod::Post, "https://example.test/upload");
    multipart.body = BodySpec::Multipart(vec![
        MultipartPart::Text(FormField {
            name: "caption".into(),
            value: "avatar".into(),
            enabled: true,
        }),
        MultipartPart::File {
            name: "file".into(),
            path: "/tmp/avatar.png".into(),
            content_type: Some("image/png".into()),
            enabled: true,
        },
    ]);
    let mut binary = HttpSpec::new(HttpMethod::Put, "https://example.test/archive");
    binary.body = BodySpec::BinaryFile("/tmp/archive.bin".into());
    let definitions = vec![
        apitest_core::ApiDefinition::new("Upload", ProtocolSpec::Http(multipart.clone())),
        apitest_core::ApiDefinition::new("Archive", ProtocolSpec::Http(binary.clone())),
    ];

    let har = export_har(&definitions).expect("HAR should export");
    let restored = import_har(&har).expect("HAR should import");
    let bodies = restored
        .iter()
        .map(|definition| definition.contract.example_protocol())
        .map(|protocol| match protocol {
            ProtocolSpec::Http(spec) => spec.body,
            _ => panic!("HAR only contains HTTP requests"),
        })
        .collect::<Vec<_>>();

    assert_eq!(bodies, vec![multipart.body, binary.body]);
}

#[test]
fn postman_inherits_collection_and_folder_authentication() {
    let source = r#"
{
  "info": {
    "name": "Auth",
    "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
  },
  "auth": {
    "type": "bearer",
    "bearer": [{"key":"token","value":"collection-token"}]
  },
  "item": [
    {
      "name": "Collection request",
      "request": {"method":"GET","url":"https://example.test/collection"}
    },
    {
      "name": "Folder",
      "auth": {
        "type": "apikey",
        "apikey": [
          {"key":"key","value":"x-api-key"},
          {"key":"value","value":"folder-key"},
          {"key":"in","value":"header"}
        ]
      },
      "item": [{
        "name": "Folder request",
        "request": {"method":"GET","url":"https://example.test/folder"}
      }]
    }
  ]
}
"#;

    let definitions = import_postman(source).expect("Postman should import");
    let specs = definitions
        .iter()
        .map(|definition| match definition.contract.example_protocol() {
            ProtocolSpec::Http(spec) => spec,
            _ => panic!("Postman request should be HTTP"),
        })
        .collect::<Vec<_>>();

    assert_eq!(specs[0].headers[0].value, "Bearer collection-token");
    assert_eq!(specs[1].headers[0].name, "x-api-key");
    assert_eq!(specs[1].headers[0].value, "folder-key");
}

#[test]
fn postman_and_har_round_trip_request_cookies() {
    let mut spec = HttpSpec::new(HttpMethod::Get, "https://example.test/users");
    spec.cookies.push(KeyValue::enabled("session", "abc123"));
    let definitions = vec![apitest_core::ApiDefinition::new(
        "Cookies",
        ProtocolSpec::Http(spec),
    )];

    let postman =
        export_postman(&Project::new("Cookies"), &definitions).expect("Postman should export");
    let postman_restored = import_postman(&postman).expect("Postman should import");
    let ProtocolSpec::Http(postman_spec) = postman_restored[0].contract.example_protocol() else {
        panic!("Postman request should be HTTP");
    };
    assert_eq!(
        postman_spec.cookies,
        [KeyValue::enabled("session", "abc123")]
    );

    let har = export_har(&definitions).expect("HAR should export");
    let har_restored = import_har(&har).expect("HAR should import");
    let ProtocolSpec::Http(har_spec) = har_restored[0].contract.example_protocol() else {
        panic!("HAR request should be HTTP");
    };
    assert_eq!(har_spec.cookies, [KeyValue::enabled("session", "abc123")]);
}

#[test]
fn postman_round_trip_preserves_disabled_request_values() {
    let mut spec = HttpSpec::new(HttpMethod::Get, "{{baseUrl}}/users");
    let mut query = KeyValue::enabled("draft", "true");
    query.enabled = false;
    let mut header = KeyValue::enabled("x-debug", "true");
    header.enabled = false;
    let mut cookie = KeyValue::enabled("preview", "true");
    cookie.enabled = false;
    spec.query.push(query);
    spec.headers.push(header);
    spec.cookies.push(cookie);
    let definitions = vec![apitest_core::ApiDefinition::new(
        "Disabled",
        ProtocolSpec::Http(spec),
    )];

    let postman =
        export_postman(&Project::new("Disabled"), &definitions).expect("Postman should export");
    let restored = import_postman(&postman).expect("Postman should import");
    let ProtocolSpec::Http(restored) = restored[0].contract.example_protocol() else {
        panic!("Postman request should be HTTP");
    };

    assert!(!restored.query[0].enabled);
    assert!(!restored.headers[0].enabled);
    assert!(!restored.cookies[0].enabled);
}

#[test]
fn imports_v1_portable_projects_and_exports_v2() {
    let project = Project::new("Legacy");
    let definition = apitest_core::ApiDefinition::new(
        "Health",
        ProtocolSpec::Http(HttpSpec::new(
            HttpMethod::Get,
            "https://example.test/health",
        )),
    );
    let protocol = definition.contract.example_protocol();
    let legacy = serde_json::json!({
        "schema_version": 1,
        "project": {
            "id": project.id,
            "name": project.name,
            "description": project.description,
            "created_at": project.created_at,
            "updated_at": project.updated_at
        },
        "definitions": [{
            "id": definition.id,
            "name": definition.name,
            "description_markdown": definition.description_markdown,
            "status": definition.status,
            "tags": definition.tags,
            "protocol": protocol,
            "created_at": definition.created_at,
            "updated_at": definition.updated_at
        }]
    });

    let restored = import_project(&legacy.to_string()).expect("v1 project should migrate");
    assert_eq!(restored.schema_version, 2);
    assert!(restored.project.components.schemas.is_empty());
    assert_eq!(
        restored.definitions[0].contract.example_protocol(),
        protocol
    );

    let exported = export_project(&restored).expect("migrated project should export");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&exported)
            .expect("portable project should be JSON")["schema_version"],
        2
    );
}
