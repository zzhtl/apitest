use std::collections::BTreeMap;

use apitest_runtime::{ScriptEngine, ScriptResponse};

#[test]
fn script_can_mutate_variables_and_assert_response() {
    let engine = ScriptEngine::default();
    let response = ScriptResponse {
        status: 201,
        headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
        body: r#"{"id":7,"name":"Ada"}"#.into(),
    };

    let result = engine
        .run(
            r#"
                apitest.setVariable("token", "abc");
                apitest.test("created", () => {
                    apitest.expect(response.status).toBe(201);
                    apitest.expect(response.json().name).toEqual("Ada");
                });
            "#,
            &BTreeMap::new(),
            Some(&response),
        )
        .expect("script should run");

    assert_eq!(
        result.variables.get("token").map(String::as_str),
        Some("abc")
    );
    assert_eq!(result.assertions.len(), 1);
    assert!(result.assertions[0].passed);
}

#[test]
fn failed_assertion_is_reported_without_aborting_following_tests() {
    let engine = ScriptEngine::default();
    let result = engine
        .run(
            r#"
                apitest.test("first", () => apitest.expect(1).toBe(2));
                apitest.test("second", () => apitest.expect(true).toBeTruthy());
            "#,
            &BTreeMap::new(),
            None,
        )
        .expect("assertion failures should be data, not engine errors");

    assert!(!result.assertions[0].passed);
    assert!(result.assertions[1].passed);
}
