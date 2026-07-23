//! Import, export, and code generation for ApiTest.

mod codegen;
mod collection;
mod openapi;
mod portable;

pub use codegen::{CodeLanguage, generate_code};
pub use collection::{
    IMPORTED_AUTH_MARKER, export_curl, export_har, export_postman, import_curl, import_har,
    import_postman,
};
pub use openapi::{
    OpenApiImport, OpenApiIssue, OpenApiIssueLevel, export_openapi, export_openapi_documentation,
    import_openapi, import_openapi_document, validate_openapi,
};
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
