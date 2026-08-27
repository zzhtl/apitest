use std::time::Duration;

use apitest_core::ApiDefinition;
use apitest_core::{ApiContract, ProtocolSpec};
use apitest_interop::CodeLanguage;
use apitest_storage::{MemorySecretStore, PageRequest};
use egui_kittest::{Harness, kittest::Queryable as _};

use super::support::test_app;
use crate::draft::{AuthDraft, EditablePair};
use crate::persistence::StorageEvent;
use crate::state::action::InteropAction;
use crate::state::workspace::WorkspaceRequest;

#[test]
fn desktop_interop_imports_curl_and_previews_openapi() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let initial_count = harness.state().requests.len();
    let imported = harness
        .state_mut()
        .import_source(
            InteropAction::ImportCurl,
            "curl --request POST 'https://example.test/users' --data '{\"name\":\"Ada\"}'",
        )
        .expect("cURL should import");
    assert_eq!(imported, 1);
    assert_eq!(harness.state().requests.len(), initial_count + 1);
    assert!(matches!(
        harness.state().requests.last().map(WorkspaceRequest::edited_protocol),
        Some(ProtocolSpec::Http(spec))
            if spec.url == "https://example.test/users"
                && spec.method == apitest_core::HttpMethod::Post
    ));
    assert!(matches!(
        harness
            .state()
            .storage_worker
            .as_ref()
            .expect("storage worker should exist")
            .recv_timeout(Duration::from_secs(1)),
        Some(StorageEvent::RequestSaved { notify: false, .. })
    ));

    let openapi = r#"
openapi: 3.1.0
info: { title: Catalog, version: 1.0.0 }
paths:
  /catalog:
    get:
      operationId: listCatalog
      summary: List catalog
      responses:
        '200': { description: OK }
components:
  schemas:
    CatalogItem: { type: object }
"#;
    assert_eq!(
        harness
            .state_mut()
            .import_source(InteropAction::ImportOpenApi, openapi)
            .expect("OpenAPI should import"),
        1
    );
    assert!(
        harness
            .state()
            .project
            .components
            .schemas
            .contains_key("CatalogItem")
    );
    assert!(matches!(
        harness
            .state()
            .storage_worker
            .as_ref()
            .expect("storage worker should exist")
            .recv_timeout(Duration::from_secs(1)),
        Some(StorageEvent::RequestSaved { notify: false, .. })
    ));

    harness
        .state_mut()
        .refresh_openapi_preview()
        .expect("preview should render");
    assert!(harness.state().openapi_source.contains("openapi: 3.1.0"));
    assert!(harness.state().openapi_source.contains("listCatalog"));
    assert!(harness.state().openapi_html.contains("<!doctype html>"));
    harness.state_mut().show_openapi_preview = true;
    harness.step();
    assert!(
        harness
            .query_by_label_contains("OpenAPI 设计与预览")
            .is_some()
    );
}

