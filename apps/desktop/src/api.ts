import { invoke } from "@tauri-apps/api/core";
import type { RecoverySnapshot } from "./request-tabs";

export interface HeaderEntry {
  name: string;
  value: string;
  enabled: boolean;
}

export interface KeyValueEntry {
  name: string;
  value: string;
  enabled: boolean;
}

export type RequestAuth =
  | { kind: "none" }
  | { kind: "inherit" }
  | { kind: "basic"; username: string; password: string }
  | { kind: "bearer"; token: string }
  | {
      kind: "apiKey";
      key: string;
      value: string;
      placement: "header" | "query";
    }
  | {
      kind: "oauth2ClientCredentials";
      issuerUrl: string;
      tokenUrl: string;
      clientId: string;
      clientSecret: string;
      scope: string;
      audience: string;
      clientAuth: "basic" | "requestBody";
      extraParameters: KeyValueEntry[];
    };

export type RequestBody =
  | { kind: "empty" }
  | { kind: "text"; content: string; contentType?: string }
  | { kind: "formUrlEncoded"; fields: KeyValueEntry[] }
  | { kind: "multipart"; parts: MultipartPart[] }
  | { kind: "binary"; path: string; contentType?: string };

export interface MultipartPart {
  name: string;
  enabled: boolean;
  value: MultipartValue;
}

export type MultipartValue =
  | { kind: "text"; content: string; contentType?: string }
  | { kind: "file"; path: string; fileName?: string; contentType?: string };

export interface RequestOptions {
  timeoutMs: number;
  maxRequestBytes: number;
  maxResponseBytes: number;
  followRedirects: boolean;
}

export interface RequestDefinition {
  method: string;
  url: string;
  queryParams: KeyValueEntry[];
  headers: HeaderEntry[];
  auth: RequestAuth;
  body: RequestBody;
  options: RequestOptions;
}

export interface ResponseSnapshot {
  status: number;
  statusText: string;
  headers: HeaderEntry[];
  body: ResponseBodySnapshot;
  timing: {
    totalMs: number;
  };
  receivedBytes: number;
}

export interface ResponseBodySnapshot {
  content: string;
  encoding: "utf8" | "base64";
  contentType: string | null;
  truncated: boolean;
}

export interface SavedRequest {
  id: string;
  name: string;
  request: RequestDefinition;
  scripts: RequestScripts;
}

export interface RequestScripts {
  preRequest: string;
  postResponse: string;
}

export interface RequestFolder {
  id: string;
  name: string;
  auth?: RequestAuth;
  folders: RequestFolder[];
  requests: SavedRequest[];
}

export interface Collection {
  id: string;
  name: string;
  auth?: RequestAuth;
  folders: RequestFolder[];
  requests: SavedRequest[];
}

export type ProxyMode = "system" | "disabled" | "custom";

export interface NetworkSettings {
  cookieJarEnabled: boolean;
  proxyMode: ProxyMode;
  proxyUrl: string;
  proxyUsername: string;
  proxyPassword: string;
  noProxy: string;
  caCertificatePath: string;
  clientCertificatePath: string;
  acceptInvalidCertificates: boolean;
}

export interface WorkspaceDocument {
  schemaVersion: number;
  id: string;
  name: string;
  collections: Collection[];
  environments: Environment[];
  protocolProfiles?: ProtocolProfile[];
  auth?: RequestAuth;
  network?: NetworkSettings;
}

export type ProtocolConfiguration =
  | {
      kind: "graphQl";
      query: string;
      variables: string;
      operationName: string;
    }
  | { kind: "webSocket"; subprotocols: string[] }
  | { kind: "serverSentEvents"; lastEventId: string }
  | {
      kind: "grpc";
      service: string;
      method: string;
      message: string;
      descriptorSetPath: string;
      useReflection: boolean;
    };

export interface ProtocolProfile {
  id: string;
  name: string;
  url: string;
  headers: HeaderEntry[];
  auth: RequestAuth;
  configuration: ProtocolConfiguration;
}

export interface ProtocolProfileCommandInput {
  profile: ProtocolProfile;
  environmentId: string | null;
}

export type ProtocolEventDirection = "inbound" | "outbound" | "system";
export type ProtocolEventKind =
  | "open"
  | "text"
  | "binary"
  | "ping"
  | "pong"
  | "event"
  | "error"
  | "closed";

export interface ProtocolEvent {
  id: string;
  timestampMs: number;
  direction: ProtocolEventDirection;
  kind: ProtocolEventKind;
  data: string;
  byteLength: number;
  eventName?: string;
  eventId?: string;
  retryMs?: number;
}

export interface ProtocolPollResult {
  events: ProtocolEvent[];
  finished: boolean;
}

export type WebSocketOutbound =
  | { kind: "text"; data: string }
  | { kind: "binary"; dataBase64: string }
  | { kind: "ping"; data: string }
  | { kind: "close" };

