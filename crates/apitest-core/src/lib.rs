//! Shared domain types and extension contracts for ApiTest.

mod automation;
mod contract;
mod environment;
mod error;
mod execution;
mod mocking;
mod model;
mod protocol;

pub use automation::{AssertionRule, ExtractorRule, ScenarioNode, TestScenario, VariableSource};
pub use contract::{
    ApiComponents, ApiContent, ApiContract, ApiParameter, ApiParameterLocation, ApiResponse,
    ApiSchema, ApiServer, HttpContract, SecurityRequirement,
};
pub use environment::{
    Environment, EnvironmentError, ResolvedText, SecretRef, Variable, VariableScope,
};
pub use error::{CoreError, ExecutionError};
pub use execution::{
    ExecutionCommand, ExecutionControlError, ExecutionEvent, ExecutionHandle, ExecutionId,
    ExecutionMetrics, ExecutionRequest, ExecutionStream, ProtocolExecutor, ResponseHead,
};
pub use mocking::{MockProfile, MockResponse, MockRule, generate_mock_rules};
pub use model::{
    ApiDefinition, ApiStatus, EntityId, Project, ProjectNode, ProjectNodeKind, RequestCase,
    RunRecord, RunState,
};
pub use protocol::{
    ApiKeyLocation, AuthSpec, BodySpec, ClientCertificate, FormField, GraphQlSpec, GrpcCallKind,
    GrpcSpec, HttpMethod, HttpProxy, HttpSpec, KeyValue, MultipartPart, OAuth2Grant, ProtocolKind,
    ProtocolSpec, SseSpec, WebSocketSpec,
};
