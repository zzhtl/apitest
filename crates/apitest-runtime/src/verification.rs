//! Assertion and extractor evaluation.
//!
//! Shared by the scenario runner and by single-request sends so both agree on
//! what "passed" means; the rules used to live inside `ScenarioRunner` and were
//! unreachable from the desktop send path.

use std::collections::BTreeMap;

use apitest_core::{
    ApiComponents, ApiResponse, ApiSchema, AssertionRule, ExecutionMetrics, ExtractorRule,
    VariableSource,
};
use regex::Regex;
use serde_json::Value;

use crate::{ScriptAssertion, ScriptEngine, ScriptResponse};

#[derive(Debug, Clone)]
pub struct ResponseSchemaContract {
    pub responses: Vec<ApiResponse>,
    pub components: ApiComponents,
}

pub struct CapturedResponse {
    pub status: Option<u16>,
    pub headers: BTreeMap<String, String>,
    pub body: String,
    pub metrics: Option<ExecutionMetrics>,
    pub error: Option<String>,
}

/// Evaluate one rule against a captured response.
pub fn evaluate_assertion(
    scripts: &ScriptEngine,
    rule: &AssertionRule,
    response: &CapturedResponse,
    variables: &mut BTreeMap<String, String>,
    script_response: &ScriptResponse,
    response_schema: Option<&ResponseSchemaContract>,
) -> ScriptAssertion {
    let (name, result) = match rule {
        AssertionRule::Status { expected } => (
            format!("Status = {expected}"),
            response
                .status
                .filter(|actual| actual == expected)
                .map(|_| ())
                .ok_or_else(|| {
                    format!(
                        "expected status {expected}, got {}",
                        response
                            .status
                            .map(|status| status.to_string())
                            .unwrap_or_else(|| "none".into())
                    )
                }),
        ),
        AssertionRule::HeaderEquals { name, expected } => {
            let actual = header_value(&response.headers, name);
            (
                format!("Header {name} = {expected}"),
                actual
                    .filter(|actual| *actual == expected)
                    .map(|_| ())
                    .ok_or_else(|| {
                        format!(
                            "expected header {name} to equal {expected}, got {}",
                            actual.unwrap_or("none")
                        )
                    }),
            )
        }
        AssertionRule::JsonPathEquals { path, expected } => {
            let result = serde_json::from_str::<Value>(&response.body)
                .map_err(|error| format!("response is not valid JSON: {error}"))
                .and_then(|body| {
                    json_path(&body, path)
                        .and_then(|value| {
                            value.ok_or_else(|| format!("JSONPath {path} did not match"))
                        })
                        .map(json_value_text)
                })
                .and_then(|actual| {
                    (actual == *expected)
                        .then_some(())
                        .ok_or_else(|| format!("expected {expected}, got {actual}"))
                });
            (format!("JSONPath {path} = {expected}"), result)
        }
        AssertionRule::Schema => (
            "Schema valid".into(),
            validate_response_schema(response_schema, response.status, &response.body),
        ),
        AssertionRule::DurationBelow { milliseconds } => {
            let actual = response.metrics.map(|metrics| metrics.elapsed_ms);
            (
                format!("Duration < {milliseconds} ms"),
                actual
                    .filter(|actual| actual < milliseconds)
                    .map(|_| ())
                    .ok_or_else(|| {
                        format!(
                            "expected duration below {milliseconds} ms, got {}",
                            actual
                                .map(|value| format!("{value} ms"))
                                .unwrap_or_else(|| "none".into())
                        )
                    }),
            )
        }
        AssertionRule::Script { expression } => (
            "Script expression".into(),
            evaluate_expression(scripts, expression, variables, Some(script_response)).and_then(
                |value| {
                    value
                        .as_bool()
                        .filter(|value| *value)
                        .map(|_| ())
                        .ok_or_else(|| "expression did not evaluate to true".into())
                },
            ),
        ),
    };
    match result {
        Ok(()) => ScriptAssertion {
            name,
            passed: true,
            error: None,
        },
        Err(error) => ScriptAssertion {
            name,
            passed: false,
            error: Some(error),
        },
    }
}

