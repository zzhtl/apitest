use apitest_core::{Environment, EnvironmentError, Variable, VariableScope};

#[test]
fn resolves_nested_variables_with_scope_precedence() {
    let environment = Environment::new("Development")
        .with_variable(Variable::plain("host", "localhost"))
        .with_variable(Variable::plain("port", "8080"))
        .with_variable(Variable::plain("base_url", "http://{{host}}:{{port}}"));

    let resolved = environment
        .resolve(
            "{{base_url}}/users",
            &[Variable::scoped("port", "9090", VariableScope::Local)],
        )
        .expect("nested variables should resolve");

    assert_eq!(resolved.value(), "http://localhost:9090/users");
    assert!(resolved.missing().is_empty());
}

#[test]
fn reports_missing_variables_without_destroying_the_template() {
    let environment = Environment::new("Empty");

    let resolved = environment
        .resolve("https://{{host}}/{{path}}", &[])
        .expect("missing values are a recoverable result");

    assert_eq!(resolved.value(), "https://{{host}}/{{path}}");
    assert_eq!(resolved.missing(), &["host", "path"]);
}

#[test]
fn rejects_variable_cycles() {
    let environment = Environment::new("Cyclic")
        .with_variable(Variable::plain("a", "{{b}}"))
        .with_variable(Variable::plain("b", "{{a}}"));

    let error = environment
        .resolve("{{a}}", &[])
        .expect_err("cycles must be rejected");

    assert!(matches!(error, EnvironmentError::Cycle { .. }));
}

#[test]
fn redacts_secret_values() {
    let environment = Environment::new("Secure")
        .with_variable(Variable::secret("token", "keyring://project/token"));

    let display = environment.redacted_variables();

    assert_eq!(display[0].value, "••••••••");
    assert!(display[0].secret_ref.is_some());
}