export interface GrpcMethodInfo {
  name: string;
  fullName: string;
  inputType: string;
  outputType: string;
  clientStreaming: boolean;
  serverStreaming: boolean;
}

export interface GrpcReflectionResult {
  services: string[];
  methods: GrpcMethodInfo[];
}

export interface GrpcInvokeResult {
  code: string;
  message: string;
  responses: unknown[];
  metadata: HeaderEntry[];
  trailers: HeaderEntry[];
  durationMs: number;
  clientStreaming: boolean;
  serverStreaming: boolean;
}

export interface EnvironmentVariable {
  id: string;
  key: string;
  value: string;
  enabled: boolean;
  secret?: boolean;
  local?: boolean;
}

export interface Environment {
  id: string;
  name: string;
  variables: EnvironmentVariable[];
}

export interface WorkspaceSession {
  rootPath: string;
  workspace: WorkspaceDocument;
}

export interface CurlImport {
  request: RequestDefinition;
  warnings: string[];
}

export type ApiDocumentKind = "postmanCollection" | "postmanEnvironment" | "openApi";

export interface ApiDocumentImport {
  kind: ApiDocumentKind;
  name: string;
  collections: Collection[];
  environments: Environment[];
  warnings: string[];
}

export type CurlShell = "posix" | "powerShell";

export type CodeLanguage = "javaScriptFetch" | "nodeAxios" | "pythonRequests";

export type ScriptPhase = "preRequest" | "postResponse";

export interface ScriptExecutionInput {
  phase: ScriptPhase;
  script: string;
  request: RequestDefinition;
  response: ResponseSnapshot | null;
  environment: Record<string, string>;
  variables: Record<string, string>;
}

export interface ScriptLog {
  level: "log" | "info" | "warn" | "error";
  message: string;
}

export interface ScriptTestResult {
  name: string;
  passed: boolean;
  error: string | null;
}

export interface ScriptExecutionResult {
  phase: ScriptPhase;
  request: RequestDefinition;
  environment: Record<string, string>;
  variables: Record<string, string>;
  logs: ScriptLog[];
  tests: ScriptTestResult[];
  error: string | null;
  durationMs: number;
}

export interface ScriptReport {
  phase: ScriptPhase;
  logs: ScriptLog[];
  tests: ScriptTestResult[];
  error: string | null;
  durationMs: number;
}

export interface RequestLifecycleInput {
  requestId: string | null;
  request: RequestDefinition;
  scripts: RequestScripts;
  environmentId: string | null;
}

export interface RequestLifecycleResult {
  response: ResponseSnapshot | null;
  scriptRuns: ScriptReport[];
  error: string | null;
  environmentUpdates: Record<string, string>;
}

export interface SecretMutation {
  variableId: string;
  value: string | null;
}

export interface HistoryResponseSummary {
  status: number;
  statusText: string;
  durationMs: number;
  receivedBytes: number;
  contentType: string | null;
}

export interface HistoryRecordInput {
  workspaceId: string;
  requestId: string | null;
  requestName: string;
  request: RequestDefinition;
  response: HistoryResponseSummary | null;
  error: string | null;
}

export interface HistoryEntry {
  id: string;
  workspaceId: string;
  requestId: string | null;
  requestName: string;
  method: string;
  url: string;
  request: RequestDefinition;
  requestBodyOmitted: boolean;
  status: number | null;
  statusText: string | null;
  durationMs: number | null;
  receivedBytes: number | null;
  contentType: string | null;
  error: string | null;
  createdAtMs: number;
  pinned: boolean;
}

export type CollectionRunTarget =
  | { kind: "collection"; collectionId: string }
  | { kind: "folder"; folderId: string };

export interface CollectionRunInput {
  target: CollectionRunTarget;
  environmentId: string | null;
  iterations: number;
  delayMs: number;
  stopOnFailure: boolean;
  dataRows: Array<Record<string, string>>;
}

export interface CollectionRunSummary {
  requestsExecuted: number;
  requestsFailed: number;
  testsPassed: number;
  testsFailed: number;
  bytesReceived: number;
}

export interface CollectionRunRequestResult {
  iteration: number;
  requestId: string;
  requestName: string;
  status: number | null;
  durationMs: number;
  receivedBytes: number;
  testsPassed: number;
  testsFailed: number;
  error: string | null;
  scriptRuns: ScriptReport[];
}

export interface CollectionRunReport {
  schemaVersion: number;
  id: string;
  workspaceId: string;
  workspaceName: string;
  targetName: string;
  environmentName: string | null;
  startedAtMs: number;
  durationMs: number;
  requestedIterations: number;
  completedIterations: number;
  stopOnFailure: boolean;
  summary: CollectionRunSummary;
  results: CollectionRunRequestResult[];
}

