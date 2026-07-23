use std::path::PathBuf;

use apitest_core::{
    ApiKeyLocation, AuthSpec, BodySpec, ClientCertificate, EntityId, FormField, HttpMethod,
    HttpProxy, HttpSpec, KeyValue, MultipartPart, OAuth2Grant, SecretRef,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditablePair {
    pub enabled: bool,
    pub name: String,
    pub value: String,
    pub description: String,
}

impl EditablePair {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            enabled: true,
            name: name.into(),
            value: value.into(),
            description: String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.name.is_empty() && self.value.is_empty() && self.description.is_empty()
    }

    fn to_key_value(&self) -> KeyValue {
        KeyValue {
            name: self.name.clone(),
            value: self.value.clone(),
            enabled: self.enabled,
            description: self.description.clone(),
        }
    }

    fn to_form_field(&self) -> FormField {
        FormField {
            name: self.name.clone(),
            value: self.value.clone(),
            enabled: self.enabled,
        }
    }
}

impl From<&KeyValue> for EditablePair {
    fn from(value: &KeyValue) -> Self {
        Self {
            enabled: value.enabled,
            name: value.name.clone(),
            value: value.value.clone(),
            description: value.description.clone(),
        }
    }
}

impl From<&FormField> for EditablePair {
    fn from(value: &FormField) -> Self {
        Self {
            enabled: value.enabled,
            name: value.name.clone(),
            value: value.value.clone(),
            description: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultipartValueMode {
    Text,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditableMultipartPart {
    pub enabled: bool,
    pub name: String,
    pub mode: MultipartValueMode,
    pub value: String,
    pub file: Option<PathBuf>,
    pub content_type: String,
}

impl EditableMultipartPart {
    pub fn empty() -> Self {
        Self {
            enabled: true,
            name: String::new(),
            mode: MultipartValueMode::Text,
            value: String::new(),
            file: None,
            content_type: String::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.value.is_empty()
            && self.file.is_none()
            && self.content_type.is_empty()
    }

    fn to_part(&self) -> MultipartPart {
        match self.mode {
            MultipartValueMode::Text => MultipartPart::Text(FormField {
                name: self.name.clone(),
                value: self.value.clone(),
                enabled: self.enabled,
            }),
            MultipartValueMode::File => MultipartPart::File {
                name: self.name.clone(),
                path: self.file.clone().unwrap_or_default(),
                content_type: (!self.content_type.trim().is_empty())
                    .then(|| self.content_type.clone()),
                enabled: self.enabled,
            },
        }
    }
}

impl From<&MultipartPart> for EditableMultipartPart {
    fn from(value: &MultipartPart) -> Self {
        match value {
            MultipartPart::Text(field) => Self {
                enabled: field.enabled,
                name: field.name.clone(),
                mode: MultipartValueMode::Text,
                value: field.value.clone(),
                file: None,
                content_type: String::new(),
            },
            MultipartPart::File {
                name,
                path,
                content_type,
                enabled,
            } => Self {
                enabled: *enabled,
                name: name.clone(),
                mode: MultipartValueMode::File,
                value: String::new(),
                file: Some(path.clone()),
                content_type: content_type.clone().unwrap_or_default(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretDraft {
    pub reference: SecretRef,
    pub replacement: String,
    pub configured: bool,
}

impl SecretDraft {
    fn for_request(id: EntityId, suffix: &str) -> Self {
        Self {
            reference: SecretRef::new(format!("keyring://request-{id}-{suffix}")),
            replacement: String::new(),
            configured: false,
        }
    }

    fn from_reference(reference: &SecretRef) -> Self {
        Self {
            reference: reference.clone(),
            replacement: String::new(),
            configured: true,
        }
    }

    fn from_optional(reference: Option<&SecretRef>, id: EntityId, suffix: &str) -> Self {
        reference.map_or_else(|| Self::for_request(id, suffix), Self::from_reference)
    }

    pub fn is_ready(&self) -> bool {
        self.configured || !self.replacement.is_empty()
    }

    pub fn has_pending_value(&self) -> bool {
        !self.replacement.is_empty()
    }

    pub fn mark_saved(&mut self) {
        self.configured = true;
        self.replacement.clear();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyDraft {
    pub url: String,
    pub username: String,
    pub password: SecretDraft,
}

impl ProxyDraft {
    pub fn new(id: EntityId) -> Self {
        Self {
            url: String::new(),
            username: String::new(),
            password: SecretDraft::for_request(id, "proxy-password"),
        }
    }

    fn from_http_proxy(proxy: &HttpProxy, id: EntityId) -> Self {
        Self {
            url: proxy.url.clone(),
            username: proxy.username.clone().unwrap_or_default(),
            password: SecretDraft::from_optional(proxy.password.as_ref(), id, "proxy-password"),
        }
    }

    fn to_http_proxy(&self) -> HttpProxy {
        HttpProxy {
            url: self.url.clone(),
            username: (!self.username.trim().is_empty()).then(|| self.username.clone()),
            password: self
                .password
                .is_ready()
                .then(|| self.password.reference.clone()),
        }
    }

    pub fn has_pending_secret(&self) -> bool {
        self.password.has_pending_value()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    None,
    Basic,
    Bearer,
    ApiKey,
    OAuth2,
    Digest,
    AwsSigV4,
    Unsupported,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AuthDraft {
    #[default]
    None,
    Basic {
        username: String,
        password: SecretDraft,
    },
    Bearer {
        token: SecretDraft,
    },
    ApiKey {
        name: String,
        value: SecretDraft,
        location: ApiKeyLocation,
    },
    OAuth2 {
        grant: OAuth2Grant,
        authorization_url: String,
        token_url: String,
        client_id: String,
        client_secret: SecretDraft,
        scopes: String,
        username: String,
        password: SecretDraft,
        access_token: SecretDraft,
    },
    Digest {
        username: String,
        password: SecretDraft,
    },
    AwsSigV4 {
        access_key: SecretDraft,
        secret_key: SecretDraft,
        session_token: SecretDraft,
        region: String,
        service: String,
    },
    Unsupported(AuthSpec),
}

impl AuthDraft {
    pub fn for_mode(mode: AuthMode, id: EntityId) -> Self {
        match mode {
            AuthMode::None | AuthMode::Unsupported => Self::None,
            AuthMode::Basic => Self::Basic {
                username: String::new(),
                password: SecretDraft::for_request(id, "basic-password"),
            },
            AuthMode::Bearer => Self::Bearer {
                token: SecretDraft::for_request(id, "bearer-token"),
            },
            AuthMode::ApiKey => Self::ApiKey {
                name: String::new(),
                value: SecretDraft::for_request(id, "api-key"),
                location: ApiKeyLocation::Header,
            },
            AuthMode::OAuth2 => Self::OAuth2 {
                grant: OAuth2Grant::ClientCredentials,
                authorization_url: String::new(),
                token_url: String::new(),
                client_id: String::new(),
                client_secret: SecretDraft::for_request(id, "oauth-client-secret"),
                scopes: String::new(),
                username: String::new(),
                password: SecretDraft::for_request(id, "oauth-password"),
                access_token: SecretDraft::for_request(id, "oauth-access-token"),
            },
            AuthMode::Digest => Self::Digest {
                username: String::new(),
                password: SecretDraft::for_request(id, "digest-password"),
            },
            AuthMode::AwsSigV4 => Self::AwsSigV4 {
                access_key: SecretDraft::for_request(id, "aws-access-key"),
                secret_key: SecretDraft::for_request(id, "aws-secret-key"),
                session_token: SecretDraft::for_request(id, "aws-session-token"),
                region: String::new(),
                service: String::new(),
            },
        }
    }

    pub fn mode(&self) -> AuthMode {
        match self {
            Self::None => AuthMode::None,
            Self::Basic { .. } => AuthMode::Basic,
            Self::Bearer { .. } => AuthMode::Bearer,
            Self::ApiKey { .. } => AuthMode::ApiKey,
            Self::OAuth2 { .. } => AuthMode::OAuth2,
            Self::Digest { .. } => AuthMode::Digest,
            Self::AwsSigV4 { .. } => AuthMode::AwsSigV4,
            Self::Unsupported(_) => AuthMode::Unsupported,
        }
    }

    pub fn secret(&self) -> Option<&SecretDraft> {
        self.secrets().into_iter().next()
    }

    pub fn secret_mut(&mut self) -> Option<&mut SecretDraft> {
        self.secrets_mut().into_iter().next()
    }

    pub fn secrets(&self) -> Vec<&SecretDraft> {
        match self {
            Self::Basic { password, .. } => vec![password],
            Self::Bearer { token } => vec![token],
            Self::ApiKey { value, .. } => vec![value],
            Self::OAuth2 {
                client_secret,
                password,
                access_token,
                ..
            } => vec![client_secret, password, access_token],
            Self::Digest { password, .. } => vec![password],
            Self::AwsSigV4 {
                access_key,
                secret_key,
                session_token,
                ..
            } => vec![access_key, secret_key, session_token],
            Self::None | Self::Unsupported(_) => Vec::new(),
        }
    }

    pub fn secrets_mut(&mut self) -> Vec<&mut SecretDraft> {
        match self {
            Self::Basic { password, .. } => vec![password],
            Self::Bearer { token } => vec![token],
            Self::ApiKey { value, .. } => vec![value],
            Self::OAuth2 {
                client_secret,
                password,
                access_token,
                ..
            } => vec![client_secret, password, access_token],
            Self::Digest { password, .. } => vec![password],
            Self::AwsSigV4 {
                access_key,
                secret_key,
                session_token,
                ..
            } => vec![access_key, secret_key, session_token],
            Self::None | Self::Unsupported(_) => Vec::new(),
        }
    }

    pub fn has_pending_secret(&self) -> bool {
        self.secrets()
            .into_iter()
            .any(SecretDraft::has_pending_value)
    }

    pub fn references_for_request(id: EntityId) -> Vec<SecretRef> {
        [
            "basic-password",
            "bearer-token",
            "api-key",
            "oauth-client-secret",
            "oauth-password",
            "oauth-access-token",
            "digest-password",
            "aws-access-key",
            "aws-secret-key",
            "aws-session-token",
            "proxy-password",
        ]
        .into_iter()
        .map(|suffix| SecretDraft::for_request(id, suffix).reference)
        .collect()
    }

    fn to_auth_spec(&self) -> AuthSpec {
        match self {
            Self::None => AuthSpec::None,
            Self::Basic { username, password } => AuthSpec::Basic {
                username: username.clone(),
                password: password.reference.clone(),
            },
            Self::Bearer { token } => AuthSpec::Bearer {
                token: token.reference.clone(),
            },
            Self::ApiKey {
                name,
                value,
                location,
            } => AuthSpec::ApiKey {
                name: name.clone(),
                value: value.reference.clone(),
                location: *location,
            },
            Self::OAuth2 {
                grant,
                authorization_url,
                token_url,
                client_id,
                client_secret,
                scopes,
                username,
                password,
                access_token,
            } => AuthSpec::OAuth2 {
                grant: *grant,
                authorization_url: (!authorization_url.trim().is_empty())
                    .then(|| authorization_url.clone()),
                token_url: token_url.clone(),
                client_id: client_id.clone(),
                client_secret: client_secret
                    .is_ready()
                    .then(|| client_secret.reference.clone()),
                scopes: scopes.split_whitespace().map(str::to_owned).collect(),
                username: (!username.trim().is_empty()).then(|| username.clone()),
                password: password.is_ready().then(|| password.reference.clone()),
                access_token: access_token
                    .is_ready()
                    .then(|| access_token.reference.clone()),
            },
            Self::Digest { username, password } => AuthSpec::Digest {
                username: username.clone(),
                password: password.reference.clone(),
            },
            Self::AwsSigV4 {
                access_key,
                secret_key,
                session_token,
                region,
                service,
            } => AuthSpec::AwsSigV4 {
                access_key: access_key.reference.clone(),
                secret_key: secret_key.reference.clone(),
                session_token: session_token
                    .is_ready()
                    .then(|| session_token.reference.clone()),
                region: region.clone(),
                service: service.clone(),
            },
            Self::Unsupported(auth) => auth.clone(),
        }
    }

    pub fn from_auth_spec(value: &AuthSpec, id: EntityId) -> Self {
        match value {
            AuthSpec::None => Self::None,
            AuthSpec::Basic { username, password } => Self::Basic {
                username: username.clone(),
                password: SecretDraft::from_reference(password),
            },
            AuthSpec::Bearer { token } => Self::Bearer {
                token: SecretDraft::from_reference(token),
            },
            AuthSpec::ApiKey {
                name,
                value,
                location,
            } => Self::ApiKey {
                name: name.clone(),
                value: SecretDraft::from_reference(value),
                location: *location,
            },
            AuthSpec::OAuth2 {
                grant,
                authorization_url,
                token_url,
                client_id,
                client_secret,
                scopes,
                username,
                password,
                access_token,
            } => Self::OAuth2 {
                grant: *grant,
                authorization_url: authorization_url.clone().unwrap_or_default(),
                token_url: token_url.clone(),
                client_id: client_id.clone(),
                client_secret: SecretDraft::from_optional(
                    client_secret.as_ref(),
                    id,
                    "oauth-client-secret",
                ),
                scopes: scopes.join(" "),
                username: username.clone().unwrap_or_default(),
                password: SecretDraft::from_optional(password.as_ref(), id, "oauth-password"),
                access_token: SecretDraft::from_optional(
                    access_token.as_ref(),
                    id,
                    "oauth-access-token",
                ),
            },
            AuthSpec::Digest { username, password } => Self::Digest {
                username: username.clone(),
                password: SecretDraft::from_reference(password),
            },
            AuthSpec::AwsSigV4 {
                access_key,
                secret_key,
                session_token,
                region,
                service,
            } => Self::AwsSigV4 {
                access_key: SecretDraft::from_reference(access_key),
                secret_key: SecretDraft::from_reference(secret_key),
                session_token: SecretDraft::from_optional(
                    session_token.as_ref(),
                    id,
                    "aws-session-token",
                ),
                region: region.clone(),
                service: service.clone(),
            },
        }
    }
}

impl From<&AuthSpec> for AuthDraft {
    fn from(value: &AuthSpec) -> Self {
        Self::from_auth_spec(value, EntityId::new())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyMode {
    None,
    Json,
    Text,
    Xml,
    FormUrlEncoded,
    Multipart,
    Binary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestDraft {
    pub method: HttpMethod,
    pub url: String,
    pub query: Vec<EditablePair>,
    pub headers: Vec<EditablePair>,
    pub cookies: Vec<EditablePair>,
    pub body_mode: BodyMode,
    pub body: String,
    pub form_fields: Vec<EditablePair>,
    pub multipart_parts: Vec<EditableMultipartPart>,
    pub binary_file: Option<PathBuf>,
    pub auth: AuthDraft,
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub validate_tls: bool,
    pub proxy: Option<ProxyDraft>,
    pub client_certificate: Option<ClientCertificate>,
}

impl RequestDraft {
    pub fn to_http_spec(&self) -> HttpSpec {
        HttpSpec {
            method: self.method,
            url: self.url.clone(),
            query: self
                .query
                .iter()
                .filter(|pair| !pair.is_empty())
                .map(EditablePair::to_key_value)
                .collect(),
            headers: self
                .headers
                .iter()
                .filter(|pair| !pair.is_empty())
                .map(EditablePair::to_key_value)
                .collect(),
            cookies: self
                .cookies
                .iter()
                .filter(|pair| !pair.is_empty())
                .map(EditablePair::to_key_value)
                .collect(),
            body: match self.body_mode {
                BodyMode::None => BodySpec::None,
                BodyMode::Json => BodySpec::Json(self.body.clone()),
                BodyMode::Text => BodySpec::Text(self.body.clone()),
                BodyMode::Xml => BodySpec::Xml(self.body.clone()),
                BodyMode::FormUrlEncoded => BodySpec::FormUrlEncoded(
                    self.form_fields
                        .iter()
                        .filter(|pair| !pair.is_empty())
                        .map(EditablePair::to_form_field)
                        .collect(),
                ),
                BodyMode::Multipart => BodySpec::Multipart(
                    self.multipart_parts
                        .iter()
                        .filter(|part| !part.is_empty())
                        .map(EditableMultipartPart::to_part)
                        .collect(),
                ),
                BodyMode::Binary => {
                    BodySpec::BinaryFile(self.binary_file.clone().unwrap_or_default())
                }
            },
            auth: self.auth.to_auth_spec(),
            timeout_ms: self.timeout_ms,
            follow_redirects: self.follow_redirects,
            validate_tls: self.validate_tls,
            proxy: self.proxy.as_ref().map(ProxyDraft::to_http_proxy),
            client_certificate: self.client_certificate.clone(),
        }
    }

    pub fn has_pending_secret(&self) -> bool {
        self.auth.has_pending_secret()
            || self
                .proxy
                .as_ref()
                .is_some_and(ProxyDraft::has_pending_secret)
    }

    pub fn ensure_empty_rows(&mut self) {
        if self.query.last().is_none_or(|pair| !pair.is_empty()) {
            self.query.push(EditablePair::new("", ""));
        }
        if self.headers.last().is_none_or(|pair| !pair.is_empty()) {
            self.headers.push(EditablePair::new("", ""));
        }
        if self.cookies.last().is_none_or(|pair| !pair.is_empty()) {
            self.cookies.push(EditablePair::new("", ""));
        }
        if self.form_fields.last().is_none_or(|pair| !pair.is_empty()) {
            self.form_fields.push(EditablePair::new("", ""));
        }
        if self
            .multipart_parts
            .last()
            .is_none_or(|part| !part.is_empty())
        {
            self.multipart_parts.push(EditableMultipartPart::empty());
        }
    }
}

impl RequestDraft {
    pub fn from_http_spec(spec: &HttpSpec, id: EntityId) -> Self {
        let mut draft = Self {
            method: spec.method,
            url: spec.url.clone(),
            query: spec.query.iter().map(EditablePair::from).collect(),
            headers: spec.headers.iter().map(EditablePair::from).collect(),
            cookies: spec.cookies.iter().map(EditablePair::from).collect(),
            body_mode: BodyMode::None,
            body: String::new(),
            form_fields: Vec::new(),
            multipart_parts: Vec::new(),
            binary_file: None,
            auth: AuthDraft::from_auth_spec(&spec.auth, id),
            timeout_ms: spec.timeout_ms,
            follow_redirects: spec.follow_redirects,
            validate_tls: spec.validate_tls,
            proxy: spec
                .proxy
                .as_ref()
                .map(|proxy| ProxyDraft::from_http_proxy(proxy, id)),
            client_certificate: spec.client_certificate.clone(),
        };
        match &spec.body {
            BodySpec::None => {}
            BodySpec::Json(body) => {
                draft.body_mode = BodyMode::Json;
                draft.body = body.clone();
            }
            BodySpec::Text(body) => {
                draft.body_mode = BodyMode::Text;
                draft.body = body.clone();
            }
            BodySpec::Xml(body) => {
                draft.body_mode = BodyMode::Xml;
                draft.body = body.clone();
            }
            BodySpec::FormUrlEncoded(fields) => {
                draft.body_mode = BodyMode::FormUrlEncoded;
                draft.form_fields = fields.iter().map(EditablePair::from).collect();
            }
            BodySpec::Multipart(parts) => {
                draft.body_mode = BodyMode::Multipart;
                draft.multipart_parts = parts.iter().map(EditableMultipartPart::from).collect();
            }
            BodySpec::BinaryFile(path) => {
                draft.body_mode = BodyMode::Binary;
                draft.binary_file = Some(path.clone());
            }
        }
        draft.ensure_empty_rows();
        draft
    }
}

impl From<&HttpSpec> for RequestDraft {
    fn from(spec: &HttpSpec) -> Self {
        Self::from_http_spec(spec, EntityId::new())
    }
}

impl Default for RequestDraft {
    fn default() -> Self {
        let mut draft = Self {
            method: HttpMethod::Get,
            url: String::new(),
            query: Vec::new(),
            headers: Vec::new(),
            cookies: Vec::new(),
            body_mode: BodyMode::None,
            body: String::new(),
            form_fields: Vec::new(),
            multipart_parts: Vec::new(),
            binary_file: None,
            auth: AuthDraft::None,
            timeout_ms: 30_000,
            follow_redirects: true,
            validate_tls: true,
            proxy: None,
            client_certificate: None,
        };
        draft.ensure_empty_rows();
        draft
    }
}
