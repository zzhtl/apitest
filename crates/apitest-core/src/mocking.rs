use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use crate::{
    ApiComponents, ApiContract, ApiDefinition, ApiParameterLocation, ApiSchema, EntityId,
    HttpMethod, KeyValue,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<KeyValue>,
    pub body: String,
    pub delay_ms: u64,
    pub script: String,
}

impl Default for MockResponse {
    fn default() -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: String::new(),
            delay_ms: 0,
            script: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockRule {
    pub id: EntityId,
    pub name: String,
    pub enabled: bool,
    pub priority: i64,
    pub method: HttpMethod,
    pub path: String,
    pub query: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub body_contains: Option<String>,
    pub path_variables: IndexMap<String, String>,
    pub response: MockResponse,
}

impl Default for MockRule {
    fn default() -> Self {
        Self {
            id: EntityId::new(),
            name: String::new(),
            enabled: true,
            priority: 0,
            method: HttpMethod::Get,
            path: "/".into(),
            query: Vec::new(),
            headers: Vec::new(),
            body_contains: None,
            path_variables: IndexMap::new(),
            response: MockResponse::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MockProfile {
    pub id: EntityId,
    pub name: String,
    pub bind_address: String,
    pub port: u16,
    pub rules: Vec<MockRule>,
}

impl Default for MockProfile {
    fn default() -> Self {
        Self {
            id: EntityId::new(),
            name: String::new(),
            bind_address: "127.0.0.1".into(),
            port: 0,
            rules: Vec::new(),
        }
    }
}

pub fn generate_mock_rules(
    definitions: &[ApiDefinition],
    components: &ApiComponents,
) -> Vec<MockRule> {
    definitions
        .iter()
        .filter_map(|definition| {
            let ApiContract::Http(contract) = &definition.contract else {
                return None;
            };
            let response = contract
                .responses
                .iter()
                .find(|response| response.status.starts_with('2'))
                .or_else(|| contract.responses.first());
            let status = response
                .and_then(|response| response.status.parse::<u16>().ok())
                .unwrap_or(200);
            let content = response.and_then(|response| response.content.first());
            let body_value = content.and_then(|content| {
                content
                    .examples
                    .values()
                    .next()
                    .cloned()
                    .or_else(|| Some(schema_example(&content.schema, components, 0)))
            });
            let mut response_headers = response
                .into_iter()
                .flat_map(|response| &response.headers)
                .filter(|header| header.enabled)
                .filter_map(|header| {
                    header.example.as_ref().map(|value| KeyValue {
                        name: header.name.clone(),
                        value: value_text(value),
                        enabled: true,
                        description: header.description.clone(),
                    })
                })
                .collect::<Vec<_>>();
            if let Some(content) = content
                && !response_headers
                    .iter()
                    .any(|header| header.name.eq_ignore_ascii_case("content-type"))
            {
                response_headers.push(KeyValue::enabled(
                    "content-type",
                    content.media_type.clone(),
                ));
            }
            let parameter = |location| {
                contract
                    .parameters
                    .iter()
                    .filter(|parameter| parameter.enabled)
                    .filter(move |parameter| parameter.location == location)
                    .filter_map(|parameter| {
                        parameter.example.as_ref().map(|value| KeyValue {
                            name: parameter.name.clone(),
                            value: value_text(value),
                            enabled: true,
                            description: parameter.description.clone(),
                        })
                    })
                    .collect::<Vec<_>>()
            };
            let path_variables = contract
                .parameters
                .iter()
                .filter(|parameter| parameter.enabled)
                .filter(|parameter| parameter.location == ApiParameterLocation::Path)
                .filter_map(|parameter| {
                    parameter
                        .example
                        .as_ref()
                        .map(|value| (parameter.name.clone(), value_text(value)))
                })
                .collect();
            Some(MockRule {
                name: definition.name.clone(),
                priority: -(status as i64),
                method: contract.method,
                path: mock_path(&contract.path),
                query: parameter(ApiParameterLocation::Query),
                headers: parameter(ApiParameterLocation::Header),
                path_variables,
                response: MockResponse {
                    status,
                    headers: response_headers,
                    body: body_value
                        .and_then(|value| {
                            (!value.is_null()).then(|| {
                                serde_json::to_string_pretty(&value)
                                    .expect("JSON mock example should serialize")
                            })
                        })
                        .unwrap_or_default(),
                    ..MockResponse::default()
                },
                ..MockRule::default()
            })
        })
        .collect()
}

fn mock_path(value: &str) -> String {
    Url::parse(value)
        .ok()
        .map(|url| url.path().to_owned())
        .unwrap_or_else(|| {
            let path = value.split('?').next().unwrap_or(value);
            if path.starts_with('/') {
                path.to_owned()
            } else {
                format!("/{path}")
            }
        })
}

fn schema_example(schema: &ApiSchema, components: &ApiComponents, depth: usize) -> Value {
    if depth >= 8 {
        return Value::Null;
    }
    let value = match schema {
        ApiSchema::Component(name) => {
            return components
                .schemas
                .get(name)
                .map(|schema| schema_example(schema, components, depth + 1))
                .unwrap_or(Value::Null);
        }
        ApiSchema::Inline(value) => value,
    };
    if let Some(example) = value.get("example") {
        return example.clone();
    }
    if let Some(default) = value.get("default") {
        return default.clone();
    }
    if let Some(value) = value
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return value.clone();
    }
    if let Some(schema) = value
        .get("oneOf")
        .or_else(|| value.get("anyOf"))
        .and_then(Value::as_array)
        .and_then(|schemas| schemas.first())
    {
        return schema_example(&ApiSchema::Inline(schema.clone()), components, depth + 1);
    }
    match value.get("type").and_then(Value::as_str) {
        Some("object") => Value::Object(
            value
                .get("properties")
                .and_then(Value::as_object)
                .map(|properties| {
                    properties
                        .iter()
                        .map(|(name, schema)| {
                            (
                                name.clone(),
                                schema_example(
                                    &ApiSchema::Inline(schema.clone()),
                                    components,
                                    depth + 1,
                                ),
                            )
                        })
                        .collect::<Map<_, _>>()
                })
                .unwrap_or_default(),
        ),
        Some("array") => Value::Array(vec![
            value
                .get("items")
                .map(|items| {
                    schema_example(&ApiSchema::Inline(items.clone()), components, depth + 1)
                })
                .unwrap_or(Value::Null),
        ]),
        Some("integer") => Value::from(0),
        Some("number") => Value::from(0.0),
        Some("boolean") => Value::Bool(true),
        Some("string") => Value::String(
            match value.get("format").and_then(Value::as_str) {
                Some("date") => "2026-01-01",
                Some("date-time") => "2026-01-01T00:00:00Z",
                Some("uuid") => "00000000-0000-4000-8000-000000000000",
                _ => "string",
            }
            .into(),
        ),
        _ => Value::Null,
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}
