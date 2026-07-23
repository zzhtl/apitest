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
    #[error("script exceeded the {timeout_ms} ms execution limit")]
    Timeout { timeout_ms: u64 },
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

        let json = self.evaluate(source)?;
        serde_json::from_str(&json).map_err(|error| ScriptError::InvalidResult(error.to_string()))
    }

    pub(crate) fn run_mock(
        &self,
        script: &str,
        request: &serde_json::Value,
        response: &serde_json::Value,
    ) -> Result<serde_json::Value, ScriptError> {
        let request = serde_json::to_string(request)?;
        let response = serde_json::to_string(response)?;
        let source = mock_script_source(script, &request, &response);
        let json = self.evaluate(source)?;
        serde_json::from_str(&json).map_err(|error| ScriptError::InvalidResult(error.to_string()))
    }

    fn evaluate(&self, source: String) -> Result<String, ScriptError> {
        let runtime = Runtime::new()?;
        runtime.set_memory_limit(self.memory_limit);
        runtime.set_max_stack_size(512 * 1024);
        let started = std::time::Instant::now();
        let timeout = self.timeout;
        runtime.set_interrupt_handler(Some(Box::new(move || started.elapsed() >= timeout)));

        let context = Context::full(&runtime)?;
        match context.with(|ctx| ctx.eval::<String, _>(source)) {
            Ok(json) => Ok(json),
            Err(_) if started.elapsed() >= timeout => Err(ScriptError::Timeout {
                timeout_ms: timeout.as_millis().min(u128::from(u64::MAX)) as u64,
            }),
            Err(error) => Err(ScriptError::Runtime(error)),
        }
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
            const __headerEntries = Object.entries(__rawResponse.headers);
            const __headers = {{
                get(name) {{
                    const expected = String(name).toLowerCase();
                    const found = __headerEntries.find(([key]) => key.toLowerCase() === expected);
                    return found ? found[1] : undefined;
                }},
                has(name) {{ return this.get(name) !== undefined; }},
                all() {{ return Object.fromEntries(__headerEntries); }}
            }};
            const __display = value => {{
                try {{ return JSON.stringify(value); }} catch (_) {{ return String(value); }}
            }};
            const __equal = (left, right) => Object.is(left, right) ||
                __display(left) === __display(right);
            const __check = (passed, message) => {{
                if (!passed) throw new Error(message);
            }};
            const __expect = actual => {{
                const strictEqual = expected => __check(
                    Object.is(actual, expected),
                    `expected ${{__display(actual)}} to equal ${{__display(expected)}}`
                );
                const deepEqual = expected => __check(
                    __equal(actual, expected),
                    `expected ${{__display(actual)}} to deeply equal ${{__display(expected)}}`
                );
                const include = expected => __check(
                    actual != null && typeof actual.includes === "function" && actual.includes(expected),
                    `expected ${{__display(actual)}} to include ${{__display(expected)}}`
                );
                const property = function(name, expected) {{
                    const present = actual != null && Object.prototype.hasOwnProperty.call(actual, name);
                    __check(present, `expected ${{__display(actual)}} to have property ${{String(name)}}`);
                    if (arguments.length > 1) {{
                        __check(__equal(actual[name], expected),
                            `expected property ${{String(name)}} to equal ${{__display(expected)}}`);
                    }}
                    return __expect(actual[name]);
                }};
                const above = expected => __check(
                    actual > expected,
                    `expected ${{__display(actual)}} to be above ${{__display(expected)}}`
                );
                const below = expected => __check(
                    actual < expected,
                    `expected ${{__display(actual)}} to be below ${{__display(expected)}}`
                );
                const be = {{
                    a(type) {{
                        const actualType = Array.isArray(actual) ? "array" : typeof actual;
                        __check(actualType === String(type).toLowerCase(),
                            `expected ${{__display(actual)}} to be a ${{String(type)}}`);
                    }},
                    above,
                    below,
                    oneOf(values) {{
                        __check(Array.isArray(values) && values.some(value => __equal(actual, value)),
                            `expected ${{__display(actual)}} to be one of ${{__display(values)}}`);
                    }}
                }};
                Object.defineProperties(be, {{
                    ok: {{ get() {{ __check(Boolean(actual), `expected ${{__display(actual)}} to be truthy`); return true; }} }},
                    true: {{ get() {{ strictEqual(true); return true; }} }},
                    false: {{ get() {{ strictEqual(false); return true; }} }},
                    empty: {{ get() {{
                        const size = actual == null ? -1 :
                            (typeof actual.length === "number" ? actual.length : Object.keys(actual).length);
                        __check(size === 0, `expected ${{__display(actual)}} to be empty`);
                        return true;
                    }} }}
                }});
                return {{
                    toBe: strictEqual,
                    toEqual: deepEqual,
                    toBeTruthy() {{ __check(Boolean(actual), `expected ${{__display(actual)}} to be truthy`); }},
                    toContain: include,
                    to: {{
                        equal: strictEqual,
                        eql: deepEqual,
                        deep: {{ equal: deepEqual }},
                        include,
                        contain: include,
                        match(pattern) {{
                            __check(pattern instanceof RegExp && pattern.test(String(actual)),
                                `expected ${{__display(actual)}} to match ${{String(pattern)}}`);
                        }},
                        above,
                        below,
                        be,
                        have: {{ property }}
                    }}
                }};
            }};
            const __variables = {{
                get(name) {{ return __state.variables[String(name)]; }},
                has(name) {{ return Object.prototype.hasOwnProperty.call(__state.variables, String(name)); }},
                set(name, value) {{ __state.variables[String(name)] = String(value); }},
                unset(name) {{ delete __state.variables[String(name)]; }}
            }};
            const __test = (name, callback) => {{
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
            }};
            const apitest = {{
                getVariable: __variables.get,
                setVariable: __variables.set,
                unsetVariable: __variables.unset,
                expect: __expect,
                test: __test
            }};
            const pm = {{
                test: __test,
                expect: __expect,
                variables: __variables,
                environment: __variables,
                collectionVariables: __variables,
                globals: __variables,
                iterationData: __variables,
                response: {{
                    code: __rawResponse.status,
                    status: String(__rawResponse.status),
                    headers: __headers,
                    text() {{ return __rawResponse.body; }},
                    json() {{ return JSON.parse(__rawResponse.body); }},
                    to: {{
                        have: {{
                            status(expected) {{
                                __check(__rawResponse.status === Number(expected),
                                    `expected response status ${{__rawResponse.status}} to equal ${{expected}}`);
                            }}
                        }}
                    }}
                }},
                info: {{ iteration: 0, iterationCount: 1 }}
            }};
            globalThis.apitest = apitest;
            globalThis.pm = pm;
            globalThis.response = response;
            (() => {{
                {script}
            }})();
            return JSON.stringify(__state);
        }})()
        "#
    )
}

fn mock_script_source(script: &str, request: &str, response: &str) -> String {
    format!(
        r#"
        (() => {{
            const request = Object.freeze({request});
            const response = {response};
            const pm = {{ request, response }};
            globalThis.request = request;
            globalThis.response = response;
            globalThis.pm = pm;
            (() => {{
                {script}
            }})();
            return JSON.stringify(response);
        }})()
        "#
    )
}
