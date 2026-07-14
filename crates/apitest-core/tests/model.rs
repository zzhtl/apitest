use apitest_core::{ApiDefinition, BodySpec, HttpMethod, HttpSpec, ProtocolSpec, RequestCase};

#[test]
fn request_case_keeps_values_separate_from_definition() {
    let definition = ApiDefinition::new(
        "Create user",
        ProtocolSpec::Http(HttpSpec::new(HttpMethod::Post, "{{base_url}}/users")),
    );
    let mut case = RequestCase::for_definition(&definition, "valid user");
    case.body = BodySpec::Json(r#"{"name":"Ada"}"#.to_owned());

    assert_eq!(case.definition_id, definition.id);
    assert_eq!(definition.name, "Create user");
    assert!(matches!(case.body, BodySpec::Json(_)));
}

#[test]
fn domain_models_round_trip_through_json() {
    let definition = ApiDefinition::new(
        "Health",
        ProtocolSpec::Http(HttpSpec::new(
            HttpMethod::Get,
            "https://example.test/health",
        )),
    );

    let encoded = serde_json::to_string(&definition).expect("definition should serialize");
    let decoded: ApiDefinition =
        serde_json::from_str(&encoded).expect("definition should deserialize");

    assert_eq!(decoded, definition);
}
