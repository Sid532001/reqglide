use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::Write,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use api_client_core::{
    HeaderEntry, KeyValueEntry, OAuth2ClientAuth, RequestAuth, RequestBody, RequestDefinition,
    RequestOptions, ResponseBody, ResponseBodyEncoding, ResponseSnapshot,
    resolve_request_variables, resolve_text_variables,
};
use api_client_history::{HistoryEntry, HistoryQuery, HistoryRecordInput, HistoryStore};
use api_client_http::{
    CookieJar, CookieSnapshot, HttpExecutionContext, HttpExecutor, ProxyConfiguration,
};
use api_client_import::{ApiDocumentImport, CodeLanguage, CurlImport, CurlShell};
use api_client_protocol::{
    GrpcInvokeInput, GrpcInvokeResult, GrpcReflectionInput, GrpcReflectionResult, ProtocolEvent,
    ProtocolSession, ServerSentEventsInput, WebSocketInput, WebSocketOutbound, invoke_grpc_unary,
    reflect_grpc_service, start_server_sent_events, start_websocket,
};
use api_client_script::{
    ScriptExecutionInput, ScriptExecutionResult, ScriptLimits, ScriptLog, ScriptPhase,
    ScriptTestResult,
};
use api_client_vault::{
    NativeSecretStore, SecretChange, SecretStore, apply_changes, transact_changes,
};
use api_client_workspace::{
    Environment, EnvironmentVariable, NetworkSettings, ProtocolConfiguration, ProtocolProfile,
    ProxyMode, RequestScripts, WorkspaceDocument, WorkspaceSession, WorkspaceStore,
};
use atomic_write_file::AtomicWriteFile;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

const MAX_RECOVERY_BYTES: usize = 8 * 1024 * 1024;
const MAX_RECOVERY_TABS: usize = 50;
const MAX_TLS_MATERIAL_BYTES: u64 = 5 * 1024 * 1024;
const MAX_OAUTH_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_OAUTH_TOKEN_BYTES: usize = 64 * 1024;
const MAX_CACHED_OAUTH_TOKENS: usize = 256;
const MAX_RUN_DATA_BYTES: usize = 2 * 1024 * 1024;
const MAX_RUN_REPORT_BYTES: usize = 16 * 1024 * 1024;
const MAX_RUN_EXECUTIONS: usize = 10_000;
const MAX_API_IMPORT_BYTES: u64 = 16 * 1024 * 1024;
mod last_workspace;
mod maintenance;
const MAX_PROTOCOL_SESSIONS: usize = 8;
const MAX_PROTOCOL_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
struct ActiveWorkspace {
    root_path: PathBuf,
    workspace: WorkspaceDocument,
}

#[derive(Default)]
struct WorkspaceState {
    active: Arc<Mutex<Option<ActiveWorkspace>>>,
    operation_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Default)]
