use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{BodySpec, KeyValue, ProtocolSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(Uuid);

impl EntityId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    pub fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiStatus {
    Designing,
    Developing,
    Testing,
    Released,
    Deprecated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiDefinition {
    pub id: EntityId,
    pub name: String,
    pub description_markdown: String,
    pub status: ApiStatus,
    pub tags: Vec<String>,
    pub protocol: ProtocolSpec,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ApiDefinition {
    pub fn new(name: impl Into<String>, protocol: ProtocolSpec) -> Self {
        let now = Utc::now();
        Self {
            id: EntityId::new(),
            name: name.into(),
            description_markdown: String::new(),
            status: ApiStatus::Designing,
            tags: Vec::new(),
            protocol,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestCase {
    pub id: EntityId,
    pub definition_id: EntityId,
    pub name: String,
    pub query_values: Vec<KeyValue>,
    pub header_values: Vec<KeyValue>,
    pub body: BodySpec,
    pub pre_request_script: String,
    pub post_response_script: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl RequestCase {
    pub fn for_definition(definition: &ApiDefinition, name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: EntityId::new(),
            definition_id: definition.id,
            name: name.into(),
            query_values: Vec::new(),
            header_values: Vec::new(),
            body: BodySpec::None,
            pre_request_script: String::new(),
            post_response_script: String::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: EntityId,
    pub name: String,
    pub description: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: EntityId::new(),
            name: name.into(),
            description: String::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectNodeKind {
    Folder,
    ApiDefinition,
    RequestCase,
    TestScenario,
    DataModel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectNode {
    pub id: EntityId,
    pub project_id: EntityId,
    pub parent_id: Option<EntityId>,
    pub entity_id: Option<EntityId>,
    pub kind: ProjectNodeKind,
    pub name: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Passed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: EntityId,
    pub request_case_id: Option<EntityId>,
    pub state: RunState,
    pub status_code: Option<u16>,
    pub elapsed_ms: u64,
    pub response_bytes: u64,
    pub body_path: Option<String>,
    pub error: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}
