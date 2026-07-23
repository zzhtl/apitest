use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex},
};

use apitest_core::{
    ApiComponents, ApiContent, ApiResponse, ApiSchema, AssertionRule, EntityId, Environment,
    ExecutionEvent, ExecutionMetrics, ExecutionRequest, ExecutionStream, ExtractorRule, HttpMethod,
    HttpSpec, ProtocolExecutor, ProtocolKind, ProtocolSpec, ResponseHead, ScenarioNode,
    TestScenario, VariableSource,
};
use apitest_runtime::{
    ResponseSchemaContract, Scenario, ScenarioCase, ScenarioRunner, ScenarioStep,
};
use bytes::Bytes;
use futures::stream;
use indexmap::IndexMap;
use tokio_util::sync::CancellationToken;

struct FakeExecutor {
    seen_tokens: Arc<Mutex<Vec<Option<String>>>>,
}

impl ProtocolExecutor for FakeExecutor {
    fn kind(&self) -> ProtocolKind {
        ProtocolKind::Http
    }

    fn execute(
        &self,
        request: ExecutionRequest,
        _cancellation: CancellationToken,
    ) -> ExecutionStream {
        let token = request
            .local_variables
            .iter()
            .find(|variable| variable.name == "token")
            .map(|variable| variable.value.clone());
        self.seen_tokens.lock().expect("record lock").push(token);
        Box::pin(stream::iter(vec![
            Ok(ExecutionEvent::ResponseHead(ResponseHead {
                status: Some(201),
                version: Some("HTTP/1.1".into()),
                headers: IndexMap::new(),
            })),
            Ok(ExecutionEvent::Data(Bytes::from_static(br#"{"ok":true}"#))),
            Ok(ExecutionEvent::Completed(ExecutionMetrics {
                elapsed_ms: 4,
                received_bytes: 11,
                sent_bytes: 0,
            })),
        ]))
    }
}

#[tokio::test]
async fn runs_steps_sequentially_and_carries_script_variables() {
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let runner = ScenarioRunner::new().with_executor(Arc::new(FakeExecutor {
        seen_tokens: seen_tokens.clone(),
    }));
    let request = || {
        ExecutionRequest::new(
            ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, "https://example.com")),
            Environment::new("test"),
        )
    };
    let scenario = Scenario {
        name: "smoke".into(),
        stop_on_failure: true,
        steps: vec![
            ScenarioStep {
                name: "login".into(),
                request: request(),
                post_script: r#"
                    pm.environment.set("token", "secret");
                    pm.test("created", () => pm.response.to.have.status(201));
                "#
                .into(),
            },
            ScenarioStep {
                name: "profile".into(),
                request: request(),
                post_script: String::new(),
            },
        ],
    };

    let report = runner
        .run(scenario, CancellationToken::new())
        .await
        .expect("scenario should run");

    assert!(report.passed);
    assert_eq!(report.steps.len(), 2);
    assert_eq!(
        *seen_tokens.lock().expect("record lock"),
        vec![None, Some("secret".into())]
    );
}

struct StructuredExecutor {
    seen_items: Arc<Mutex<Vec<Option<String>>>>,
}

impl ProtocolExecutor for StructuredExecutor {
    fn kind(&self) -> ProtocolKind {
        ProtocolKind::Http
    }

