use std::pin::Pin;

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::Stream;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionCommand {
    SendMessage {
        media_type: Option<String>,
        data: Bytes,
    },
    CompleteInput,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExecutionControlError {
    #[error("this protocol execution does not accept interactive commands")]
    Unsupported,
    #[error("the protocol execution is no longer running")]
    Closed,
    #[error("the protocol command queue is full")]
    Full,
}

pub struct ExecutionHandle {
    pub id: ExecutionId,
    pub events: ExecutionStream,
    cancellation: CancellationToken,
    commands: Option<mpsc::Sender<ExecutionCommand>>,
}

impl ExecutionHandle {
    pub fn from_stream(
        id: ExecutionId,
        events: ExecutionStream,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            id,
            events,
            cancellation,
            commands: None,
        }
    }

    pub fn with_commands(
        id: ExecutionId,
        events: ExecutionStream,
        cancellation: CancellationToken,
        commands: mpsc::Sender<ExecutionCommand>,
    ) -> Self {
        Self {
            id,
            events,
            cancellation,
            commands: Some(commands),
        }
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
        if let Some(commands) = &self.commands {
            let _ = commands.try_send(ExecutionCommand::Cancel);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    pub fn command_sender(&self) -> Option<mpsc::Sender<ExecutionCommand>> {
        self.commands.clone()
    }

    pub fn try_command(&self, command: ExecutionCommand) -> Result<(), ExecutionControlError> {
        let commands = self
            .commands
            .as_ref()
            .ok_or(ExecutionControlError::Unsupported)?;
        commands.try_send(command).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => ExecutionControlError::Full,
            mpsc::error::TrySendError::Closed(_) => ExecutionControlError::Closed,
        })
    }
}

/// Executes one protocol without depending on the UI or storage implementation.
pub trait ProtocolExecutor: Send + Sync {
    fn kind(&self) -> ProtocolKind;

    fn execute(
        &self,
        request: ExecutionRequest,
        cancellation: CancellationToken,
    ) -> ExecutionStream;

    fn start(&self, request: ExecutionRequest) -> ExecutionHandle {
        let id = request.id;
        let cancellation = CancellationToken::new();
        let events = self.execute(request, cancellation.clone());
        ExecutionHandle::from_stream(id, events, cancellation)
    }
}