export interface CollectionRunReportSummary {
  id: string;
  targetName: string;
  environmentName: string | null;
  startedAtMs: number;
  durationMs: number;
  requestedIterations: number;
  completedIterations: number;
  summary: CollectionRunSummary;
}

export interface HistoryQuery {
  workspaceId: string;
  search: string;
  limit: number;
  offset: number;
}

export interface CookieSnapshot {
  name: string;
  value: string;
  domain: string;
  path: string;
  expiresAtMs: number | null;
  secure: boolean;
  httpOnly: boolean;
  sameSite: string | null;
}

export async function executeHttpRequest(
  request: RequestDefinition,
): Promise<ResponseSnapshot> {
  return invoke<ResponseSnapshot>("execute_http_request", { request });
}

export async function cancelHttpRequest(): Promise<boolean> {
  return invoke<boolean>("cancel_http_request");
}

export async function importCurl(command: string): Promise<CurlImport> {
  return invoke<CurlImport>("import_curl", { command });
}

export async function importApiDocumentFile(): Promise<ApiDocumentImport | null> {
  return invoke<ApiDocumentImport | null>("import_api_document_file");
}

export async function exportCurl(
  request: RequestDefinition,
  shell: CurlShell,
): Promise<string> {
  return invoke<string>("export_curl", { request, shell });
}

export async function exportCurlForEnvironment(
  request: RequestDefinition,
  environmentId: string | null,
  shell: CurlShell,
): Promise<string> {
  return invoke<string>("export_curl_for_environment", {
    request,
    environmentId,
    shell,
  });
}

export async function exportCodeForEnvironment(
  request: RequestDefinition,
  environmentId: string | null,
  language: CodeLanguage,
): Promise<string> {
  return invoke<string>("export_code_for_environment", {
    request,
    environmentId,
    language,
  });
}

export async function executeRequestLifecycle(
  input: RequestLifecycleInput,
): Promise<RequestLifecycleResult> {
  return invoke<RequestLifecycleResult>("execute_request_lifecycle", { input });
}

export async function runCollection(
  input: CollectionRunInput,
): Promise<CollectionRunReport> {
  return invoke<CollectionRunReport>("run_collection", { input });
}

export async function listCollectionRunReports(
  workspaceId: string,
): Promise<CollectionRunReportSummary[]> {
  return invoke<CollectionRunReportSummary[]>("list_collection_run_reports", {
    workspaceId,
  });
}

export async function loadCollectionRunReport(
  workspaceId: string,
  reportId: string,
): Promise<CollectionRunReport> {
  return invoke<CollectionRunReport>("load_collection_run_report", {
    workspaceId,
    reportId,
  });
}

export async function deleteCollectionRunReport(
  workspaceId: string,
  reportId: string,
): Promise<void> {
  return invoke("delete_collection_run_report", { workspaceId, reportId });
}

export async function exportCollectionRunReport(
  workspaceId: string,
  reportId: string,
  format: "json" | "junit",
): Promise<string | null> {
  return invoke<string | null>("export_collection_run_report", {
    workspaceId,
    reportId,
    format,
  });
}

export async function executeRequestScript(
  input: ScriptExecutionInput,
): Promise<ScriptExecutionResult> {
  return invoke<ScriptExecutionResult>("execute_request_script", { input });
}

export async function pickRequestFile(): Promise<string | null> {
  return invoke<string | null>("pick_request_file");
}

export async function pickNetworkFile(
  kind: "caCertificate" | "clientIdentity",
): Promise<string | null> {
  return invoke<string | null>("pick_network_file", { kind });
}

export async function saveResponseBody(
  body: ResponseBodySnapshot,
  suggestedName: string,
): Promise<string | null> {
  return invoke<string | null>("save_response_body", { body, suggestedName });
}

export async function createWorkspace(
  name: string,
): Promise<WorkspaceSession | null> {
  return invoke<WorkspaceSession | null>("create_workspace", { name });
}

export async function openWorkspace(): Promise<WorkspaceSession | null> {
  return invoke<WorkspaceSession | null>("open_workspace");
}

export async function reopenLastWorkspace(): Promise<WorkspaceSession | null> {
  return invoke<WorkspaceSession | null>("reopen_last_workspace");
}

export async function rememberOpenWorkspace(workspaceId: string): Promise<boolean> {
  return invoke<boolean>("remember_open_workspace", { workspaceId });
}

export async function exportWorkspaceBackup(): Promise<string | null> {
  return invoke<string | null>("export_workspace_backup");
}

export async function restoreWorkspaceBackup(): Promise<WorkspaceSession | null> {
  return invoke<WorkspaceSession | null>("restore_workspace_backup");
}

export async function exportSupportDiagnostics(): Promise<string | null> {
  return invoke<string | null>("export_support_diagnostics");
}