/// Run a boolean JavaScript expression against the response and variables.
pub fn evaluate_expression(
    scripts: &ScriptEngine,
    expression: &str,
    variables: &mut BTreeMap<String, String>,
    response: Option<&ScriptResponse>,
) -> Result<Value, String> {
    const RESULT_VARIABLE: &str = "__apitest_expression_result";
    let previous = variables.get(RESULT_VARIABLE).cloned();
    let script =
        format!("apitest.setVariable(\"{RESULT_VARIABLE}\", JSON.stringify(({expression})));");
    let mut result = scripts
        .run(&script, variables, response)
        .map_err(|error| error.to_string())?;
    let raw = result
        .variables
        .remove(RESULT_VARIABLE)
        .ok_or_else(|| "expression returned no value".to_owned())?;
    restore_variable(&mut result.variables, RESULT_VARIABLE, previous);
    *variables = result.variables;
    serde_json::from_str(&raw).map_err(|error| format!("expression returned invalid JSON: {error}"))
}

pub fn validate_response_schema(
    contract: Option<&ResponseSchemaContract>,
    status: Option<u16>,
    body: &str,
) -> Result<(), String> {
    let contract = contract.ok_or_else(|| "request has no response contract schema".to_owned())?;
    let status = status.ok_or_else(|| "response has no status for schema selection".to_owned())?;
    let response = contract
        .responses
        .iter()
        .find(|response| response.status == status.to_string())
        .or_else(|| {
            contract.responses.iter().find(|response| {
                let pattern = response.status.as_bytes();
                pattern.len() == 3
                    && pattern[0].is_ascii_digit()
                    && pattern[0] == status.to_string().as_bytes()[0]
                    && pattern[1..].eq_ignore_ascii_case(b"xx")
            })
        })
        .or_else(|| {
            contract
                .responses
                .iter()
                .find(|response| response.status.eq_ignore_ascii_case("default"))
        })
        .ok_or_else(|| format!("contract has no response schema for status {status}"))?;
    let content = response
        .content
        .iter()
        .find(|content| {
            let media_type = content.media_type.to_ascii_lowercase();
            media_type == "application/json" || media_type.ends_with("+json")
        })
        .or_else(|| response.content.first())
        .ok_or_else(|| format!("contract response {status} has no content schema"))?;
    let instance = serde_json::from_str::<Value>(body)
        .map_err(|error| format!("response is not valid JSON: {error}"))?;
    let schema = schema_document(&content.schema, &contract.components);
    let validator = cached_validator(&schema)?;
    validator
        .validate(&instance)
        .map_err(|error| format!("response does not match contract schema: {error}"))
}

/// Compile (or fetch) the validator for `schema`.
///
/// Contracts are rebuilt per send, so the cache keys on the serialized schema
/// itself; compiling a validator per assertion dwarfed the validation.
fn cached_validator(schema: &Value) -> Result<std::sync::Arc<jsonschema::Validator>, String> {
    use std::sync::{Arc, Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Arc<jsonschema::Validator>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = schema.to_string();
    if let Some(validator) = cache
        .lock()
        .expect("schema validator cache should not be poisoned")
        .get(&key)
    {
        return Ok(Arc::clone(validator));
    }
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| format!("contract response schema is invalid: {error}"))?;
    let validator = Arc::new(validator);
    let mut cache = cache
        .lock()
        .expect("schema validator cache should not be poisoned");
    if cache.len() >= 32 {
        cache.clear();
    }
    cache.insert(key, Arc::clone(&validator));
    Ok(validator)
}

pub fn schema_document(schema: &ApiSchema, components: &ApiComponents) -> Value {
    let definitions = components
        .schemas
        .iter()
        .map(|(name, schema)| (name.clone(), schema_value(schema)))
        .collect::<serde_json::Map<_, _>>();
    let mut root = schema_value(schema);
    match &mut root {
        Value::Object(object) => {
            object.insert("$defs".into(), Value::Object(definitions));
        }
        _ => {
            root = serde_json::json!({
                "allOf": [root],
                "$defs": definitions,
            });
        }
    }
    normalize_component_references(&mut root);
    root
}