#[test]
fn securing_openapi_examples_preserves_the_full_contract() {
    let source = r#"
openapi: 3.1.0
info: { title: Secure, version: 1.0.0 }
servers: [{ url: https://example.test }]
paths:
  /users:
    get:
      operationId: listUsers
      security: [{ apiKey: [] }]
      parameters:
        - name: x-api-key
          in: header
          schema: { type: string }
          example: contract-secret
      requestBody:
        content:
          application/json:
            schema: { type: object }
            examples:
              primary: { value: { password: body-secret-one } }
              secondary: { value: { token: body-secret-two } }
      responses:
        '201':
          description: Users
          content:
            application/json:
              schema: { type: array, items: { type: string } }
components:
  securitySchemes:
    apiKey: { type: apiKey, in: header, name: x-api-key }
"#;
    let imported = apitest_interop::import_openapi_document(source).expect("OpenAPI should import");
    let definition = imported
        .definitions
        .into_iter()
        .next()
        .expect("one operation");
    let mut request = WorkspaceRequest::from_definition(definition, None);
    let secrets = MemorySecretStore::default();

    crate::services::secrets::secure_imported_request(&mut request, &secrets)
        .expect("sensitive examples should be secured");

    let ApiContract::Http(contract) = &request.definition.contract else {
        panic!("OpenAPI operation should remain HTTP");
    };
    assert_eq!(contract.responses[0].status, "201");
    assert_eq!(contract.security[0].scheme, "apiKey");
    assert_eq!(contract.extensions["operationId"], "listUsers");
    assert_eq!(contract.servers[0].url, "https://example.test");
    assert_eq!(
        contract.parameters[0].example,
        Some(serde_json::json!("{{__import_secret_0}}"))
    );
    let stored = serde_json::to_string(&request.definition.contract)
        .expect("secured contract should serialize");
    assert!(!stored.contains("contract-secret"));
    assert!(!stored.contains("body-secret-one"));
    assert!(!stored.contains("body-secret-two"));
    assert_eq!(
        contract
            .request_body
            .as_ref()
            .expect("request body should remain")
            .examples
            .len(),
        2
    );
}

#[test]
fn importing_openapi_merges_existing_project_components() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    harness
        .state_mut()
        .project
        .components
        .schemas
        .insert("Existing".into(), apitest_core::ApiSchema::string());
    let source = r#"
openapi: 3.1.0
info: { title: New, version: 1.0.0 }
paths:
  /new:
    get:
      responses:
        '200': { description: OK }
components:
  schemas:
    Imported: { type: object }
"#;

    harness
        .state_mut()
        .import_source(InteropAction::ImportOpenApi, source)
        .expect("OpenAPI should import");

    assert!(
        harness
            .state()
            .project
            .components
            .schemas
            .contains_key("Existing")
    );
    assert!(
        harness
            .state()
            .project
            .components
            .schemas
            .contains_key("Imported")
    );
}

#[test]
fn imported_authentication_values_are_moved_to_the_secret_store() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let initial_count = harness.state().requests.len();
    let postman = r#"
        {
          "info": {
            "name": "Secure import",
            "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
          },
          "item": [
            {
              "name": "Bearer request",
              "request": {
                "method": "GET",
                "url": "https://example.test/bearer",
                "auth": {
                  "type": "bearer",
                  "bearer": [{"key":"token","value":"bearer-secret"}]
                }
              }
            },
            {
              "name": "Basic request",
              "request": {
                "method": "GET",
                "url": "https://example.test/basic",
                "auth": {
                  "type": "basic",
                  "basic": [
                    {"key":"username","value":"alice"},
                    {"key":"password","value":"basic-secret"}
                  ]
                }
              }
            },
            {
              "name": "Custom API key",
              "request": {
                "method": "GET",
                "url": "https://example.test/key",
                "auth": {
                  "type": "apikey",
                  "apikey": [
                    {"key":"key","value":"X-Client-Credential"},
                    {"key":"value","value":"custom-secret"},
                    {"key":"in","value":"header"}
                  ]
                }
              }
            }
          ]
        }
        "#;

    assert_eq!(
        harness
            .state_mut()
            .import_source(InteropAction::ImportPostman, postman)
            .expect("Postman authentication should import"),
        3
    );
    for _ in 0..3 {
        assert!(matches!(
            harness
                .state()
                .storage_worker
                .as_ref()
                .expect("storage worker should exist")
                .recv_timeout(Duration::from_secs(1)),
            Some(StorageEvent::RequestSaved { notify: false, .. })
        ));
    }

    let imported = &harness.state().requests[initial_count..];
    let AuthDraft::Bearer { token } = &imported[0].draft.auth else {
        panic!("bearer authentication should remain editable");
    };
    assert!(token.configured);
    assert!(token.replacement.is_empty());
    assert_eq!(
        harness
            .state()
            .secrets
            .get(&token.reference)
            .expect("bearer secret should load")
            .as_deref(),
        Some("bearer-secret")
    );
    let AuthDraft::Basic { username, password } = &imported[1].draft.auth else {
        panic!("basic authentication should remain editable");
    };
    assert_eq!(username, "alice");
    assert_eq!(
        harness
            .state()
            .secrets
            .get(&password.reference)
            .expect("basic secret should load")
            .as_deref(),
        Some("basic-secret")
    );
    assert_eq!(imported[2].draft.headers[0].value, "{{__import_secret_0}}");
    assert!(imported[2].draft.headers[0].description.is_empty());
    let api_key_reference = imported[2].request_case.local_variables[0]
        .secret_ref
        .as_ref()
        .expect("API key should use a secret reference");
    assert_eq!(
        harness
            .state()
            .secrets
            .get(api_key_reference)
            .expect("API key secret should load")
            .as_deref(),
        Some("custom-secret")
    );

    let database = harness
        .state()
        .database
        .as_ref()
        .expect("database should exist");
    for request in imported {
        let definition = database
            .get_definition(request.id())
            .expect("definition should load")
            .expect("definition should exist");
        let cases = database
            .list_request_cases(
                harness.state().project.id,
                request.id(),
                PageRequest::new(0, 10),
            )
            .expect("request case should load");
        let stored = serde_json::to_string(&(definition, cases.items))
            .expect("stored documents should serialize");
        for secret in ["bearer-secret", "basic-secret", "custom-secret"] {
            assert!(!stored.contains(secret), "stored document leaked {secret}");
        }
    }
}

