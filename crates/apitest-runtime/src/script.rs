use std::{collections::BTreeMap, time::Duration};

use rquickjs::{Context, Runtime};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize)]
pub struct ScriptResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScriptAssertion {
    pub name: String,
    pub passed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ScriptResult {
    pub variables: BTreeMap<String, String>,
    pub assertions: Vec<ScriptAssertion>,
}

#[derive(Debug, Error)]
pub enum ScriptError {
    #[error("failed to initialize JavaScript runtime: {0}")]
    Runtime(#[from] rquickjs::Error),
    #[error("failed to serialize script input: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("script returned invalid data: {0}")]
    InvalidResult(String),
}

#[derive(Debug, Clone)]
pub struct ScriptEngine {
    timeout: Duration,
    memory_limit: usize,
}

impl ScriptEngine {
    pub fn new(timeout: Duration, memory_limit: usize) -> Self {
        Self {
            timeout,
            memory_limit,
        }
    }

    pub fn run(
        &self,
        script: &str,
        variables: &BTreeMap<String, String>,
        response: Option<&ScriptResponse>,
    ) -> Result<ScriptResult, ScriptError> {
        let variables = serde_json::to_string(variables)?;
        let response = serde_json::to_string(&response.cloned().unwrap_or(ScriptResponse {
            status: 0,
            headers: BTreeMap::new(),
            body: String::new(),
        }))?;
        let source = script_source(script, &variables, &response);

        let runtime = Runtime::new()?;
        runtime.set_memory_limit(self.memory_limit);
        runtime.set_max_stack_size(512 * 1024);
        let started = std::time::Instant::now();
        let timeout = self.timeout;
        runtime.set_interrupt_handler(Some(Box::new(move || started.elapsed() >= timeout)));

        let context = Context::full(&runtime)?;
        let json = context.with(|ctx| ctx.eval::<String, _>(source))?;
        serde_json::from_str(&json).map_err(|error| ScriptError::InvalidResult(error.to_string()))
    }
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new(Duration::from_secs(2), 16 * 1024 * 1024)
    }
}

fn script_source(script: &str, variables: &str, response: &str) -> String {
    format!(
        r#"
        (() => {{
            const __state = {{ variables: {variables}, assertions: [] }};
            const __rawResponse = {response};
            const response = {{
                ...__rawResponse,
                json() {{ return JSON.parse(this.body); }}
            }};
            const __display = value => {{
                try {{ return JSON.stringify(value); }} catch (_) {{ return String(value); }}
            }};
            const __equal = (left, right) => Object.is(left, right) ||
                __display(left) === __display(right);
            const __check = (passed, message) => {{
                if (!passed) throw new Error(message);
            }};
            const apitest = {{
                getVariable(name) {{ return __state.variables[String(name)]; }},
                setVariable(name, value) {{ __state.variables[String(name)] = String(value); }},
                unsetVariable(name) {{ delete __state.variables[String(name)]; }},
                expect(actual) {{
                    return {{
                        toBe(expected) {{
                            __check(Object.is(actual, expected),
                                `expected ${{__display(actual)}} to be ${{__display(expected)}}`);
                        }},
                        toEqual(expected) {{
                            __check(__equal(actual, expected),
                                `expected ${{__display(actual)}} to equal ${{__display(expected)}}`);
                        }},
                        toBeTruthy() {{ __check(Boolean(actual), `expected ${{__display(actual)}} to be truthy`); }},
                        toContain(expected) {{
                            __check(actual != null && typeof actual.includes === "function" && actual.includes(expected),
                                `expected ${{__display(actual)}} to contain ${{__display(expected)}}`);
                        }}
                    }};
                }},
                test(name, callback) {{
                    try {{
                        callback();
                        __state.assertions.push({{ name: String(name), passed: true, error: null }});
                    }} catch (error) {{
                        __state.assertions.push({{
                            name: String(name),
                            passed: false,
                            error: String(error && error.message ? error.message : error)
                        }});
                    }}
                }}
            }};
            globalThis.apitest = apitest;
            globalThis.response = response;
            (() => {{
                {script}
            }})();
            return JSON.stringify(__state);
        }})()
        "#
    )
}
