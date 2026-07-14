use apitest_core::{ApiDefinition, BodySpec, HttpMethod, HttpSpec, KeyValue, ProtocolSpec};
use serde_json::{Map, Value};

use crate::InteropError;

pub fn import_openapi(source: &str) -> Result<Vec<ApiDefinition>, InteropError> {
    let document: Value = serde_yaml::from_str(source)?;
    if document.get("openapi").and_then(Value::as_str).is_none() {
        return Err(InteropError::Unsupported(
            "document is not an OpenAPI 3.x definition".into(),
        ));
    }
    let base_url = document
        .pointer("/servers/0/url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end_matches('/');
    let paths = document
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| InteropError::Unsupported("OpenAPI document has no paths".into()))?;

    let mut definitions = Vec::new();
    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for (method_name, method) in methods() {
            let Some(operation) = item.get(method_name).and_then(Value::as_object) else {
                continue;
            };
            definitions.push(import_operation(base_url, path, method, item, operation));
        }
    }
    Ok(definitions)
}

fn methods() -> [(&'static str, HttpMethod); 9] {
    [
        ("get", HttpMethod::Get),
        ("post", HttpMethod::Post),
        ("put", HttpMethod::Put),
        ("patch", HttpMethod::Patch),
        ("delete", HttpMethod::Delete),
        ("head", HttpMethod::Head),
        ("options", HttpMethod::Options),
        ("trace", HttpMethod::Trace),
        ("connect", HttpMethod::Connect),
    ]
}

fn import_operation(
    base_url: &str,
    path: &str,
    method: HttpMethod,
    path_item: &Map<String, Value>,
    operation: &Map<String, Value>,
) -> ApiDefinition {
    let mut rendered_path = path.to_owned();
    let mut spec = HttpSpec::new(method, "");
    for parameter in parameters(path_item, operation) {
        let Some(name) = parameter.get("name").and_then(Value::as_str) else {
            continue;
        };
        let value = example_value(parameter);
        match parameter.get("in").and_then(Value::as_str) {
            Some("path") => {
                rendered_path = rendered_path.replace(&format!("{{{name}}}"), &value);
            }
            Some("query") => spec.query.push(KeyValue::enabled(name, value)),
            Some("header") => spec.headers.push(KeyValue::enabled(name, value)),
            _ => {}
        }
    }
    spec.url = format!("{base_url}{rendered_path}");
    spec.body = request_body(operation);

    let name = operation
        .get("summary")
        .or_else(|| operation.get("operationId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{method} {path}"));
    let mut definition = ApiDefinition::new(name, ProtocolSpec::Http(spec));
    definition.description_markdown = operation
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    definition.tags = operation
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    definition
}

fn parameters<'a>(
    path_item: &'a Map<String, Value>,
    operation: &'a Map<String, Value>,
) -> impl Iterator<Item = &'a Value> {
    path_item
        .get("parameters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            operation
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten(),
        )
}

fn example_value(parameter: &Value) -> String {
    parameter
        .get("example")
        .or_else(|| parameter.pointer("/schema/example"))
        .or_else(|| parameter.pointer("/schema/default"))
        .map(value_text)
        .unwrap_or_default()
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn request_body(operation: &Map<String, Value>) -> BodySpec {
    let Some(content) = operation
        .get("requestBody")
        .and_then(|body| body.get("content"))
        .and_then(Value::as_object)
    else {
        return BodySpec::None;
    };
    if let Some(json) = content.get("application/json") {
        if let Some(example) = json.get("example") {
            return BodySpec::Json(
                serde_json::to_string_pretty(example).unwrap_or_else(|_| example.to_string()),
            );
        }
        return BodySpec::Json("{}".into());
    }
    content
        .iter()
        .next()
        .and_then(|(_, media)| media.get("example"))
        .map(|example| BodySpec::Text(value_text(example)))
        .unwrap_or(BodySpec::None)
}
