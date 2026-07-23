use serde::{Deserialize, Serialize};

use crate::EntityId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssertionRule {
    Status { expected: u16 },
    HeaderEquals { name: String, expected: String },
    JsonPathEquals { path: String, expected: String },
    Schema,
    DurationBelow { milliseconds: u64 },
    Script { expression: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum VariableSource {
    JsonPath(String),
    Header(String),
    Cookie(String),
    Regex(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractorRule {
    pub name: String,
    pub source: VariableSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScenarioNode {
    Request {
        case_id: EntityId,
        assertions: Vec<AssertionRule>,
        extractors: Vec<ExtractorRule>,
    },
    Group {
        name: String,
        nodes: Vec<ScenarioNode>,
    },
    If {
        expression: String,
        then_nodes: Vec<ScenarioNode>,
        else_nodes: Vec<ScenarioNode>,
    },
    Loop {
        source: String,
        item_name: String,
        max_iterations: u32,
        nodes: Vec<ScenarioNode>,
    },
    Delay {
        millis: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestScenario {
    pub id: EntityId,
    pub name: String,
    pub stop_on_failure: bool,
    pub dataset_path: Option<String>,
    pub nodes: Vec<ScenarioNode>,
}

impl Default for TestScenario {
    fn default() -> Self {
        Self {
            id: EntityId::new(),
            name: String::new(),
            stop_on_failure: true,
            dataset_path: None,
            nodes: Vec::new(),
        }
    }
}