struct RequestExecutionState {
    current: Mutex<Option<(u64, tokio::task::AbortHandle)>>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct SecretVaultState {
    store: NativeSecretStore,
}

#[derive(Default)]
struct RecoveryState {
    operation_lock: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct HistoryState {
    operation_lock: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct RunReportState {
    operation_lock: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct CookieState {
    jars: Mutex<HashMap<String, Arc<CookieJar>>>,
}

#[derive(Default)]
struct OAuthTokenState {
    entries: Arc<Mutex<HashMap<OAuthCacheKey, CachedOAuthToken>>>,
}

#[derive(Default)]
struct ProtocolSessionState {
    sessions: Mutex<HashMap<String, Arc<ProtocolSession>>>,
}

impl ProtocolSessionState {
    fn insert(&self, session: ProtocolSession) -> Result<String, String> {
        let id = session.id().to_owned();
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "protocol session state is unavailable".to_owned())?;
        sessions.retain(|_, session| !session.is_finished());
        if sessions.len() >= MAX_PROTOCOL_SESSIONS {
            return Err(format!(
                "no more than {MAX_PROTOCOL_SESSIONS} realtime protocol sessions can run at once"
            ));
        }
        sessions.insert(id.clone(), Arc::new(session));
        Ok(id)
    }

    fn get(&self, session_id: &str) -> Result<Arc<ProtocolSession>, String> {
        self.sessions
            .lock()
            .map_err(|_| "protocol session state is unavailable".to_owned())?
            .get(session_id)
            .cloned()
            .ok_or_else(|| "protocol session no longer exists".to_owned())
    }

    fn remove(&self, session_id: &str) -> Result<Option<Arc<ProtocolSession>>, String> {
        self.sessions
            .lock()
            .map_err(|_| "protocol session state is unavailable".to_owned())
            .map(|mut sessions| sessions.remove(session_id))
    }

    fn stop_all(&self) -> Result<(), String> {
        let sessions = self
            .sessions
            .lock()
            .map_err(|_| "protocol session state is unavailable".to_owned())?
            .drain()
            .map(|(_, session)| session)
            .collect::<Vec<_>>();
        for session in sessions {
            session.stop();
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProtocolProfileCommandInput {
    profile: ProtocolProfile,
    environment_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProtocolPollResult {
    events: Vec<ProtocolEvent>,
    finished: bool,
}

struct PreparedProtocolProfile {
    url: String,
    headers: Vec<HeaderEntry>,
    configuration: ProtocolConfiguration,
    options: RequestOptions,
    context: HttpExecutionContext,
    redactions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OAuthCacheKey {
    workspace_id: String,
    configuration_fingerprint: u64,
}

#[derive(Debug, Clone)]
struct CachedOAuthToken {
    access_token: String,
    expires_at: Instant,
}

impl CookieState {
    fn jar(&self, workspace_id: &str) -> Result<Arc<CookieJar>, String> {
        let mut jars = self
            .jars
            .lock()
            .map_err(|_| "cookie state is unavailable".to_owned())?;
        Ok(jars
            .entry(workspace_id.to_owned())
            .or_insert_with(|| Arc::new(CookieJar::default()))
            .clone())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SecretMutationInput {
    variable_id: String,
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestLifecycleInput {
    request_id: Option<String>,
    request: RequestDefinition,
    scripts: RequestScripts,
    environment_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestLifecycleResult {
    response: Option<ResponseSnapshot>,
    script_runs: Vec<ScriptReport>,
    error: Option<String>,
    /// Plain (non-secret, non-local) environment values changed by scripts during this send,
    /// so the frontend can persist them for use by subsequent, separate Send calls.
    environment_updates: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScriptReport {
    phase: ScriptPhase,
    logs: Vec<ScriptLog>,
    tests: Vec<ScriptTestResult>,
    error: Option<String>,
    duration_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectionRunInput {
    target: CollectionRunTarget,
    environment_id: Option<String>,
    iterations: u32,
    delay_ms: u64,
    stop_on_failure: bool,
    #[serde(default)]
    data_rows: Vec<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum CollectionRunTarget {
    Collection { collection_id: String },
    Folder { folder_id: String },
}

#[derive(Debug, Clone)]
struct RunnableRequest {
    id: String,
    name: String,
    request: RequestDefinition,
    scripts: RequestScripts,
    inherited_auth: RequestAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectionRunReport {
    schema_version: u32,
    id: String,
    workspace_id: String,
    workspace_name: String,
    target_name: String,
    environment_name: Option<String>,
    started_at_ms: u64,
    duration_ms: u64,
    requested_iterations: u32,
    completed_iterations: u32,
    stop_on_failure: bool,
    summary: CollectionRunSummary,
    results: Vec<CollectionRunRequestResult>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectionRunSummary {
    requests_executed: u32,
    requests_failed: u32,
    tests_passed: u32,
    tests_failed: u32,
    bytes_received: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollectionRunRequestResult {
    iteration: u32,
    request_id: String,
    request_name: String,
    status: Option<u16>,
    duration_ms: u64,
    received_bytes: u64,
    tests_passed: u32,
    tests_failed: u32,
    error: Option<String>,
    script_runs: Vec<ScriptReport>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectionRunReportSummary {
    id: String,
    target_name: String,
    environment_name: Option<String>,
    started_at_ms: u64,
    duration_ms: u64,
    requested_iterations: u32,
    completed_iterations: u32,
    summary: CollectionRunSummary,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum CollectionRunExportFormat {
    Json,
    Junit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoverySnapshotDocument {
    schema_version: u32,
    workspace_id: String,
    active_tab_id: String,
    active_environment_id: Option<String>,
    tabs: Vec<RecoveryRequestTab>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryRequestTab {
    id: String,
    request_id: Option<String>,
    title: String,
    request: RequestDefinition,
    scripts: RequestScripts,
    dirty: bool,
    request_tab: String,
    response_tab: String,
    script_tab: String,
}

#[tauri::command]
async fn execute_http_request(
    state: State<'_, RequestExecutionState>,
    request: RequestDefinition,
) -> Result<ResponseSnapshot, String> {
    let request_id = state.next_id.fetch_add(1, Ordering::Relaxed);
    let task = {
        let mut current = state
            .current
            .lock()
            .map_err(|_| "request execution state is unavailable".to_owned())?;
        if current.is_some() {
            return Err("another request is already running".to_owned());
        }
        let task = tokio::spawn(async move { HttpExecutor.execute(request).await });
        *current = Some((request_id, task.abort_handle()));
        task
    };

    let result = task.await;
    let mut current = state
        .current
        .lock()
        .map_err(|_| "request execution state is unavailable".to_owned())?;
    if current
        .as_ref()
        .is_some_and(|(active_id, _)| *active_id == request_id)
    {
        *current = None;
    }
    drop(current);

    match result {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(error) if error.is_cancelled() => Err("request cancelled".to_owned()),
        Err(_) => Err("request execution stopped unexpectedly".to_owned()),
    }
}

#[tauri::command]
fn cancel_http_request(state: State<'_, RequestExecutionState>) -> Result<bool, String> {
    let mut current = state
        .current
        .lock()
        .map_err(|_| "request execution state is unavailable".to_owned())?;
    let Some((_, handle)) = current.take() else {
        return Ok(false);
    };
    handle.abort();
    Ok(true)
}

#[tauri::command]
async fn execute_request_lifecycle(
    workspace_state: State<'_, WorkspaceState>,
    execution_state: State<'_, RequestExecutionState>,
    vault_state: State<'_, SecretVaultState>,
    cookie_state: State<'_, CookieState>,
    oauth_state: State<'_, OAuthTokenState>,
    input: RequestLifecycleInput,
) -> Result<RequestLifecycleResult, String> {
    let (workspace_id, network_settings, environment_context, inherited_auth) =
        active_request_context(
            &workspace_state,
            input.environment_id.as_deref(),
            input.request_id.as_deref(),
        )?;
    let cookie_jar = workspace_id
        .as_deref()
        .filter(|_| network_settings.cookie_jar_enabled)
        .map(|workspace_id| cookie_state.jar(workspace_id))
        .transpose()?;
    let oauth_workspace_id = workspace_id.unwrap_or_else(|| "scratch".to_owned());
    let oauth_cache = Arc::clone(&oauth_state.entries);

    let request_id = execution_state.next_id.fetch_add(1, Ordering::Relaxed);
    let store = vault_state.store;
    let environment_lock = Arc::clone(&workspace_state.operation_lock);
    let task = {
        let mut current = execution_state
            .current
            .lock()
            .map_err(|_| "request execution state is unavailable".to_owned())?;
        if current.is_some() {
            return Err("another request is already running".to_owned());
        }
        let task = tokio::spawn(async move {
            run_request_lifecycle(
                input,
                environment_context,
                store,
                cookie_jar,
                network_settings,
                inherited_auth,
                oauth_workspace_id,
                oauth_cache,
                environment_lock,
            )
            .await
        });
        *current = Some((request_id, task.abort_handle()));
        task
    };

    let result = task.await;
    let mut current = execution_state
        .current
        .lock()
        .map_err(|_| "request execution state is unavailable".to_owned())?;
    if current
        .as_ref()
        .is_some_and(|(active_id, _)| *active_id == request_id)
    {
        *current = None;
    }
    drop(current);

    match result {
        Ok(result) => result,
        Err(error) if error.is_cancelled() => Err("request cancelled".to_owned()),
        Err(_) => Err("request execution stopped unexpectedly".to_owned()),
    }
}

// Tauri injects each managed state as a separate command parameter.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn run_collection(
    app: AppHandle,
    workspace_state: State<'_, WorkspaceState>,
    execution_state: State<'_, RequestExecutionState>,
    vault_state: State<'_, SecretVaultState>,
    cookie_state: State<'_, CookieState>,
    oauth_state: State<'_, OAuthTokenState>,
    report_state: State<'_, RunReportState>,
    input: CollectionRunInput,
) -> Result<CollectionRunReport, String> {
    validate_collection_run_input(&input)?;
    let active = workspace_state
        .active
        .lock()
        .map_err(|_| "workspace state is unavailable".to_owned())?
        .as_ref()
        .cloned()
        .ok_or_else(|| "no workspace is currently open".to_owned())?;
    let (target_name, requests) = select_run_requests(&active.workspace, &input.target)?;
    let iterations = if input.data_rows.is_empty() {
        input.iterations as usize
    } else {
        input.data_rows.len()
    };
    let execution_count = requests
        .len()
        .checked_mul(iterations)
        .ok_or_else(|| "collection run is too large".to_owned())?;
    if execution_count > MAX_RUN_EXECUTIONS {
        return Err(format!(
            "collection run would execute {execution_count} requests; the limit is {MAX_RUN_EXECUTIONS}"
        ));
    }

    let environment = match input.environment_id.as_deref() {
        Some(environment_id) => Some(
            active
                .workspace
                .environments
                .iter()
                .find(|environment| environment.id == environment_id)
                .cloned()
                .ok_or_else(|| "the selected environment no longer exists".to_owned())?,
        ),
        None => None,
    };
    let environment_name = environment
        .as_ref()
        .map(|environment| environment.name.clone());
    let environment_context =
        environment.map(|environment| (active.workspace.id.clone(), environment));
    let cookie_jar = active
        .workspace
        .network
        .cookie_jar_enabled
        .then(|| cookie_state.jar(&active.workspace.id))
        .transpose()?;
    let oauth_cache = Arc::clone(&oauth_state.entries);
    let oauth_workspace_id = active.workspace.id.clone();
    let workspace_id = active.workspace.id.clone();
    let workspace_name = active.workspace.name.clone();
    let network_settings = active.workspace.network;
    let store = vault_state.store;
    let environment_lock = Arc::clone(&workspace_state.operation_lock);

    let execution_id = execution_state.next_id.fetch_add(1, Ordering::Relaxed);
    let task = {
        let mut current = execution_state
            .current
            .lock()
            .map_err(|_| "request execution state is unavailable".to_owned())?;
        if current.is_some() {
            return Err("another request or collection run is already active".to_owned());
        }
        let task = tokio::spawn(async move {
            run_collection_internal(
                input,
                requests,
                environment_context,
                environment_name,
                store,
                cookie_jar,
                network_settings,
                workspace_id,
                workspace_name,
                target_name,
                oauth_workspace_id,
                oauth_cache,
                environment_lock,
            )
            .await
        });
        *current = Some((execution_id, task.abort_handle()));
        task
    };

    let result = task.await;
    {
        let mut current = execution_state
            .current
            .lock()
            .map_err(|_| "request execution state is unavailable".to_owned())?;
        if current
            .as_ref()
            .is_some_and(|(active_id, _)| *active_id == execution_id)
        {
            *current = None;
        }
    }

    let report = match result {
        Ok(result) => result?,
        Err(error) if error.is_cancelled() => return Err("collection run cancelled".to_owned()),
        Err(_) => return Err("collection run stopped unexpectedly".to_owned()),
    };
    let _operation = report_state.operation_lock.lock().await;
    persist_collection_run_report(&app, &report).await?;
    Ok(report)
}

#[allow(clippy::too_many_arguments)]
async fn run_collection_internal<S: SecretStore + Clone + 'static>(
    input: CollectionRunInput,
    requests: Vec<RunnableRequest>,
    environment_context: Option<(String, Environment)>,
    environment_name: Option<String>,
    store: S,
    cookie_jar: Option<Arc<CookieJar>>,
    network_settings: NetworkSettings,
    workspace_id: String,
    workspace_name: String,
    target_name: String,
    oauth_workspace_id: String,
    oauth_cache: Arc<Mutex<HashMap<OAuthCacheKey, CachedOAuthToken>>>,
    environment_lock: Arc<tokio::sync::Mutex<()>>,
) -> Result<CollectionRunReport, String> {
    let (environment, secret_keys, redactions) = {
        let _operation = environment_lock.lock().await;
        load_environment(environment_context.clone(), store.clone()).await?
    };
    let mut runtime = RequestRuntime {
        environment,
        variables: BTreeMap::new(),
        secret_keys,
        redactions,
    };
    let started_at_ms = unix_timestamp_ms()?;
    let started = Instant::now();
    let requested_iterations = if input.data_rows.is_empty() {
        input.iterations
    } else {
        input.data_rows.len() as u32
    };
    let mut completed_iterations = 0u32;
    let mut summary = CollectionRunSummary::default();
    let mut results = Vec::with_capacity(requests.len() * requested_iterations as usize);

    'iterations: for iteration_index in 0..requested_iterations as usize {
        runtime.variables = input
            .data_rows
            .get(iteration_index)
            .cloned()
            .unwrap_or_default();
        for (key, value) in &runtime.variables {
            if is_sensitive_variable_name(key) {
                push_redaction(&mut runtime.redactions, value);
            }
        }

        for (request_index, runnable) in requests.iter().enumerate() {
            let request_started = Instant::now();
            let initial_environment = runtime.environment.clone();
            let mut lifecycle = run_request_with_runtime(
                RequestLifecycleInput {
                    request_id: Some(runnable.id.clone()),
                    request: runnable.request.clone(),
                    scripts: runnable.scripts.clone(),
                    environment_id: None,
                },
                &mut runtime,
                cookie_jar.clone(),
                network_settings.clone(),
                Some(runnable.inherited_auth.clone()),
                &oauth_workspace_id,
                &oauth_cache,
            )
            .await?;
            if lifecycle.response.is_some()
                && let Err(error) = persist_script_environment(
                    environment_context.clone(),
                    &initial_environment,
                    &runtime.environment,
                    store.clone(),
                    Arc::clone(&environment_lock),
                )
                .await
            {
                lifecycle.error = Some(redact_text(&error, &runtime.redactions));
            }
            let tests_passed = lifecycle
                .script_runs
                .iter()
                .flat_map(|run| &run.tests)
                .filter(|test| test.passed)
                .count() as u32;
            let tests_failed = lifecycle
                .script_runs
                .iter()
                .flat_map(|run| &run.tests)
                .filter(|test| !test.passed)
                .count() as u32;
            let script_error = lifecycle
                .script_runs
                .iter()
                .find_map(|run| run.error.clone());
            let error = lifecycle.error.clone().or(script_error);
            let status = lifecycle.response.as_ref().map(|response| response.status);
            let received_bytes = lifecycle
                .response
                .as_ref()
                .map_or(0, |response| response.received_bytes);
            let duration_ms = lifecycle.response.as_ref().map_or_else(
                || request_started.elapsed().as_millis() as u64,
                |response| response.timing.total_ms,
            );
            let failed = error.is_some() || lifecycle.response.is_none() || tests_failed > 0;

            summary.requests_executed += 1;
            summary.requests_failed += u32::from(failed);
            summary.tests_passed += tests_passed;
            summary.tests_failed += tests_failed;
            summary.bytes_received = summary.bytes_received.saturating_add(received_bytes);
            results.push(CollectionRunRequestResult {
                iteration: iteration_index as u32 + 1,
                request_id: runnable.id.clone(),
                request_name: runnable.name.clone(),
                status,
                duration_ms,
                received_bytes,
                tests_passed,
                tests_failed,
                error,
                script_runs: lifecycle.script_runs,
            });

            if failed && input.stop_on_failure {
                if request_index + 1 == requests.len() {
                    completed_iterations = iteration_index as u32 + 1;
                }
                break 'iterations;
            }
            let is_last_execution = iteration_index + 1 == requested_iterations as usize
                && request_index + 1 == requests.len();
            if input.delay_ms > 0 && !is_last_execution {
                tokio::time::sleep(Duration::from_millis(input.delay_ms)).await;
            }
        }
        completed_iterations = iteration_index as u32 + 1;
    }

    Ok(CollectionRunReport {
        schema_version: 1,
        id: Uuid::new_v4().to_string(),
        workspace_id,
        workspace_name,
        target_name,
        environment_name,
        started_at_ms,
        duration_ms: started.elapsed().as_millis() as u64,
        requested_iterations,
        completed_iterations,
        stop_on_failure: input.stop_on_failure,
        summary,
        results,
    })
}

fn validate_collection_run_input(input: &CollectionRunInput) -> Result<(), String> {
    if input.iterations == 0 || input.iterations > 1_000 {
        return Err("collection run iterations must be between 1 and 1,000".to_owned());
    }
    if input.delay_ms > 60_000 {
        return Err("collection run delay cannot exceed 60 seconds".to_owned());
    }
    if input.data_rows.len() > 1_000 {
        return Err("collection run data cannot contain more than 1,000 rows".to_owned());
    }
    let data_size = serde_json::to_vec(&input.data_rows)
        .map_err(|_| "collection run data could not be validated".to_owned())?
        .len();
    if data_size > MAX_RUN_DATA_BYTES {
        return Err("collection run data exceeds the 2 MiB limit".to_owned());
    }
    Ok(())
}

fn select_run_requests(
    workspace: &WorkspaceDocument,
    target: &CollectionRunTarget,
) -> Result<(String, Vec<RunnableRequest>), String> {
    let workspace_auth = workspace.auth.clone().unwrap_or_default();
    match target {
        CollectionRunTarget::Collection { collection_id } => {
            let collection = workspace
                .collections
                .iter()
                .find(|collection| collection.id == *collection_id)
                .ok_or_else(|| "the selected collection no longer exists".to_owned())?;
            let collection_auth = collection
                .auth
                .clone()
                .unwrap_or_else(|| workspace_auth.clone());
            let mut requests = collection
                .requests
                .iter()
                .map(|request| runnable_request(request, &collection_auth))
                .collect::<Vec<_>>();
            for folder in &collection.folders {
                collect_folder_requests(folder, &collection_auth, &mut requests);
            }
            if requests.is_empty() {
                return Err("the selected collection contains no requests".to_owned());
            }
            Ok((collection.name.clone(), requests))
        }
        CollectionRunTarget::Folder { folder_id } => {
            for collection in &workspace.collections {
                let collection_auth = collection
                    .auth
                    .clone()
                    .unwrap_or_else(|| workspace_auth.clone());
                for folder in &collection.folders {
                    if let Some((name, requests)) =
                        find_folder_run_requests(folder, folder_id, &collection_auth)
                    {
                        if requests.is_empty() {
                            return Err("the selected folder contains no requests".to_owned());
                        }
                        return Ok((name, requests));
                    }
                }
            }
            Err("the selected folder no longer exists".to_owned())
        }
    }
}

fn find_folder_run_requests(
    folder: &api_client_workspace::RequestFolder,
    folder_id: &str,
    parent_auth: &RequestAuth,
) -> Option<(String, Vec<RunnableRequest>)> {
    let folder_auth = folder.auth.clone().unwrap_or_else(|| parent_auth.clone());
    if folder.id == folder_id {
        let mut requests = Vec::new();
        collect_folder_requests(folder, parent_auth, &mut requests);
        return Some((folder.name.clone(), requests));
    }
    folder
        .folders
        .iter()
        .find_map(|child| find_folder_run_requests(child, folder_id, &folder_auth))
}

fn collect_folder_requests(
    folder: &api_client_workspace::RequestFolder,
    parent_auth: &RequestAuth,
    output: &mut Vec<RunnableRequest>,
) {
    let folder_auth = folder.auth.clone().unwrap_or_else(|| parent_auth.clone());
    output.extend(
        folder
            .requests
            .iter()
            .map(|request| runnable_request(request, &folder_auth)),
    );
    for child in &folder.folders {
        collect_folder_requests(child, &folder_auth, output);
    }
}

fn runnable_request(
    request: &api_client_workspace::SavedRequest,
    inherited_auth: &RequestAuth,
) -> RunnableRequest {
    RunnableRequest {
        id: request.id.clone(),
        name: request.name.clone(),
        request: request.request.clone(),
        scripts: request.scripts.clone(),
        inherited_auth: inherited_auth.clone(),
    }
}

fn is_sensitive_variable_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("secret")
        || name.contains("token")
        || name.contains("password")
        || name.contains("authorization")
        || name.contains("cookie")
        || name.ends_with("key")
}

fn unix_timestamp_ms() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .map_err(|_| "system clock is before the Unix epoch".to_owned())
}

#[allow(clippy::too_many_arguments)]
async fn run_request_lifecycle<S: SecretStore + Clone + 'static>(
    input: RequestLifecycleInput,
    environment_context: Option<(String, Environment)>,
    store: S,
    cookie_jar: Option<Arc<CookieJar>>,
    network_settings: NetworkSettings,
    inherited_auth: Option<RequestAuth>,
    oauth_workspace_id: String,
    oauth_cache: Arc<Mutex<HashMap<OAuthCacheKey, CachedOAuthToken>>>,
    environment_lock: Arc<tokio::sync::Mutex<()>>,
) -> Result<RequestLifecycleResult, String> {
    let (environment, secret_keys, redactions) = {
        let _operation = environment_lock.lock().await;
        load_environment(environment_context.clone(), store.clone()).await?
    };
    let initial_environment = environment.clone();
    let mut runtime = RequestRuntime {
        environment,
        variables: BTreeMap::new(),
        secret_keys,
        redactions,
    };
    let mut result = run_request_with_runtime(
        input,
        &mut runtime,
        cookie_jar,
        network_settings,
        inherited_auth,
        &oauth_workspace_id,
        &oauth_cache,
    )
    .await?;
    if result.response.is_some()
        && let Err(error) = persist_script_environment(
            environment_context,
            &initial_environment,
            &runtime.environment,
            store,
            environment_lock,
        )
        .await
    {
        result.error = Some(redact_text(&error, &runtime.redactions));
    }
    Ok(result)
}

/// Script writes to existing protected variables must reach the vault before a
/// subsequent Send loads its environment. Their values never enter the IPC diff.
async fn persist_script_environment<S: SecretStore + 'static>(
    context: Option<(String, Environment)>,
    initial: &BTreeMap<String, String>,
    updated: &BTreeMap<String, String>,
    store: S,
    operation_lock: Arc<tokio::sync::Mutex<()>>,
) -> Result<(), String> {
    let Some((workspace_id, environment)) = context else {
        return Ok(());
    };
    let changes = environment
        .variables
        .iter()
        .filter(|variable| {
            variable.enabled
                && !variable.key.trim().is_empty()
                && (variable.secret || variable.local)
        })
        .filter_map(|variable| {
            let key = variable.key.trim();
            let value = updated.get(key)?;
            (initial.get(key) != Some(value)).then(|| {
                (
                    key.to_owned(),
                    initial.get(key).cloned(),
                    SecretChange {
                        variable_id: variable.id.clone(),
                        value: Some(value.clone()),
                    },
                )
            })
        })
        .collect::<Vec<_>>();
    if changes.is_empty() {
        return Ok(());
    }

    let operation = operation_lock.lock_owned().await;
    tokio::task::spawn_blocking(move || {
        // Keep serialization in the worker even if the request task is cancelled.
        let _operation = operation;
        for (key, expected, change) in &changes {
            let current = store
                .get(&workspace_id, &change.variable_id)
                .map_err(|_| "Could not read the environment credential store.".to_owned())?;
            if &current != expected {
                return Err(format!(
                    "Environment variable '{{{{{key}}}}}' changed while the request was running. Its newer value was preserved; run the token request again."
                ));
            }
        }
        let changes = changes.into_iter().map(|(_, _, change)| change).collect::<Vec<_>>();
        apply_changes(&store, &workspace_id, &changes)
            .map_err(|_| "Could not save script-updated environment credentials. Run the token request again before using them.".to_owned())?;
        Ok(())
    })
    .await
    .map_err(|_| "Saving script-updated environment credentials stopped unexpectedly.".to_owned())?
}

struct RequestRuntime {
    environment: BTreeMap<String, String>,
    variables: BTreeMap<String, String>,
    secret_keys: BTreeSet<String>,
    redactions: Vec<String>,
}

async fn run_request_with_runtime(
    input: RequestLifecycleInput,
    runtime: &mut RequestRuntime,
    cookie_jar: Option<Arc<CookieJar>>,
    network_settings: NetworkSettings,
    inherited_auth: Option<RequestAuth>,
    oauth_workspace_id: &str,
    oauth_cache: &Arc<Mutex<HashMap<OAuthCacheKey, CachedOAuthToken>>>,
) -> Result<RequestLifecycleResult, String> {
    let initial_environment = runtime.environment.clone();
    let mut request = input.request;
    if matches!(request.auth, RequestAuth::Inherit) {
        let Some(parent_auth) = inherited_auth else {
            return Ok(RequestLifecycleResult {
                response: None,
                script_runs: Vec::new(),
                error: Some("Authentication cannot be inherited by an unsaved request.".to_owned()),
                environment_updates: BTreeMap::new(),
            });
        };
        request.auth = parent_auth;
    }
    let mut script_runs = Vec::new();

    if !input.scripts.pre_request.trim().is_empty() {
        let result = run_script(ScriptExecutionInput {
            phase: ScriptPhase::PreRequest,
            script: input.scripts.pre_request,
            request,
            response: None,
            environment: runtime.environment.clone(),
            variables: runtime.variables.clone(),
        })
        .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let error = redact_text(&error, &runtime.redactions);
                script_runs.push(script_error_report(ScriptPhase::PreRequest, &error));
                return Ok(RequestLifecycleResult {
                    response: None,
                    script_runs,
                    error: Some(format!("Pre-request script: {error}")),
                    environment_updates: BTreeMap::new(),
                });
            }
        };
        add_secret_redactions(
            &mut runtime.redactions,
            &runtime.secret_keys,
            &result.environment,
            &result.variables,
        );
        let script_error = result.error.clone();
        script_runs.push(script_report(&result, &runtime.redactions));
        if let Some(error) = script_error {
            return Ok(RequestLifecycleResult {
                response: None,
                script_runs,
                error: Some(format!(
                    "Pre-request script: {}",
                    redact_text(&error, &runtime.redactions)
                )),
                environment_updates: BTreeMap::new(),
            });
        }
        request = result.request;
        runtime.environment = result.environment;
        runtime.variables = result.variables;
    }

    let mut resolution_values = runtime.environment.clone();
    resolution_values.extend(runtime.variables.clone());
    let resolution = resolve_request_variables(&request, &resolution_values);
    if let Err(error) = ensure_variables_resolved(
        &resolution.errors,
        &resolution.cycles,
        &resolution.unresolved,
    ) {
        return Ok(RequestLifecycleResult {
            response: None,
            script_runs,
            error: Some(redact_text(&error, &runtime.redactions)),
            environment_updates: BTreeMap::new(),
        });
    }

    let mut resolved_request = resolution.request;
    let http_context = match build_http_execution_context(
        network_settings,
        &resolution_values,
        cookie_jar,
    )
    .await
    {
        Ok(context) => context,
        Err(error) => {
            return Ok(RequestLifecycleResult {
                response: None,
                script_runs,
                error: Some(redact_text(&error, &runtime.redactions)),
                environment_updates: BTreeMap::new(),
            });
        }
    };
    add_request_auth_redactions(&resolved_request.auth, &mut runtime.redactions);
    if let Err(error) = materialize_oauth_auth(
        &mut resolved_request,
        &http_context,
        oauth_workspace_id,
        oauth_cache,
        &mut runtime.redactions,
    )
    .await
    {
        return Ok(RequestLifecycleResult {
            response: None,
            script_runs,
            error: Some(redact_text(&error, &runtime.redactions)),
            environment_updates: BTreeMap::new(),
        });
    }
    let response = match HttpExecutor
        .execute_with_context(resolved_request.clone(), http_context)
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return Ok(RequestLifecycleResult {
                response: None,
                script_runs,
                error: Some(redact_text(&error.to_string(), &runtime.redactions)),
                environment_updates: BTreeMap::new(),
            });
        }
    };

    if !input.scripts.post_response.trim().is_empty() {
        match run_script(ScriptExecutionInput {
            phase: ScriptPhase::PostResponse,
            script: input.scripts.post_response,
            request: resolved_request,
            response: Some(response.clone()),
            environment: runtime.environment.clone(),
            variables: runtime.variables.clone(),
        })
        .await
        {
            Ok(result) => {
                add_secret_redactions(
                    &mut runtime.redactions,
                    &runtime.secret_keys,
                    &result.environment,
                    &result.variables,
                );
                script_runs.push(script_report(&result, &runtime.redactions));
                runtime.environment = result.environment;
                runtime.variables = result.variables;
            }
            Err(error) => {
                script_runs.push(script_error_report(
                    ScriptPhase::PostResponse,
                    &redact_text(&error, &runtime.redactions),
                ));
            }
        }
    }

    Ok(RequestLifecycleResult {
        response: Some(response),
        script_runs,
        error: None,
        environment_updates: environment_updates_diff(
            &initial_environment,
            &runtime.environment,
            &runtime.secret_keys,
        ),
    })
}

/// Diffs environment values changed by scripts, excluding secret/local-scoped keys whose
/// real values live in the OS credential store rather than the workspace document.
fn environment_updates_diff(
    initial: &BTreeMap<String, String>,
    updated: &BTreeMap<String, String>,
    protected_keys: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    updated
        .iter()
        .filter(|(key, value)| !protected_keys.contains(*key) && initial.get(*key) != Some(*value))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

async fn build_http_execution_context(
    settings: NetworkSettings,
    values: &BTreeMap<String, String>,
    cookie_jar: Option<Arc<CookieJar>>,
) -> Result<HttpExecutionContext, String> {
    let proxy = match settings.proxy_mode {
        ProxyMode::System => ProxyConfiguration::System,
        ProxyMode::Disabled => ProxyConfiguration::Disabled,
        ProxyMode::Custom => ProxyConfiguration::Custom {
            url: resolve_network_value("proxy URL", &settings.proxy_url, values)?,
            username: resolve_network_value("proxy username", &settings.proxy_username, values)?,
            password: resolve_network_value("proxy password", &settings.proxy_password, values)?,
            no_proxy: resolve_network_value("no-proxy rules", &settings.no_proxy, values)?,
        },
    };

    let ca_certificate_path =
        resolve_network_value("CA certificate path", &settings.ca_certificate_path, values)?;
    let client_certificate_path = resolve_network_value(
        "client certificate path",
        &settings.client_certificate_path,
        values,
    )?;
    let root_certificate_pem = read_tls_material("CA certificate", &ca_certificate_path).await?;
    let client_identity_pem =
        read_tls_material("client certificate identity", &client_certificate_path).await?;

    Ok(HttpExecutionContext {
        cookie_jar,
        proxy,
        root_certificate_pem,
        client_identity_pem,
        accept_invalid_certificates: settings.accept_invalid_certificates,
    })
}

fn resolve_network_value(
    label: &str,
    input: &str,
    values: &BTreeMap<String, String>,
) -> Result<String, String> {
    let resolution = resolve_text_variables(input, values);
    ensure_variables_resolved(
        &resolution.errors,
        &resolution.cycles,
        &resolution.unresolved,
    )
    .map_err(|error| format!("{label}: {error}"))?;
    Ok(resolution.value)
}

async fn read_tls_material(label: &str, path: &str) -> Result<Vec<u8>, String> {
    if path.trim().is_empty() {
        return Ok(Vec::new());
    }

    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| format!("{label} file could not be read"))?;
    if !metadata.is_file() {
        return Err(format!("{label} path is not a regular file"));
    }
    if metadata.len() > MAX_TLS_MATERIAL_BYTES {
        return Err(format!(
            "{label} exceeds the {} MiB limit",
            MAX_TLS_MATERIAL_BYTES / (1024 * 1024)
        ));
    }

    let contents = tokio::fs::read(path)
        .await
        .map_err(|_| format!("{label} file could not be read"))?;
    if contents.len() as u64 > MAX_TLS_MATERIAL_BYTES {
        return Err(format!(
            "{label} exceeds the {} MiB limit",
            MAX_TLS_MATERIAL_BYTES / (1024 * 1024)
        ));
    }
    Ok(contents)
}

#[derive(Clone)]
struct OAuthClientCredentials {
    issuer_url: String,
    token_url: String,
    client_id: String,
    client_secret: String,
    scope: String,
    audience: String,
    client_auth: OAuth2ClientAuth,
    extra_parameters: Vec<KeyValueEntry>,
}

async fn materialize_oauth_auth(
    request: &mut RequestDefinition,
    context: &HttpExecutionContext,
    workspace_id: &str,
    cache: &Arc<Mutex<HashMap<OAuthCacheKey, CachedOAuthToken>>>,
    redactions: &mut Vec<String>,
) -> Result<(), String> {
    let RequestAuth::OAuth2ClientCredentials {
        issuer_url,
        token_url,
        client_id,
        client_secret,
        scope,
        audience,
        client_auth,
        extra_parameters,
    } = &request.auth
    else {
        return Ok(());
    };
    let credentials = OAuthClientCredentials {
        issuer_url: issuer_url.trim().to_owned(),
        token_url: token_url.trim().to_owned(),
        client_id: client_id.clone(),
        client_secret: client_secret.clone(),
        scope: scope.trim().to_owned(),
        audience: audience.trim().to_owned(),
        client_auth: *client_auth,
        extra_parameters: extra_parameters.clone(),
    };
    validate_oauth_credentials(&credentials)?;

    let key = OAuthCacheKey {
        workspace_id: workspace_id.to_owned(),
        configuration_fingerprint: oauth_configuration_fingerprint(&credentials),
    };
    if let Some(token) = cached_oauth_token(cache, &key)? {
        push_redaction(redactions, &token);
        request.auth = RequestAuth::Bearer { token };
        return Ok(());
    }

    let mut oauth_context = context.clone();
    oauth_context.cookie_jar = None;
    let token_url = if credentials.token_url.is_empty() {
        discover_oidc_token_endpoint(&credentials.issuer_url, oauth_context.clone()).await?
    } else {
        credentials.token_url.clone()
    };
    let token = request_client_credentials_token(&token_url, &credentials, oauth_context).await?;
    cache_oauth_token(cache, key, &token)?;
    push_redaction(redactions, &token.access_token);
    request.auth = RequestAuth::Bearer {
        token: token.access_token,
    };
    Ok(())
}

fn validate_oauth_credentials(credentials: &OAuthClientCredentials) -> Result<(), String> {
    if credentials.token_url.is_empty() && credentials.issuer_url.is_empty() {
        return Err("OAuth 2 requires a token URL or an OpenID Connect issuer URL".to_owned());
    }
    if credentials.client_id.trim().is_empty() {
        return Err("OAuth 2 client ID cannot be empty".to_owned());
    }
    if credentials.client_id.len() > 64 * 1024 || credentials.client_secret.len() > 64 * 1024 {
        return Err("OAuth 2 client credentials exceed the 64 KiB limit".to_owned());
    }
    if credentials.extra_parameters.len() > 128 {
        return Err("OAuth 2 cannot contain more than 128 extra parameters".to_owned());
    }
    const RESERVED: [&str; 5] = [
        "grant_type",
        "client_id",
        "client_secret",
        "scope",
        "audience",
    ];
    for parameter in credentials
        .extra_parameters
        .iter()
        .filter(|parameter| parameter.enabled)
    {
        if RESERVED
            .iter()
            .any(|reserved| parameter.name.trim().eq_ignore_ascii_case(reserved))
        {
            return Err(format!(
                "OAuth 2 extra parameter '{}' is managed by the authentication editor",
                parameter.name.trim()
            ));
        }
    }
    Ok(())
}

async fn discover_oidc_token_endpoint(
    issuer_url: &str,
    context: HttpExecutionContext,
) -> Result<String, String> {
    let mut issuer = url::Url::parse(issuer_url)
        .map_err(|_| "OpenID Connect issuer URL is invalid".to_owned())?;
    if !matches!(issuer.scheme(), "http" | "https")
        || !issuer.username().is_empty()
        || issuer.password().is_some()
    {
        return Err(
            "OpenID Connect issuer URL must be an HTTP(S) URL without credentials".to_owned(),
        );
    }
    issuer.set_query(None);
    issuer.set_fragment(None);
    let discovery_path = format!(
        "{}/.well-known/openid-configuration",
        issuer.path().trim_end_matches('/')
    );
    issuer.set_path(&discovery_path);

    let response = HttpExecutor
        .execute_with_context(oauth_request("GET", issuer.into()), context)
        .await
        .map_err(|error| format!("OpenID Connect discovery failed: {error}"))?;
    let document = oauth_json_response(response, "OpenID Connect discovery")?;
    let discovered_issuer = document
        .get("issuer")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "OpenID Connect discovery document has no issuer".to_owned())?;
    if discovered_issuer != issuer_url {
        return Err("OpenID Connect discovery returned a different issuer".to_owned());
    }
    let token_endpoint = document
        .get("token_endpoint")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "OpenID Connect discovery document has no token endpoint".to_owned())?;
    validate_oauth_endpoint(token_endpoint, "discovered token endpoint")?;
    Ok(token_endpoint.to_owned())
}

async fn request_client_credentials_token(
    token_url: &str,
    credentials: &OAuthClientCredentials,
    context: HttpExecutionContext,
) -> Result<CachedOAuthToken, String> {
    validate_oauth_endpoint(token_url, "token URL")?;
    let mut fields = vec![KeyValueEntry {
        name: "grant_type".to_owned(),
        value: "client_credentials".to_owned(),
        enabled: true,
    }];
    if !credentials.scope.is_empty() {
        fields.push(KeyValueEntry {
            name: "scope".to_owned(),
            value: credentials.scope.clone(),
            enabled: true,
        });
    }
    if !credentials.audience.is_empty() {
        fields.push(KeyValueEntry {
            name: "audience".to_owned(),
            value: credentials.audience.clone(),
            enabled: true,
        });
    }
    let auth = match credentials.client_auth {
        OAuth2ClientAuth::Basic => RequestAuth::Basic {
            username: credentials.client_id.clone(),
            password: credentials.client_secret.clone(),
        },
        OAuth2ClientAuth::RequestBody => {
            fields.extend([
                KeyValueEntry {
                    name: "client_id".to_owned(),
                    value: credentials.client_id.clone(),
                    enabled: true,
                },
                KeyValueEntry {
                    name: "client_secret".to_owned(),
                    value: credentials.client_secret.clone(),
                    enabled: true,
                },
            ]);
            RequestAuth::None
        }
    };
    fields.extend(
        credentials
            .extra_parameters
            .iter()
            .filter(|parameter| parameter.enabled)
            .cloned(),
    );

    let mut request = oauth_request("POST", token_url.to_owned());
    request.auth = auth;
    request.body = RequestBody::FormUrlEncoded { fields };
    let response = HttpExecutor
        .execute_with_context(request, context)
        .await
        .map_err(|error| format!("OAuth 2 token request failed: {error}"))?;
    let document = oauth_json_response(response, "OAuth 2 token request")?;
    let access_token = document
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "OAuth 2 token response has no access token".to_owned())?;
    if access_token.is_empty() || access_token.len() > MAX_OAUTH_TOKEN_BYTES {
        return Err("OAuth 2 access token has an invalid size".to_owned());
    }
    if let Some(token_type) = document
        .get("token_type")
        .and_then(serde_json::Value::as_str)
        && !token_type.eq_ignore_ascii_case("bearer")
    {
        return Err("OAuth 2 token type is not supported; expected Bearer".to_owned());
    }
    let expires_in = document
        .get("expires_in")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .unwrap_or(300)
        .clamp(30, 24 * 60 * 60);
    Ok(CachedOAuthToken {
        access_token: access_token.to_owned(),
        expires_at: Instant::now() + Duration::from_secs(expires_in),
    })
}

fn oauth_request(method: &str, url: String) -> RequestDefinition {
    RequestDefinition {
        method: method.to_owned(),
        url,
        query_params: Vec::new(),
        headers: vec![HeaderEntry {
            name: "Accept".to_owned(),
            value: "application/json".to_owned(),
            enabled: true,
        }],
        auth: RequestAuth::None,
        body: RequestBody::Empty,
        options: RequestOptions {
            timeout_ms: 30_000,
            max_request_bytes: MAX_OAUTH_RESPONSE_BYTES,
            max_response_bytes: MAX_OAUTH_RESPONSE_BYTES,
            follow_redirects: true,
        },
    }
}

fn oauth_json_response(
    response: ResponseSnapshot,
    operation: &str,
) -> Result<serde_json::Value, String> {
    if response.body.truncated {
        return Err(format!("{operation} response exceeds the 1 MiB limit"));
    }
    if response.body.encoding != ResponseBodyEncoding::Utf8 {
        return Err(format!("{operation} returned a non-text response"));
    }
    let document: serde_json::Value = serde_json::from_str(&response.body.content)
        .map_err(|_| format!("{operation} returned invalid JSON"))?;
    if !(200..300).contains(&response.status) {
        let code = document
            .get("error")
            .and_then(serde_json::Value::as_str)
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 80
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_- .".contains(&byte))
            });
        return Err(match code {
            Some(code) => format!("{operation} failed with HTTP {} ({code})", response.status),
            None => format!("{operation} failed with HTTP {}", response.status),
        });
    }
    Ok(document)
}

fn validate_oauth_endpoint(endpoint: &str, label: &str) -> Result<(), String> {
    let endpoint = url::Url::parse(endpoint).map_err(|_| format!("OAuth 2 {label} is invalid"))?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
    {
        return Err(format!(
            "OAuth 2 {label} must be an HTTP(S) URL without embedded credentials"
        ));
    }
    Ok(())
}

fn oauth_configuration_fingerprint(credentials: &OAuthClientCredentials) -> u64 {
    let mut hasher = DefaultHasher::new();
    credentials.issuer_url.hash(&mut hasher);
    credentials.token_url.hash(&mut hasher);
    credentials.client_id.hash(&mut hasher);
    credentials.client_secret.hash(&mut hasher);
    credentials.scope.hash(&mut hasher);
    credentials.audience.hash(&mut hasher);
    match credentials.client_auth {
        OAuth2ClientAuth::Basic => 0u8,
        OAuth2ClientAuth::RequestBody => 1u8,
    }
    .hash(&mut hasher);
    for parameter in credentials
        .extra_parameters
        .iter()
        .filter(|parameter| parameter.enabled)
    {
        parameter.name.hash(&mut hasher);
        parameter.value.hash(&mut hasher);
    }
    hasher.finish()
}

fn cached_oauth_token(
    cache: &Arc<Mutex<HashMap<OAuthCacheKey, CachedOAuthToken>>>,
    key: &OAuthCacheKey,
) -> Result<Option<String>, String> {
    let now = Instant::now();
    let mut entries = cache
        .lock()
        .map_err(|_| "OAuth 2 token cache is unavailable".to_owned())?;
    entries.retain(|_, token| token.expires_at > now);
    Ok(entries.get(key).and_then(|token| {
        (token.expires_at > now + Duration::from_secs(30)).then(|| token.access_token.clone())
    }))
}

fn cache_oauth_token(
    cache: &Arc<Mutex<HashMap<OAuthCacheKey, CachedOAuthToken>>>,
    key: OAuthCacheKey,
    token: &CachedOAuthToken,
) -> Result<(), String> {
    let mut entries = cache
        .lock()
        .map_err(|_| "OAuth 2 token cache is unavailable".to_owned())?;
    entries.retain(|_, cached| cached.expires_at > Instant::now());
    if entries.len() >= MAX_CACHED_OAUTH_TOKENS
        && let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, cached)| cached.expires_at)
            .map(|(key, _)| key.clone())
    {
        entries.remove(&oldest);
    }
    entries.insert(key, token.clone());
    Ok(())
}

async fn load_environment<S: SecretStore + 'static>(
    environment_context: Option<(String, Environment)>,
    store: S,
) -> Result<(BTreeMap<String, String>, BTreeSet<String>, Vec<String>), String> {
    tokio::task::spawn_blocking(move || {
        let Some((workspace_id, environment)) = environment_context else {
            return Ok((BTreeMap::new(), BTreeSet::new(), Vec::new()));
        };
        let mut values = BTreeMap::new();
        let mut secret_keys = BTreeSet::new();
        let mut redactions = Vec::new();
        for variable in environment
            .variables
            .into_iter()
            .filter(|variable| variable.enabled && !variable.key.trim().is_empty())
        {
            let key = variable.key.trim().to_owned();
            if variable.secret || variable.local {
                secret_keys.insert(key.clone());
                let Some(value) = store
                    .get(&workspace_id, &variable.id)
                    .map_err(|error| error.to_string())?
                else {
                    // Imported or newly declared secrets can be unset. Leave
                    // them unresolved so only references to them fail; token
                    // requests can populate them in a post-response script.
                    continue;
                };
                // Secret and local variables are excluded from the environment-update diff too,
                // since their real values must stay in the credential store, not workspace.yaml.
                if !value.is_empty() && (variable.secret || is_sensitive_variable_name(&key)) {
                    redactions.push(value.clone());
                }
                values.insert(key, value);
            } else {
                values.insert(key, variable.value);
            }
        }
        sort_redactions(&mut redactions);
        Ok((values, secret_keys, redactions))
    })
    .await
    .map_err(|_| "environment loading stopped unexpectedly".to_owned())?
}

async fn run_script(input: ScriptExecutionInput) -> Result<ScriptExecutionResult, String> {
    tokio::task::spawn_blocking(move || {
        api_client_script::execute_script(input, ScriptLimits::default())
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "script runtime stopped unexpectedly".to_owned())?
}

fn add_secret_redactions(
    redactions: &mut Vec<String>,
    secret_keys: &BTreeSet<String>,
    environment: &BTreeMap<String, String>,
    variables: &BTreeMap<String, String>,
) {
    for key in secret_keys {
        if let Some(value) = variables.get(key).or_else(|| environment.get(key))
            && !value.is_empty()
            && !redactions.contains(value)
        {
            redactions.push(value.clone());
        }
    }
    sort_redactions(redactions);
}

fn add_request_auth_redactions(auth: &RequestAuth, redactions: &mut Vec<String>) {
    match auth {
        RequestAuth::Basic { password, .. } => push_redaction(redactions, password),
        RequestAuth::Bearer { token } => {
            push_redaction(redactions, token);
            // The Authorization header is built from the trimmed token; redact that form too so
            // logs/errors are covered whether they echo the raw field or the sent header value.
            push_redaction(redactions, token.trim());
        }
        RequestAuth::ApiKey { value, .. } => push_redaction(redactions, value),
        RequestAuth::OAuth2ClientCredentials {
            client_secret,
            extra_parameters,
            ..
        } => {
            push_redaction(redactions, client_secret);
            for parameter in extra_parameters.iter().filter(|parameter| {
                let name = parameter.name.to_ascii_lowercase();
                name.contains("secret")
                    || name.contains("token")
                    || name.contains("password")
                    || name.ends_with("key")
            }) {
                push_redaction(redactions, &parameter.value);
            }
        }
        RequestAuth::None | RequestAuth::Inherit => {}
    }
    sort_redactions(redactions);
}

fn push_redaction(redactions: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !redactions.iter().any(|existing| existing == value) {
        redactions.push(value.to_owned());
    }
    sort_redactions(redactions);
}

fn sort_redactions(redactions: &mut [String]) {
    redactions.sort_by_key(|value| std::cmp::Reverse(value.len()));
}

fn redact_text(value: &str, redactions: &[String]) -> String {
    redactions.iter().fold(value.to_owned(), |text, secret| {
        text.replace(secret, "[REDACTED]")
    })
}

fn script_report(result: &ScriptExecutionResult, redactions: &[String]) -> ScriptReport {
    ScriptReport {
        phase: result.phase,
        logs: result
            .logs
            .iter()
            .cloned()
            .map(|mut log| {
                log.message = redact_text(&log.message, redactions);
                log
            })
            .collect(),
        tests: result
            .tests
            .iter()
            .cloned()
            .map(|mut test| {
                test.name = redact_text(&test.name, redactions);
                test.error = test
                    .error
                    .as_deref()
                    .map(|error| redact_text(error, redactions));
                test
            })
            .collect(),
        error: result
            .error
            .as_deref()
            .map(|error| redact_text(error, redactions)),
        duration_ms: result.duration_ms,
    }
}

fn script_error_report(phase: ScriptPhase, error: &str) -> ScriptReport {
    ScriptReport {
        phase,
        logs: Vec::new(),
        tests: Vec::new(),
        error: Some(error.to_owned()),
        duration_ms: 0,
    }
}

#[tauri::command]
async fn execute_graphql_profile(
    workspace_state: State<'_, WorkspaceState>,
    vault_state: State<'_, SecretVaultState>,
    cookie_state: State<'_, CookieState>,
    oauth_state: State<'_, OAuthTokenState>,
    input: ProtocolProfileCommandInput,
) -> Result<ResponseSnapshot, String> {
    let prepared = prepare_protocol_profile(
        &workspace_state,
        &vault_state,
        &cookie_state,
        &oauth_state,
        input,
    )
    .await?;
    let ProtocolConfiguration::GraphQl {
        query,
        variables,
        operation_name,
    } = prepared.configuration
    else {
        return Err("the selected protocol profile is not GraphQL".to_owned());
    };
    let variables: serde_json::Value = serde_json::from_str(&variables)
        .map_err(|error| format!("GraphQL variables are not valid JSON: {error}"))?;
    if !variables.is_object() && !variables.is_null() {
        return Err("GraphQL variables must be a JSON object or null".to_owned());
    }
    let payload = serde_json::json!({
        "query": query,
        "variables": variables,
        "operationName": if operation_name.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(operation_name)
        }
    });
    let mut headers = prepared.headers;
    if !headers
        .iter()
        .any(|header| header.enabled && header.name.eq_ignore_ascii_case("accept"))
    {
        headers.push(HeaderEntry {
            name: "Accept".to_owned(),
            value: "application/graphql-response+json, application/json".to_owned(),
            enabled: true,
        });
    }
    let request = RequestDefinition {
        method: "POST".to_owned(),
        url: prepared.url,
        query_params: Vec::new(),
        headers,
        auth: RequestAuth::None,
        body: RequestBody::Text {
            content: serde_json::to_string(&payload)
                .map_err(|_| "GraphQL request could not be encoded".to_owned())?,
            content_type: Some("application/json".to_owned()),
        },
        options: prepared.options,
    };
    HttpExecutor
        .execute_with_context(request, prepared.context)
        .await
        .map_err(|error| redact_text(&error.to_string(), &prepared.redactions))
}

#[tauri::command]
async fn start_websocket_session(
    workspace_state: State<'_, WorkspaceState>,
    vault_state: State<'_, SecretVaultState>,
    cookie_state: State<'_, CookieState>,
    oauth_state: State<'_, OAuthTokenState>,
    protocol_state: State<'_, ProtocolSessionState>,
    input: ProtocolProfileCommandInput,
) -> Result<String, String> {
    let prepared = prepare_protocol_profile(
        &workspace_state,
        &vault_state,
        &cookie_state,
        &oauth_state,
        input,
    )
    .await?;
    let ProtocolConfiguration::WebSocket { subprotocols } = prepared.configuration else {
        return Err("the selected protocol profile is not WebSocket".to_owned());
    };
    let max_message_bytes = prepared
        .options
        .max_response_bytes
        .min(MAX_PROTOCOL_MESSAGE_BYTES as u64) as usize;
    let session = start_websocket(WebSocketInput {
        url: prepared.url,
        headers: prepared.headers,
        subprotocols,
        options: prepared.options,
        context: prepared.context,
        max_message_bytes,
        redactions: prepared.redactions,
    })?;
    protocol_state.insert(session)
}

#[tauri::command]
async fn start_sse_session(
    workspace_state: State<'_, WorkspaceState>,
    vault_state: State<'_, SecretVaultState>,
    cookie_state: State<'_, CookieState>,
    oauth_state: State<'_, OAuthTokenState>,
    protocol_state: State<'_, ProtocolSessionState>,
    input: ProtocolProfileCommandInput,
) -> Result<String, String> {
    let prepared = prepare_protocol_profile(
        &workspace_state,
        &vault_state,
        &cookie_state,
        &oauth_state,
        input,
    )
    .await?;
    let ProtocolConfiguration::ServerSentEvents { last_event_id } = prepared.configuration else {
        return Err("the selected protocol profile is not Server-Sent Events".to_owned());
    };
    let session = start_server_sent_events(ServerSentEventsInput {
        url: prepared.url,
        headers: prepared.headers,
        last_event_id,
        options: prepared.options,
        context: prepared.context,
        redactions: prepared.redactions,
    })?;
    protocol_state.insert(session)
}

#[tauri::command]
fn poll_protocol_session(
    protocol_state: State<'_, ProtocolSessionState>,
    session_id: String,
    maximum: usize,
) -> Result<ProtocolPollResult, String> {
    let session = protocol_state.get(&session_id)?;
    let events = session.drain_events(maximum)?;
    let finished = session.is_finished();
    if finished {
        let _ = protocol_state.remove(&session_id)?;
    }
    Ok(ProtocolPollResult { events, finished })
}

#[tauri::command]
async fn send_websocket_message(
    protocol_state: State<'_, ProtocolSessionState>,
    session_id: String,
    message: WebSocketOutbound,
) -> Result<(), String> {
    protocol_state.get(&session_id)?.send(message).await
}

#[tauri::command]
fn stop_protocol_session(
    protocol_state: State<'_, ProtocolSessionState>,
    session_id: String,
) -> Result<bool, String> {
    let Some(session) = protocol_state.remove(&session_id)? else {
        return Ok(false);
    };
    session.stop();
    Ok(true)
}

#[tauri::command]
async fn reflect_grpc_profile(
    workspace_state: State<'_, WorkspaceState>,
    vault_state: State<'_, SecretVaultState>,
    cookie_state: State<'_, CookieState>,
    oauth_state: State<'_, OAuthTokenState>,
    input: ProtocolProfileCommandInput,
) -> Result<GrpcReflectionResult, String> {
    let prepared = prepare_protocol_profile(
        &workspace_state,
        &vault_state,
        &cookie_state,
        &oauth_state,
        input,
    )
    .await?;
    let ProtocolConfiguration::Grpc {
        service,
        descriptor_set_path,
        use_reflection,
        ..
    } = prepared.configuration
    else {
        return Err("the selected protocol profile is not gRPC".to_owned());
    };
    reflect_grpc_service(GrpcReflectionInput {
        endpoint: prepared.url,
        service,
        metadata: prepared.headers,
        descriptor_set_path,
        use_reflection,
        timeout_ms: prepared.options.timeout_ms,
        context: prepared.context,
        redactions: prepared.redactions,
    })
    .await
}

#[tauri::command]
async fn invoke_grpc_profile(
    workspace_state: State<'_, WorkspaceState>,
    vault_state: State<'_, SecretVaultState>,
    cookie_state: State<'_, CookieState>,
    oauth_state: State<'_, OAuthTokenState>,
    input: ProtocolProfileCommandInput,
) -> Result<GrpcInvokeResult, String> {
    let prepared = prepare_protocol_profile(
        &workspace_state,
        &vault_state,
        &cookie_state,
        &oauth_state,
        input,
    )
    .await?;
    let ProtocolConfiguration::Grpc {
        service,
        method,
        message,
        descriptor_set_path,
        use_reflection,
    } = prepared.configuration
    else {
        return Err("the selected protocol profile is not gRPC".to_owned());
    };
    invoke_grpc_unary(GrpcInvokeInput {
        endpoint: prepared.url,
        service,
        method,
        message,
        metadata: prepared.headers,
        descriptor_set_path,
        use_reflection,
        timeout_ms: prepared.options.timeout_ms,
        max_request_bytes: prepared
            .options
            .max_request_bytes
            .min(MAX_PROTOCOL_MESSAGE_BYTES as u64) as usize,
        max_response_bytes: prepared
            .options
            .max_response_bytes
            .min(MAX_PROTOCOL_MESSAGE_BYTES as u64) as usize,
        context: prepared.context,
        redactions: prepared.redactions,
    })
    .await
}

#[tauri::command]
fn pick_grpc_descriptor_set(app: AppHandle) -> Result<Option<String>, String> {
    app.dialog()
        .file()
        .set_title("Choose a compiled protobuf descriptor set")
        .add_filter(
            "Protobuf descriptor sets",
            &["bin", "desc", "pb", "protoset"],
        )
        .blocking_pick_file()
        .map(|path| {
            path.into_path()
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|error| format!("selected file cannot be used: {error}"))
        })
        .transpose()
}

async fn prepare_protocol_profile(
    workspace_state: &State<'_, WorkspaceState>,
    vault_state: &State<'_, SecretVaultState>,
    cookie_state: &State<'_, CookieState>,
    oauth_state: &State<'_, OAuthTokenState>,
    input: ProtocolProfileCommandInput,
) -> Result<PreparedProtocolProfile, String> {
    validate_protocol_command_profile(&input.profile)?;
    let active = workspace_state
        .active
        .lock()
        .map_err(|_| "workspace state is unavailable".to_owned())?
        .as_ref()
        .cloned()
        .ok_or_else(|| "no workspace is currently open".to_owned())?;
    let environment = input
        .environment_id
        .as_deref()
        .map(|environment_id| {
            active
                .workspace
                .environments
                .iter()
                .find(|environment| environment.id == environment_id)
                .cloned()
                .ok_or_else(|| "the selected environment no longer exists".to_owned())
        })
        .transpose()?;
    let environment_context =
        environment.map(|environment| (active.workspace.id.clone(), environment));
    let (values, _, mut redactions) =
        load_environment(environment_context, vault_state.store).await?;
    let cookie_jar = active
        .workspace
        .network
        .cookie_jar_enabled
        .then(|| cookie_state.jar(&active.workspace.id))
        .transpose()?;
    let context =
        build_http_execution_context(active.workspace.network, &values, cookie_jar).await?;

    let mut auth = input.profile.auth;
    if matches!(auth, RequestAuth::Inherit) {
        auth = active.workspace.auth.unwrap_or_default();
    }
    let request = RequestDefinition {
        method: "GET".to_owned(),
        url: input.profile.url,
        query_params: Vec::new(),
        headers: input.profile.headers,
        auth,
        body: RequestBody::Empty,
        options: RequestOptions::default(),
    };
    let resolution = resolve_request_variables(&request, &values);
    ensure_variables_resolved(
        &resolution.errors,
        &resolution.cycles,
        &resolution.unresolved,
    )?;
    let mut resolved = resolution.request;
    add_request_auth_redactions(&resolved.auth, &mut redactions);
    materialize_oauth_auth(
        &mut resolved,
        &context,
        &active.workspace.id,
        &oauth_state.entries,
        &mut redactions,
    )
    .await?;

    let mut configuration = input.profile.configuration;
    resolve_protocol_configuration(&mut configuration, &values)?;
    let allow_query_auth = !matches!(configuration, ProtocolConfiguration::Grpc { .. });
    apply_protocol_auth(
        &mut resolved.url,
        &mut resolved.headers,
        resolved.auth,
        allow_query_auth,
        &mut redactions,
    )?;
    Ok(PreparedProtocolProfile {
        url: resolved.url,
        headers: resolved.headers,
        configuration,
        options: resolved.options,
        context,
        redactions,
    })
}

fn validate_protocol_command_profile(profile: &ProtocolProfile) -> Result<(), String> {
    if profile.url.len() > 16_384 {
        return Err("protocol profile URL exceeds the 16,384-byte limit".to_owned());
    }
    if profile.headers.len() > 4_096 {
        return Err("protocol profile contains more than 4,096 headers".to_owned());
    }
    if profile.headers.iter().any(|header| {
        header.name.len() > 64 * 1024 || header.value.len() > MAX_PROTOCOL_MESSAGE_BYTES
    }) {
        return Err("protocol profile contains an oversized header".to_owned());
    }
    Ok(())
}

fn ensure_variables_resolved(
    errors: &[String],
    cycles: &[String],
    unresolved: &[String],
) -> Result<(), String> {
    if !errors.is_empty() {
        return Err(format!("Variable expansion failed: {}", errors.join("; ")));
    }
    if !cycles.is_empty() {
        return Err(format!("Variable cycle: {}", cycles.join(", ")));
    }
    if !unresolved.is_empty() {
        return Err(format!(
            "Unresolved variables: {}",
            unresolved
                .iter()
                .map(|key| format!("{{{{{key}}}}}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
}

fn resolve_protocol_configuration(
    configuration: &mut ProtocolConfiguration,
    values: &BTreeMap<String, String>,
) -> Result<(), String> {
    let resolve = |label: &str, value: &mut String| -> Result<(), String> {
        *value = resolve_network_value(label, value, values)?;
        Ok(())
    };
    match configuration {
        ProtocolConfiguration::GraphQl {
            query,
            variables,
            operation_name,
        } => {
            resolve("GraphQL query", query)?;
            resolve("GraphQL variables", variables)?;
            resolve("GraphQL operation name", operation_name)?;
        }
        ProtocolConfiguration::WebSocket { subprotocols } => {
            for protocol in subprotocols {
                resolve("WebSocket subprotocol", protocol)?;
            }
        }
        ProtocolConfiguration::ServerSentEvents { last_event_id } => {
            resolve("SSE last event ID", last_event_id)?;
        }
        ProtocolConfiguration::Grpc {
            service,
            method,
            message,
            descriptor_set_path,
            ..
        } => {
            resolve("gRPC service", service)?;
            resolve("gRPC method", method)?;
            resolve("gRPC message", message)?;
            resolve("gRPC descriptor-set path", descriptor_set_path)?;
        }
    }
    Ok(())
}

fn apply_protocol_auth(
    url: &mut String,
    headers: &mut Vec<HeaderEntry>,
    auth: RequestAuth,
    allow_query_auth: bool,
    redactions: &mut Vec<String>,
) -> Result<(), String> {
    let mut set_header = |name: String, value: String| {
        headers.retain(|header| !header.name.trim().eq_ignore_ascii_case(&name));
        headers.push(HeaderEntry {
            name,
            value,
            enabled: true,
        });
    };
    match auth {
        RequestAuth::None => {}
        RequestAuth::Basic { username, password } => {
            let credentials = BASE64.encode(format!("{username}:{password}"));
            push_redaction(redactions, &credentials);
            set_header("Authorization".to_owned(), format!("Basic {credentials}"));
        }
        RequestAuth::Bearer { token } => {
            set_header(
                "Authorization".to_owned(),
                format!("Bearer {}", token.trim()),
            );
        }
        RequestAuth::ApiKey {
            key,
            value,
            placement: api_client_core::ApiKeyPlacement::Header,
        } => set_header(key.trim().to_owned(), value),
        RequestAuth::ApiKey {
            key,
            value,
            placement: api_client_core::ApiKeyPlacement::Query,
        } => {
            if !allow_query_auth {
                return Err("gRPC does not support query-string API key authentication".to_owned());
            }
            let mut parsed = url::Url::parse(url)
                .map_err(|_| "protocol URL is invalid for API key authentication".to_owned())?;
            parsed.query_pairs_mut().append_pair(key.trim(), &value);
            *url = parsed.into();
        }
        RequestAuth::Inherit => {
            return Err("protocol authentication could not be inherited".to_owned());
        }
        RequestAuth::OAuth2ClientCredentials { .. } => {
            return Err("OAuth 2 token materialization did not complete".to_owned());
        }
    }
    Ok(())
}

#[tauri::command]
fn import_curl(command: String) -> Result<CurlImport, String> {
    api_client_import::import_curl(&command).map_err(|error| error.to_string())
}

#[tauri::command]
async fn import_api_document_file(app: AppHandle) -> Result<Option<ApiDocumentImport>, String> {
    let Some(selected) = app
        .dialog()
        .file()
        .set_title("Import a Postman, OpenAPI, or Swagger document")
        .add_filter("API documents", &["json", "yaml", "yml"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|error| format!("selected file cannot be used: {error}"))?;
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|_| "selected API document could not be inspected".to_owned())?;
    if !metadata.is_file() {
        return Err("selected API document is not a regular file".to_owned());
    }
    if metadata.len() > MAX_API_IMPORT_BYTES {
        return Err(format!(
            "API document exceeds the {} MiB import limit",
            MAX_API_IMPORT_BYTES / (1024 * 1024)
        ));
    }
    let contents = tokio::fs::read(&path)
        .await
        .map_err(|_| "selected API document could not be read".to_owned())?;
    if contents.len() as u64 > MAX_API_IMPORT_BYTES {
        return Err(format!(
            "API document exceeds the {} MiB import limit",
            MAX_API_IMPORT_BYTES / (1024 * 1024)
        ));
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("api-document")
        .to_owned();
    let imported = tokio::task::spawn_blocking(move || {
        api_client_import::import_api_document(&contents, &file_name)
    })
    .await
    .map_err(|_| "API document importer stopped unexpectedly".to_owned())?
    .map_err(|error| error.to_string())?;
    Ok(Some(imported))
}

#[tauri::command]
fn export_curl(request: RequestDefinition, shell: CurlShell) -> Result<String, String> {
    api_client_import::export_curl(&request, shell).map_err(|error| error.to_string())
}

#[tauri::command]
async fn export_curl_for_environment(
    workspace_state: State<'_, WorkspaceState>,
    vault_state: State<'_, SecretVaultState>,
    request: RequestDefinition,
    environment_id: Option<String>,
    shell: CurlShell,
) -> Result<String, String> {
    let environment_context =
        selected_environment_context(&workspace_state, environment_id.as_deref())?;
    let (values, _, _) = load_environment(environment_context, vault_state.store).await?;
    let resolution = resolve_request_variables(&request, &values);
    ensure_variables_resolved(
        &resolution.errors,
        &resolution.cycles,
        &resolution.unresolved,
    )?;
    api_client_import::export_curl(&resolution.request, shell).map_err(|error| error.to_string())
}

#[tauri::command]
async fn export_code_for_environment(
    workspace_state: State<'_, WorkspaceState>,
    vault_state: State<'_, SecretVaultState>,
    request: RequestDefinition,
    environment_id: Option<String>,
    language: CodeLanguage,
) -> Result<String, String> {
    let environment_context =
        selected_environment_context(&workspace_state, environment_id.as_deref())?;
    let (values, _, _) = load_environment(environment_context, vault_state.store).await?;
    let resolution = resolve_request_variables(&request, &values);
    ensure_variables_resolved(
        &resolution.errors,
        &resolution.cycles,
        &resolution.unresolved,
    )?;
    api_client_import::export_code(&resolution.request, language).map_err(|error| error.to_string())
}

#[tauri::command]
async fn execute_request_script(
    input: ScriptExecutionInput,
) -> Result<ScriptExecutionResult, String> {
    tokio::task::spawn_blocking(move || {
        api_client_script::execute_script(input, ScriptLimits::default())
    })
    .await
    .map_err(|_| "script runtime stopped unexpectedly".to_owned())?
    .map_err(|error| error.to_string())
}

#[tauri::command]
fn pick_request_file(app: AppHandle) -> Result<Option<String>, String> {
    app.dialog()
        .file()
        .set_title("Choose a request body file")
        .blocking_pick_file()
        .map(|path| {
            path.into_path()
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|error| format!("selected file cannot be used: {error}"))
        })
        .transpose()
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum NetworkFileKind {
    CaCertificate,
    ClientIdentity,
}

#[tauri::command]
fn pick_network_file(app: AppHandle, kind: NetworkFileKind) -> Result<Option<String>, String> {
    let title = match kind {
        NetworkFileKind::CaCertificate => "Choose a custom CA certificate bundle",
        NetworkFileKind::ClientIdentity => "Choose a combined client identity PEM",
    };
    app.dialog()
        .file()
        .set_title(title)
        .add_filter("PEM certificates", &["pem", "crt", "cer"])
        .blocking_pick_file()
        .map(|path| {
            path.into_path()
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|error| format!("selected file cannot be used: {error}"))
        })
        .transpose()
}

#[tauri::command]
async fn save_response_body(
    app: AppHandle,
    body: ResponseBody,
    suggested_name: String,
) -> Result<Option<String>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .set_title("Save response body")
        .set_file_name(suggested_name)
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|error| format!("selected file cannot be used: {error}"))?;
    let bytes = match body.encoding {
        ResponseBodyEncoding::Utf8 => body.content.into_bytes(),
        ResponseBodyEncoding::Base64 => BASE64
            .decode(body.content)
            .map_err(|_| "binary response data is invalid".to_owned())?,
    };
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|_| "response body could not be written to the selected file".to_owned())?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn export_workspace_backup(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
) -> Result<Option<String>, String> {
    let workspace = {
        let _operation = state.operation_lock.lock().await;
        state
            .active
            .lock()
            .map_err(|_| "workspace state is unavailable".to_owned())?
            .as_ref()
            .map(|active| active.workspace.clone())
            .ok_or_else(|| "Open a workspace before exporting a backup.".to_owned())?
    };
    let Some(path) = app
        .dialog()
        .file()
        .set_title("Save private workspace backup")
        .set_file_name("workspace.apic-backup.json")
        .add_filter("Workspace backup", &["json"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|_| "The selected backup path is unavailable.".to_owned())?;
    tokio::task::spawn_blocking(move || {
        maintenance::write_json_atomically(&path, &maintenance::encode_backup(&workspace)?)?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|_| "Workspace backup export stopped unexpectedly.".to_owned())?
}

#[tauri::command]
async fn restore_workspace_backup(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    protocol_state: State<'_, ProtocolSessionState>,
) -> Result<Option<WorkspaceSession>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .set_title("Choose a workspace backup")
        .add_filter("Workspace backup", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|_| "The selected backup path is unavailable.".to_owned())?;
    let workspace = tokio::task::spawn_blocking(move || maintenance::read_backup(&path))
        .await
        .map_err(|_| "Reading the workspace backup stopped unexpectedly.".to_owned())??;
    let Some(parent) = pick_folder(&app, "Choose a parent folder for the restored workspace")?
    else {
        return Ok(None);
    };
    let operation = Arc::clone(&state.operation_lock).lock_owned().await;
    protocol_state.stop_all()?;
    let active = Arc::clone(&state.active);
    tokio::task::spawn_blocking(move || {
        let _operation = operation;
        let mut current = active
            .lock()
            .map_err(|_| "workspace state is unavailable".to_owned())?;
        let session =
            WorkspaceStore::restore(&parent, &workspace).map_err(|error| error.to_string())?;
        *current = Some(ActiveWorkspace {
            root_path: PathBuf::from(&session.root_path),
            workspace: session.workspace.clone(),
        });
        Ok(Some(session))
    })
    .await
    .map_err(|_| "Workspace restore stopped unexpectedly.".to_owned())?
}

#[tauri::command]
async fn export_support_diagnostics(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
) -> Result<Option<String>, String> {
    let workspace = state
        .active
        .lock()
        .map_err(|_| "workspace state is unavailable".to_owned())?
        .as_ref()
        .map(|active| active.workspace.clone());
    let Some(path) = app
        .dialog()
        .file()
        .set_title("Save privacy-safe support diagnostics")
        .set_file_name("support.apic-diagnostics.json")
        .add_filter("Diagnostics", &["json"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|_| "The selected diagnostics path is unavailable.".to_owned())?;
    tokio::task::spawn_blocking(move || {
        maintenance::write_json_atomically(
            &path,
            &maintenance::encode_diagnostics(workspace.as_ref())?,
        )?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|_| "Support diagnostics export stopped unexpectedly.".to_owned())?
}

#[tauri::command]
async fn create_workspace(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    protocol_state: State<'_, ProtocolSessionState>,
    name: String,
) -> Result<Option<WorkspaceSession>, String> {
    let Some(parent) = pick_folder(&app, "Choose where to create the workspace")? else {
        return Ok(None);
    };

    let _operation = state.operation_lock.lock().await;
    let session = run_blocking(move || WorkspaceStore::create(&parent, &name)).await?;
    protocol_state.stop_all()?;
    set_active_workspace(&state, &session)?;
    Ok(Some(session))
}

#[tauri::command]
async fn reopen_last_workspace(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
) -> Result<Option<WorkspaceSession>, String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let _operation = tokio::time::timeout_at(deadline, state.operation_lock.lock())
        .await
        .map_err(|_| {
            "Reopening the last workspace took too long. Use Open to choose a workspace.".to_owned()
        })?;
    let current = {
        state
            .active
            .lock()
            .map_err(|_| "workspace state is unavailable".to_owned())?
            .as_ref()
            .map(|active| WorkspaceSession {
                root_path: active.root_path.to_string_lossy().into_owned(),
                workspace: active.workspace.clone(),
            })
    };
    if current.is_some() {
        return Ok(current);
    }
    let settings = last_workspace_path(&app)?;
    // This worker only reads files. If it outlives the startup deadline, it cannot
    // switch the workspace after the user has chosen another folder.
    let session = tokio::time::timeout_at(
        deadline,
        tokio::task::spawn_blocking(move || last_workspace::reopen(&settings)),
    )
    .await
    .map_err(|_| {
        "Reopening the last workspace took too long. Use Open to choose a workspace.".to_owned()
    })?
    .map_err(|_| {
        "Could not reopen the last workspace. Use Open to choose a workspace.".to_owned()
    })??;
    if let Some(session) = &session {
        set_active_workspace(&state, session)?;
    }
    Ok(session)
}

#[tauri::command]
async fn remember_open_workspace(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    workspace_id: String,
) -> Result<bool, String> {
    let settings = last_workspace_path(&app)?;
    let operation = Arc::clone(&state.operation_lock).lock_owned().await;
    let active = Arc::clone(&state.active);
    tokio::task::spawn_blocking(move || {
        let _operation = operation;
        let current = active
            .lock()
            .map_err(|_| "workspace state is unavailable".to_owned())?;
        let Some(current) = current
            .as_ref()
            .filter(|current| current.workspace.id == workspace_id)
        else {
            return Ok(false);
        };
        last_workspace::remember(&settings, &current.root_path, &current.workspace.id)?;
        Ok(true)
    })
    .await
    .map_err(|_| "Could not remember the open workspace.".to_owned())?
}

fn last_workspace_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_local_data_dir()
        .map_err(|_| "Local application storage is unavailable.".to_owned())?
        .join("last-workspace.json"))
}

#[tauri::command]
async fn open_workspace(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    protocol_state: State<'_, ProtocolSessionState>,
) -> Result<Option<WorkspaceSession>, String> {
    let Some(root) = pick_folder(&app, "Open a Reqglide workspace")? else {
        return Ok(None);
    };

    let _operation = state.operation_lock.lock().await;
    let session = run_blocking(move || WorkspaceStore::load(&root)).await?;
    protocol_state.stop_all()?;
    set_active_workspace(&state, &session)?;
    Ok(Some(session))
}

#[tauri::command]
async fn save_workspace(
    state: State<'_, WorkspaceState>,
    workspace: WorkspaceDocument,
) -> Result<WorkspaceSession, String> {
    let _operation = state.operation_lock.lock().await;
    let active = state
        .active
        .lock()
        .map_err(|_| "workspace state is unavailable".to_owned())?
        .clone()
        .ok_or_else(|| "no workspace is currently open".to_owned())?;

    if workspace.id != active.workspace.id {
        return Err("workspace identity does not match the open workspace".to_owned());
    }

    let session = run_blocking(move || {
        WorkspaceStore::save(&active.root_path, &active.workspace, &workspace)
    })
    .await?;
    set_active_workspace(&state, &session)?;
    Ok(session)
}

#[tauri::command]
async fn environment_secret_statuses(
    workspace_state: State<'_, WorkspaceState>,
    vault_state: State<'_, SecretVaultState>,
) -> Result<BTreeMap<String, bool>, String> {
    let workspace = workspace_state
        .active
        .lock()
        .map_err(|_| "workspace state is unavailable".to_owned())?
        .as_ref()
        .map(|active| active.workspace.clone())
        .ok_or_else(|| "no workspace is currently open".to_owned())?;
    let store = vault_state.store;
    tokio::task::spawn_blocking(move || {
        let mut statuses = BTreeMap::new();
        for variable in vault_variables(&workspace) {
            statuses.insert(
                variable.id.clone(),
                store
                    .get(&workspace.id, &variable.id)
                    .map_err(|error| error.to_string())?
                    .is_some(),
            );
        }
        Ok(statuses)
    })
    .await
    .map_err(|_| "secret status lookup stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn environment_local_values(
    workspace_state: State<'_, WorkspaceState>,
    vault_state: State<'_, SecretVaultState>,
) -> Result<BTreeMap<String, String>, String> {
    let workspace = workspace_state
        .active
        .lock()
        .map_err(|_| "workspace state is unavailable".to_owned())?
        .as_ref()
        .map(|active| active.workspace.clone())
        .ok_or_else(|| "no workspace is currently open".to_owned())?;
    let store = vault_state.store;
    tokio::task::spawn_blocking(move || {
        let mut values = BTreeMap::new();
        for variable in workspace
            .environments
            .iter()
            .flat_map(|environment| environment.variables.iter())
            .filter(|variable| variable.local)
        {
            if let Some(value) = store
                .get(&workspace.id, &variable.id)
                .map_err(|error| error.to_string())?
            {
                values.insert(variable.id.clone(), value);
            }
        }
        Ok(values)
    })
    .await
    .map_err(|_| "local variable lookup stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn save_workspace_with_secrets(
    workspace_state: State<'_, WorkspaceState>,
    vault_state: State<'_, SecretVaultState>,
    workspace: WorkspaceDocument,
    mutations: Vec<SecretMutationInput>,
) -> Result<WorkspaceSession, String> {
    let operation = Arc::clone(&workspace_state.operation_lock)
        .lock_owned()
        .await;
    let active_state = Arc::clone(&workspace_state.active);
    let store = vault_state.store;
    tokio::task::spawn_blocking(move || {
        // Cancellation of the waiting command must not release serialization
        // while this worker is still changing the vault or workspace on disk.
        let _operation = operation;
        let mut state = active_state
            .lock()
            .map_err(|_| "workspace state is unavailable".to_owned())?;
        let active = state
            .as_ref()
            .ok_or_else(|| "no workspace is currently open".to_owned())?;
        let session = save_workspace_secret_transaction(&store, active, workspace, mutations)?;
        *state = Some(ActiveWorkspace {
            root_path: PathBuf::from(&session.root_path),
            workspace: session.workspace.clone(),
        });
        Ok(session)
    })
    .await
    .map_err(|_| "workspace and secret save stopped unexpectedly".to_owned())?
}

fn save_workspace_secret_transaction(
    store: &dyn SecretStore,
    active: &ActiveWorkspace,
    workspace: WorkspaceDocument,
    mutations: Vec<SecretMutationInput>,
) -> Result<WorkspaceSession, String> {
    if workspace.id != active.workspace.id {
        return Err("workspace identity does not match the open workspace".to_owned());
    }
    workspace.validate().map_err(|error| error.to_string())?;
    let secret_ids = vault_variables(&workspace)
        .map(|variable| variable.id.as_str())
        .collect::<HashSet<_>>();
    let previous_ids = vault_variables(&active.workspace)
        .map(|variable| variable.id.as_str())
        .collect::<HashSet<_>>();
    let mut mutation_ids = HashSet::new();
    let changes = mutations
        .into_iter()
        .map(|mutation| {
            if !mutation_ids.insert(mutation.variable_id.clone()) {
                return Err("a secret variable was updated more than once".to_owned());
            }
            let allowed = if mutation.value.is_some() {
                secret_ids.contains(mutation.variable_id.as_str())
            } else {
                previous_ids.contains(mutation.variable_id.as_str())
            };
            if !allowed {
                return Err(
                    "credential changes must refer to protected variables in this workspace"
                        .to_owned(),
                );
            }
            Ok(SecretChange {
                variable_id: mutation.variable_id,
                value: mutation.value,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    transact_changes(store, &workspace.id, &changes, || {
        for variable in vault_variables(&workspace) {
            if store
                .get(&workspace.id, &variable.id)
                .map_err(|error| error.to_string())?
                .is_none()
            {
                return Err(format!(
                    "secret variable '{}' has no value in the operating system credential store",
                    variable.key
                ));
            }
        }
        WorkspaceStore::save(&active.root_path, &active.workspace, &workspace)
            .map_err(|error| error.to_string())
    })
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn close_workspace(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    protocol_state: State<'_, ProtocolSessionState>,
) -> Result<(), String> {
    let settings = last_workspace_path(&app)?;
    let operation = Arc::clone(&state.operation_lock).lock_owned().await;
    protocol_state.stop_all()?;
    let active = Arc::clone(&state.active);
    tokio::task::spawn_blocking(move || {
        let _operation = operation;
        let mut current = active
            .lock()
            .map_err(|_| "workspace state is unavailable".to_owned())?;
        last_workspace::forget(&settings)?;
        *current = None;
        Ok(())
    })
    .await
    .map_err(|_| "Could not close the workspace.".to_owned())?
}

#[tauri::command]
async fn save_recovery_snapshot(
    app: AppHandle,
    state: State<'_, RecoveryState>,
    workspace_id: String,
    snapshot: RecoverySnapshotDocument,
) -> Result<(), String> {
    let _operation = state.operation_lock.lock().await;
    let path = recovery_path(&app, &workspace_id)?;
    let payload = validate_recovery_snapshot(&workspace_id, &snapshot)?;
    tokio::task::spawn_blocking(move || {
        let parent = path
            .parent()
            .ok_or_else(|| "recovery directory is unavailable".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create recovery directory: {error}"))?;
        let mut file = AtomicWriteFile::options()
            .open(&path)
            .map_err(|error| format!("could not prepare recovery snapshot: {error}"))?;
        file.write_all(&payload)
            .map_err(|error| format!("could not write recovery snapshot: {error}"))?;
        file.commit()
            .map_err(|error| format!("could not commit recovery snapshot: {error}"))
    })
    .await
    .map_err(|_| "recovery save stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn load_recovery_snapshot(
    app: AppHandle,
    state: State<'_, RecoveryState>,
    workspace_id: String,
) -> Result<Option<RecoverySnapshotDocument>, String> {
    let _operation = state.operation_lock.lock().await;
    let path = recovery_path(&app, &workspace_id)?;
    tokio::task::spawn_blocking(move || {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("could not read recovery snapshot: {error}")),
        };
        if bytes.len() > MAX_RECOVERY_BYTES {
            return Err("recovery snapshot exceeds the 8 MiB safety limit".to_owned());
        }
        let snapshot: RecoverySnapshotDocument = serde_json::from_slice(&bytes)
            .map_err(|error| format!("recovery snapshot is invalid JSON: {error}"))?;
        validate_recovery_snapshot(&workspace_id, &snapshot)?;
        Ok(Some(snapshot))
    })
    .await
    .map_err(|_| "recovery load stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn clear_recovery_snapshot(
    app: AppHandle,
    state: State<'_, RecoveryState>,
    workspace_id: String,
) -> Result<(), String> {
    let _operation = state.operation_lock.lock().await;
    let path = recovery_path(&app, &workspace_id)?;
    tokio::task::spawn_blocking(move || match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not clear recovery snapshot: {error}")),
    })
    .await
    .map_err(|_| "recovery cleanup stopped unexpectedly".to_owned())?
}

async fn persist_collection_run_report(
    app: &AppHandle,
    report: &CollectionRunReport,
) -> Result<(), String> {
    let path = collection_run_report_path(app, &report.workspace_id, &report.id)?;
    let payload = serde_json::to_vec_pretty(report)
        .map_err(|_| "collection run report could not be serialized".to_owned())?;
    if payload.len() > MAX_RUN_REPORT_BYTES {
        return Err("collection run report exceeds the 16 MiB limit".to_owned());
    }
    tokio::task::spawn_blocking(move || {
        let parent = path
            .parent()
            .ok_or_else(|| "collection run report path is invalid".to_owned())?;
        fs::create_dir_all(parent).map_err(|error| {
            format!("could not create collection run report directory: {error}")
        })?;
        let mut file = AtomicWriteFile::options()
            .open(&path)
            .map_err(|error| format!("could not prepare collection run report: {error}"))?;
        file.write_all(&payload)
            .map_err(|error| format!("could not write collection run report: {error}"))?;
        file.commit()
            .map_err(|error| format!("could not commit collection run report: {error}"))
    })
    .await
    .map_err(|_| "collection run report save stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn list_collection_run_reports(
    app: AppHandle,
    workspace_state: State<'_, WorkspaceState>,
    report_state: State<'_, RunReportState>,
    workspace_id: String,
) -> Result<Vec<CollectionRunReportSummary>, String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    let _operation = report_state.operation_lock.lock().await;
    let directory = collection_run_report_directory(&app, &workspace_id)?;
    tokio::task::spawn_blocking(move || {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("could not list collection run reports: {error}")),
        };
        let mut summaries = Vec::new();
        for entry in entries.take(1_000) {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_file() || metadata.len() > MAX_RUN_REPORT_BYTES as u64 {
                continue;
            }
            let Ok(bytes) = fs::read(path) else { continue };
            let Ok(report) = serde_json::from_slice::<CollectionRunReport>(&bytes) else {
                continue;
            };
            if report.schema_version != 1 || report.workspace_id != workspace_id {
                continue;
            }
            summaries.push(CollectionRunReportSummary::from(&report));
        }
        summaries.sort_by_key(|report| std::cmp::Reverse(report.started_at_ms));
        summaries.truncate(100);
        Ok(summaries)
    })
    .await
    .map_err(|_| "collection run report lookup stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn load_collection_run_report(
    app: AppHandle,
    workspace_state: State<'_, WorkspaceState>,
    report_state: State<'_, RunReportState>,
    workspace_id: String,
    report_id: String,
) -> Result<CollectionRunReport, String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    let _operation = report_state.operation_lock.lock().await;
    let path = collection_run_report_path(&app, &workspace_id, &report_id)?;
    tokio::task::spawn_blocking(move || {
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("could not read collection run report: {error}"))?;
        if !metadata.is_file() || metadata.len() > MAX_RUN_REPORT_BYTES as u64 {
            return Err("collection run report exceeds its safety limit".to_owned());
        }
        let bytes = fs::read(path)
            .map_err(|error| format!("could not read collection run report: {error}"))?;
        let report: CollectionRunReport = serde_json::from_slice(&bytes)
            .map_err(|_| "collection run report is invalid".to_owned())?;
        if report.schema_version != 1 || report.workspace_id != workspace_id {
            return Err("collection run report does not belong to this workspace".to_owned());
        }
        Ok(report)
    })
    .await
    .map_err(|_| "collection run report load stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn delete_collection_run_report(
    app: AppHandle,
    workspace_state: State<'_, WorkspaceState>,
    report_state: State<'_, RunReportState>,
    workspace_id: String,
    report_id: String,
) -> Result<(), String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    let _operation = report_state.operation_lock.lock().await;
    let path = collection_run_report_path(&app, &workspace_id, &report_id)?;
    tokio::task::spawn_blocking(move || {
        fs::remove_file(path)
            .map_err(|error| format!("could not delete collection run report: {error}"))
    })
    .await
    .map_err(|_| "collection run report deletion stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn export_collection_run_report(
    app: AppHandle,
    workspace_state: State<'_, WorkspaceState>,
    report_state: State<'_, RunReportState>,
    workspace_id: String,
    report_id: String,
    format: CollectionRunExportFormat,
) -> Result<Option<String>, String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    let report = {
        let _operation = report_state.operation_lock.lock().await;
        let path = collection_run_report_path(&app, &workspace_id, &report_id)?;
        tokio::task::spawn_blocking(move || read_collection_run_report(&path, &workspace_id))
            .await
            .map_err(|_| "collection run report load stopped unexpectedly".to_owned())??
    };
    let (payload, extension, filter_name) = match format {
        CollectionRunExportFormat::Json => (
            serde_json::to_vec_pretty(&report)
                .map_err(|_| "collection run report could not be serialized".to_owned())?,
            "json",
            "JSON report",
        ),
        CollectionRunExportFormat::Junit => (
            collection_run_junit(&report).into_bytes(),
            "xml",
            "JUnit XML report",
        ),
    };
    let suggested_name = format!(
        "{}-{}.{}",
        safe_report_file_name(&report.target_name),
        report.started_at_ms,
        extension
    );
    let Some(path) = app
        .dialog()
        .file()
        .set_title("Export collection run report")
        .set_file_name(suggested_name)
        .add_filter(filter_name, &[extension])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|error| format!("selected file cannot be used: {error}"))?;
    tokio::fs::write(&path, payload)
        .await
        .map_err(|error| format!("collection run report could not be exported: {error}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

impl From<&CollectionRunReport> for CollectionRunReportSummary {
    fn from(report: &CollectionRunReport) -> Self {
        Self {
            id: report.id.clone(),
            target_name: report.target_name.clone(),
            environment_name: report.environment_name.clone(),
            started_at_ms: report.started_at_ms,
            duration_ms: report.duration_ms,
            requested_iterations: report.requested_iterations,
            completed_iterations: report.completed_iterations,
            summary: report.summary.clone(),
        }
    }
}

fn read_collection_run_report(
    path: &std::path::Path,
    workspace_id: &str,
) -> Result<CollectionRunReport, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("could not read collection run report: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_RUN_REPORT_BYTES as u64 {
        return Err("collection run report exceeds its safety limit".to_owned());
    }
    let bytes =
        fs::read(path).map_err(|error| format!("could not read collection run report: {error}"))?;
    let report: CollectionRunReport = serde_json::from_slice(&bytes)
        .map_err(|_| "collection run report is invalid".to_owned())?;
    if report.schema_version != 1 || report.workspace_id != workspace_id {
        return Err("collection run report does not belong to this workspace".to_owned());
    }
    Ok(report)
}

fn collection_run_junit(report: &CollectionRunReport) -> String {
    let failures = report
        .results
        .iter()
        .filter(|result| result.error.is_some() || result.tests_failed > 0)
        .count();
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" time=\"{:.3}\">\n",
        escape_xml(&report.target_name),
        report.results.len(),
        failures,
        report.duration_ms as f64 / 1000.0
    );
    for result in &report.results {
        xml.push_str(&format!(
            "  <testcase classname=\"iteration.{}\" name=\"{}\" time=\"{:.3}\">",
            result.iteration,
            escape_xml(&result.request_name),
            result.duration_ms as f64 / 1000.0
        ));
        if result.error.is_some() || result.tests_failed > 0 {
            let failed_tests = result
                .script_runs
                .iter()
                .flat_map(|run| &run.tests)
                .filter(|test| !test.passed)
                .map(|test| test.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let message = result
                .error
                .as_deref()
                .or((!failed_tests.is_empty()).then_some(failed_tests.as_str()))
                .unwrap_or("request failed");
            xml.push_str(&format!(
                "<failure message=\"{}\">{}</failure>",
                escape_xml(message),
                escape_xml(message)
            ));
        }
        xml.push_str("</testcase>\n");
    }
    xml.push_str("</testsuite>\n");
    xml
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn safe_report_file_name(value: &str) -> String {
    let name = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let name = name.trim_matches('-');
    if name.is_empty() {
        "collection-run".to_owned()
    } else {
        name.chars().take(80).collect()
    }
}

#[tauri::command]
async fn record_history(
    app: AppHandle,
    workspace_state: State<'_, WorkspaceState>,
    history_state: State<'_, HistoryState>,
    input: HistoryRecordInput,
) -> Result<HistoryEntry, String> {
    ensure_active_workspace(&workspace_state, &input.workspace_id)?;
    let _operation = history_state.operation_lock.lock().await;
    let path = history_database_path(&app)?;
    tokio::task::spawn_blocking(move || {
        HistoryStore::open(path)
            .and_then(|store| store.record(input))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "history recording stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn list_history(
    app: AppHandle,
    workspace_state: State<'_, WorkspaceState>,
    history_state: State<'_, HistoryState>,
    query: HistoryQuery,
) -> Result<Vec<HistoryEntry>, String> {
    ensure_active_workspace(&workspace_state, &query.workspace_id)?;
    let _operation = history_state.operation_lock.lock().await;
    let path = history_database_path(&app)?;
    tokio::task::spawn_blocking(move || {
        HistoryStore::open(path)
            .and_then(|store| store.list(&query))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "history lookup stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn set_history_pinned(
    app: AppHandle,
    workspace_state: State<'_, WorkspaceState>,
    history_state: State<'_, HistoryState>,
    workspace_id: String,
    entry_id: String,
    pinned: bool,
) -> Result<(), String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    let _operation = history_state.operation_lock.lock().await;
    let path = history_database_path(&app)?;
    tokio::task::spawn_blocking(move || {
        HistoryStore::open(path)
            .and_then(|store| store.set_pinned(&workspace_id, &entry_id, pinned))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "history update stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn delete_history_entry(
    app: AppHandle,
    workspace_state: State<'_, WorkspaceState>,
    history_state: State<'_, HistoryState>,
    workspace_id: String,
    entry_id: String,
) -> Result<(), String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    let _operation = history_state.operation_lock.lock().await;
    let path = history_database_path(&app)?;
    tokio::task::spawn_blocking(move || {
        HistoryStore::open(path)
            .and_then(|store| store.delete(&workspace_id, &entry_id))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "history deletion stopped unexpectedly".to_owned())?
}

#[tauri::command]
async fn clear_history(
    app: AppHandle,
    workspace_state: State<'_, WorkspaceState>,
    history_state: State<'_, HistoryState>,
    workspace_id: String,
    include_pinned: bool,
) -> Result<usize, String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    let _operation = history_state.operation_lock.lock().await;
    let path = history_database_path(&app)?;
    tokio::task::spawn_blocking(move || {
        HistoryStore::open(path)
            .and_then(|store| store.clear(&workspace_id, include_pinned))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "history cleanup stopped unexpectedly".to_owned())?
}

#[tauri::command]
fn list_workspace_cookies(
    workspace_state: State<'_, WorkspaceState>,
    cookie_state: State<'_, CookieState>,
    workspace_id: String,
) -> Result<Vec<CookieSnapshot>, String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    cookie_state
        .jar(&workspace_id)?
        .list()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn add_workspace_cookie(
    workspace_state: State<'_, WorkspaceState>,
    cookie_state: State<'_, CookieState>,
    workspace_id: String,
    url: String,
    set_cookie: String,
) -> Result<(), String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    if set_cookie.len() > 16 * 1024 {
        return Err("cookie input exceeds the 16 KiB safety limit".to_owned());
    }
    let url = url::Url::parse(&url).map_err(|error| format!("cookie URL is invalid: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("cookies can only be added for HTTP or HTTPS URLs".to_owned());
    }
    cookie_state
        .jar(&workspace_id)?
        .add(&set_cookie, &url)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_workspace_cookie(
    workspace_state: State<'_, WorkspaceState>,
    cookie_state: State<'_, CookieState>,
    workspace_id: String,
    domain: String,
    path: String,
    name: String,
) -> Result<bool, String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    cookie_state
        .jar(&workspace_id)?
        .remove(&domain, &path, &name)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn clear_workspace_cookies(
    workspace_state: State<'_, WorkspaceState>,
    cookie_state: State<'_, CookieState>,
    workspace_id: String,
) -> Result<(), String> {
    ensure_active_workspace(&workspace_state, &workspace_id)?;
    cookie_state
        .jar(&workspace_id)?
        .clear()
        .map_err(|error| error.to_string())
}

fn history_database_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|path| path.join("history.sqlite3"))
        .map_err(|error| format!("application data directory is unavailable: {error}"))
}

fn collection_run_report_directory(app: &AppHandle, workspace_id: &str) -> Result<PathBuf, String> {
    let workspace_id = Uuid::parse_str(workspace_id)
        .map_err(|_| "collection run workspace ID is not a valid UUID".to_owned())?;
    app.path()
        .app_local_data_dir()
        .map(|path| path.join("collection-runs").join(workspace_id.to_string()))
        .map_err(|error| format!("application data directory is unavailable: {error}"))
}

fn collection_run_report_path(
    app: &AppHandle,
    workspace_id: &str,
    report_id: &str,
) -> Result<PathBuf, String> {
    let report_id = Uuid::parse_str(report_id)
        .map_err(|_| "collection run report ID is not a valid UUID".to_owned())?;
    Ok(collection_run_report_directory(app, workspace_id)?.join(format!("{report_id}.json")))
}

fn ensure_active_workspace(
    state: &State<'_, WorkspaceState>,
    workspace_id: &str,
) -> Result<(), String> {
    let matches = state
        .active
        .lock()
        .map_err(|_| "workspace state is unavailable".to_owned())?
        .as_ref()
        .is_some_and(|active| active.workspace.id == workspace_id);
    if matches {
        Ok(())
    } else {
        Err("history operation does not match the open workspace".to_owned())
    }
}

type ActiveRequestContext = (
    Option<String>,
    NetworkSettings,
    Option<(String, Environment)>,
    Option<RequestAuth>,
);

fn active_request_context(
    state: &State<'_, WorkspaceState>,
    environment_id: Option<&str>,
    request_id: Option<&str>,
) -> Result<ActiveRequestContext, String> {
    let active = state
        .active
        .lock()
        .map_err(|_| "workspace state is unavailable".to_owned())?
        .as_ref()
        .cloned();
    let Some(active) = active else {
        if environment_id.is_some() {
            return Err("no workspace is currently open".to_owned());
        }
        if request_id.is_some() {
            return Err("no workspace is currently open".to_owned());
        }
        return Ok((None, NetworkSettings::default(), None, None));
    };

    let workspace_id = active.workspace.id.clone();
    let environment = match environment_id {
        Some(environment_id) => Some(
            active
                .workspace
                .environments
                .iter()
                .find(|environment| environment.id == environment_id)
                .cloned()
                .ok_or_else(|| "the selected environment no longer exists".to_owned())?,
        ),
        None => None,
    };
    let environment_context = environment.map(|environment| (workspace_id.clone(), environment));
    let inherited_auth = request_id
        .map(|request_id| inherited_auth_for_request(&active.workspace, request_id))
        .transpose()?;
    Ok((
        Some(workspace_id),
        active.workspace.network,
        environment_context,
        inherited_auth,
    ))
}

fn inherited_auth_for_request(
    workspace: &WorkspaceDocument,
    request_id: &str,
) -> Result<RequestAuth, String> {
    let workspace_auth = workspace.auth.clone().unwrap_or_default();
    for collection in &workspace.collections {
        let collection_auth = collection
            .auth
            .clone()
            .unwrap_or_else(|| workspace_auth.clone());
        if collection
            .requests
            .iter()
            .any(|request| request.id == request_id)
        {
            return Ok(collection_auth);
        }
        for folder in &collection.folders {
            if let Some(auth) = inherited_auth_in_folder(folder, request_id, &collection_auth) {
                return Ok(auth);
            }
        }
    }
    Err("the selected request no longer exists in the open workspace".to_owned())
}

fn inherited_auth_in_folder(
    folder: &api_client_workspace::RequestFolder,
    request_id: &str,
    parent_auth: &RequestAuth,
) -> Option<RequestAuth> {
    let folder_auth = folder.auth.clone().unwrap_or_else(|| parent_auth.clone());
    if folder
        .requests
        .iter()
        .any(|request| request.id == request_id)
    {
        return Some(folder_auth);
    }
    folder
        .folders
        .iter()
        .find_map(|child| inherited_auth_in_folder(child, request_id, &folder_auth))
}

fn recovery_path(app: &AppHandle, workspace_id: &str) -> Result<PathBuf, String> {
    let workspace_id = Uuid::parse_str(workspace_id)
        .map_err(|_| "recovery workspace ID is not a valid UUID".to_owned())?;
    let root = app
        .path()
        .app_local_data_dir()
        .map_err(|error| format!("application data directory is unavailable: {error}"))?;
    Ok(root.join("recovery").join(format!("{workspace_id}.json")))
}

fn validate_recovery_snapshot(
    workspace_id: &str,
    snapshot: &RecoverySnapshotDocument,
) -> Result<Vec<u8>, String> {
    let expected_id = Uuid::parse_str(workspace_id)
        .map_err(|_| "recovery workspace ID is not a valid UUID".to_owned())?;
    let snapshot_id = Uuid::parse_str(&snapshot.workspace_id)
        .map_err(|_| "recovery snapshot has no valid workspace ID".to_owned())?;
    if snapshot_id != expected_id {
        return Err("recovery snapshot belongs to a different workspace".to_owned());
    }
    if snapshot.schema_version != 1 {
        return Err("recovery snapshot schema is not supported".to_owned());
    }
    if snapshot.tabs.is_empty() || snapshot.tabs.len() > MAX_RECOVERY_TABS {
        return Err(format!(
            "recovery snapshot must contain between 1 and {MAX_RECOVERY_TABS} tabs"
        ));
    }
    let mut tab_ids = HashSet::new();
    for tab in &snapshot.tabs {
        Uuid::parse_str(&tab.id)
            .map_err(|_| "recovery snapshot contains an invalid tab ID".to_owned())?;
        if !tab_ids.insert(tab.id.as_str()) {
            return Err("recovery snapshot contains duplicate tab IDs".to_owned());
        }
        if let Some(request_id) = &tab.request_id {
            Uuid::parse_str(request_id)
                .map_err(|_| "recovery snapshot contains an invalid saved request ID".to_owned())?;
        }
        if tab.title.trim().is_empty() || tab.title.chars().count() > 200 {
            return Err("recovery snapshot contains an invalid tab title".to_owned());
        }
        // Recovery snapshots are a best-effort local draft, saved continuously while the user
        // is still mid-edit (e.g. just switched auth type before typing a token), so they must
        // not enforce full request business-rule validation the way Send/Save do.
        if !matches!(
            tab.request_tab.as_str(),
            "params" | "auth" | "headers" | "body" | "scripts" | "settings"
        ) || !matches!(
            tab.response_tab.as_str(),
            "body" | "headers" | "tests" | "console"
        ) || !matches!(tab.script_tab.as_str(), "preRequest" | "postResponse")
        {
            return Err("recovery snapshot contains an invalid editor tab".to_owned());
        }
    }
    if !tab_ids.contains(snapshot.active_tab_id.as_str()) {
        return Err("recovery snapshot active tab does not exist".to_owned());
    }
    if let Some(environment_id) = &snapshot.active_environment_id {
        Uuid::parse_str(environment_id)
            .map_err(|_| "recovery snapshot contains an invalid environment ID".to_owned())?;
    }
    let payload = serde_json::to_vec(snapshot)
        .map_err(|error| format!("recovery snapshot could not be encoded: {error}"))?;
    if payload.len() > MAX_RECOVERY_BYTES {
        return Err("recovery snapshot exceeds the 8 MiB safety limit".to_owned());
    }
    Ok(payload)
}

fn pick_folder(app: &AppHandle, title: &str) -> Result<Option<PathBuf>, String> {
    app.dialog()
        .file()
        .set_title(title)
        .set_can_create_directories(true)
        .blocking_pick_folder()
        .map(|path| {
            path.into_path()
                .map_err(|error| format!("selected folder cannot be used: {error}"))
        })
        .transpose()
}

fn vault_variables(workspace: &WorkspaceDocument) -> impl Iterator<Item = &EnvironmentVariable> {
    workspace
        .environments
        .iter()
        .flat_map(|environment| environment.variables.iter())
        .filter(|variable| variable.secret || variable.local)
}

fn selected_environment_context(
    state: &State<'_, WorkspaceState>,
    environment_id: Option<&str>,
) -> Result<Option<(String, Environment)>, String> {
    let Some(environment_id) = environment_id else {
        return Ok(None);
    };
    let active = state
        .active
        .lock()
        .map_err(|_| "workspace state is unavailable".to_owned())?
        .as_ref()
        .cloned()
        .ok_or_else(|| "no workspace is currently open".to_owned())?;
    let environment = active
        .workspace
        .environments
        .iter()
        .find(|environment| environment.id == environment_id)
        .cloned()
        .ok_or_else(|| "the selected environment no longer exists".to_owned())?;
    Ok(Some((active.workspace.id, environment)))
}

fn set_active_workspace(
    state: &State<'_, WorkspaceState>,
    session: &WorkspaceSession,
) -> Result<(), String> {
    *state
        .active
        .lock()
        .map_err(|_| "workspace state is unavailable".to_owned())? = Some(ActiveWorkspace {
        root_path: PathBuf::from(&session.root_path),
        workspace: session.workspace.clone(),
    });
    Ok(())
}

async fn run_blocking<T, F>(operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, api_client_workspace::WorkspaceError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| format!("workspace operation stopped unexpectedly: {error}"))?
        .map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(WorkspaceState::default())
        .manage(RequestExecutionState::default())
        .manage(SecretVaultState::default())
        .manage(RecoveryState::default())
        .manage(HistoryState::default())
        .manage(RunReportState::default())
        .manage(CookieState::default())
        .manage(OAuthTokenState::default())
        .manage(ProtocolSessionState::default())
        .invoke_handler(tauri::generate_handler![
            execute_http_request,
            cancel_http_request,
            execute_request_lifecycle,
            run_collection,
            execute_graphql_profile,
            start_websocket_session,
            start_sse_session,
            poll_protocol_session,
            send_websocket_message,
            stop_protocol_session,
            reflect_grpc_profile,
            invoke_grpc_profile,
            import_curl,
            import_api_document_file,
            export_curl,
            export_curl_for_environment,
            export_code_for_environment,
            execute_request_script,
            pick_request_file,
            pick_network_file,
            pick_grpc_descriptor_set,
            save_response_body,
            export_workspace_backup,
            restore_workspace_backup,
            export_support_diagnostics,
            create_workspace,
            open_workspace,
            reopen_last_workspace,
            remember_open_workspace,
            save_workspace,
            environment_secret_statuses,
            environment_local_values,
            save_workspace_with_secrets,
            close_workspace,
            save_recovery_snapshot,
            load_recovery_snapshot,
            clear_recovery_snapshot,
            record_history,
            list_history,
            set_history_pinned,
            delete_history_entry,
            clear_history,
            list_collection_run_reports,
            load_collection_run_report,
            delete_collection_run_report,
            export_collection_run_report,
            list_workspace_cookies,
            add_workspace_cookie,
            delete_workspace_cookie,
            clear_workspace_cookies
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Reqglide");
}

#[cfg(test)]
mod script_environment_tests;

#[cfg(test)]
mod tests {
    use super::{
        OAuthClientCredentials, RecoverySnapshotDocument, add_request_auth_redactions,
        apply_protocol_auth, inherited_auth_for_request, redact_text,
        request_client_credentials_token, resolve_network_value, sort_redactions,
        validate_recovery_snapshot,
    };
    use api_client_core::{
        ApiKeyPlacement, HeaderEntry, OAuth2ClientAuth, RequestAuth, RequestBody,
        RequestDefinition, RequestOptions,
    };
    use api_client_http::{HttpExecutionContext, ProxyConfiguration};
    use api_client_workspace::{RequestFolder, SavedRequest, WorkspaceDocument};
    use serde_json::json;
    use std::collections::BTreeMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn redacts_secret_values_longest_first() {
        let mut redactions = vec!["token".to_owned(), "token-with-suffix".to_owned()];
        sort_redactions(&mut redactions);

        assert_eq!(
            redact_text("token-with-suffix and token", &redactions),
            "[REDACTED] and [REDACTED]"
        );
    }

    #[test]
    fn ignores_an_empty_redaction_set() {
        assert_eq!(redact_text("safe", &[]), "safe");
    }

    #[test]
    fn applies_protocol_bearer_auth_trimmed_and_replaces_manual_header() {
        let mut url = "wss://example.com/socket".to_owned();
        let mut headers = vec![HeaderEntry {
            name: "Authorization".to_owned(),
            value: "stale-value".to_owned(),
            enabled: true,
        }];
        let mut redactions = Vec::new();

        apply_protocol_auth(
            &mut url,
            &mut headers,
            RequestAuth::Bearer {
                token: "  eyJhbGciOiJIUzI1NiJ9.payload.sig \n".to_owned(),
            },
            false,
            &mut redactions,
        )
        .unwrap();

        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name, "Authorization");
        assert_eq!(headers[0].value, "Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig");
    }

    #[test]
    fn redacts_both_raw_and_trimmed_bearer_token_forms() {
        let mut redactions = Vec::new();
        add_request_auth_redactions(
            &RequestAuth::Bearer {
                token: "  secret-token \n".to_owned(),
            },
            &mut redactions,
        );

        assert_eq!(
            redact_text(
                "field: '  secret-token \n', header: 'Bearer secret-token'",
                &redactions
            ),
            "field: '[REDACTED]', header: 'Bearer [REDACTED]'"
        );
    }

    #[test]
    fn resolves_network_values_without_exposing_secret_contents() {
        let values = BTreeMap::from([(
            "proxyPassword".to_owned(),
            "correct-horse-battery-staple".to_owned(),
        )]);
        assert_eq!(
            resolve_network_value("proxy password", "{{proxyPassword}}", &values).unwrap(),
            "correct-horse-battery-staple"
        );

        let error =
            resolve_network_value("proxy password", "{{missingSecret}}", &values).unwrap_err();
        assert!(error.contains("{{missingSecret}}"));
        assert!(!error.contains("correct-horse-battery-staple"));
    }

    #[test]
    fn resolves_the_nearest_parent_authentication_policy() {
        let mut workspace = WorkspaceDocument::new("Inheritance");
        workspace.auth = Some(RequestAuth::Bearer {
            token: "workspace-token".to_owned(),
        });
        workspace.collections[0].auth = Some(RequestAuth::Basic {
            username: "collection-user".to_owned(),
            password: "collection-password".to_owned(),
        });
        let mut folder = RequestFolder::new("Protected");
        folder.auth = Some(RequestAuth::ApiKey {
            key: "X-API-Key".to_owned(),
            value: "folder-key".to_owned(),
            placement: ApiKeyPlacement::Header,
        });
        let request = SavedRequest::new(
            "Inherited request",
            RequestDefinition {
                method: "GET".to_owned(),
                url: "https://example.com".to_owned(),
                query_params: Vec::new(),
                headers: Vec::new(),
                auth: RequestAuth::Inherit,
                body: RequestBody::Empty,
                options: RequestOptions::default(),
            },
        );
        let request_id = request.id.clone();
        folder.requests.push(request);
        workspace.collections[0].folders.push(folder);

        assert!(matches!(
            inherited_auth_for_request(&workspace, &request_id).unwrap(),
            RequestAuth::ApiKey { ref value, .. } if value == "folder-key"
        ));

        workspace.collections[0].folders[0].auth = None;
        assert!(matches!(
            inherited_auth_for_request(&workspace, &request_id).unwrap(),
            RequestAuth::Basic { ref username, .. } if username == "collection-user"
        ));
    }

    #[tokio::test]
    async fn acquires_an_oauth_client_credentials_token_natively() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0u8; 2048];
                let read = stream.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length: ")
                            .or_else(|| line.strip_prefix("Content-Length: "))
                    })
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            let body = r#"{"access_token":"native-token","token_type":"Bearer","expires_in":3600}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });

        let credentials = OAuthClientCredentials {
            issuer_url: String::new(),
            token_url: format!("http://{address}/token"),
            client_id: "desktop-client".to_owned(),
            client_secret: "local-secret".to_owned(),
            scope: "read write".to_owned(),
            audience: "payments".to_owned(),
            client_auth: OAuth2ClientAuth::Basic,
            extra_parameters: Vec::new(),
        };
        let token = request_client_credentials_token(
            &credentials.token_url,
            &credentials,
            HttpExecutionContext {
                proxy: ProxyConfiguration::Disabled,
                ..HttpExecutionContext::default()
            },
        )
        .await
        .unwrap();
        let raw_request = String::from_utf8(server.await.unwrap()).unwrap();

        assert_eq!(token.access_token, "native-token");
        assert!(raw_request.starts_with("POST /token HTTP/1.1"));
        assert!(raw_request.contains("authorization: Basic ZGVza3RvcC1jbGllbnQ6bG9jYWwtc2VjcmV0"));
        assert!(raw_request.contains("grant_type=client_credentials"));
        assert!(raw_request.contains("scope=read+write"));
        assert!(raw_request.contains("audience=payments"));
    }

    #[test]
    fn validates_bounded_workspace_scoped_recovery_snapshots() {
        let workspace_id = "7b85bb25-2f43-4d7b-a606-df8f09a273a4";
        let mut snapshot: RecoverySnapshotDocument = serde_json::from_value(json!({
            "schemaVersion": 1,
            "workspaceId": workspace_id,
            "activeTabId": "bca7cf3d-d28b-443b-8a2f-d7fc072909e2",
            "activeEnvironmentId": null,
            "tabs": [{
                "id": "bca7cf3d-d28b-443b-8a2f-d7fc072909e2",
                "requestId": null,
                "title": "Scratch request",
                "request": {
                    "method": "GET",
                    "url": "https://example.com",
                    "queryParams": [],
                    "headers": [],
                    "auth": { "kind": "none" },
                    "body": { "kind": "empty" },
                    "options": {
                        "timeoutMs": 30000,
                        "maxRequestBytes": 1024,
                        "maxResponseBytes": 1024,
                        "followRedirects": true
                    }
                },
                "scripts": { "preRequest": "", "postResponse": "" },
                "dirty": true,
                "requestTab": "headers",
                "responseTab": "body",
                "scriptTab": "preRequest"
            }]
        }))
        .expect("test recovery document should deserialize");

        assert!(validate_recovery_snapshot(workspace_id, &snapshot).is_ok());
        assert!(
            validate_recovery_snapshot("3bd9b127-1a48-4eb7-b8ac-4cf179c7f637", &snapshot)
                .unwrap_err()
                .contains("different workspace")
        );
        snapshot.active_tab_id = "missing".to_owned();
        assert!(
            validate_recovery_snapshot(workspace_id, &snapshot)
                .unwrap_err()
                .contains("active tab")
        );
    }

    #[test]
    fn accepts_recovery_snapshots_with_transiently_invalid_requests() {
        // A mid-edit request (e.g. Bearer Token just selected before a value is typed) must not
        // block the local draft safety net, unlike the stricter Send/Save validation.
        let workspace_id = "7b85bb25-2f43-4d7b-a606-df8f09a273a4";
        let snapshot: RecoverySnapshotDocument = serde_json::from_value(json!({
            "schemaVersion": 1,
            "workspaceId": workspace_id,
            "activeTabId": "bca7cf3d-d28b-443b-8a2f-d7fc072909e2",
            "activeEnvironmentId": null,
            "tabs": [{
                "id": "bca7cf3d-d28b-443b-8a2f-d7fc072909e2",
                "requestId": null,
                "title": "Scratch request",
                "request": {
                    "method": "GET",
                    "url": "https://example.com",
                    "queryParams": [],
                    "headers": [],
                    "auth": { "kind": "bearer", "token": "" },
                    "body": { "kind": "empty" },
                    "options": {
                        "timeoutMs": 30000,
                        "maxRequestBytes": 1024,
                        "maxResponseBytes": 1024,
                        "followRedirects": true
                    }
                },
                "scripts": { "preRequest": "", "postResponse": "" },
                "dirty": true,
                "requestTab": "auth",
                "responseTab": "body",
                "scriptTab": "preRequest"
            }]
        }))
        .expect("test recovery document should deserialize");

        assert!(validate_recovery_snapshot(workspace_id, &snapshot).is_ok());
    }

    #[test]
    fn rejects_empty_recovery_tab_lists() {
        let workspace_id = "7b85bb25-2f43-4d7b-a606-df8f09a273a4";
        let snapshot: RecoverySnapshotDocument = serde_json::from_value(json!({
            "schemaVersion": 1,
            "workspaceId": workspace_id,
            "activeTabId": "tab",
            "activeEnvironmentId": null,
            "tabs": []
        }))
        .expect("empty recovery document should deserialize");

        assert!(
            validate_recovery_snapshot(workspace_id, &snapshot)
                .unwrap_err()
                .contains("between 1 and")
        );
    }
}
