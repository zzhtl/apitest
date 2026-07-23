use thiserror::Error;

use crate::ProtocolKind;

/// Errors raised while manipulating ApiTest domain data.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("entity not found: {0}")]
    NotFound(String),
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Stable error categories emitted by protocol executors.
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("request was cancelled")]
    Cancelled,
    #[error("request timed out after {timeout_ms} ms")]
    Timeout { timeout_ms: u64 },
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("no executor is registered for {0:?}")]
    UnsupportedProtocol(ProtocolKind),
    #[error("authentication failed: {0}")]
    Authentication(String),
    #[error("network failure: {0}")]
    Network(String),
    #[error("TLS failure: {0}")]
    Tls(String),
    #[error("protocol failure: {0}")]
    Protocol(String),
    #[error("script failure: {0}")]
    Script(String),
    #[error("storage failure: {0}")]
    Storage(String),
}