    fn execute(
        &self,
        request: ExecutionRequest,
        _cancellation: CancellationToken,
    ) -> ExecutionStream {
        let ProtocolSpec::Http(spec) = request.protocol else {
            panic!("structured test only uses HTTP");
        };
        let item = request
            .local_variables
            .iter()
            .find(|variable| variable.name == "item")
            .map(|variable| variable.value.clone());
        self.seen_items.lock().expect("record lock").push(item);
        let (status, headers, body, elapsed_ms) = if spec.url.ends_with("/login") {
            (
                201,
                IndexMap::from([("x-token".into(), vec!["abc".into()])]),
                Bytes::from_static(br#"{"items":[1,2,3]}"#),
                4,
            )
        } else {
            (
                200,
                IndexMap::new(),
                Bytes::from_static(br#"{"ok":true}"#),
                2,
            )
        };
        Box::pin(stream::iter(vec![
            Ok(ExecutionEvent::ResponseHead(ResponseHead {
                status: Some(status),
                version: Some("HTTP/1.1".into()),
                headers,
            })),
            Ok(ExecutionEvent::Data(body.clone())),
            Ok(ExecutionEvent::Completed(ExecutionMetrics {
                elapsed_ms,
                received_bytes: body.len() as u64,
                sent_bytes: 0,
            })),
        ]))
    }
}

fn scenario_case(name: &str, url: &str) -> ScenarioCase {
    ScenarioCase {
        name: name.into(),
        request: ExecutionRequest::new(
            ProtocolSpec::Http(HttpSpec::new(HttpMethod::Get, url)),
            Environment::new("test"),
        ),
        pre_script: String::new(),
        post_script: String::new(),
        assertions: Vec::new(),
        extractors: Vec::new(),
        response_schema: None,
    }
}

#[tokio::test]
async fn schema_assertions_validate_the_contract_response_schema() {
    let seen_items = Arc::new(Mutex::new(Vec::new()));
    let runner = ScenarioRunner::new().with_executor(Arc::new(StructuredExecutor { seen_items }));
    let case_id = EntityId::new();
    let scenario = TestScenario {
        name: "schema".into(),
        nodes: vec![ScenarioNode::Request {
            case_id,
            assertions: vec![AssertionRule::Schema],
            extractors: Vec::new(),
        }],
        ..TestScenario::default()
    };
    let mut case = scenario_case("invalid response", "https://example.test/item");
    case.response_schema = Some(ResponseSchemaContract {
        responses: vec![ApiResponse {
            status: "200".into(),
            description: "User".into(),
            headers: Vec::new(),
            content: vec![ApiContent::json(ApiSchema::component("User"))],
        }],
        components: ApiComponents {
            schemas: indexmap::IndexMap::from([(
                "User".into(),
                ApiSchema::Inline(serde_json::json!({
                    "type": "object",
                    "required": ["id"],
                    "properties": { "id": { "type": "integer" } }
                })),
            )]),
            ..ApiComponents::default()
        },
    });

    let report = runner
        .run_test_scenario(
            scenario,
            HashMap::from([(case_id, case)]),
            BTreeMap::new(),
            CancellationToken::new(),
        )
        .await
        .expect("schema failures belong in the report");

    assert!(!report.passed);
    assert!(!report.steps[0].assertions[0].passed);
    assert!(
        report.steps[0].assertions[0]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("id"))
    );
}

#[tokio::test]
async fn runs_nested_conditions_loops_assertions_and_extractors() {
    let seen_items = Arc::new(Mutex::new(Vec::new()));
    let runner = ScenarioRunner::new().with_executor(Arc::new(StructuredExecutor {
        seen_items: seen_items.clone(),
    }));
    let login_id = EntityId::new();
    let item_id = EntityId::new();
    let scenario = TestScenario {
        name: "structured".into(),
        stop_on_failure: true,
        nodes: vec![
            ScenarioNode::Group {
                name: "login".into(),
                nodes: vec![ScenarioNode::Request {
                    case_id: login_id,
                    assertions: vec![
                        AssertionRule::Status { expected: 201 },
                        AssertionRule::JsonPathEquals {
                            path: "$.items[0]".into(),
                            expected: "1".into(),
                        },
                        AssertionRule::DurationBelow { milliseconds: 5 },
                    ],
                    extractors: vec![
                        ExtractorRule {
                            name: "token".into(),
                            source: VariableSource::Header("x-token".into()),
                        },
                        ExtractorRule {
                            name: "items".into(),
                            source: VariableSource::JsonPath("$.items".into()),
                        },
                    ],
                }],
            },
            ScenarioNode::If {
                expression: r#"pm.variables.get("token") === "abc""#.into(),
                then_nodes: vec![ScenarioNode::Loop {
                    source: r#"JSON.parse(pm.variables.get("items"))"#.into(),
                    item_name: "item".into(),
                    max_iterations: 2,
                    nodes: vec![ScenarioNode::Request {
                        case_id: item_id,
                        assertions: vec![AssertionRule::Status { expected: 200 }],
                        extractors: Vec::new(),
                    }],
                }],
                else_nodes: vec![ScenarioNode::Delay { millis: 1 }],
            },
        ],
        ..TestScenario::default()
    };
    let cases = HashMap::from([
        (
            login_id,
            scenario_case("login", "https://example.test/login"),
        ),
        (item_id, scenario_case("item", "https://example.test/item")),
    ]);

    let report = runner
        .run_test_scenario(scenario, cases, BTreeMap::new(), CancellationToken::new())
        .await
        .expect("structured scenario should run");

    assert!(report.passed);
    assert_eq!(report.steps.len(), 3);
    assert!(report.steps.iter().all(|step| step.passed));
    assert_eq!(
        *seen_items.lock().expect("record lock"),
        vec![None, Some("1".into()), Some("2".into())]
    );
}

#[tokio::test]
async fn stops_structured_scenario_after_failed_assertion() {
    let seen_items = Arc::new(Mutex::new(Vec::new()));
    let runner = ScenarioRunner::new().with_executor(Arc::new(StructuredExecutor {
        seen_items: seen_items.clone(),
    }));
    let first_id = EntityId::new();
    let second_id = EntityId::new();
    let scenario = TestScenario {
        name: "stop on failure".into(),
        stop_on_failure: true,
        nodes: vec![
            ScenarioNode::Request {
                case_id: first_id,
                assertions: vec![AssertionRule::Status { expected: 418 }],
                extractors: Vec::new(),
            },
            ScenarioNode::Request {
                case_id: second_id,
                assertions: Vec::new(),
                extractors: Vec::new(),
            },
        ],
        ..TestScenario::default()
    };
    let cases = HashMap::from([
        (
            first_id,
            scenario_case("first", "https://example.test/item"),
        ),
        (
            second_id,
            scenario_case("second", "https://example.test/item"),
        ),
    ]);

    let report = runner
        .run_test_scenario(scenario, cases, BTreeMap::new(), CancellationToken::new())
        .await
        .expect("assertion failures belong in the report");

    assert!(!report.passed);
    assert_eq!(report.steps.len(), 1);
    assert_eq!(seen_items.lock().expect("record lock").len(), 1);
    assert_eq!(report.steps[0].assertions[0].name, "Status = 418");
    assert!(!report.steps[0].assertions[0].passed);
}
