//! Import, export, and code generation for ApiTest.

mod codegen;
mod openapi;
mod portable;

pub use codegen::{CodeLanguage, generate_code};
pub use openapi::import_openapi;
pub use portable::{PortableProject, export_project, import_project};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum InteropError {
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("unsupported document: {0}")]
    Unsupported(String),
}
