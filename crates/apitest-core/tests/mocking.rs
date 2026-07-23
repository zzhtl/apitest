use apitest_core::{
    ApiComponents, ApiContent, ApiDefinition, ApiParameter, ApiParameterLocation, ApiResponse,
    ApiSchema, HttpContract, HttpMethod, HttpSpec, generate_mock_rules,
};
use indexmap::IndexMap;

#[test]
fn smart_mock_rules_use_contract_examples_and_component_schemas() {
    let mut components = ApiComponents::default();
    components.schemas.insert(
        "User".into(),
        ApiSchema::Inline(serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "example": 7 },
                "name": { "type": "string", "example": "Ada" },
                "active": { "type": "boolean" }
            }
        })),
    );
    let mut contract = HttpContract::from(HttpSpec::new(
        HttpMethod::Get,
        "https://api.example.test/users/{id}",
    ));
    contract.path = "/users/{id}".into();
    let mut id = ApiParameter::new("id", ApiParameterLocation::Path, ApiSchema::string());
    id.example = Some(serde_json::json!(7));
    let mut verbose = ApiParameter::query("verbose", ApiSchema::boolean());
    verbose.example = Some(serde_json::json!(true));
    contract.parameters = vec![id, verbose];
    contract.responses = vec![ApiResponse {
        status: "200".into(),
        description: "User".into(),
        headers: Vec::new(),
        content: vec![ApiContent {
            media_type: "application/json".into(),
            schema: ApiSchema::component("User"),
            examples: IndexMap::new(),
        }],
    }];
    let definition = ApiDefinition::new("Get user", contract);

    let rules = generate_mock_rules(&[definition], &components);

    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].path, "/users/{id}");
    assert_eq!(rules[0].path_variables["id"], "7");
    assert_eq!(rules[0].query[0].name, "verbose");
    assert_eq!(rules[0].query[0].value, "true");
    assert_eq!(rules[0].response.status, 200);
    assert_eq!(rules[0].response.headers[0].value, "application/json");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&rules[0].response.body)
            .expect("mock body should be JSON"),
        serde_json::json!({ "id": 7, "name": "Ada", "active": true })
    );
}
