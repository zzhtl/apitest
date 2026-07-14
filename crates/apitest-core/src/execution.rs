use std::pin::Pin;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::Stream;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{EntityId, Environment, ExecutionError, ProtocolKind, ProtocolSpec, Variable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExecutionId(Uuid);

impl ExecutionId {
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for ExecutionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRequest {
    pub id: ExecutionId,
    pub case_id: Option<EntityId>,
    pub protocol: ProtocolSpec,
    pub environment: Environment,
    pub local_variables: Vec<Variable>,
}

impl ExecutionRequest {
    pub fn new(protocol: ProtocolSpec, environment: Environment) -> Self {
        Self {
            id: ExecutionId::new(),
            case_id: None,
            protocol,
            environment,
            local_variables: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: Option<u16>,
    pub version: Option<String>,
    pub headers: IndexMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExecutionMetrics {
    pub elapsed_ms: u64,
    pub received_bytes: u64,
    pub sent_bytes: u64,
}

#[derive(Debug, Clone)]
pub enum ExecutionEvent {
    Started {
        id: ExecutionId,
        at: DateTime<Utc>,
    },
    ResponseHead(ResponseHead),
    Data(Bytes),
    Message {
        outgoing: bool,
        media_type: Option<String>,
        data: Bytes,
        at: DateTime<Utc>,
    },
    Metrics(ExecutionMetrics),
    Completed(ExecutionMetrics),
}

pub type ExecutionStream =
    Pin<Box<dyn Stream<Item = Result<ExecutionEvent, ExecutionError>> + Send + 'static>>;

/// Executes one protocol without depending on the UI or storage implementation.
pub trait ProtocolExecutor: Send + Sync {
    fn kind(&self) -> ProtocolKind;

    fn execute(
        &self,
        request: ExecutionRequest,
        cancellation: CancellationToken,
    ) -> ExecutionStream;
}
