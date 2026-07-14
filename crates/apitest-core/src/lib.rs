//! Shared domain types and extension contracts for ApiTest.

mod environment;
mod error;
mod execution;
mod model;
mod protocol;

pub use environment::{
    Environment, EnvironmentError, ResolvedText, SecretRef, Variable, VariableScope,
};
pub use error::{CoreError, ExecutionError};
pub use execution::{
    ExecutionEvent, ExecutionId, ExecutionMetrics, ExecutionRequest, ExecutionStream,
    ProtocolExecutor, ResponseHead,
};
pub use model::{
    ApiDefinition, ApiStatus, EntityId, Project, ProjectNode, ProjectNodeKind, RequestCase,
    RunRecord, RunState,
};
pub use protocol::{
    ApiKeyLocation, AuthSpec, BodySpec, FormField, GraphQlSpec, GrpcCallKind, GrpcSpec, HttpMethod,
    HttpSpec, KeyValue, MultipartPart, OAuth2Grant, ProtocolKind, ProtocolSpec, SseSpec,
    WebSocketSpec,
};
