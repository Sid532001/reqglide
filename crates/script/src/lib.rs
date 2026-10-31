//! Resource-limited JavaScript execution for request lifecycle scripts.
//! Each invocation gets a fresh QuickJS runtime with no filesystem, process,
//! network, native module, or desktop bridge bindings.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use api_client_core::{RequestDefinition, ResponseSnapshot};
use rquickjs::{Context, Runtime};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_SCRIPT_TIMEOUT_MS: u64 = 2_000;
pub const DEFAULT_SCRIPT_MEMORY_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_SCRIPT_STACK_BYTES: usize = 512 * 1024;
pub const MAX_SCRIPT_BYTES: usize = 256 * 1024;
pub const MAX_SCRIPT_CONTEXT_BYTES: usize = 12 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScriptPhase {
    PreRequest,
    PostResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptExecutionInput {
    pub phase: ScriptPhase,
    pub script: String,
    pub request: RequestDefinition,
    pub response: Option<ResponseSnapshot>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptExecutionResult {
    pub phase: ScriptPhase,
    pub request: RequestDefinition,
    pub environment: BTreeMap<String, String>,
    pub variables: BTreeMap<String, String>,
    pub logs: Vec<ScriptLog>,
    pub tests: Vec<ScriptTestResult>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptLog {
    pub level: ScriptLogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScriptLogLevel {
    Log,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptTestResult {
    pub name: String,
    pub passed: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ScriptLimits {
    pub timeout: Duration,
    pub memory_bytes: usize,
    pub stack_bytes: usize,
}

impl Default for ScriptLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_millis(DEFAULT_SCRIPT_TIMEOUT_MS),
            memory_bytes: DEFAULT_SCRIPT_MEMORY_BYTES,
            stack_bytes: DEFAULT_SCRIPT_STACK_BYTES,
        }
    }
}

pub fn execute_script(
    input: ScriptExecutionInput,
    limits: ScriptLimits,
) -> Result<ScriptExecutionResult, ScriptExecutionError> {
    if input.script.len() > MAX_SCRIPT_BYTES {
        return Err(ScriptExecutionError::SourceTooLarge {
            actual: input.script.len(),
            limit: MAX_SCRIPT_BYTES,
        });
    }

    let started_at = Instant::now();
    let deadline = started_at + limits.timeout;
    let input_json = serde_json::to_string(&input)
        .map_err(|source| ScriptExecutionError::SerializeInput { source })?;
    if input_json.len() > MAX_SCRIPT_CONTEXT_BYTES {
        return Err(ScriptExecutionError::ContextTooLarge {
            actual: input_json.len(),
            limit: MAX_SCRIPT_CONTEXT_BYTES,
        });
    }
    let source = SCRIPT_HARNESS.replace("__SCRIPT_INPUT_JSON__", &input_json);

    let runtime = Runtime::new().map_err(|_| ScriptExecutionError::InitializeRuntime)?;
    runtime.set_memory_limit(limits.memory_bytes);
    runtime.set_max_stack_size(limits.stack_bytes);
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let context = Context::full(&runtime).map_err(|_| ScriptExecutionError::InitializeContext)?;

    let output = context.with(|context| context.eval::<String, _>(source));
    let output = match output {
        Ok(output) => output,
        Err(_) if Instant::now() >= deadline => return Err(ScriptExecutionError::Timeout),
        Err(_) => return Err(ScriptExecutionError::RuntimeFailure),
    };
    let mut result: ScriptExecutionResult = serde_json::from_str(&output)
        .map_err(|source| ScriptExecutionError::DeserializeOutput { source })?;
    result.duration_ms = started_at.elapsed().as_millis() as u64;
    Ok(result)
}

#[derive(Debug, Error)]
pub enum ScriptExecutionError {
    #[error("script source is {actual} bytes, exceeding the {limit}-byte limit")]
    SourceTooLarge { actual: usize, limit: usize },
    #[error("script context is {actual} bytes, exceeding the {limit}-byte limit")]
    ContextTooLarge { actual: usize, limit: usize },
    #[error("script input could not be prepared")]
    SerializeInput {
        #[source]
        source: serde_json::Error,
    },
    #[error("the isolated script runtime could not be initialized")]
    InitializeRuntime,
    #[error("the isolated script context could not be initialized")]
    InitializeContext,
    #[error("script execution exceeded the time limit")]
    Timeout,
    #[error("script execution was stopped by the isolated runtime")]
    RuntimeFailure,
    #[error("script output was invalid")]
    DeserializeOutput {
        #[source]
        source: serde_json::Error,
    },
}

const SCRIPT_HARNESS: &str = r#"
(() => {
  "use strict";
  const input = __SCRIPT_INPUT_JSON__;
  const logs = [];
  const tests = [];
  const environment = { ...input.environment };
  const variables = { ...input.variables };
  const request = input.request;
  const headers = Array.isArray(request.headers) ? request.headers : [];

  const text = (value) => {
    if (typeof value === "string") return value;
    try {
      const encoded = JSON.stringify(value);
      return encoded === undefined ? String(value) : encoded;
    } catch (_) {
      return String(value);
    }
  };
  const message = (error) => text(error && error.message !== undefined ? error.message : error).slice(0, 4096);
  const log = (level, values) => {
    if (logs.length >= 200) return;
    logs.push({ level, message: values.map(text).join(" ").slice(0, 4096) });
  };
  const safeConsole = Object.freeze({
    log: (...values) => log("log", values),
    info: (...values) => log("info", values),
    warn: (...values) => log("warn", values),
    error: (...values) => log("error", values),
  });

  const replaceIn = (source, getter) => String(source).replace(/{{\s*([^{}]+?)\s*}}/g, (token, key) => {
    const value = getter(String(key).trim());
    return value === undefined ? token : value;
  });
  const store = (values) => Object.freeze({
    get: (key) => Object.prototype.hasOwnProperty.call(values, String(key)) ? values[String(key)] : undefined,
    has: (key) => Object.prototype.hasOwnProperty.call(values, String(key)),
    set: (key, value) => { values[String(key)] = String(value); },
    unset: (key) => { delete values[String(key)]; },
    clear: () => { for (const key of Object.keys(values)) delete values[key]; },
    replaceIn: (source) => replaceIn(source, (key) => values[key]),
    toObject: () => ({ ...values }),
  });
  const environmentApi = store(environment);
  const variablesApi = Object.freeze({
    get: (key) => Object.prototype.hasOwnProperty.call(variables, String(key))
      ? variables[String(key)]
      : environment[String(key)],
    has: (key) => Object.prototype.hasOwnProperty.call(variables, String(key))
      || Object.prototype.hasOwnProperty.call(environment, String(key)),
    set: (key, value) => { variables[String(key)] = String(value); },
    unset: (key) => { delete variables[String(key)]; },
    clear: () => { for (const key of Object.keys(variables)) delete variables[key]; },
    replaceIn: (source) => replaceIn(source, (key) => Object.prototype.hasOwnProperty.call(variables, key)
      ? variables[key]
      : environment[key]),
    toObject: () => ({ ...environment, ...variables }),
  });

  const headerName = (header) => String(header && (header.name ?? header.key) || "");
  const headerApi = Object.freeze({
    get: (name) => {
      const found = headers.find((header) => header.enabled !== false && headerName(header).toLowerCase() === String(name).toLowerCase());
      return found ? String(found.value ?? "") : undefined;
    },
    has: (name) => headers.some((header) => header.enabled !== false && headerName(header).toLowerCase() === String(name).toLowerCase()),
    add: (header) => headers.push({ name: headerName(header), value: String(header && header.value || ""), enabled: true }),
    upsert: (header) => {
      const name = headerName(header);
      const found = headers.find((candidate) => headerName(candidate).toLowerCase() === name.toLowerCase());
      if (found) {
        found.name = name;
        found.value = String(header && header.value || "");
        found.enabled = true;
      } else {
        headers.push({ name, value: String(header && header.value || ""), enabled: true });
      }
    },
    remove: (name) => {
      for (let index = headers.length - 1; index >= 0; index -= 1) {
        if (headerName(headers[index]).toLowerCase() === String(name).toLowerCase()) headers.splice(index, 1);
      }
    },
    all: () => headers.map((header) => ({ key: headerName(header), value: String(header.value ?? "") })),
  });
  const requestApi = { headers: headerApi };
  for (const property of ["method", "url", "body", "auth"]) {
    Object.defineProperty(requestApi, property, {
      enumerable: true,
      get: () => request[property],
      set: (value) => { request[property] = value; },
    });
  }

  const same = (left, right) => {
    if (Object.is(left, right)) return true;
    try { return JSON.stringify(left) === JSON.stringify(right); } catch (_) { return false; }
  };
  const assertion = (actual, negate = false) => {
    const chain = {};
    const check = (condition, description) => {
      if (negate ? condition : !condition) throw new Error(`expected ${text(actual)} ${negate ? "not " : ""}${description}`);
      return chain;
    };
    for (const key of ["to", "be", "been", "is", "that", "which", "and", "has", "have", "with", "at", "of", "same"]) {
      Object.defineProperty(chain, key, { get: () => chain });
    }
    Object.defineProperty(chain, "not", { get: () => assertion(actual, !negate) });
    Object.defineProperty(chain, "ok", { get: () => check(Boolean(actual), "to be truthy") });
    Object.defineProperty(chain, "true", { get: () => check(actual === true, "to be true") });
    Object.defineProperty(chain, "false", { get: () => check(actual === false, "to be false") });
    Object.defineProperty(chain, "null", { get: () => check(actual === null, "to be null") });
    Object.defineProperty(chain, "undefined", { get: () => check(actual === undefined, "to be undefined") });
    chain.equal = (expected) => check(Object.is(actual, expected), `to equal ${text(expected)}`);
    chain.equals = chain.equal;
    chain.eq = chain.equal;
    chain.eql = (expected) => check(same(actual, expected), `to deeply equal ${text(expected)}`);
    chain.include = (expected) => check(
      typeof actual === "string" || Array.isArray(actual)
        ? actual.includes(expected)
        : actual && typeof actual === "object" && Object.keys(expected || {}).every((key) => same(actual[key], expected[key])),
      `to include ${text(expected)}`,
    );
    chain.match = (expected) => check(expected instanceof RegExp && expected.test(String(actual)), `to match ${text(expected)}`);
    chain.above = (expected) => check(actual > expected, `to be above ${text(expected)}`);
    chain.below = (expected) => check(actual < expected, `to be below ${text(expected)}`);
    chain.property = (...propertyArguments) => {
      const [name, expected] = propertyArguments;
      const present = actual !== null && actual !== undefined && Object.prototype.hasOwnProperty.call(Object(actual), name);
      check(present, `to have property ${String(name)}`);
      if (propertyArguments.length > 1) check(same(actual[name], expected), `property ${String(name)} to equal ${text(expected)}`);
      return assertion(actual[name], negate);
    };
    return chain;
  };

  const response = input.response;
  const responseApi = response ? Object.freeze({
    code: response.status,
    status: response.statusText,
    responseTime: response.timing.totalMs,
    headers: Object.freeze({
      get: (name) => {
        const found = response.headers.find((header) => header.name.toLowerCase() === String(name).toLowerCase());
        return found ? found.value : undefined;
      },
      has: (name) => response.headers.some((header) => header.name.toLowerCase() === String(name).toLowerCase()),
      all: () => response.headers.map((header) => ({ key: header.name, value: header.value })),
    }),
    text: () => response.body.content,
    json: () => JSON.parse(response.body.content),
    to: Object.freeze({
      have: Object.freeze({
        status: (expected) => {
          if (response.status !== expected) throw new Error(`expected response status ${expected}, received ${response.status}`);
        },
      }),
    }),
  }) : undefined;

  const pendingTests = [];
  const pm = Object.freeze({
    info: Object.freeze({ eventName: input.phase, iteration: 0, iterationCount: 1 }),
    environment: environmentApi,
    variables: variablesApi,
    collectionVariables: variablesApi,
    globals: store({}),
    request: requestApi,
    response: responseApi,
    expect: assertion,
    test: (name, callback) => {
      try {
        const returned = callback();
        if (returned && typeof returned.then === "function") {
          pendingTests.push({ name: String(name), promise: returned });
        } else {
          tests.push({ name: String(name).slice(0, 512), passed: true, error: null });
        }
      } catch (error) {
        tests.push({ name: String(name).slice(0, 512), passed: false, error: message(error) });
      }
    },
    sendRequest: () => { throw new Error("pm.sendRequest is not enabled in this runtime yet"); },
  });

  let error = null;
  try {
    const run = Function("pm", "console", `"use strict";\n${input.script}\n`);
    const returned = run(pm, safeConsole);
    if (returned && typeof returned.then === "function") {
      throw new Error("top-level asynchronous scripts are not enabled yet");
    }
    if (pendingTests.length) {
      throw new Error("asynchronous pm.test callbacks are not enabled yet");
    }
  } catch (cause) {
    error = message(cause);
  }

  request.headers = headers;
  return JSON.stringify({
    phase: input.phase,
    request,
    environment,
    variables,
    logs,
    tests,
    error,
    durationMs: 0,
  });
})()
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use api_client_core::{
        RequestAuth, RequestBody, RequestOptions, ResponseBody, ResponseBodyEncoding,
        ResponseTiming,
    };

    fn input(script: &str) -> ScriptExecutionInput {
        ScriptExecutionInput {
            phase: ScriptPhase::PreRequest,
            script: script.to_owned(),
            request: RequestDefinition {
                method: "GET".to_owned(),
                url: "https://example.com/{{path}}".to_owned(),
                query_params: Vec::new(),
                headers: Vec::new(),
                auth: RequestAuth::None,
                body: RequestBody::Empty,
                options: RequestOptions::default(),
            },
            response: None,
            environment: BTreeMap::from([("path".to_owned(), "before".to_owned())]),
            variables: BTreeMap::new(),
        }
    }

    #[test]
    fn mutates_request_variables_and_captures_logs() {
        let result = execute_script(
            input(
                r#"
                pm.environment.set("path", "after");
                pm.variables.set("token", "local");
                pm.request.method = "POST";
                pm.request.headers.upsert({ key: "X-Test", value: "yes" });
                console.info("prepared", { ok: true });
                pm.test("sandbox has no fetch", () => pm.expect(typeof fetch).to.equal("undefined"));
                "#,
            ),
            ScriptLimits::default(),
        )
        .unwrap();

        assert_eq!(result.request.method, "POST");
        assert_eq!(result.request.headers[0].name, "X-Test");
        assert_eq!(result.environment["path"], "after");
        assert_eq!(result.variables["token"], "local");
        assert_eq!(result.logs[0].message, "prepared {\"ok\":true}");
        assert!(result.tests[0].passed);
        assert_eq!(result.error, None);
    }

    #[test]
    fn captures_assertion_failures_without_failing_the_script() {
        let result = execute_script(
            input(r#"pm.test("math", () => pm.expect(2 + 2).to.equal(5));"#),
            ScriptLimits::default(),
        )
        .unwrap();

        assert!(!result.tests[0].passed);
        assert!(
            result.tests[0]
                .error
                .as_deref()
                .unwrap()
                .contains("to equal 5")
        );
        assert_eq!(result.error, None);
    }

    #[test]
    fn exposes_post_response_status_headers_and_json() {
        let mut script_input = input(
            r#"
            pm.test("status", () => pm.response.to.have.status(201));
            pm.test("json", () => pm.expect(pm.response.json()).to.have.property("id", 42));
            pm.test("header", () => pm.expect(pm.response.headers.get("content-type")).to.include("json"));
            "#,
        );
        script_input.phase = ScriptPhase::PostResponse;
        script_input.response = Some(ResponseSnapshot {
            status: 201,
            status_text: "Created".to_owned(),
            headers: vec![api_client_core::HeaderEntry {
                name: "Content-Type".to_owned(),
                value: "application/json".to_owned(),
                enabled: true,
            }],
            body: ResponseBody {
                content: r#"{"id":42}"#.to_owned(),
                encoding: ResponseBodyEncoding::Utf8,
                content_type: Some("application/json".to_owned()),
                truncated: false,
            },
            timing: ResponseTiming { total_ms: 8 },
            received_bytes: 9,
        });

        let result = execute_script(script_input, ScriptLimits::default()).unwrap();
        assert_eq!(result.tests.len(), 3);
        assert!(result.tests.iter().all(|test| test.passed));
        assert_eq!(result.error, None);
    }

    #[test]
    fn interrupts_runaway_scripts() {
        let error = execute_script(
            input("while (true) {}"),
            ScriptLimits {
                timeout: Duration::from_millis(25),
                ..ScriptLimits::default()
            },
        )
        .unwrap_err();

        assert!(matches!(error, ScriptExecutionError::Timeout));
    }
}
