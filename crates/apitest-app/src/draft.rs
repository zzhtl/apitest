use apitest_core::{BodySpec, HttpMethod, HttpSpec, KeyValue};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyMode {
    None,
    Json,
    Text,
    Xml,
}

#[derive(Debug, Clone)]
pub struct RequestDraft {
    pub method: HttpMethod,
    pub url: String,
    pub query: Vec<EditablePair>,
    pub headers: Vec<EditablePair>,
    pub body_mode: BodyMode,
    pub body: String,
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub validate_tls: bool,
}

impl RequestDraft {
    pub fn to_http_spec(&self) -> HttpSpec {
        HttpSpec {
            method: self.method,
            url: self.url.clone(),
            query: self
                .query
                .iter()
                .filter(|pair| !pair.is_empty() && pair.enabled)
                .map(EditablePair::to_key_value)
                .collect(),
            headers: self
                .headers
                .iter()
                .filter(|pair| !pair.is_empty() && pair.enabled)
                .map(EditablePair::to_key_value)
                .collect(),
            body: match self.body_mode {
                BodyMode::None => BodySpec::None,
                BodyMode::Json => BodySpec::Json(self.body.clone()),
                BodyMode::Text => BodySpec::Text(self.body.clone()),
                BodyMode::Xml => BodySpec::Xml(self.body.clone()),
            },
            auth: Default::default(),
            timeout_ms: self.timeout_ms,
            follow_redirects: self.follow_redirects,
            validate_tls: self.validate_tls,
        }
    }

    pub fn ensure_empty_rows(&mut self) {
        if self.query.last().is_none_or(|pair| !pair.is_empty()) {
            self.query.push(EditablePair::new("", ""));
        }
        if self.headers.last().is_none_or(|pair| !pair.is_empty()) {
            self.headers.push(EditablePair::new("", ""));
        }
    }
}

impl From<&HttpSpec> for RequestDraft {
    fn from(spec: &HttpSpec) -> Self {
        let (body_mode, body) = match &spec.body {
            BodySpec::None => (BodyMode::None, String::new()),
            BodySpec::Json(body) => (BodyMode::Json, body.clone()),
            BodySpec::Text(body) => (BodyMode::Text, body.clone()),
            BodySpec::Xml(body) => (BodyMode::Xml, body.clone()),
            _ => (BodyMode::None, String::new()),
        };
        let mut draft = Self {
            method: spec.method,
            url: spec.url.clone(),
            query: spec.query.iter().map(EditablePair::from).collect(),
            headers: spec.headers.iter().map(EditablePair::from).collect(),
            body_mode,
            body,
            timeout_ms: spec.timeout_ms,
            follow_redirects: spec.follow_redirects,
            validate_tls: spec.validate_tls,
        };
        draft.ensure_empty_rows();
        draft
    }
}

impl Default for RequestDraft {
    fn default() -> Self {
        let mut draft = Self {
            method: HttpMethod::Get,
            url: String::new(),
            query: Vec::new(),
            headers: Vec::new(),
            body_mode: BodyMode::None,
            body: String::new(),
            timeout_ms: 30_000,
            follow_redirects: true,
            validate_tls: true,
        };
        draft.ensure_empty_rows();
        draft
    }
}
