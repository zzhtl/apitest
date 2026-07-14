use apitest_core::{ApiDefinition, Project};
use serde::{Deserialize, Serialize};

use crate::InteropError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortableProject {
    pub schema_version: u32,
    pub project: Project,
    pub definitions: Vec<ApiDefinition>,
}

impl PortableProject {
    pub fn new(project: Project, definitions: Vec<ApiDefinition>) -> Self {
        Self {
            schema_version: 1,
            project,
            definitions,
        }
    }
}

pub fn export_project(project: &PortableProject) -> Result<String, InteropError> {
    Ok(serde_json::to_string_pretty(project)?)
}

pub fn import_project(source: &str) -> Result<PortableProject, InteropError> {
    let project: PortableProject = serde_json::from_str(source)?;
    if project.schema_version != 1 {
        return Err(InteropError::Unsupported(format!(
            "project schema version {}",
            project.schema_version
        )));
    }
    Ok(project)
}
