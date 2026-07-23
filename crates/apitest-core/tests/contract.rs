use apitest_core::{
    ApiContent, ApiContract, ApiDefinition, ApiParameter, ApiParameterLocation, ApiResponse,
    ApiSchema, ApiServer, BodySpec, HttpContract, HttpMethod, Project, ProtocolSpec, RequestCase,
};
use serde_json::json;

#[test]
fn request_case_values_do_not_mutate_the_api_contract() {
    let contract = HttpContract {
        method: HttpMethod::Post,
        path: "/users".into(),
        servers: vec![ApiServer::new("https://api.example.test")],
        parameters: vec![ApiParameter::query("trace", ApiSchema::boolean())],
        request_body: Some(ApiContent::json(ApiSchema::object())),
        responses: vec![ApiResponse::new("201", "Created")],
        ..HttpContract::default()
    };
    let definition = ApiDefinition::new("Create user", ApiContract::Http(contract.clone()));
    let mut request_case = RequestCase::for_definition(&definition, "valid user");
    let ProtocolSpec::Http(request) = &mut request_case.protocol else {
        panic!("HTTP contract should create an HTTP request case");
    };
    request.body = BodySpec::Json(json!({ "name": "Ada" }).to_string());

    assert_eq!(definition.contract, ApiContract::Http(contract));
    assert!(matches!(request_case.protocol, ProtocolSpec::Http(_)));
}

#[test]
fn structured_contract_round_trips_without_losing_openapi_fields() {
    let mut parameter =
        ApiParameter::new("user_id", ApiParameterLocation::Path, ApiSchema::string());
    parameter.required = true;
    parameter.example = Some(json!("usr_123"));
    let contract = HttpContract {
        method: HttpMethod::Get,
        path: "/users/{user_id}".into(),
        servers: vec![ApiServer::new("https://api.example.test/v1")],
        parameters: vec![parameter],
        responses: vec![ApiResponse::json(
            "200",
            "User",
            ApiSchema::component("User"),
        )],
        ..HttpContract::default()
    };
    let definition = ApiDefinition::new("Get user", ApiContract::Http(contract));

    let encoded = serde_json::to_string(&definition).expect("contract should serialize");
    let decoded: ApiDefinition =
        serde_json::from_str(&encoded).expect("contract should deserialize");

    assert_eq!(decoded, definition);
}

#[test]
fn project_components_are_shared_by_all_operations() {
    let mut project = Project::new("Billing");
    project
        .components
        .schemas
        .insert("Money".into(), ApiSchema::number());

    assert_eq!(
        project.components.schemas.get("Money"),
        Some(&ApiSchema::number())
    );
}

#[test]
fn contract_conversion_preserves_disabled_parameters() {
    let mut spec = apitest_core::HttpSpec::new(HttpMethod::Get, "https://example.test/users");
    let mut query = apitest_core::KeyValue::enabled("draft", "true");
    query.enabled = false;
    spec.query.push(query);

    let contract = HttpContract::from(spec);
    let restored = contract.example_spec();

    assert!(!restored.query[0].enabled);
}
