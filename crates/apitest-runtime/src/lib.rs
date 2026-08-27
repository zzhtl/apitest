//! Protocol and automation runtime for ApiTest.

mod cookies;
mod grpc;
mod http;
mod mock;
mod registry;
mod scenario;
mod script;
mod verification;
mod websocket;

pub use cookies::{PersistentCookieJar, StoredCookie};
pub use grpc::{
    GrpcDescriptor, GrpcDescriptorError, GrpcDescriptorFileError, GrpcExecutor, GrpcMethod,
};
pub use http::HttpExecutor;
pub use mock::{MockRoute, MockServer, MockServerError};
pub use registry::ExecutorRegistry;
pub use scenario::{
    Scenario, ScenarioCase, ScenarioError, ScenarioReport, ScenarioRunner, ScenarioStep, StepReport,
};
pub use script::{ScriptAssertion, ScriptEngine, ScriptError, ScriptResponse, ScriptResult};
pub use verification::{
    CapturedResponse, ResponseSchemaContract, evaluate_assertion, evaluate_expression,
    extract_value,
};
pub use websocket::{WebSocketExecutor, WebSocketSession};
