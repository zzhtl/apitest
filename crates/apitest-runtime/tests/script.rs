use std::{collections::BTreeMap, time::Duration};

use apitest_runtime::{ScriptEngine, ScriptError, ScriptResponse};

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

#[test]
fn postman_pm_api_reads_responses_and_persists_variables() {
    let engine = ScriptEngine::default();
    let response = ScriptResponse {
        status: 201,
        headers: BTreeMap::from([("Content-Type".into(), "application/json".into())]),
        body: r#"{"id":7,"name":"Ada"}"#.into(),
    };

    let result = engine
        .run(
            r#"
                pm.environment.set("user_id", pm.response.json().id);
                pm.test("Postman-compatible response", () => {
                    pm.response.to.have.status(201);
                    pm.expect(pm.response.code).to.equal(201);
                    pm.expect(pm.response.headers.get("content-type")).to.include("json");
                    pm.expect(pm.response.json()).to.have.property("name", "Ada");
                    pm.expect(pm.variables.get("user_id")).to.eql("7");
                });
            "#,
            &BTreeMap::new(),
            Some(&response),
        )
        .expect("pm script should run");

    assert_eq!(
        result.variables.get("user_id").map(String::as_str),
        Some("7")
    );
    assert_eq!(result.assertions.len(), 1);
    assert!(result.assertions[0].passed, "{:?}", result.assertions[0]);
}

#[test]
fn sandbox_does_not_expose_network_process_or_module_capabilities() {
    let result = ScriptEngine::default()
        .run(
            r#"
                pm.test("isolated", () => {
                    pm.expect(typeof fetch).to.equal("undefined");
                    pm.expect(typeof process).to.equal("undefined");
                    pm.expect(typeof require).to.equal("undefined");
                    pm.expect(typeof pm.sendRequest).to.equal("undefined");
                });
            "#,
            &BTreeMap::new(),
            None,
        )
        .expect("capability check should run");

    assert!(result.assertions[0].passed, "{:?}", result.assertions[0]);
}

#[test]
fn runaway_scripts_are_interrupted_by_the_execution_deadline() {
    let engine = ScriptEngine::new(Duration::from_millis(20), 16 * 1024 * 1024);

    let error = engine
        .run("while (true) {}", &BTreeMap::new(), None)
        .expect_err("infinite script should be interrupted");

    assert!(matches!(error, ScriptError::Timeout { timeout_ms: 20 }));
}