/// The desktop can reach the code generator that `apitest-interop` has always
/// exposed; it previously had no call site at all.
#[test]
fn code_snippets_cover_every_supported_language() {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    harness.state_mut().requests[0].draft.url = "https://api.example.com/v1/users".into();
    harness.state_mut().requests[0]
        .draft
        .headers
        .push(EditablePair::new("X-Trace", "abc"));
    harness.state_mut().show_snippet = true;

    for language in [
        CodeLanguage::Curl,
        CodeLanguage::JavaScriptFetch,
        CodeLanguage::PythonRequests,
        CodeLanguage::RustReqwest,
    ] {
        harness.state_mut().snippet_language = language;
        harness.run_steps(2);
        let spec = match harness.state().requests[0].edited_protocol() {
            apitest_core::ProtocolSpec::Http(spec) => spec,
            other => panic!("expected HTTP, got {:?}", other.kind()),
        };
        let code = apitest_interop::generate_code(&spec, language);
        assert!(
            code.contains("https://api.example.com/v1/users"),
            "{language:?} snippet should carry the URL: {code}",
        );
    }
}

/// Search must reach requests that are only in the index, not just the page the
/// sidebar happens to have loaded.
#[test]
fn search_finds_requests_that_are_not_loaded_in_memory() {
    let harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_eframe(test_app);
    let project_id = harness.state().project.id;
    let definition = ApiDefinition::new(
        "订单查询接口",
        apitest_core::ProtocolSpec::Http(apitest_core::HttpSpec::new(
            apitest_core::HttpMethod::Get,
            "https://api.example.com/orders",
        )),
    );
    let stored_id = definition.id;
    harness
        .state()
        .database
        .as_ref()
        .expect("database")
        .save_definition(project_id, &definition)
        .expect("definition should save");

    // Deliberately not added to `requests`: this is the not-yet-loaded case.
    assert!(
        !harness
            .state()
            .requests
            .iter()
            .any(|request| request.id() == stored_id)
    );

    let (hits, _) = harness.state().search_hits("订单");
    assert!(
        hits.iter().any(|hit| hit.id == stored_id),
        "full-text search should surface the stored definition, got {:?}",
        hits.iter().map(|hit| &hit.name).collect::<Vec<_>>(),
    );
}
