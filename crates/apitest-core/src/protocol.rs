use std::{fmt, path::PathBuf, str::FromStr, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{CoreError, SecretRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolKind {
    Http,
    GraphQl,
    Sse,
    WebSocket,
    Grpc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "config", rename_all = "snake_case")]
pub enum ProtocolSpec {
    Http(HttpSpec),
    GraphQl(GraphQlSpec),
    Sse(SseSpec),
    WebSocket(WebSocketSpec),
    Grpc(GrpcSpec),
}

impl ProtocolSpec {
    pub fn kind(&self) -> ProtocolKind {
        match self {
            Self::Http(_) => ProtocolKind::Http,
            Self::GraphQl(_) => ProtocolKind::GraphQl,
            Self::Sse(_) => ProtocolKind::Sse,
            Self::WebSocket(_) => ProtocolKind::WebSocket,
            Self::Grpc(_) => ProtocolKind::Grpc,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Trace,
    Connect,
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Trace => "TRACE",
            Self::Connect => "CONNECT",
        })
    }
}

impl FromStr for HttpMethod {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_uppercase().as_str() {
            "GET" => Ok(Self::Get),
            "POST" => Ok(Self::Post),
            "PUT" => Ok(Self::Put),
            "PATCH" => Ok(Self::Patch),
            "DELETE" => Ok(Self::Delete),
            "HEAD" => Ok(Self::Head),
            "OPTIONS" => Ok(Self::Options),
            "TRACE" => Ok(Self::Trace),
            "CONNECT" => Ok(Self::Connect),
            other => Err(CoreError::InvalidInput(format!(
                "unsupported HTTP method `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub name: String,
    pub value: String,
    pub enabled: bool,
    pub description: String,
}

impl KeyValue {
    pub fn enabled(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
            description: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormField {
    pub name: String,
    pub value: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MultipartPart {
    Text(FormField),
    File {
        name: String,
        path: PathBuf,
        content_type: Option<String>,
        enabled: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum BodySpec {
    #[default]
    None,
    Json(String),
    Text(String),
    Xml(String),
    FormUrlEncoded(Vec<FormField>),
    Multipart(Vec<MultipartPart>),
    BinaryFile(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpSpec {
    pub method: HttpMethod,
    pub url: String,
    pub query: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub cookies: Vec<KeyValue>,
    pub body: BodySpec,
    pub auth: AuthSpec,
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub validate_tls: bool,
    #[serde(default)]
    pub proxy: Option<HttpProxy>,
    #[serde(default)]
    pub client_certificate: Option<ClientCertificate>,
}

impl HttpSpec {
    pub fn new(method: HttpMethod, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            query: Vec::new(),
            headers: Vec::new(),
            cookies: Vec::new(),
            body: BodySpec::None,
            auth: AuthSpec::None,
            timeout_ms: Duration::from_secs(30).as_millis() as u64,
            follow_redirects: true,
            validate_tls: true,
            proxy: None,
            client_certificate: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpProxy {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<SecretRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCertificate {
    pub pem_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphQlSpec {
    pub endpoint: String,
    pub query: String,
    pub variables: String,
    pub operation_name: Option<String>,
    pub headers: Vec<KeyValue>,
    pub auth: AuthSpec,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseSpec {
    pub request: HttpSpec,
    pub reconnect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSocketSpec {
    pub url: String,
    pub query: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub subprotocols: Vec<String>,
    pub validate_tls: bool,
    pub connect_timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrpcCallKind {
    Unary,
    ServerStreaming,
    ClientStreaming,
    BidirectionalStreaming,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrpcSpec {
    pub endpoint: String,
    pub service: String,
    pub method: String,
    pub call_kind: GrpcCallKind,
    pub descriptor_set: Option<PathBuf>,
    pub proto_files: Vec<PathBuf>,
    pub import_paths: Vec<PathBuf>,
    pub use_reflection: bool,
    pub metadata: Vec<KeyValue>,
    pub message_json: String,
    pub validate_tls: bool,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyLocation {
    Header,
    Query,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuth2Grant {
    AuthorizationCodePkce,
    ClientCredentials,
    Password,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthSpec {
    #[default]
    None,
    Basic {
        username: String,
        password: SecretRef,
    },
    Bearer {
        token: SecretRef,
    },
    ApiKey {
        name: String,
        value: SecretRef,
        location: ApiKeyLocation,
    },
    OAuth2 {
        grant: OAuth2Grant,
        authorization_url: Option<String>,
        token_url: String,
        client_id: String,
        client_secret: Option<SecretRef>,
        scopes: Vec<String>,
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<SecretRef>,
        #[serde(default)]
        access_token: Option<SecretRef>,
    },
    Digest {
        username: String,
        password: SecretRef,
    },
    AwsSigV4 {
        access_key: SecretRef,
        secret_key: SecretRef,
        session_token: Option<SecretRef>,
        region: String,
        service: String,
    },
}