pub fn schema_value(schema: &ApiSchema) -> Value {
    match schema {
        ApiSchema::Inline(value) => value.clone(),
        ApiSchema::Component(name) => serde_json::json!({
            "$ref": format!("#/$defs/{}", json_pointer_token(name)),
        }),
    }
}

pub fn normalize_component_references(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref")
                && let Some(name) = reference.strip_prefix("#/components/schemas/")
            {
                *reference = format!("#/$defs/{name}");
            }
            for value in object.values_mut() {
                normalize_component_references(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_component_references(value);
            }
        }
        _ => {}
    }
}

pub fn json_pointer_token(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

pub fn header_value<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

pub fn extract_value(rule: &ExtractorRule, response: &CapturedResponse) -> Result<String, String> {
    match &rule.source {
        VariableSource::JsonPath(path) => {
            let body = serde_json::from_str::<Value>(&response.body)
                .map_err(|error| format!("response is not valid JSON: {error}"))?;
            json_path(&body, path)?
                .map(json_value_text)
                .ok_or_else(|| format!("JSONPath {path} did not match"))
        }
        VariableSource::Header(name) => header_value(&response.headers, name)
            .map(str::to_owned)
            .ok_or_else(|| format!("response header {name} was not found")),
        VariableSource::Cookie(name) => {
            let set_cookie = header_value(&response.headers, "set-cookie")
                .ok_or_else(|| "response has no set-cookie header".to_owned())?;
            let pattern = Regex::new(&format!(r"(?:^|[,;]\s*){}=([^;,]*)", regex::escape(name)))
                .map_err(|error| error.to_string())?;
            pattern
                .captures(set_cookie)
                .and_then(|captures| captures.get(1))
                .map(|value| value.as_str().to_owned())
                .ok_or_else(|| format!("response cookie {name} was not found"))
        }
        VariableSource::Regex(pattern) => {
            let pattern = Regex::new(pattern).map_err(|error| error.to_string())?;
            let captures = pattern
                .captures(&response.body)
                .ok_or_else(|| "regular expression did not match the response body".to_owned())?;
            captures
                .get(1)
                .or_else(|| captures.get(0))
                .map(|value| value.as_str().to_owned())
                .ok_or_else(|| "regular expression produced no capture".to_owned())
        }
    }
}

pub fn json_path<'a>(value: &'a Value, path: &str) -> Result<Option<&'a Value>, String> {
    let Some(mut rest) = path.strip_prefix('$') else {
        return Err("JSONPath must start with $".into());
    };
    let mut current = Some(value);
    while !rest.is_empty() {
        if let Some(after_dot) = rest.strip_prefix('.') {
            let end = after_dot.find(['.', '[']).unwrap_or(after_dot.len());
            let name = &after_dot[..end];
            if name.is_empty() {
                return Err("JSONPath property name cannot be empty".into());
            }
            current = current.and_then(|value| value.get(name));
            rest = &after_dot[end..];
            continue;
        }
        if let Some(after_bracket) = rest.strip_prefix('[') {
            let end = after_bracket
                .find(']')
                .ok_or_else(|| "JSONPath bracket is not closed".to_owned())?;
            let token = after_bracket[..end].trim();
            if let Ok(index) = token.parse::<usize>() {
                current = current.and_then(|value| value.get(index));
            } else {
                let name = token
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .or_else(|| {
                        token
                            .strip_prefix('\'')
                            .and_then(|value| value.strip_suffix('\''))
                    })
                    .ok_or_else(|| {
                        "JSONPath brackets must contain an array index or quoted property"
                            .to_owned()
                    })?;
                current = current.and_then(|value| value.get(name));
            }
            rest = &after_bracket[end + 1..];
            continue;
        }
        return Err(format!("unsupported JSONPath segment {rest}"));
    }
    Ok(current)
}

pub fn json_value_text(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(value).expect("JSON value should serialize"))
}

pub(crate) fn restore_variable(
    variables: &mut BTreeMap<String, String>,
    name: &str,
    previous: Option<String>,
) {
    if let Some(value) = previous {
        variables.insert(name.to_owned(), value);
    } else {
        variables.remove(name);
    }
}
