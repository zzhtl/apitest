use apitest_core::{
    AssertionRule, EntityId, ExtractorRule, ScenarioNode, TestScenario, VariableSource,
};

#[test]
fn scenario_flow_preserves_control_nodes_and_rules() {
    let case_id = EntityId::new();
    let scenario = TestScenario {
        name: "Create and read user".into(),
        nodes: vec![
            ScenarioNode::Request {
                case_id,
                assertions: vec![AssertionRule::Status { expected: 201 }],
                extractors: vec![ExtractorRule {
                    name: "user_id".into(),
                    source: VariableSource::JsonPath("$.id".into()),
                }],
            },
            ScenarioNode::If {
                expression: "user_id != null".into(),
                then_nodes: vec![ScenarioNode::Delay { millis: 10 }],
                else_nodes: Vec::new(),
            },
        ],
        ..TestScenario::default()
    };

    let json = serde_json::to_string(&scenario).expect("scenario should serialize");
    let restored: TestScenario = serde_json::from_str(&json).expect("scenario should deserialize");

    assert_eq!(restored, scenario);
}

#[test]
fn loop_iteration_limit_is_part_of_the_persisted_model() {
    let node = ScenarioNode::Loop {
        source: "rows".into(),
        item_name: "row".into(),
        max_iterations: 100,
        nodes: Vec::new(),
    };

    let encoded = serde_json::to_value(&node).expect("loop should serialize");

    assert_eq!(encoded["max_iterations"], 100);
}