export async function saveWorkspace(
  workspace: WorkspaceDocument,
): Promise<WorkspaceSession> {
  return invoke<WorkspaceSession>("save_workspace", { workspace });
}

export async function environmentSecretStatuses(): Promise<Record<string, boolean>> {
  return invoke<Record<string, boolean>>("environment_secret_statuses");
}

export async function environmentLocalValues(): Promise<Record<string, string>> {
  return invoke<Record<string, string>>("environment_local_values");
}

export async function saveWorkspaceWithSecrets(
  workspace: WorkspaceDocument,
  mutations: SecretMutation[],
): Promise<WorkspaceSession> {
  return invoke<WorkspaceSession>("save_workspace_with_secrets", {
    workspace,
    mutations,
  });
}

export async function closeWorkspace(): Promise<void> {
  return invoke<void>("close_workspace");
}

export async function saveRecoverySnapshot(snapshot: RecoverySnapshot): Promise<void> {
  return invoke<void>("save_recovery_snapshot", {
    workspaceId: snapshot.workspaceId,
    snapshot,
  });
}

export async function loadRecoverySnapshot(
  workspaceId: string,
): Promise<RecoverySnapshot | null> {
  return invoke<RecoverySnapshot | null>("load_recovery_snapshot", { workspaceId });
}

export async function clearRecoverySnapshot(workspaceId: string): Promise<void> {
  return invoke<void>("clear_recovery_snapshot", { workspaceId });
}

export async function recordHistory(input: HistoryRecordInput): Promise<HistoryEntry> {
  return invoke<HistoryEntry>("record_history", { input });
}

export async function listHistory(query: HistoryQuery): Promise<HistoryEntry[]> {
  return invoke<HistoryEntry[]>("list_history", { query });
}

export async function setHistoryPinned(
  workspaceId: string,
  entryId: string,
  pinned: boolean,
): Promise<void> {
  return invoke<void>("set_history_pinned", { workspaceId, entryId, pinned });
}

export async function deleteHistoryEntry(
  workspaceId: string,
  entryId: string,
): Promise<void> {
  return invoke<void>("delete_history_entry", { workspaceId, entryId });
}

export async function clearHistory(
  workspaceId: string,
  includePinned: boolean,
): Promise<number> {
  return invoke<number>("clear_history", { workspaceId, includePinned });
}

export async function listWorkspaceCookies(
  workspaceId: string,
): Promise<CookieSnapshot[]> {
  return invoke<CookieSnapshot[]>("list_workspace_cookies", { workspaceId });
}

export async function addWorkspaceCookie(
  workspaceId: string,
  url: string,
  setCookie: string,
): Promise<void> {
  return invoke<void>("add_workspace_cookie", { workspaceId, url, setCookie });
}

export async function deleteWorkspaceCookie(
  workspaceId: string,
  cookie: Pick<CookieSnapshot, "domain" | "path" | "name">,
): Promise<boolean> {
  return invoke<boolean>("delete_workspace_cookie", { workspaceId, ...cookie });
}

export async function clearWorkspaceCookies(workspaceId: string): Promise<void> {
  return invoke<void>("clear_workspace_cookies", { workspaceId });
}

export async function executeGraphqlProfile(
  input: ProtocolProfileCommandInput,
): Promise<ResponseSnapshot> {
  return invoke<ResponseSnapshot>("execute_graphql_profile", { input });
}

export async function startWebsocketSession(
  input: ProtocolProfileCommandInput,
): Promise<string> {
  return invoke<string>("start_websocket_session", { input });
}

export async function startSseSession(
  input: ProtocolProfileCommandInput,
): Promise<string> {
  return invoke<string>("start_sse_session", { input });
}

export async function pollProtocolSession(
  sessionId: string,
  maximum = 200,
): Promise<ProtocolPollResult> {
  return invoke<ProtocolPollResult>("poll_protocol_session", {
    sessionId,
    maximum,
  });
}

export async function sendWebsocketMessage(
  sessionId: string,
  message: WebSocketOutbound,
): Promise<void> {
  return invoke<void>("send_websocket_message", { sessionId, message });
}

export async function stopProtocolSession(sessionId: string): Promise<boolean> {
  return invoke<boolean>("stop_protocol_session", { sessionId });
}

export async function reflectGrpcProfile(
  input: ProtocolProfileCommandInput,
): Promise<GrpcReflectionResult> {
  return invoke<GrpcReflectionResult>("reflect_grpc_profile", { input });
}

export async function invokeGrpcProfile(
  input: ProtocolProfileCommandInput,
): Promise<GrpcInvokeResult> {
  return invoke<GrpcInvokeResult>("invoke_grpc_profile", { input });
}

export async function pickGrpcDescriptorSet(): Promise<string | null> {
  return invoke<string | null>("pick_grpc_descriptor_set");
}
