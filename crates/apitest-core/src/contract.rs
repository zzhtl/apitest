use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    AuthSpec, BodySpec, ClientCertificate, FormField, GraphQlSpec, GrpcSpec, HttpMethod, HttpProxy,
    HttpSpec, KeyValue, MultipartPart, ProtocolKind, ProtocolSpec, SseSpec, WebSocketSpec,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ApiSchema {
    Inline(Value),
    Component(String),
}

impl ApiSchema {
    pub fn string() -> Self {
        Self::Inline(json!({ "type": "string" }))
    }

    pub fn boolean() -> Self {
        Self::Inline(json!({ "type": "boolean" }))
    }

    pub fn number() -> Self {
        Self::Inline(json!({ "type": "number" }))
    }

    pub fn object() -> Self {
        Self::Inline(json!({ "type": "object" }))
    }

    pub fn component(name: impl Into<String>) -> Self {
        Self::Component(name.into())
    }
}

impl Default for ApiSchema {
    fn default() -> Self {
        Self::Inline(Value::Bool(true))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiServer {
    pub url: String,
    pub description: String,
    pub variables: IndexMap<String, String>,
}

impl ApiServer {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            description: String::new(),
            variables: IndexMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiParameterLocation {
    Path,
    Query,
    Header,
    Cookie,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiParameter {
    pub name: String,
    pub location: ApiParameterLocation,
    pub required: bool,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    pub description: String,
    pub schema: ApiSchema,
    pub example: Option<Value>,
}

impl ApiParameter {
    pub fn new(name: impl Into<String>, location: ApiParameterLocation, schema: ApiSchema) -> Self {
        Self {
            name: name.into(),
            location,
            required: location == ApiParameterLocation::Path,
            enabled: true,
            description: String::new(),
            schema,
            example: None,
        }
    }

    pub fn query(name: impl Into<String>, schema: ApiSchema) -> Self {
        Self::new(name, ApiParameterLocation::Query, schema)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiContent {
    pub media_type: String,
    pub schema: ApiSchema,
    pub examples: IndexMap<String, Value>,
}

impl ApiContent {
    pub fn json(schema: ApiSchema) -> Self {
        Self {
            media_type: "application/json".into(),
            schema,
            examples: IndexMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiResponse {
    pub status: String,
    pub description: String,
    pub headers: Vec<ApiParameter>,
    pub content: Vec<ApiContent>,
}

impl ApiResponse {
    pub fn new(status: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            status: status.into(),
            description: description.into(),
            headers: Vec::new(),
            content: Vec::new(),
        }
    }

    pub fn json(
        status: impl Into<String>,
        description: impl Into<String>,
        schema: ApiSchema,
    ) -> Self {
        let mut response = Self::new(status, description);
        response.content.push(ApiContent::json(schema));
        response
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecurityRequirement {
    pub scheme: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ApiComponents {
    pub schemas: IndexMap<String, ApiSchema>,
    pub examples: IndexMap<String, Value>,
    pub security_schemes: IndexMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpContract {
    pub method: HttpMethod,
    pub path: String,
    pub servers: Vec<ApiServer>,
    pub parameters: Vec<ApiParameter>,
    pub request_body: Option<ApiContent>,
    pub body_example: BodySpec,
    pub responses: Vec<ApiResponse>,
    pub security: Vec<SecurityRequirement>,
    pub default_auth: AuthSpec,
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub validate_tls: bool,
    #[serde(default)]
    pub proxy: Option<HttpProxy>,
    #[serde(default)]
    pub client_certificate: Option<ClientCertificate>,
    pub extensions: IndexMap<String, Value>,
}

impl HttpContract {
    pub fn example_spec(&self) -> HttpSpec {
        let mut path = self.path.clone();
        let mut query = Vec::new();
        let mut headers = Vec::new();
        let mut cookies = Vec::new();
        for parameter in &self.parameters {
            let value = parameter
                .example
                .as_ref()
                .map(value_text)
                .unwrap_or_default();
            match parameter.location {
                ApiParameterLocation::Path => {
                    path = path.replace(&format!("{{{}}}", parameter.name), &value);
                }
                ApiParameterLocation::Query => query.push(KeyValue {
                    name: parameter.name.clone(),
                    value,
                    enabled: parameter.enabled,
                    description: parameter.description.clone(),
                }),
                ApiParameterLocation::Header => headers.push(KeyValue {
                    name: parameter.name.clone(),
                    value,
                    enabled: parameter.enabled,
                    description: parameter.description.clone(),
                }),
                ApiParameterLocation::Cookie => cookies.push(KeyValue {
                    name: parameter.name.clone(),
                    value,
                    enabled: parameter.enabled,
                    description: parameter.description.clone(),
                }),
            }
        }
        let url = if path.starts_with("http://") || path.starts_with("https://") {
            path
        } else if let Some(server) = self.servers.first() {
            format!(
                "{}{}",
                server.url.trim_end_matches('/'),
                ensure_leading_slash(&path)
            )
        } else {
            path
        };
        HttpSpec {
            method: self.method,
            url,
            query,
            headers,
            cookies,
            body: self.body_example.clone(),
            auth: self.default_auth.clone(),
            timeout_ms: self.timeout_ms,
            follow_redirects: self.follow_redirects,
            validate_tls: self.validate_tls,
            proxy: self.proxy.clone(),
            client_certificate: self.client_certificate.clone(),
        }
    }
}

impl Default for HttpContract {
    fn default() -> Self {
        Self {
            method: HttpMethod::Get,
            path: String::new(),
            servers: Vec::new(),
            parameters: Vec::new(),
            request_body: None,
            body_example: BodySpec::None,
            responses: vec![ApiResponse::new("200", "Successful response")],
            security: Vec::new(),
            default_auth: AuthSpec::None,
            timeout_ms: 30_000,
            follow_redirects: true,
            validate_tls: true,
            proxy: None,
            client_certificate: None,
            extensions: IndexMap::new(),
        }
    }
}

impl From<HttpSpec> for HttpContract {
    fn from(spec: HttpSpec) -> Self {
        let parameters = spec
            .query
            .iter()
            .map(|value| parameter_from_key_value(value, ApiParameterLocation::Query))
            .chain(
                spec.headers
                    .iter()
                    .map(|value| parameter_from_key_value(value, ApiParameterLocation::Header)),
            )
            .chain(
                spec.cookies
                    .iter()
                    .map(|value| parameter_from_key_value(value, ApiParameterLocation::Cookie)),
            )
            .collect();
        let request_body = content_from_body(&spec.body);
        Self {
            method: spec.method,
            path: spec.url,
            parameters,
            request_body,
            body_example: spec.body,
            default_auth: spec.auth,
            timeout_ms: spec.timeout_ms,
            follow_redirects: spec.follow_redirects,
            validate_tls: spec.validate_tls,
            proxy: spec.proxy,
            client_certificate: spec.client_certificate,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "contract", rename_all = "snake_case")]
// HTTP is the dominant contract type; boxing it would add an allocation to the common path.
#[allow(clippy::large_enum_variant)]
pub enum ApiContract {
    Http(HttpContract),
    GraphQl(GraphQlSpec),
    Sse(SseSpec),
    WebSocket(WebSocketSpec),
    Grpc(GrpcSpec),
}

impl ApiContract {
    pub fn kind(&self) -> ProtocolKind {
        match self {
            Self::Http(_) => ProtocolKind::Http,
            Self::GraphQl(_) => ProtocolKind::GraphQl,
            Self::Sse(_) => ProtocolKind::Sse,
            Self::WebSocket(_) => ProtocolKind::WebSocket,
            Self::Grpc(_) => ProtocolKind::Grpc,
        }
    }

    pub fn example_protocol(&self) -> ProtocolSpec {
        match self {
            Self::Http(contract) => ProtocolSpec::Http(contract.example_spec()),
            Self::GraphQl(spec) => ProtocolSpec::GraphQl(spec.clone()),
            Self::Sse(spec) => ProtocolSpec::Sse(spec.clone()),
            Self::WebSocket(spec) => ProtocolSpec::WebSocket(spec.clone()),
            Self::Grpc(spec) => ProtocolSpec::Grpc(spec.clone()),
        }
    }
}

impl From<ProtocolSpec> for ApiContract {
    fn from(spec: ProtocolSpec) -> Self {
        match spec {
            ProtocolSpec::Http(spec) => Self::Http(spec.into()),
            ProtocolSpec::GraphQl(spec) => Self::GraphQl(spec),
            ProtocolSpec::Sse(spec) => Self::Sse(spec),
            ProtocolSpec::WebSocket(spec) => Self::WebSocket(spec),
            ProtocolSpec::Grpc(spec) => Self::Grpc(spec),
        }
    }
}

impl From<HttpContract> for ApiContract {
    fn from(contract: HttpContract) -> Self {
        Self::Http(contract)
    }
}

fn parameter_from_key_value(value: &KeyValue, location: ApiParameterLocation) -> ApiParameter {
    ApiParameter {
        name: value.name.clone(),
        location,
        required: false,
        enabled: value.enabled,
        description: value.description.clone(),
        schema: ApiSchema::string(),
        example: Some(Value::String(value.value.clone())),
    }
}

fn enabled_by_default() -> bool {
    true
}

fn content_from_body(body: &BodySpec) -> Option<ApiContent> {
    let (media_type, example) = match body {
        BodySpec::None => return None,
        BodySpec::Json(value) => (
            "application/json",
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.clone())),
        ),
        BodySpec::Text(value) => ("text/plain", Value::String(value.clone())),
        BodySpec::Xml(value) => ("application/xml", Value::String(value.clone())),
        BodySpec::FormUrlEncoded(fields) => {
            ("application/x-www-form-urlencoded", form_value(fields))
        }
        BodySpec::Multipart(parts) => ("multipart/form-data", multipart_value(parts)),
        BodySpec::BinaryFile(path) => (
            "application/octet-stream",
            Value::String(path.display().to_string()),
        ),
    };
    let mut content = ApiContent {
        media_type: media_type.into(),
        schema: ApiSchema::default(),
        examples: IndexMap::new(),
    };
    content.examples.insert("default".into(), example);
    Some(content)
}

fn form_value(fields: &[FormField]) -> Value {
    Value::Object(
        fields
            .iter()
            .filter(|field| field.enabled)
            .map(|field| (field.name.clone(), Value::String(field.value.clone())))
            .collect(),
    )
}

fn multipart_value(parts: &[MultipartPart]) -> Value {
    Value::Object(
        parts
            .iter()
            .filter_map(|part| match part {
                MultipartPart::Text(field) if field.enabled => {
                    Some((field.name.clone(), Value::String(field.value.clone())))
                }
                MultipartPart::File {
                    name,
                    path,
                    enabled: true,
                    ..
                } => Some((name.clone(), Value::String(path.display().to_string()))),
                MultipartPart::Text(_) | MultipartPart::File { .. } => None,
            })
            .collect(),
    )
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn ensure_leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}
