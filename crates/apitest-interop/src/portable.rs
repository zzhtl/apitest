use apitest_core::{ApiComponents, ApiContract, ApiDefinition, Project, ProtocolSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
            schema_version: 2,
            project,
            definitions,
        }
    }
}

pub fn export_project(project: &PortableProject) -> Result<String, InteropError> {
    Ok(serde_json::to_string_pretty(project)?)
}

pub fn import_project(source: &str) -> Result<PortableProject, InteropError> {
    let mut document: Value = serde_json::from_str(source)?;
    let schema_version = document
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if schema_version == 1 {
        migrate_v1(&mut document)?;
    } else if schema_version != 2 {
        return Err(InteropError::Unsupported(format!(
            "project schema version {}",
            schema_version
        )));
    }
    Ok(serde_json::from_value(document)?)
}

fn migrate_v1(document: &mut Value) -> Result<(), InteropError> {
    document["schema_version"] = Value::from(2);
    let project = document
        .get_mut("project")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| InteropError::Unsupported("project document is missing".into()))?;
    project.insert(
        "components".into(),
        serde_json::to_value(ApiComponents::default())?,
    );
    let definitions = document
        .get_mut("definitions")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| InteropError::Unsupported("project definitions are missing".into()))?;
    for definition in definitions {
        let definition = definition.as_object_mut().ok_or_else(|| {
            InteropError::Unsupported("project definition must be an object".into())
        })?;
        let protocol = definition.remove("protocol").ok_or_else(|| {
            InteropError::Unsupported("v1 project definition protocol is missing".into())
        })?;
        let protocol: ProtocolSpec = serde_json::from_value(protocol)?;
        definition.insert(
            "contract".into(),
            serde_json::to_value(ApiContract::from(protocol))?,
        );
    }
    Ok(())
}
