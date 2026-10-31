import {
  type ClipboardEvent,
  type CSSProperties,
  type FormEvent,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  cancelHttpRequest,
  clearRecoverySnapshot,
  closeWorkspace,
  createWorkspace,
  environmentLocalValues,
  environmentSecretStatuses,
  executeRequestLifecycle,
  exportCurlForEnvironment,
  exportSupportDiagnostics,
  exportWorkspaceBackup,
  importApiDocumentFile,
  importCurl,
  loadRecoverySnapshot,
  recordHistory,
  openWorkspace,
  restoreWorkspaceBackup,
  pickRequestFile,
  saveResponseBody,
  saveRecoverySnapshot,
  saveWorkspace,
  saveWorkspaceWithSecrets,
  type ApiDocumentImport,
  type Collection,
  type Environment,
  type HeaderEntry,
  type HistoryEntry,
  type KeyValueEntry,
  type MultipartPart,
  type NetworkSettings,
  type ProtocolProfile,
  type RequestAuth,
  type RequestDefinition,
  type RequestFolder,
  type RequestOptions,
  type RequestScripts,
  type ResponseSnapshot,
  type SavedRequest,
  type ScriptReport,
  type SecretMutation,
  type WorkspaceDocument,
  type WorkspaceSession,
} from "./api";
import { AuthEditor } from "./AuthEditor";
import { AppIcon } from "./AppIcon";
import { curlCommandFromClipboard } from "./curl-paste";
import { AuthPolicyEditor } from "./AuthPolicyEditor";
import { BodyEditor, type BodyMode } from "./BodyEditor";
import { CollectionRunner } from "./CollectionRunner";
import { CodeSnippetModal } from "./CodeSnippetModal";
import { CommandPalette, type PaletteAction, type PaletteRequestEntry } from "./CommandPalette";
import { CookieManager } from "./CookieManager";
import { EnvironmentEditor } from "./EnvironmentEditor";
import { applyScriptEnvironmentUpdates, saveExtractedVariable } from "./environment-secrets";
import { formatBytes, responseFileName } from "./format";
import { HeaderEditor } from "./HeaderEditor";
import { describeNetworkFailure } from "./request-failures";
import { DEFAULT_RAW_CONTENT_TYPE, alignContentTypeHeaders, hasJsonFormContentTypeMismatch } from "./request-framing";
import { HistoryPanel } from "./HistoryPanel";
import { KeyValueEditor } from "./KeyValueEditor";
import { NetworkSettingsEditor } from "./NetworkSettingsEditor";
import { ProtocolWorkbench } from "./ProtocolWorkbench";
import { composeRequestUrl, splitRequestUrl } from "./request-url";
import { ResponseBodyViewer } from "./ResponseBodyViewer";
import { RequestSettings } from "./RequestSettings";
import {
  duplicateRequestTab,
  markRequestTabSaved,
  requestTabAfterClose,
  restoreRecoveryTabs,
  toRecoverySnapshot,
  type OpenRequestTab,
  type RequestEditorTab as RequestTab,
  type ResponseEditorTab as ResponseTab,
  type ScriptEditorTab as ScriptTab,
} from "./request-tabs";
import { WorkspaceTree } from "./WorkspaceTree";
import { WorkspaceMaintenanceDialog, type WorkspaceMaintenanceAction } from "./WorkspaceMaintenanceDialog";
import {
  addCollection,
  addFolder,
  canMoveFolderTo,
  countRequests,
  duplicateWorkspaceItem,
  filterWorkspace,
  findFolderLocation,
  findRequestLocation,
  findSavedRequest,
  getAuthPolicy,
  mergeApiDocumentImport,
  moveFolder,
  removeWorkspaceItem,
  renameWorkspaceItem,
  reorderWorkspaceItem,
  setAuthPolicy,
  type AuthPolicyTarget,
  type RequestLocation,
  type WorkspaceItemTarget,
  upsertRequest,
} from "./workspace-model";
import { createWorkspacePersistence, type WorkspaceUpdate } from "./workspace-persistence";

type Notice = { kind: "error" | "success" | "info"; text: string };
type ModalState =
  | { kind: "newWorkspace"; preserveRequest?: boolean }
  | { kind: "newCollection" }
  | { kind: "newFolder"; preferredLocation?: RequestLocation }
  | { kind: "saveRequest"; preferredLocation?: RequestLocation }
  | { kind: "renameItem"; target: WorkspaceItemTarget }
  | { kind: "moveRequest"; target: Extract<WorkspaceItemTarget, { kind: "request" }> }
  | { kind: "moveFolder"; target: Extract<WorkspaceItemTarget, { kind: "folder" }> };

interface DestinationOption {
  key: string;
  label: string;
  location: RequestLocation;
}

const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
const DEFAULT_URL = "https://httpbin.org/anything";
const DEFAULT_OPTIONS: RequestOptions = {
  timeoutMs: 30_000,
  maxRequestBytes: 100 * 1024 * 1024,
  maxResponseBytes: 10 * 1024 * 1024,
  followRedirects: true,
};

function initialHeaders(): HeaderEntry[] {
  return [{ name: "Accept", value: "application/json", enabled: true }];
}

const LAYOUT_STORAGE_PREFIX = "apiClient.layout.";

function readStoredLayoutNumber(key: string, fallback: number): number {
  try {
    const raw = window.localStorage.getItem(`${LAYOUT_STORAGE_PREFIX}${key}`);
    const parsed = raw === null ? NaN : Number(raw);
    return Number.isFinite(parsed) ? parsed : fallback;
  } catch {
    return fallback;
  }
}

function writeStoredLayoutNumber(key: string, value: number) {
  try {
    window.localStorage.setItem(`${LAYOUT_STORAGE_PREFIX}${key}`, String(value));
  } catch {
    // Best-effort only; layout falls back to defaults when storage is unavailable.
  }
}

function createScratchTab(): OpenRequestTab {
  return {
    id: crypto.randomUUID(),
    requestId: null,
    title: "Scratch request",
    request: {
      method: "GET",
      url: DEFAULT_URL,
      queryParams: [],
      headers: initialHeaders(),
      auth: { kind: "none" },
      body: { kind: "empty" },
      options: { ...DEFAULT_OPTIONS },
    },
    scripts: { preRequest: "", postResponse: "" },
    dirty: false,
    requestTab: "headers",
    responseTab: "body",
    scriptTab: "preRequest",
    response: null,
    scriptRuns: [],
    requestError: null,
  };
}

function createSavedRequestTab(request: SavedRequest): OpenRequestTab {
  return {
    id: crypto.randomUUID(),
    requestId: request.id,
    title: request.name,
    request: structuredClone(request.request),
    scripts: structuredClone(request.scripts),
    dirty: false,
    requestTab: "headers",
    responseTab: "body",
    scriptTab: "preRequest",
    response: null,
    scriptRuns: [],
    requestError: null,
  };
}

export function App() {
  const [method, setMethod] = useState("GET");
  const [url, setUrl] = useState(DEFAULT_URL);
  const [queryParams, setQueryParams] = useState<KeyValueEntry[]>([]);
  const [headers, setHeaders] = useState<HeaderEntry[]>(initialHeaders);
  const [auth, setAuth] = useState<RequestAuth>({ kind: "none" });
  const [bodyMode, setBodyMode] = useState<BodyMode>("none");
  const [body, setBody] = useState("");
  const [bodyContentType, setBodyContentType] = useState(DEFAULT_RAW_CONTENT_TYPE);
  const [formFields, setFormFields] = useState<KeyValueEntry[]>([]);
  const [multipartParts, setMultipartParts] = useState<MultipartPart[]>([]);
  const [binaryFilePath, setBinaryFilePath] = useState("");
  const [binaryContentType, setBinaryContentType] = useState(
    "application/octet-stream",
  );
  const [requestOptions, setRequestOptions] = useState<RequestOptions>(DEFAULT_OPTIONS);
  const [requestTab, setRequestTab] = useState<RequestTab>("headers");
  const [scriptTab, setScriptTab] = useState<ScriptTab>("preRequest");
  const [preRequestScript, setPreRequestScript] = useState("");
  const [postResponseScript, setPostResponseScript] = useState("");
  const [responseTab, setResponseTab] = useState<ResponseTab>("body");
  const [response, setResponse] = useState<ResponseSnapshot | null>(null);
  const [requestError, setRequestError] = useState<string | null>(null);
  const [isSending, setIsSending] = useState(false);
  const [isSavingResponse, setIsSavingResponse] = useState(false);
  const [scriptRuns, setScriptRuns] = useState<ScriptReport[]>([]);

  const [requestTabs, setRequestTabs] = useState<OpenRequestTab[]>(() => [
    createScratchTab(),
  ]);
  const [activeRequestTabId, setActiveRequestTabId] = useState(
    () => requestTabs[0]!.id,
  );
  const [recoveryReadyWorkspaceId, setRecoveryReadyWorkspaceId] = useState<
    string | null
  >(null);

  const [session, setSession] = useState<WorkspaceSession | null>(null);
  const sessionRef = useRef(session);
  const sendingRef = useRef(false);
  const cancelledSendRef = useRef(false);
  const workspaceTransitionRef = useRef(false);
  const [selectedRequestId, setSelectedRequestId] = useState<string | null>(null);
  const [isDirty, setIsDirty] = useState(false);
  const [isWorkspaceBusy, setIsWorkspaceBusy] = useState(false);
  const [maintenanceAction, setMaintenanceAction] = useState<WorkspaceMaintenanceAction | null>(null);
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());
  const [searchQuery, setSearchQuery] = useState("");
  const [notice, setNotice] = useState<Notice | null>(null);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [sidebarWidth, setSidebarWidth] = useState(() => readStoredLayoutNumber("sidebarWidth", 250));
  const [requestPanelPct, setRequestPanelPct] = useState(() =>
    readStoredLayoutNumber("requestPanelPct", 42),
  );
  const workspaceRef = useRef<HTMLElement | null>(null);
  const [activeEnvironmentId, setActiveEnvironmentId] = useState<string | null>(null);
  const [environmentEditorOpen, setEnvironmentEditorOpen] = useState(false);
  const [secretStatuses, setSecretStatuses] = useState<Record<string, boolean>>({});
  const [localEnvironmentValues, setLocalEnvironmentValues] = useState<
    Record<string, string>
  >({});
  const [isLoadingSecrets, setIsLoadingSecrets] = useState(false);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyRevision, setHistoryRevision] = useState(0);
  const [cookieManagerOpen, setCookieManagerOpen] = useState(false);
  const [codeSnippetOpen, setCodeSnippetOpen] = useState(false);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const [networkSettingsOpen, setNetworkSettingsOpen] = useState(false);
  const [authPolicyTarget, setAuthPolicyTarget] = useState<AuthPolicyTarget | null>(null);
  const [collectionRunnerOpen, setCollectionRunnerOpen] = useState(false);
  const [protocolWorkbenchOpen, setProtocolWorkbenchOpen] = useState(false);

  const [curlImportOpen, setCurlImportOpen] = useState(false);
  const [curlText, setCurlText] = useState("");
  const [curlImportError, setCurlImportError] = useState<string | null>(null);
  const [isImportingCurl, setIsImportingCurl] = useState(false);
  const importingCurlRef = useRef(false);
  const urlInputRef = useRef<HTMLInputElement>(null);
  const [pendingApiImport, setPendingApiImport] = useState<ApiDocumentImport | null>(null);
  const [isImportingApiDocument, setIsImportingApiDocument] = useState(false);

  const [modal, setModal] = useState<ModalState | null>(null);
  const [modalName, setModalName] = useState("");
  const [destinationKey, setDestinationKey] = useState("");
  const [workspaceAction, setWorkspaceAction] = useState<WorkspaceItemTarget | null>(null);
  const persistenceRef = useRef<ReturnType<typeof createWorkspacePersistence> | null>(null);
  if (!persistenceRef.current) {
    persistenceRef.current = createWorkspacePersistence({
      current: () => sessionRef.current,
      save: (workspace, mutations) => mutations?.length
        ? saveWorkspaceWithSecrets(workspace, mutations)
        : saveWorkspace(workspace),
      onSaved: (saved) => {
        sessionRef.current = saved;
        setSession(saved);
        setRequestTabs((current) => current.map((tab) => {
          const request = tab.requestId ? findSavedRequest(saved.workspace, tab.requestId) : undefined;
          return request ? { ...tab, title: request.name } : tab;
        }));
      },
      onBusy: setIsWorkspaceBusy,
    });
  }
  const editorRef = useRef({ tabId: activeRequestTabId, request: buildRequest(), scripts: buildScripts() });
  editorRef.current = { tabId: activeRequestTabId, request: buildRequest(), scripts: buildScripts() };

  const selectedRequest = useMemo(
    () =>
      session && selectedRequestId
        ? findSavedRequest(session.workspace, selectedRequestId)
        : undefined,
    [session, selectedRequestId],
  );
  const activeEnvironment = useMemo(
    () =>
      session?.workspace.environments.find(
        (environment) => environment.id === activeEnvironmentId,
      ),
    [activeEnvironmentId, session],
  );
  const handlePanelError = useCallback(
    (message: string) => setNotice({ kind: "error", text: message }),
    [],
  );
  const filteredWorkspace = useMemo(
    () => (session ? filterWorkspace(session.workspace, searchQuery) : null),
    [session, searchQuery],
  );
  const destinations = useMemo(
    () => (session ? workspaceDestinations(session.workspace) : []),
    [session],
  );
  const paletteRequests = useMemo(
    () => (session ? flattenWorkspaceRequests(session.workspace) : []),
    [session],
  );
  const paletteActions: PaletteAction[] = (() => {
    const actions: PaletteAction[] = [
      { id: "new-request", label: "New request", hint: "Ctrl+T", run: startScratchRequest },
      { id: "send-request", label: "Send request", hint: "Ctrl+Enter", run: () => void sendRequest() },
      { id: "save-request", label: "Save request", hint: "Ctrl+S", run: () => void handleSaveRequest() },
      {
        id: "import-curl",
        label: "Import cURL",
        run: () => {
          setCurlText("");
          setCurlImportError(null);
          setCurlImportOpen(true);
        },
      },
      { id: "generate-code", label: "Generate code", run: () => setCodeSnippetOpen(true) },
      {
        id: "toggle-sidebar",
        label: sidebarOpen ? "Hide sidebar" : "Show sidebar",
        run: () => setSidebarOpen((open) => !open),
      },
    ];
    if (session) {
      actions.push(
        { id: "history", label: "History", run: () => setHistoryOpen(true) },
        { id: "environments", label: "Manage environments", run: () => void openEnvironmentEditor() },
        { id: "collection-runner", label: "Collection runner", run: () => setCollectionRunnerOpen(true) },
        { id: "protocol-workbench", label: "Protocol workbench", run: () => setProtocolWorkbenchOpen(true) },
        { id: "cookies", label: "Cookies", run: () => setCookieManagerOpen(true) },
        { id: "network-settings", label: "Network and TLS", run: () => setNetworkSettingsOpen(true) },
      );
    } else {
      actions.push(
        { id: "new-workspace", label: "New workspace", run: () => showModal({ kind: "newWorkspace" }) },
        { id: "open-workspace", label: "Open workspace", run: () => void handleOpenWorkspace() },
      );
    }
    return actions;
  })();
  const modalDestinations = useMemo(
    () =>
      modal?.kind === "moveFolder" && session
        ? destinations.filter((destination) =>
            canMoveFolderTo(
              session.workspace,
              modal.target.folderId,
              destination.location,
            ),
          )
        : destinations,
    [destinations, modal, session],
  );
  const scriptTests = useMemo(
    () => scriptRuns.flatMap((run) => run.tests.map((test) => ({ ...test, phase: run.phase }))),
    [scriptRuns],
  );
  const scriptLogs = useMemo(
    () =>
      scriptRuns.flatMap((run) => [
        ...run.logs.map((log) => ({ ...log, phase: run.phase })),
        ...(run.error
          ? [{ level: "error" as const, message: run.error, phase: run.phase }]
          : []),
      ]),
    [scriptRuns],
  );
  const responseTabs = useMemo<ResponseTab[]>(
    () => [
      "body",
      "headers",
      ...(scriptTests.length ? (["tests"] as const) : []),
      ...(scriptLogs.length ? (["console"] as const) : []),
    ],
    [scriptLogs.length, scriptTests.length],
  );
  const hasUnsavedTabs = isDirty || requestTabs.some((tab) => tab.dirty);
  const networkFailure = requestError
    ? describeNetworkFailure(requestError, url, requestOptions.timeoutMs, session?.workspace.network?.proxyMode ?? "system")
    : null;

  useEffect(() => {
    if (!notice || notice.kind === "error") return;
    const timer = window.setTimeout(() => setNotice((current) => current === notice ? null : current), 5000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  useEffect(() => {
    function protectUnsavedWork(event: BeforeUnloadEvent) {
      if (hasUnsavedTabs) {
        event.preventDefault();
      }
    }
    window.addEventListener("beforeunload", protectUnsavedWork);
    return () => window.removeEventListener("beforeunload", protectUnsavedWork);
  }, [hasUnsavedTabs]);

  useEffect(() => {
    function handleGlobalKeyDown(event: globalThis.KeyboardEvent) {
      const withModifier = event.ctrlKey || event.metaKey;
      if (!withModifier) {
        return;
      }
      const key = event.key.toLowerCase();
      if (key === "enter") {
        event.preventDefault();
        void sendRequest();
      } else if (key === "t") {
        event.preventDefault();
        startScratchRequest();
      } else if (key === "w") {
        if (activeRequestTabId) {
          event.preventDefault();
          closeOpenRequestTab(activeRequestTabId);
        }
      } else if (key === "s") {
        event.preventDefault();
        void handleSaveRequest();
      } else if (key === "k") {
        event.preventDefault();
        setCommandPaletteOpen((open) => !open);
      }
    }
    window.addEventListener("keydown", handleGlobalKeyDown);
    return () => window.removeEventListener("keydown", handleGlobalKeyDown);
  }, [activeRequestTabId, closeOpenRequestTab, handleSaveRequest, sendRequest, startScratchRequest]);

  useEffect(() => {
    const snapshot: Omit<OpenRequestTab, "id" | "title"> = {
      requestId: selectedRequestId,
      request: buildRequest(),
      scripts: buildScripts(),
      dirty: isDirty,
      requestTab,
      responseTab,
      scriptTab,
      response,
      scriptRuns,
      requestError,
    };
    setRequestTabs((current) =>
      current.map((tab) =>
        tab.id === activeRequestTabId ? { id: tab.id, title: selectedRequest?.name ?? tab.title, ...snapshot } : tab,
      ),
    );
  }, [
    activeRequestTabId,
    auth,
    binaryContentType,
    binaryFilePath,
    body,
    bodyContentType,
    bodyMode,
    formFields,
    headers,
    isDirty,
    method,
    multipartParts,
    postResponseScript,
    preRequestScript,
    queryParams,
    requestError,
    requestOptions,
    requestTab,
    response,
    responseTab,
    scriptRuns,
    scriptTab,
    selectedRequest?.name,
    selectedRequestId,
    url,
  ]);

  useEffect(() => {
    const workspaceId = session?.workspace.id;
    if (!workspaceId || recoveryReadyWorkspaceId !== workspaceId) {
      return;
    }
    const timer = window.setTimeout(() => {
      const snapshot = toRecoverySnapshot(
        workspaceId,
        requestTabs,
        activeRequestTabId,
        activeEnvironmentId,
      );
      void saveRecoverySnapshot(snapshot).catch((cause: unknown) => {
        // This is a background safety net, not the request itself — don't alarm the user with
        // an error-styled banner over a draft backup hiccup; Send/Save are unaffected.
        setNotice({
          kind: "info",
          text: `Draft recovery could not be saved: ${errorMessage(cause)}`,
        });
      });
    }, 650);
    return () => window.clearTimeout(timer);
  }, [
    activeEnvironmentId,
    activeRequestTabId,
    recoveryReadyWorkspaceId,
    requestTabs,
    session?.workspace.id,
  ]);

  function buildRequest(): RequestDefinition {
    const requestBody: RequestDefinition["body"] = (() => {
      switch (bodyMode) {
        case "none":
          return { kind: "empty" };
        case "raw":
          return { kind: "text", content: body, contentType: bodyContentType };
        case "formUrlEncoded":
          return { kind: "formUrlEncoded", fields: formFields };
        case "multipart":
          return { kind: "multipart", parts: multipartParts };
        case "binary":
          return {
            kind: "binary",
            path: binaryFilePath,
            contentType: binaryContentType || undefined,
          };
      }
    })();

    return {
      method,
      url: url.trim(),
      queryParams,
      headers,
      auth,
      body: requestBody,
      options: requestOptions,
    };
  }

  function buildScripts(): RequestScripts {
    return {
      preRequest: preRequestScript,
      postResponse: postResponseScript,
    };
  }

  // Persists plain environment values a script wrote via pm.environment.set() so the next,
  // separately-sent request can resolve {{variable}} references to the new value (secret and
  // local-scoped variables are never included; they stay in the OS credential store).
  async function applyEnvironmentUpdates(updates: Record<string, string>) {
    if (!session || !activeEnvironmentId || Object.keys(updates).length === 0) {
      return;
    }
    await persistWorkspace((workspace) => ({ workspace: {
      ...workspace,
      environments: applyScriptEnvironmentUpdates(
        workspace.environments, activeEnvironmentId, updates, () => crypto.randomUUID(),
      ),
    } }));
  }

  // One-click alternative to writing a post-response script: lets the user save a value picked
  // straight from the response body (e.g. access_token) as an environment variable, so the very
  // next, separately-sent request can reference it via {{variable}} without any scripting.
  async function saveResponseValueAsVariable(key: string, value: string): Promise<boolean> {
    const trimmedKey = key.trim();
    if (!trimmedKey) {
      return false;
    }
    if (!session) {
      setNotice({ kind: "error", text: "Open a workspace before saving a response value as a variable." });
      return false;
    }
    if (!activeEnvironmentId) {
      setNotice({
        kind: "error",
        text: "Select or create an active environment (bottom-right) before saving a variable.",
      });
      return false;
    }
    const targetEnvironment = session.workspace.environments.find(
      (environment) => environment.id === activeEnvironmentId,
    );
    if (!targetEnvironment) {
      setNotice({ kind: "error", text: "The active environment no longer exists." });
      return false;
    }

    const saved = await persistWorkspace((workspace) => {
      const change = saveExtractedVariable(
        workspace.environments, activeEnvironmentId, trimmedKey, value, () => crypto.randomUUID(),
      );
      return { workspace: { ...workspace, environments: change.environments }, mutations: change.mutations };
    });
    if (saved) {
      setSecretStatuses(await environmentSecretStatuses().catch(() => ({})));
      setNotice({
        kind: "success",
        text: `Saved {{${trimmedKey}}} to ${targetEnvironment.name}.`,
      });
    }
    return saved;
  }

  const MIN_SIDEBAR_WIDTH = 200;
  const MAX_SIDEBAR_WIDTH = 480;
  const MIN_REQUEST_PANEL_PCT = 20;
  const MAX_REQUEST_PANEL_PCT = 75;

  function startSidebarResize(event: ReactPointerEvent<HTMLDivElement>) {
    event.preventDefault();
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    handle.classList.add("active");
    const startX = event.clientX;
    const startWidth = sidebarWidth;
    const shell = handle.closest<HTMLElement>(".app-shell");
    let nextWidth = startWidth;

    function onMove(moveEvent: PointerEvent) {
      nextWidth = Math.min(
        MAX_SIDEBAR_WIDTH,
        Math.max(MIN_SIDEBAR_WIDTH, startWidth + (moveEvent.clientX - startX)),
      );
      shell?.style.setProperty("--sidebar-width", `${nextWidth}px`);
    }
    function onUp() {
      handle.classList.remove("active");
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      setSidebarWidth(nextWidth);
      writeStoredLayoutNumber("sidebarWidth", nextWidth);
    }
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  }

  function startWorkspaceResize(event: ReactPointerEvent<HTMLDivElement>) {
    event.preventDefault();
    const handle = event.currentTarget;
    const container = workspaceRef.current;
    if (!container) {
      return;
    }
    handle.setPointerCapture(event.pointerId);
    handle.classList.add("active");
    const startY = event.clientY;
    const containerHeight = container.clientHeight;
    const panelHeight = container.querySelector(".request-panel")?.clientHeight ?? 0;
    const startPct = panelHeight / Math.max(containerHeight, 1) * 100;
    let nextPct = startPct;

    function onMove(moveEvent: PointerEvent) {
      if (containerHeight <= 0) {
        return;
      }
      const deltaPct = ((moveEvent.clientY - startY) / containerHeight) * 100;
      nextPct = Math.min(
        MAX_REQUEST_PANEL_PCT,
        Math.max(MIN_REQUEST_PANEL_PCT, startPct + deltaPct),
      );
      container?.style.setProperty("--request-panel-pct", `${nextPct}%`);
    }
    function onUp() {
      handle.classList.remove("active");
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      setRequestPanelPct(nextPct);
      writeStoredLayoutNumber("requestPanelPct", nextPct);
    }
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  }

  async function sendRequest() {
    if (!url.trim() || sendingRef.current || importingCurlRef.current || workspaceTransitionRef.current) {
      return;
    }

    sendingRef.current = true;
    cancelledSendRef.current = false;
    setIsSending(true);
    setRequestError(null);
    setResponseTab("body");
    setScriptRuns([]);

    try {
      const historyRequest = structuredClone(buildRequest());
      const historyWorkspaceId = session?.workspace.id;
      const historyRequestId = selectedRequestId;
      const historyRequestName = selectedRequest?.name ?? "Scratch request";
      await persistenceRef.current!.idle();
      if (cancelledSendRef.current) {
        throw new Error("request cancelled");
      }
      const result = await executeRequestLifecycle({
        requestId: selectedRequestId,
        request: historyRequest,
        scripts: buildScripts(),
        environmentId: activeEnvironmentId,
      });
      setScriptRuns(result.scriptRuns);
      await applyEnvironmentUpdates(result.environmentUpdates);
      if (historyWorkspaceId) {
        void recordHistory({
          workspaceId: historyWorkspaceId,
          requestId: historyRequestId,
          requestName: historyRequestName,
          request: historyRequest,
          response: result.response
            ? {
                status: result.response.status,
                statusText: result.response.statusText,
                durationMs: result.response.timing.totalMs,
                receivedBytes: result.response.receivedBytes,
                contentType: result.response.body.contentType,
              }
            : null,
          error:
            result.error ??
            result.scriptRuns.find((run) => run.error)?.error ??
            null,
        })
          .then(() => setHistoryRevision((current) => current + 1))
          .catch((cause: unknown) => {
            setNotice({
              kind: "error",
              text: `Request completed, but history was not saved: ${errorMessage(cause)}`,
            });
          });
      }

      if (!result.response) {
        throw new Error(result.error ?? "Request execution returned no response.");
      }

      setResponse(result.response);
      const failedScript = result.scriptRuns.find((run) => run.error);
      if (result.error || failedScript) {
        setNotice({
          kind: "error",
          text: result.error ?? `Script failed: ${failedScript?.error}`,
        });
        setResponseTab("console");
      } else if (result.scriptRuns.some((run) => run.tests.some((test) => !test.passed))) {
        setResponseTab("tests");
      }
    } catch (cause) {
      setResponse(null);
      const message = errorMessage(cause);
      if (message.toLocaleLowerCase() === "request cancelled") {
        setRequestError(null);
        setNotice({ kind: "info", text: "Request cancelled." });
      } else {
        setRequestError(message);
      }
    } finally {
      sendingRef.current = false;
      setIsSending(false);
    }
  }

  async function cancelRequest() {
    cancelledSendRef.current = true;
    try {
      await cancelHttpRequest();
    } catch (cause) {
      setNotice({ kind: "error", text: errorMessage(cause) });
    }
  }

  async function copyResponseBody(content: string) {
    try {
      await navigator.clipboard.writeText(content);
      setNotice({ kind: "success", text: "Response body copied." });
    } catch {
      setNotice({ kind: "error", text: "Response body could not be copied." });
    }
  }

  async function copyCurrentRequestAsCurl() {
    try {
      const shell = navigator.userAgent.includes("Windows") ? "powerShell" : "posix";
      const command = await exportCurlForEnvironment(
        buildRequest(),
        activeEnvironmentId,
        shell,
      );
      await navigator.clipboard.writeText(command);
      setNotice({
        kind: "success",
        text: "cURL copied. Review it before sharing because it may contain credentials.",
      });
    } catch (cause) {
      setNotice({ kind: "error", text: errorMessage(cause) });
    }
  }

  function restoreHistoryEntry(entry: HistoryEntry) {
    if (isSending) {
      return;
    }
    const tab = createScratchTab();
    tab.title = `${entry.requestName} (history)`;
    tab.request = structuredClone(entry.request);
    tab.dirty = true;
    tab.requestTab = entry.request.body.kind === "empty" ? "headers" : "body";
    setRequestTabs((current) => [...current, tab]);
    setActiveRequestTabId(tab.id);
    applyRequestTab(tab);
    setHistoryOpen(false);
    setNotice({
      kind: entry.requestBodyOmitted ? "info" : "success",
      text: entry.requestBodyOmitted
        ? "History restored without its oversized request body."
        : "History restored into a new unsaved request tab.",
    });
  }

  async function saveCurrentResponse() {
    if (!response || isSavingResponse) {
      return;
    }
    setIsSavingResponse(true);
    try {
      const path = await saveResponseBody(response.body, responseFileName(response));
      if (path) {
        setNotice({ kind: "success", text: "Response body saved." });
      }
    } catch (cause) {
      setNotice({ kind: "error", text: errorMessage(cause) });
    } finally {
      setIsSavingResponse(false);
    }
  }

  function handleSubmit(event: FormEvent) {
    event.preventDefault();
    void sendRequest();
  }

  function handleUrlKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter" && curlCommandFromClipboard(event.currentTarget.value)) {
      event.preventDefault();
      event.stopPropagation();
      void handleCurlImport(event.currentTarget.value);
      return;
    }
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
      event.preventDefault();
      event.stopPropagation();
      void sendRequest();
    }
  }

  function confirmDiscard(): boolean {
    return (
      !hasUnsavedTabs ||
      window.confirm("One or more request tabs have unsaved changes. Discard them?")
    );
  }

  function startScratchRequest() {
    if (isSending) {
      setNotice({ kind: "info", text: "Finish or cancel the active request first." });
      return;
    }
    const tab = createScratchTab();
    setRequestTabs((current) => [...current, tab]);
    setActiveRequestTabId(tab.id);
    applyRequestTab(tab);
  }

  // Postman-style "add request" from a folder/collection row: opens a fresh tab and jumps
  // straight to naming + saving it into that exact location, instead of Save Request's generic
  // destination picker defaulting to wherever it last left off.
  function startRequestForLocation(collectionId: string, folderId: string | null) {
    if (isSending) {
      setNotice({ kind: "info", text: "Finish or cancel the active request first." });
      return;
    }
    const tab = createScratchTab();
    setRequestTabs((current) => [...current, tab]);
    setActiveRequestTabId(tab.id);
    applyRequestTab(tab);
    showModal({ kind: "saveRequest", preferredLocation: { collectionId, folderId } });
  }

  function applyRequestTab(tab: OpenRequestTab) {
    setSelectedRequestId(tab.requestId);
    setMethod(tab.request.method);
    const splitUrl = splitRequestUrl(tab.request.url);
    setUrl(splitUrl.url);
    setQueryParams([
      ...splitUrl.queryParams,
      ...tab.request.queryParams.map((parameter) => ({ ...parameter })),
    ]);
    setHeaders(tab.request.headers.map((header) => ({ ...header })));
    setAuth(structuredClone(tab.request.auth));
    setRequestOptions({ ...tab.request.options });
    setEditorBody(tab.request.body, tab.request.headers);
    setPreRequestScript(tab.scripts.preRequest);
    setPostResponseScript(tab.scripts.postResponse);
    setRequestTab(tab.requestTab);
    setScriptTab(tab.scriptTab);
    setResponse(tab.response);
    setScriptRuns(structuredClone(tab.scriptRuns));
    setResponseTab(tab.responseTab);
    setRequestError(tab.requestError);
    setIsDirty(tab.dirty);
  }

  function resetRequestTabs() {
    const tab = createScratchTab();
    setRequestTabs([tab]);
    setActiveRequestTabId(tab.id);
    applyRequestTab(tab);
  }

  function activateRequestTab(tabId: string) {
    if (tabId === activeRequestTabId || isSending) {
      return;
    }
    const tab = requestTabs.find((candidate) => candidate.id === tabId);
    if (!tab) {
      return;
    }
    setActiveRequestTabId(tab.id);
    applyRequestTab(tab);
  }

  function closeOpenRequestTab(tabId: string) {
    if (isSending) {
      setNotice({ kind: "info", text: "Finish or cancel the active request first." });
      return;
    }
    const closing = requestTabs.find((tab) => tab.id === tabId);
    if (!closing) {
      return;
    }
    const dirty = tabId === activeRequestTabId ? isDirty : closing.dirty;
    if (dirty && !window.confirm(`Close "${closing.title}" with unsaved changes?`)) {
      return;
    }

    const nextActive = requestTabAfterClose(requestTabs, tabId);
    const remaining = requestTabs.filter((tab) => tab.id !== tabId);
    if (tabId !== activeRequestTabId) {
      setRequestTabs(remaining);
      return;
    }
    if (nextActive) {
      setRequestTabs(remaining);
      setActiveRequestTabId(nextActive.id);
      applyRequestTab(nextActive);
    } else {
      resetRequestTabs();
    }
  }

  function duplicateActiveRequestTab() {
    if (isSending) {
      return;
    }
    const active = requestTabs.find((tab) => tab.id === activeRequestTabId);
    if (!active) {
      return;
    }
    const source: OpenRequestTab = {
      ...active,
      requestId: selectedRequestId,
      title: selectedRequest?.name ?? "Scratch request",
      request: structuredClone(buildRequest()),
      scripts: structuredClone(buildScripts()),
      dirty: isDirty,
      requestTab,
      responseTab,
      scriptTab,
      response,
      scriptRuns,
      requestError,
    };
    const duplicate = duplicateRequestTab(source, crypto.randomUUID());
    setRequestTabs((current) => [...current, duplicate]);
    setActiveRequestTabId(duplicate.id);
    applyRequestTab(duplicate);
  }

  function setEditorBody(nextBody: RequestDefinition["body"], nextHeaders: HeaderEntry[]) {
    setBody("");
    setBodyContentType(DEFAULT_RAW_CONTENT_TYPE);
    setFormFields([]);
    setMultipartParts([]);
    setBinaryFilePath("");
    setBinaryContentType("application/octet-stream");

    switch (nextBody.kind) {
      case "empty":
        setBodyMode("none");
        break;
      case "text":
        setBodyMode("raw");
        setBody(nextBody.content);
        setBodyContentType(
          nextHeaders.find((header) => header.enabled && header.name.trim().toLowerCase() === "content-type")?.value
          ?? nextBody.contentType
          ?? DEFAULT_RAW_CONTENT_TYPE,
        );
        break;
      case "formUrlEncoded":
        setBodyMode("formUrlEncoded");
        setFormFields(nextBody.fields.map((field) => ({ ...field })));
        break;
      case "multipart":
        setBodyMode("multipart");
        setMultipartParts(
          nextBody.parts.map((part) => ({ ...part, value: { ...part.value } })),
        );
        break;
      case "binary":
        setBodyMode("binary");
        setBinaryFilePath(nextBody.path);
        setBinaryContentType(nextBody.contentType ?? "application/octet-stream");
        break;
    }
  }

  async function chooseRequestFile(): Promise<string | null> {
    try {
      return await pickRequestFile();
    } catch (cause) {
      setNotice({ kind: "error", text: errorMessage(cause) });
      return null;
    }
  }

  function loadRequest(request: SavedRequest) {
    if (request.id === selectedRequestId || isSending) {
      return;
    }
    const open = requestTabs.find((tab) => tab.requestId === request.id);
    const tab = open ?? createSavedRequestTab(request);
    if (!open) {
      setRequestTabs((current) => [...current, tab]);
    }
    setActiveRequestTabId(tab.id);
    applyRequestTab(tab);
  }

  async function applySession(
    next: WorkspaceSession,
    offerRecovery = true,
  ): Promise<boolean> {
    let recoveredTabs: OpenRequestTab[] | null = null;
    let recoveredActiveTabId: string | null = null;
    let recoveredEnvironmentId: string | null = null;
    if (offerRecovery) {
      try {
        const recovery = await loadRecoverySnapshot(next.workspace.id);
        if (
          recovery &&
          window.confirm(
            `Recovered ${recovery.tabs.length} request tab${recovery.tabs.length === 1 ? "" : "s"} from the previous session. Restore them?`,
          )
        ) {
          recoveredTabs = restoreRecoveryTabs(recovery).map((tab) => {
            if (!tab.requestId || findSavedRequest(next.workspace, tab.requestId)) {
              return tab;
            }
            return {
              ...tab,
              requestId: null,
              title: `${tab.title} (recovered)`,
              dirty: true,
            };
          });
          recoveredActiveTabId = recoveredTabs.some(
            (tab) => tab.id === recovery.activeTabId,
          )
            ? recovery.activeTabId
            : (recoveredTabs[0]?.id ?? null);
          recoveredEnvironmentId = next.workspace.environments.some(
            (environment) => environment.id === recovery.activeEnvironmentId,
          )
            ? recovery.activeEnvironmentId
            : null;
        } else if (recovery) {
          await clearRecoverySnapshot(next.workspace.id);
        }
      } catch (cause) {
        setNotice({
          kind: "error",
          text: `Draft recovery was ignored: ${errorMessage(cause)}`,
        });
      }
    }

    sessionRef.current = next;
    setSession(next);
    try {
      setSecretStatuses(await environmentSecretStatuses());
    } catch {
      setSecretStatuses({});
    }
    setExpandedIds(new Set(next.workspace.collections.map((collection) => collection.id)));
    setSearchQuery("");
    if (recoveredTabs?.length && recoveredActiveTabId) {
      const active = recoveredTabs.find((tab) => tab.id === recoveredActiveTabId)!;
      setRequestTabs(recoveredTabs);
      setActiveRequestTabId(active.id);
      setActiveEnvironmentId(recoveredEnvironmentId);
      applyRequestTab(active);
    } else {
      setActiveEnvironmentId(next.workspace.environments[0]?.id ?? null);
      resetRequestTabs();
    }
    setRecoveryReadyWorkspaceId(next.workspace.id);
    return Boolean(recoveredTabs?.length);
  }

  function applyImportedRequest(request: RequestDefinition, targetTabId: string) {
    const tab = createScratchTab();
    tab.title = "Imported request";
    tab.request = request;
    tab.dirty = true;
    tab.requestTab = request.body.kind === "empty" ? "headers" : "body";
    setRequestTabs((current) => {
      const target = current.find((candidate) => candidate.id === targetTabId);
      return target && !target.dirty && !target.requestId && !target.response
        ? current.map((candidate) => candidate.id === targetTabId ? tab : candidate)
        : [...current, tab];
    });
    setActiveRequestTabId(tab.id);
    applyRequestTab(tab);
  }

  async function handleCurlImport(command: string): Promise<boolean> {
    if (!command.trim() || importingCurlRef.current) {
      return false;
    }
    if (sendingRef.current || workspaceTransitionRef.current) {
      setNotice({ kind: "info", text: "Finish or cancel the active operation before importing a request." });
      return false;
    }

    importingCurlRef.current = true;
    setIsImportingCurl(true);
    setNotice(null);
    setCurlImportError(null);
    try {
      const imported = await importCurl(curlCommandFromClipboard(command) ?? command);
      applyImportedRequest(imported.request, activeRequestTabId);
      setNotice({
        kind: imported.warnings.length ? "info" : "success",
        text: imported.warnings.length
          ? `cURL imported. ${imported.warnings.join(" ")}`
          : "cURL imported. URL, method, headers, and body are ready. Select Send to run it.",
      });
      return true;
    } catch (cause) {
      const message = errorMessage(cause);
      setCurlImportError(message);
      setNotice({ kind: "error", text: message });
      return false;
    } finally {
      importingCurlRef.current = false;
      setIsImportingCurl(false);
    }
  }

  async function handleApiDocumentPick() {
    if (!session || isWorkspaceBusy || isImportingApiDocument) {
      return;
    }
    setIsImportingApiDocument(true);
    setNotice(null);
    try {
      const imported = await importApiDocumentFile();
      if (imported) {
        setPendingApiImport(imported);
      }
    } catch (cause) {
      setNotice({ kind: "error", text: errorMessage(cause) });
    } finally {
      setIsImportingApiDocument(false);
    }
  }

  async function handleApiDocumentConfirm() {
    if (!session || !pendingApiImport || isWorkspaceBusy) {
      return;
    }
    const imported = pendingApiImport;
    const saved = await persistWorkspace(
      (workspace) => ({ workspace: mergeApiDocumentImport(workspace, imported) }),
    );
    if (!saved) {
      return;
    }
    setExpandedIds((current) => {
      const next = new Set(current);
      for (const collection of imported.collections) {
        next.add(collection.id);
      }
      return next;
    });
    if (!activeEnvironmentId && imported.environments[0]) {
      setActiveEnvironmentId(imported.environments[0].id);
    }
    setPendingApiImport(null);
    const requests = imported.collections.reduce(
      (total, collection) => total + countRequests(collection),
      0,
    );
    setNotice({
      kind: imported.warnings.length ? "info" : "success",
      text: `Imported ${imported.name}: ${requests} request${requests === 1 ? "" : "s"}, ${imported.environments.length} environment${imported.environments.length === 1 ? "" : "s"}.`,
    });
  }

  function handleUrlPaste(event: ClipboardEvent<HTMLInputElement>) {
    const pasted = event.clipboardData.getData("text/plain") || event.clipboardData.getData("text");
    const command = curlCommandFromClipboard(pasted);
    if (command) {
      event.preventDefault();
      void handleCurlImport(command).then((imported) => {
        if (imported) urlInputRef.current?.focus();
      });
    }
  }

  async function persistWorkspace(update: WorkspaceUpdate): Promise<boolean> {
    const workspaceId = sessionRef.current?.workspace.id;
    if (!workspaceId || workspaceTransitionRef.current) {
      return false;
    }
    setNotice(null);
    try {
      await persistenceRef.current!.enqueue(workspaceId, update);
      return true;
    } catch (cause) {
      setNotice({ kind: "error", text: errorMessage(cause) });
      return false;
    }
  }

  function showModal(next: ModalState) {
    setModal(next);
    setModalName(
      next.kind === "saveRequest"
        ? "Untitled request"
        : next.kind === "renameItem"
          ? next.target.name
          : "",
    );

    if (
      next.kind === "saveRequest" ||
      next.kind === "newFolder" ||
      next.kind === "moveRequest" ||
      next.kind === "moveFolder"
    ) {
      const preferredLocation =
        next.kind === "moveRequest"
          ? session
            ? findRequestLocation(session.workspace, next.target.requestId)
            : undefined
          : next.kind === "moveFolder"
            ? session
              ? findFolderLocation(session.workspace, next.target.folderId)
              : undefined
          : next.preferredLocation;
      const availableDestinations =
        next.kind === "moveFolder" && session
          ? destinations.filter((destination) =>
              canMoveFolderTo(
                session.workspace,
                next.target.folderId,
                destination.location,
              ),
            )
          : destinations;
      const preferredKey = preferredLocation
        ? destinationId(preferredLocation)
        : undefined;
      const preferred = availableDestinations.some(
        (destination) => destination.key === preferredKey,
      )
        ? preferredKey
        : availableDestinations[0]?.key;
      setDestinationKey(preferred ?? "");
    }
  }

  async function handleWorkspaceMaintenance(action: WorkspaceMaintenanceAction) {
    if (sendingRef.current || workspaceTransitionRef.current) {
      throw new Error("Wait for the current request or workspace operation to finish.");
    }
    if (action === "restore" && !confirmDiscard()) return;
    workspaceTransitionRef.current = true;
    setNotice(null);
    try {
      await persistenceRef.current!.idle();
      setIsWorkspaceBusy(true);
      if (action === "restore") {
        const restored = await restoreWorkspaceBackup();
        if (restored) {
          await applySession(restored, false);
          setNotice({ kind: "success", text: `Restored ${restored.workspace.name} as a separate workspace. Re-enter Secret and Local variable values before use.` });
        }
      } else {
        const path = action === "backup" ? await exportWorkspaceBackup() : await exportSupportDiagnostics();
        if (path) {
          setNotice({ kind: "success", text: action === "backup"
            ? "Private backup saved. Only saved workspace data was included; device-only values stay in the OS credential store."
            : "Diagnostics saved locally. Request data and credentials were excluded." });
        }
      }
    } finally {
      workspaceTransitionRef.current = false;
      setIsWorkspaceBusy(false);
    }
  }

  async function handleOpenWorkspace() {
    if (sendingRef.current || workspaceTransitionRef.current || persistenceRef.current!.busy || !confirmDiscard()) {
      return;
    }

    workspaceTransitionRef.current = true;
    setIsWorkspaceBusy(true);
    setNotice(null);
    try {
      const opened = await openWorkspace();
      if (opened) {
        const recovered = await applySession(opened);
        setNotice({
          kind: "success",
          text: recovered
            ? `Opened ${opened.workspace.name} and restored your request tabs.`
            : `Opened ${opened.workspace.name}`,
        });
      }
    } catch (cause) {
      setNotice({ kind: "error", text: errorMessage(cause) });
    } finally {
      workspaceTransitionRef.current = false;
      setIsWorkspaceBusy(false);
    }
  }

  async function handleCloseWorkspace() {
    if (!session || sendingRef.current || workspaceTransitionRef.current || persistenceRef.current!.busy || !confirmDiscard()) {
      return;
    }

    workspaceTransitionRef.current = true;
    setIsWorkspaceBusy(true);
    try {
      await clearRecoverySnapshot(session.workspace.id);
      await closeWorkspace();
      sessionRef.current = null;
      setSession(null);
      setRecoveryReadyWorkspaceId(null);
      setActiveEnvironmentId(null);
      setSecretStatuses({});
      setExpandedIds(new Set());
      resetRequestTabs();
      setNotice({ kind: "info", text: "Workspace closed. Files remain on disk." });
    } catch (cause) {
      setNotice({ kind: "error", text: errorMessage(cause) });
    } finally {
      workspaceTransitionRef.current = false;
      setIsWorkspaceBusy(false);
    }
  }

  function finishRequestSave(tabId: string, saved: SavedRequest) {
    setRequestTabs((current) => current.map((tab) => tab.id === tabId ? markRequestTabSaved(tab, saved) : tab));
    const editor = editorRef.current;
    if (editor.tabId === tabId) {
      setSelectedRequestId(saved.id);
      setIsDirty(JSON.stringify(editor.request) !== JSON.stringify(saved.request)
        || JSON.stringify(editor.scripts) !== JSON.stringify(saved.scripts));
    }
  }

  async function handleSaveRequest() {
    if (!session) {
      setNotice({
        kind: "info",
        text: "Create or open a workspace before saving this request.",
      });
      showModal({ kind: "newWorkspace", preserveRequest: true });
      return;
    }

    if (!session.workspace.collections.length) {
      setNotice({
        kind: "info",
        text: "Create a collection before saving this request.",
      });
      showModal({ kind: "newCollection" });
      return;
    }

    if (!selectedRequest) {
      showModal({ kind: "saveRequest" });
      return;
    }

    const updated: SavedRequest = {
      ...selectedRequest,
      request: buildRequest(),
      scripts: buildScripts(),
    };
    const saved = await persistWorkspace((workspace) => ({ workspace: upsertRequest(workspace, updated) }));
    if (saved) {
      finishRequestSave(activeRequestTabId, updated);
      setNotice({ kind: "success", text: `Saved ${updated.name}` });
    }
  }

  async function handleModalSubmit(event: FormEvent) {
    event.preventDefault();
    const name = modalName.trim();
    if (!modal || isWorkspaceBusy) {
      return;
    }
    if (modal.kind !== "moveRequest" && modal.kind !== "moveFolder" && !name) {
      return;
    }

    if (modal.kind === "newWorkspace") {
      if (sendingRef.current || workspaceTransitionRef.current || persistenceRef.current!.busy) {
        return;
      }
      if (!modal.preserveRequest && !confirmDiscard()) {
        return;
      }
      workspaceTransitionRef.current = true;
      setIsWorkspaceBusy(true);
      setNotice(null);
      try {
        const created = await createWorkspace(name);
        if (created) {
          if (modal.preserveRequest) {
            sessionRef.current = created;
            setSession(created);
            setRecoveryReadyWorkspaceId(created.workspace.id);
            setExpandedIds(
              new Set(created.workspace.collections.map((collection) => collection.id)),
            );
            setSearchQuery("");
            setSelectedRequestId(null);
            const firstDestination = workspaceDestinations(created.workspace)[0];
            setDestinationKey(firstDestination?.key ?? "");
            setModalName("Untitled request");
            setModal({ kind: "saveRequest" });
          } else {
            await applySession(created, false);
            setModal(null);
          }
          setNotice({
            kind: "success",
            text: `Created ${created.workspace.name} at ${created.rootPath}`,
          });
        }
      } catch (cause) {
        setNotice({ kind: "error", text: errorMessage(cause) });
      } finally {
        workspaceTransitionRef.current = false;
        setIsWorkspaceBusy(false);
      }
      return;
    }

    if (!session) {
      setNotice({ kind: "error", text: "No workspace is open." });
      return;
    }

    if (modal.kind === "renameItem") {
      const saved = await persistWorkspace(
        (workspace) => ({ workspace: renameWorkspaceItem(workspace, modal.target, name) }),
      );
      if (saved) {
        setModal(null);
        setNotice({ kind: "success", text: `Renamed to ${name}` });
      }
      return;
    }

    if (modal.kind === "moveRequest") {
      const request = findSavedRequest(session.workspace, modal.target.requestId);
      const destination = destinations.find((option) => option.key === destinationKey);
      if (!request || !destination) {
        setNotice({ kind: "error", text: "Choose a valid request destination." });
        return;
      }
      const saved = await persistWorkspace(
        (workspace) => ({ workspace: upsertRequest(workspace, request, destination.location) }),
      );
      if (saved) {
        setExpandedIds((current) => {
          const next = new Set(current);
          next.add(destination.location.collectionId);
          if (destination.location.folderId) {
            next.add(destination.location.folderId);
          }
          return next;
        });
        setModal(null);
        setNotice({ kind: "success", text: `Moved ${request.name}` });
      }
      return;
    }

    if (modal.kind === "moveFolder") {
      const destination = modalDestinations.find(
        (option) => option.key === destinationKey,
      );
      if (!destination) {
        setNotice({ kind: "error", text: "Choose a valid folder destination." });
        return;
      }
      const saved = await persistWorkspace(
        (workspace) => ({ workspace: moveFolder(
          workspace,
          modal.target.folderId,
          destination.location,
        ) }),
      );
      if (saved) {
        setExpandedIds((current) => {
          const next = new Set(current);
          next.add(destination.location.collectionId);
          next.add(modal.target.folderId);
          if (destination.location.folderId) {
            next.add(destination.location.folderId);
          }
          return next;
        });
        setModal(null);
        setNotice({ kind: "success", text: `Moved ${modal.target.name}` });
      }
      return;
    }

    if (modal.kind === "newCollection") {
      const collection: Collection = {
        id: crypto.randomUUID(),
        name,
        folders: [],
        requests: [],
      };
      const saved = await persistWorkspace((workspace) => ({ workspace: addCollection(workspace, collection) }));
      if (saved) {
        setExpandedIds((current) => new Set(current).add(collection.id));
        setModal(null);
      }
      return;
    }

    if (modal.kind === "newFolder") {
      const destination = destinations.find((option) => option.key === destinationKey);
      if (!destination) {
        setNotice({ kind: "error", text: "Choose a collection or parent folder." });
        return;
      }
      const folder: RequestFolder = {
        id: crypto.randomUUID(),
        name,
        folders: [],
        requests: [],
      };
      const saved = await persistWorkspace(
        (workspace) => ({ workspace: addFolder(
          workspace,
          destination.location.collectionId,
          destination.location.folderId,
          folder,
        ) }),
      );
      if (saved) {
        setExpandedIds((current) => {
          const next = new Set(current);
          next.add(destination.location.collectionId);
          next.add(folder.id);
          if (destination.location.folderId) {
            next.add(destination.location.folderId);
          }
          return next;
        });
        setModal(null);
      }
      return;
    }

    const destination = destinations.find((option) => option.key === destinationKey);
    if (!destination) {
      setNotice({ kind: "error", text: "Choose a collection or folder." });
      return;
    }

    const request: SavedRequest = {
      id: crypto.randomUUID(),
      name,
      request: buildRequest(),
      scripts: buildScripts(),
    };
    const saved = await persistWorkspace(
      (workspace) => ({ workspace: upsertRequest(workspace, request, destination.location) }),
    );
    if (saved) {
      finishRequestSave(activeRequestTabId, request);
      setExpandedIds((current) => {
        const next = new Set(current);
        next.add(destination.location.collectionId);
        if (destination.location.folderId) {
          next.add(destination.location.folderId);
        }
        return next;
      });
      setModal(null);
      setNotice({ kind: "success", text: `Saved ${request.name}` });
    }
  }

  async function handleSaveEnvironments(
    environments: Environment[],
    mutations: SecretMutation[],
  ): Promise<boolean> {
    if (!session) {
      setNotice({ kind: "error", text: "Open a workspace before adding environments." });
      return false;
    }

    const saved = await persistWorkspace((workspace) => ({
      workspace: { ...workspace, environments }, mutations,
    }));
    if (saved) {
      setSecretStatuses(
        Object.fromEntries(
          environments.flatMap((environment) =>
            environment.variables
              .filter((variable) => variable.secret || variable.local)
              .map((variable) => [variable.id, true]),
          ),
        ),
      );
      if (!environments.some((environment) => environment.id === activeEnvironmentId)) {
        setActiveEnvironmentId(environments[0]?.id ?? null);
      }
      setNotice({ kind: "success", text: "Environments saved." });
      return true;
    }
    return false;
  }

  async function openEnvironmentEditor() {
    if (!session || isLoadingSecrets) {
      return;
    }
    setIsLoadingSecrets(true);
    setNotice(null);
    try {
      const [statuses, localValues] = await Promise.all([
        environmentSecretStatuses(),
        environmentLocalValues(),
      ]);
      setSecretStatuses(statuses);
      setLocalEnvironmentValues(localValues);
      setEnvironmentEditorOpen(true);
    } catch (cause) {
      setNotice({ kind: "error", text: errorMessage(cause) });
    } finally {
      setIsLoadingSecrets(false);
    }
  }

  async function handleSaveNetworkSettings(settings: NetworkSettings): Promise<boolean> {
    if (!session) {
      setNotice({ kind: "error", text: "Open a workspace before changing network settings." });
      return false;
    }

    const saved = await persistWorkspace((workspace) => ({ workspace: { ...workspace, network: settings } }));
    if (saved) {
      setNetworkSettingsOpen(false);
      setNotice({ kind: "success", text: "Network and TLS settings saved." });
    }
    return saved;
  }

  async function handleSaveProtocolProfiles(
    protocolProfiles: ProtocolProfile[],
  ): Promise<boolean> {
    if (!session) {
      setNotice({ kind: "error", text: "Open a workspace before saving protocol profiles." });
      return false;
    }
    const saved = await persistWorkspace((workspace) => ({ workspace: { ...workspace, protocolProfiles } }));
    if (saved) {
      setNotice({ kind: "success", text: "Protocol profiles saved locally." });
    }
    return saved;
  }

  async function handleSaveAuthPolicy(
    nextAuth: RequestAuth | undefined,
  ): Promise<boolean> {
    if (!session || !authPolicyTarget) {
      setNotice({ kind: "error", text: "The authentication policy target is unavailable." });
      return false;
    }
    const targetName = authPolicyTarget.name;
    const saved = await persistWorkspace(
      (workspace) => ({ workspace: setAuthPolicy(workspace, authPolicyTarget, nextAuth) }),
    );
    if (saved) {
      setAuthPolicyTarget(null);
      setNotice({ kind: "success", text: `Authentication policy saved for ${targetName}.` });
    }
    return saved;
  }

  async function handleDuplicateWorkspaceItem(target: WorkspaceItemTarget) {
    if (!session || isWorkspaceBusy) {
      return;
    }
    const duplicateId = crypto.randomUUID();
    const saved = await persistWorkspace((workspace) => ({ workspace: duplicateWorkspaceItem(
      workspace,
      target,
      duplicateId,
      () => crypto.randomUUID(),
    ) }));
    if (saved) {
      if (target.kind !== "request") {
        setExpandedIds((current) => new Set(current).add(duplicateId));
      }
      setWorkspaceAction(null);
      setNotice({ kind: "success", text: `Duplicated ${target.name}` });
    }
  }

  async function handleReorderWorkspaceItem(
    target: WorkspaceItemTarget,
    offset: -1 | 1,
  ) {
    if (!session || isWorkspaceBusy) {
      return;
    }
    const next = reorderWorkspaceItem(session.workspace, target, offset);
    if (next === session.workspace) {
      setWorkspaceAction(null);
      setNotice({
        kind: "info",
        text: `${target.name} is already at that edge of its group.`,
      });
      return;
    }
    const saved = await persistWorkspace((workspace) => ({ workspace: reorderWorkspaceItem(workspace, target, offset) }));
    if (saved) {
      setWorkspaceAction(null);
      setNotice({
        kind: "success",
        text: `Moved ${target.name} ${offset < 0 ? "up" : "down"}.`,
      });
    }
  }

  async function handleDeleteWorkspaceItem(target: WorkspaceItemTarget) {
    if (!session || isWorkspaceBusy) {
      return;
    }
    const includesChildren = target.kind !== "request";
    const message = includesChildren
      ? `Delete ${target.name} and everything inside it? This cannot be undone.`
      : `Delete ${target.name}? This cannot be undone.`;
    if (!window.confirm(message)) {
      return;
    }

    const next = removeWorkspaceItem(session.workspace, target);
    const saved = await persistWorkspace((workspace) => ({ workspace: removeWorkspaceItem(workspace, target) }));
    if (saved) {
      setWorkspaceAction(null);
      const remainingTabs = requestTabs.filter(
        (tab) => !tab.requestId || Boolean(findSavedRequest(next, tab.requestId)),
      );
      const activeWasRemoved = !remainingTabs.some(
        (tab) => tab.id === activeRequestTabId,
      );
      if (activeWasRemoved) {
        const nextTab = remainingTabs[0];
        if (nextTab) {
          setRequestTabs(remainingTabs);
          setActiveRequestTabId(nextTab.id);
          applyRequestTab(nextTab);
        } else {
          resetRequestTabs();
        }
      } else {
        setRequestTabs(remainingTabs);
      }
      setNotice({ kind: "success", text: `Deleted ${target.name}` });
    }
  }

  async function handleCurlModalSubmit(event: FormEvent) {
    event.preventDefault();
    if (await handleCurlImport(curlText)) {
      setCurlImportOpen(false);
      setCurlText("");
    }
  }

  function toggleTreeNode(id: string) {
    setExpandedIds((current) => {
      const next = new Set(current);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }

  return (
    <div
      className={sidebarOpen ? "app-shell sidebar-open" : "app-shell"}
      style={{ "--sidebar-width": `${sidebarWidth}px` } as CSSProperties}
    >
      <header className="topbar">
        <button
          className="icon-button sidebar-toggle"
          type="button"
          aria-label={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
          title={sidebarOpen ? "Hide sidebar" : "Show sidebar"}
          onClick={() => setSidebarOpen((open) => !open)}
        >
          <AppIcon name="panel" />
        </button>
        <div className="brand-mark" aria-hidden="true">
          <AppIcon name="terminal" />
        </div>
        <div className="brand-copy">
          <strong>Reqglide</strong>
          <span>{session?.workspace.name ?? "Local workspace"}</span>
        </div>
        <div className="privacy-pill">
          <AppIcon name="shield" /> Local only
        </div>
        <button className="command-shortcut" type="button" aria-label="Open command palette" title="Search commands (Ctrl+K)" onClick={() => setCommandPaletteOpen(true)}>
          <AppIcon name="search" /><span>Quick actions</span><kbd>Ctrl K</kbd>
        </button>
        {session?.workspace.network?.acceptInvalidCertificates && (
          <button
            className="tls-warning-pill"
            type="button"
            title="TLS certificate verification is disabled for this workspace"
            onClick={() => setNetworkSettingsOpen(true)}
          >
            TLS verification off
          </button>
        )}
        <button
          className="ghost-button"
          type="button"
          onClick={() => {
            setCurlText("");
            setCurlImportError(null);
            setCurlImportOpen(true);
          }}
        >
          <AppIcon name="import" /> Import cURL
        </button>
        <details className="topbar-menu">
          <summary>Tools</summary>
          <div className="topbar-menu-popover">
            <button
              type="button"
              disabled={!session || isWorkspaceBusy || isSending}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                setMaintenanceAction("backup");
              }}
            >
              <strong>Back up workspace</strong>
              <span>Private export of saved workspace data</span>
            </button>
            <button
              type="button"
              disabled={isWorkspaceBusy || isSending}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                setMaintenanceAction("restore");
              }}
            >
              <strong>Restore workspace backup</strong>
              <span>Restore into a separate workspace folder</span>
            </button>
            <button
              type="button"
              disabled={isWorkspaceBusy || isSending}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                setMaintenanceAction("diagnostics");
              }}
            >
              <strong>Export support diagnostics</strong>
              <span>App and configuration summary only</span>
            </button>
            <button
              type="button"
              disabled={!session || isWorkspaceBusy || isImportingApiDocument}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                void handleApiDocumentPick();
              }}
            >
              <strong>{isImportingApiDocument ? "Importing…" : "Import API file"}</strong>
              <span>Postman, OpenAPI, or Swagger</span>
            </button>
            <button
              type="button"
              disabled={!session || isWorkspaceBusy || isLoadingSecrets}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                void openEnvironmentEditor();
              }}
            >
              <strong>{isLoadingSecrets ? "Opening…" : "Environments"}</strong>
              <span>Variables and OS-keychain secrets</span>
            </button>
            <button
              type="button"
              disabled={!session}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                setHistoryOpen(true);
              }}
            >
              <strong>History</strong>
              <span>Search, pin, and restore requests</span>
            </button>
            <button
              type="button"
              disabled={!session || isWorkspaceBusy}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                setCollectionRunnerOpen(true);
              }}
            >
              <strong>Collection runner</strong>
              <span>Iterations, data files, and reports</span>
            </button>
            <button
              type="button"
              disabled={!session || isWorkspaceBusy}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                setProtocolWorkbenchOpen(true);
              }}
            >
              <strong>Protocol workbench</strong>
              <span>GraphQL, WebSocket, SSE, and gRPC</span>
            </button>
            <button
              type="button"
              disabled={!session}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                setCookieManagerOpen(true);
              }}
            >
              <strong>Cookies</strong>
              <span>Inspect the local workspace jar</span>
            </button>
            <button
              type="button"
              disabled={!session || isWorkspaceBusy}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                setNetworkSettingsOpen(true);
              }}
            >
              <strong>Network and TLS</strong>
              <span>Proxy, CA, mTLS, and verification</span>
            </button>
            <button
              type="button"
              disabled={!session || isWorkspaceBusy}
              onClick={(event) => {
                closeDetailsMenu(event.currentTarget);
                if (session) {
                  setAuthPolicyTarget({ kind: "workspace", name: session.workspace.name });
                }
              }}
            >
              <strong>Workspace authentication</strong>
              <span>Inherited credentials and OAuth 2.0</span>
            </button>
          </div>
        </details>
        <button
          className="ghost-button"
          type="button"
          disabled={isWorkspaceBusy || isSending}
          onClick={() => showModal({ kind: "newWorkspace" })}
        >
          New workspace
        </button>
        <button
          className="ghost-button"
          type="button"
          disabled={isWorkspaceBusy || isSending}
          onClick={() => void handleOpenWorkspace()}
        >
          Open
        </button>
        <button
          className="primary-small"
          type="button"
          disabled={isWorkspaceBusy}
          onClick={() => void handleSaveRequest()}
        >
          <AppIcon name="save" /> {isWorkspaceBusy ? "Saving…" : "Save request"}
        </button>
      </header>

      <aside className="sidebar">
        <div className="sidebar-eyebrow">EXPLORER <span>LOCAL WORKSPACE</span></div>
        <div className="sidebar-heading">
          <span title={session?.rootPath}>{session?.workspace.name ?? "Workspace"}</span>
          <button
            type="button"
            aria-label="Create collection"
            title="Create collection"
            disabled={!session || isWorkspaceBusy}
            onClick={() => showModal({ kind: "newCollection" })}
          >
            +
          </button>
        </div>
        <label className="search-box">
          <AppIcon name="search" />
          <input
            aria-label="Search workspace"
            placeholder="Search requests or paste cURL"
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            onPaste={handleUrlPaste}
          />
        </label>

        {session && (
          <div className="sidebar-create-actions">
            <button
              type="button"
              disabled={isWorkspaceBusy}
              onClick={() => showModal({ kind: "newCollection" })}
            >
              + Collection
            </button>
            <button
              type="button"
              disabled={!destinations.length || isWorkspaceBusy}
              onClick={() => showModal({ kind: "newFolder" })}
            >
              + Folder
            </button>
          </div>
        )}

        <nav aria-label="Workspace requests" className="tree-scroll">
          <button
            className={`tree-item scratch ${selectedRequestId ? "" : "active"}`}
            type="button"
            onClick={startScratchRequest}
          >
            <span className={`method-badge ${method.toLowerCase()}`}>{method}</span>
            <span className="tree-label">Scratch request</span>
          </button>

          {filteredWorkspace ? (
            <WorkspaceTree
              workspace={filteredWorkspace}
              selectedRequestId={selectedRequestId}
              expandedIds={expandedIds}
              forceExpanded={Boolean(searchQuery.trim())}
              onToggle={toggleTreeNode}
              onSelectRequest={loadRequest}
              onAddRequest={(collectionId, parentFolderId) =>
                startRequestForLocation(collectionId, parentFolderId)
              }
              onAddFolder={(collectionId, parentFolderId) =>
                showModal({
                  kind: "newFolder",
                  preferredLocation: { collectionId, folderId: parentFolderId },
                })
              }
              onManageItem={setWorkspaceAction}
            />
          ) : (
            <div className="workspace-onboarding">
              <strong>Your APIs stay local</strong>
              <span>Create a workspace or open an existing workspace folder.</span>
              <button type="button" onClick={() => showModal({ kind: "newWorkspace" })}>
                Create workspace
              </button>
              <button type="button" onClick={() => void handleOpenWorkspace()}>
                Open workspace
              </button>
            </div>
          )}
        </nav>

        <div className="sidebar-footer">
          <span>No account required</span>
          <span>{session ? session.rootPath : "Data stays on this device"}</span>
        </div>
      </aside>

      {sidebarOpen && (
        <div
          className="sidebar-resize-handle"
          role="separator"
          aria-orientation="vertical"
          aria-label="Resize sidebar"
          onPointerDown={startSidebarResize}
          onDoubleClick={() => {
            setSidebarWidth(250);
            writeStoredLayoutNumber("sidebarWidth", 250);
          }}
        />
      )}

      <main
        className="workspace"
        ref={workspaceRef}
        style={{ "--request-panel-pct": `${requestPanelPct}%` } as CSSProperties}
      >
        <div className="request-titlebar" role="tablist" aria-label="Open requests">
          <div className="open-request-tabs">
            {requestTabs.map((tab) => {
              const active = tab.id === activeRequestTabId;
              const tabMethod = active ? method : tab.request.method;
              const dirty = active ? isDirty : tab.dirty;
              return (
                <div className={`open-request-tab ${active ? "active" : ""}`} key={tab.id}>
                  <button
                    className="open-request-tab-main"
                    type="button"
                    role="tab"
                    aria-selected={active}
                    title={tab.title}
                    onClick={() => activateRequestTab(tab.id)}
                  >
                    <span className={`method-badge ${tabMethod.toLowerCase()}`}>
                      {tabMethod}
                    </span>
                    <span className="open-request-tab-title">{tab.title}</span>
                    {dirty && <span className="unsaved-dot" title="Unsaved changes" />}
                  </button>
                  <button
                    className="open-request-tab-close"
                    type="button"
                    aria-label={`Close ${tab.title}`}
                    title="Close request tab"
                    onClick={() => closeOpenRequestTab(tab.id)}
                  >
                    ×
                  </button>
                </div>
              );
            })}
          </div>
          <button className="new-request-button" type="button" onClick={startScratchRequest}>
            +
            <span>New</span>
          </button>
          <button
            className="duplicate-tab-button"
            type="button"
            title="Duplicate active request tab"
            onClick={duplicateActiveRequestTab}
          >
            Duplicate
          </button>
          {session && (
            <button
              className="icon-button"
              type="button"
              title="Close workspace"
              aria-label="Close workspace"
              disabled={isWorkspaceBusy || isSending}
              onClick={() => void handleCloseWorkspace()}
            >
              ×
            </button>
          )}
        </div>

        <form className="request-panel" onSubmit={handleSubmit}>
          <div className="request-context">
            <div className="request-context-title"><span>HTTP REQUEST</span><strong>{selectedRequest?.name ?? requestTabs.find(tab => tab.id === activeRequestTabId)?.title ?? "Untitled request"}</strong><small>{isDirty ? "Edited" : selectedRequest ? "Saved" : "Draft"}</small></div>
            <div className="request-context-actions">
              <button type="button" onClick={() => void copyCurrentRequestAsCurl()}><AppIcon name="terminal" />Copy cURL</button>
              <button type="button" onClick={() => setCodeSnippetOpen(true)}><AppIcon name="code" />Generate code</button>
            </div>
          </div>
          <div className={`url-row ${isImportingCurl ? "is-importing" : ""}`} aria-busy={isImportingCurl}>
            <select
              aria-label="HTTP method"
              className={`method-select ${method.toLowerCase()}`}
              value={method}
              onChange={(event) => {
                setMethod(event.target.value);
                setIsDirty(true);
              }}
            >
              {(METHODS.includes(method) ? METHODS : [method, ...METHODS]).map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </select>
            <input
              aria-label="Request URL"
              ref={urlInputRef}
              className="url-input"
              spellCheck={false}
              autoComplete="off"
              placeholder="Enter a request URL or paste a cURL command"
              aria-describedby="url-paste-hint"
              disabled={isImportingCurl}
              value={composeRequestUrl(url, queryParams)}
              onChange={(event) => {
                const next = splitRequestUrl(event.target.value);
                setUrl(next.url);
                setQueryParams(next.queryParams);
                setIsDirty(true);
              }}
              onPaste={handleUrlPaste}
              onKeyDown={handleUrlKeyDown}
            />
            <button
              className={`send-button ${isSending ? "cancel" : ""}`}
              type="button"
              disabled={isImportingCurl}
              onClick={() => void (isSending ? cancelRequest() : sendRequest())}
            >
              {!isSending && <AppIcon name="send" />}
              {isSending ? "Cancel" : isImportingCurl ? "Importing…" : "Send"}
              {!isSending && <kbd>⌃↵</kbd>}
            </button>
          </div>
          <div className="composer-hint" id="url-paste-hint">
            <span><AppIcon name="terminal" />{isImportingCurl ? "Reading cURL command…" : "Paste cURL to fill this request automatically"}</span>
            <span><kbd>Ctrl</kbd><kbd>Enter</kbd> to send</span>
          </div>

          <div className="tabs" role="tablist" aria-label="Request configuration">
            {(["params", "auth", "headers", "body", "scripts", "settings"] as const).map(
              (tab) => (
                <button
                  key={tab}
                  className={requestTab === tab ? "active" : undefined}
                  type="button"
                  role="tab"
                  aria-label={tab[0]?.toUpperCase() + tab.slice(1)}
                  aria-selected={requestTab === tab}
                  onClick={() => setRequestTab(tab)}
                >
                  {tab[0]?.toUpperCase() + tab.slice(1)}
                  {tab === "params" && queryParams.length > 0 && (
                    <span className="tab-count">{queryParams.length}</span>
                  )}
                  {tab === "auth" && auth.kind !== "none" && (
                    <span className="tab-count">•</span>
                  )}
                  {tab === "headers" && <span className="tab-count">{headers.length}</span>}
                  {tab === "body" && bodyMode !== "none" && (
                    <span className="tab-count">•</span>
                  )}
                  {tab === "scripts" && (preRequestScript || postResponseScript) && (
                    <span className="tab-count">•</span>
                  )}
                </button>
              ),
            )}
          </div>

          <div className="request-editor">
            {hasJsonFormContentTypeMismatch(buildRequest()) && (
              <div className="request-framing-warning" role="status">
                <span>This JSON body is being sent as application/x-www-form-urlencoded. If this endpoint expects JSON, use application/json.</span>
                <button type="button" onClick={() => {
                  setBodyContentType(DEFAULT_RAW_CONTENT_TYPE);
                  setHeaders((current) => alignContentTypeHeaders(current, DEFAULT_RAW_CONTENT_TYPE));
                  setIsDirty(true);
                }}>Use application/json</button>
              </div>
            )}

            {requestTab === "params" && (
              <KeyValueEditor
                entries={queryParams}
                nameLabel="Parameter"
                addLabel="Add parameter"
                onChange={(entries) => {
                  setQueryParams(entries);
                  setIsDirty(true);
                }}
              />
            )}

            {requestTab === "auth" && (
              <AuthEditor
                auth={auth}
                onChange={(nextAuth) => {
                  setAuth(nextAuth);
                  setIsDirty(true);
                }}
              />
            )}

            {requestTab === "headers" && (
              <HeaderEditor
                request={buildRequest()}
                environment={activeEnvironment}
                environmentName={activeEnvironment?.name ?? null}
                secretStatuses={secretStatuses}
                cookieJarEnabled={session?.workspace.network?.cookieJarEnabled ?? true}
                onChange={(nextHeaders) => {
                  setHeaders(nextHeaders);
                  setIsDirty(true);
                }}
                onOpenEnvironments={() => void openEnvironmentEditor()}
              />
            )}

            {requestTab === "body" && (
              <BodyEditor
                mode={bodyMode}
                rawContent={body}
                rawContentType={bodyContentType}
                formFields={formFields}
                multipartParts={multipartParts}
                binaryFilePath={binaryFilePath}
                binaryContentType={binaryContentType}
                onModeChange={(nextMode) => {
                  setBodyMode(nextMode);
                  setIsDirty(true);
                }}
                onRawContentChange={(content) => {
                  setBody(content);
                  setIsDirty(true);
                }}
                onRawContentTypeChange={(contentType) => {
                  setBodyContentType(contentType);
                  setHeaders((current) => alignContentTypeHeaders(current, contentType));
                  setIsDirty(true);
                }}
                onFormFieldsChange={(fields) => {
                  setFormFields(fields);
                  setIsDirty(true);
                }}
                onMultipartPartsChange={(parts) => {
                  setMultipartParts(parts);
                  setIsDirty(true);
                }}
                onBinaryFilePathChange={(path) => {
                  setBinaryFilePath(path);
                  setIsDirty(true);
                }}
                onBinaryContentTypeChange={(contentType) => {
                  setBinaryContentType(contentType);
                  setIsDirty(true);
                }}
                onPickFile={chooseRequestFile}
              />
            )}

            {requestTab === "scripts" && (
              <div className="script-editor-shell">
                <div className="script-tabs" role="tablist" aria-label="Request scripts">
                  <button
                    className={scriptTab === "preRequest" ? "active" : undefined}
                    type="button"
                    role="tab"
                    aria-selected={scriptTab === "preRequest"}
                    onClick={() => setScriptTab("preRequest")}
                  >
                    Pre-request
                  </button>
                  <button
                    className={scriptTab === "postResponse" ? "active" : undefined}
                    type="button"
                    role="tab"
                    aria-selected={scriptTab === "postResponse"}
                    onClick={() => setScriptTab("postResponse")}
                  >
                    Post-response / Tests
                  </button>
                  <span>JavaScript</span>
                </div>
                <textarea
                  className="script-editor"
                  aria-label={
                    scriptTab === "preRequest"
                      ? "Pre-request script"
                      : "Post-response script"
                  }
                  spellCheck={false}
                  value={
                    scriptTab === "preRequest" ? preRequestScript : postResponseScript
                  }
                  placeholder={
                    scriptTab === "preRequest"
                      ? "// Runs before the request\n// pm.environment.get('baseUrl')"
                      : "// Runs after the response\n// pm.test('status is 200', () => { ... })"
                  }
                  onChange={(event) => {
                    if (scriptTab === "preRequest") {
                      setPreRequestScript(event.target.value);
                    } else {
                      setPostResponseScript(event.target.value);
                    }
                    setIsDirty(true);
                  }}
                />
                <div className="script-runtime-note">
                  Runs in a fresh native QuickJS sandbox on Send: 2 s deadline, 64 MiB
                  heap, no filesystem, network, modules, process, or desktop bridge. Initial
                  `pm` support includes variables, request headers, response access,
                  console, `pm.test`, and common `pm.expect` assertions.
                </div>
              </div>
            )}

            {requestTab === "settings" && (
              <RequestSettings
                options={requestOptions}
                onChange={(options) => {
                  setRequestOptions(options);
                  setIsDirty(true);
                }}
              />
            )}
          </div>
        </form>

        <div
          className="workspace-resize-handle"
          role="separator"
          aria-orientation="horizontal"
          aria-label="Resize request panel"
          onPointerDown={startWorkspaceResize}
          onDoubleClick={() => {
            setRequestPanelPct(42);
            writeStoredLayoutNumber("requestPanelPct", 42);
          }}
        />

        <section className="response-panel" aria-label="Response">
          <div className="response-toolbar">
            <div className="tabs response-tabs" role="tablist" aria-label="Response data">
              {responseTabs.map((tab) => (
                <button
                  key={tab}
                  className={responseTab === tab ? "active" : undefined}
                  type="button"
                  role="tab"
                  aria-selected={responseTab === tab}
                  onClick={() => setResponseTab(tab)}
                >
                  {tab[0]?.toUpperCase() + tab.slice(1)}
                  {tab === "headers" && response && (
                    <span className="tab-count">{response.headers.length}</span>
                  )}
                  {tab === "tests" && <span className="tab-count">{scriptTests.length}</span>}
                  {tab === "console" && <span className="tab-count">{scriptLogs.length}</span>}
                </button>
              ))}
            </div>

            {response && (
              <div className="response-metrics">
                <span className={response.status < 400 ? "status-success" : "status-error"}>
                  {response.status} {response.statusText}
                </span>
                <span>{response.timing.totalMs} ms</span>
                <span>{formatBytes(response.receivedBytes)}</span>
              </div>
            )}
          </div>

          <div className="response-content">
            {requestError && (
              <div className="error-state" role="alert">
                <strong>Request failed</strong>
                <span>{requestError}</span>
                {networkFailure && (
                  <>
                    <span>{networkFailure.help}</span>
                    <span>{networkFailure.destination} · {networkFailure.proxy} · Timeout: {networkFailure.timeout}</span>
                    <div className="network-error-actions">
                      <button type="button" disabled={!session} onClick={() => setNetworkSettingsOpen(true)}>Network &amp; TLS settings</button>
                      <button type="button" onClick={() => setRequestTab("settings")}>Request timeout settings</button>
                    </div>
                  </>
                )}
              </div>
            )}

            {!response && !requestError && !isSending && (
              <div className="empty-state">
                <div className="empty-icon" aria-hidden="true"><AppIcon name="terminal" /></div>
                <small className="empty-eyebrow">YOUR NEXT RESPONSE STARTS HERE</small>
                <strong>Build. Send. Explore.</strong>
                <span>Paste a cURL command or enter a URL above.<br />Your workspace stays local. No account required.</span>
                <div className="empty-shortcuts"><span><kbd>Ctrl T</kbd> New request</span><span><kbd>Ctrl K</kbd> Quick actions</span></div>
              </div>
            )}

            {isSending && (
              <div className="empty-state">
                <div className="spinner" />
                <strong>Sending request</strong>
                <span>Waiting for a response. You can cancel at any time.</span>
              </div>
            )}

            {response && response.body.truncated && (
              <div className="warning-banner">
                Response display stopped at the configured safety limit.
              </div>
            )}

            {response && responseTab === "body" && (
              <ResponseBodyViewer
                response={response}
                isSaving={isSavingResponse}
                onCopy={copyResponseBody}
                onSave={saveCurrentResponse}
                activeEnvironmentName={activeEnvironment?.name ?? null}
                onSaveVariable={saveResponseValueAsVariable}
              />
            )}

            {response && responseTab === "headers" && (
              <div className="response-headers">
                {response.headers.map((header, index) => (
                  <div key={`${header.name}-${index}`}>
                    <span>{header.name}</span>
                    <code>{header.value}</code>
                  </div>
                ))}
              </div>
            )}

            {response && responseTab === "tests" && (
              <div className="script-test-results">
                <div className="script-result-summary">
                  <strong>
                    {scriptTests.filter((test) => test.passed).length} passed
                  </strong>
                  <span>
                    {scriptTests.filter((test) => !test.passed).length} failed · {" "}
                    {scriptRuns.reduce((total, run) => total + run.durationMs, 0)} ms
                  </span>
                </div>
                {scriptTests.map((test, index) => (
                  <div
                    className={`script-test-row ${test.passed ? "passed" : "failed"}`}
                    key={`${test.phase}-${index}-${test.name}`}
                  >
                    <span aria-hidden="true">{test.passed ? "✓" : "×"}</span>
                    <div>
                      <strong>{test.name}</strong>
                      <small>
                        {test.phase === "preRequest" ? "Pre-request" : "Post-response"}
                        {test.error ? ` · ${test.error}` : ""}
                      </small>
                    </div>
                  </div>
                ))}
              </div>
            )}

            {response && responseTab === "console" && (
              <div className="script-console-output">
                {scriptLogs.map((log, index) => (
                  <div className={`script-log ${log.level}`} key={`${log.phase}-${index}`}>
                    <span>{log.phase === "preRequest" ? "PRE" : "POST"}</span>
                    <span>{log.level.toUpperCase()}</span>
                    <code>{log.message}</code>
                  </div>
                ))}
              </div>
            )}
          </div>
        </section>
      </main>

      <footer className="statusbar">
        <span>
          <i className="connected-dot" /> Local engine
        </span>
        <span>{session ? `Workspace: ${session.workspace.name}` : "No workspace open"}</span>
        <label className="status-environment">
          <span>Environment:</span>
          <select
            aria-label="Active environment"
            value={activeEnvironmentId ?? ""}
            disabled={!session || isSending || isWorkspaceBusy}
            onChange={(event) => setActiveEnvironmentId(event.target.value || null)}
          >
            <option value="">None</option>
            {session?.workspace.environments.map((environment) => (
              <option key={environment.id} value={environment.id}>
                {environment.name}
              </option>
            ))}
          </select>
        </label>
        <span className="status-spacer" />
        <span>{isDirty ? "Unsaved changes" : "Saved"}</span>
        <span>UTF-8</span>
      </footer>

      {notice && (
        <div
          className={`app-notice ${notice.kind}`}
          role={notice.kind === "error" ? "alert" : "status"}
        >
          <span>{notice.text}</span>
          <button type="button" aria-label="Dismiss notification" onClick={() => setNotice(null)}>
            ×
          </button>
        </div>
      )}

      {modal && (
        <div className="modal-backdrop" role="presentation">
          <form
            className="modal-card"
            role="dialog"
            aria-modal="true"
            aria-labelledby="modal-title"
            onSubmit={(event) => void handleModalSubmit(event)}
          >
            <div className="modal-heading">
              <div>
                <strong id="modal-title">{modalTitle(modal)}</strong>
                <span>{modalDescription(modal)}</span>
              </div>
              <button
                type="button"
                aria-label="Close dialog"
                disabled={isWorkspaceBusy}
                onClick={() => setModal(null)}
              >
                ×
              </button>
            </div>
            {modal.kind !== "moveRequest" && modal.kind !== "moveFolder" && (
              <label className="modal-field">
                <span>Name</span>
                <input
                  autoFocus
                  maxLength={120}
                  value={modalName}
                  placeholder={modalPlaceholder(modal)}
                  onChange={(event) => setModalName(event.target.value)}
                />
              </label>
            )}

            {(modal.kind === "saveRequest" ||
              modal.kind === "newFolder" ||
              modal.kind === "moveRequest" ||
              modal.kind === "moveFolder") && (
              <label className="modal-field">
                <span>
                  {modal.kind === "saveRequest"
                    ? "Save to"
                    : modal.kind === "moveRequest" || modal.kind === "moveFolder"
                      ? "Move to"
                      : "Create in"}
                </span>
                <select
                  value={destinationKey}
                  onChange={(event) => setDestinationKey(event.target.value)}
                >
                  {modalDestinations.map((destination) => (
                    <option key={destination.key} value={destination.key}>
                      {destination.label}
                    </option>
                  ))}
                </select>
              </label>
            )}

            <div className="modal-actions">
              <button
                className="ghost-button"
                type="button"
                disabled={isWorkspaceBusy}
                onClick={() => setModal(null)}
              >
                Cancel
              </button>
              <button
                className="primary-small"
                type="submit"
                disabled={
                  ((modal.kind !== "moveRequest" && modal.kind !== "moveFolder") &&
                    !modalName.trim()) ||
                  ((modal.kind === "moveRequest" || modal.kind === "moveFolder") &&
                    !destinationKey) ||
                  isWorkspaceBusy
                }
              >
                {isWorkspaceBusy ? "Working…" : modalAction(modal)}
              </button>
            </div>
          </form>
        </div>
      )}

      {maintenanceAction && (
        <WorkspaceMaintenanceDialog
          action={maintenanceAction}
          onRun={handleWorkspaceMaintenance}
          onClose={() => setMaintenanceAction(null)}
        />
      )}

      {workspaceAction && (
        <div className="modal-backdrop" role="presentation">
          <div
            className="modal-card workspace-action-card"
            role="dialog"
            aria-modal="true"
            aria-labelledby="workspace-action-title"
          >
            <div className="modal-heading">
              <div>
                <strong id="workspace-action-title">Manage {workspaceAction.name}</strong>
                <span>
                  {workspaceAction.kind[0]?.toUpperCase() + workspaceAction.kind.slice(1)}
                  {workspaceAction.kind !== "request" ? " and its contents" : ""}
                </span>
              </div>
              <button
                type="button"
                aria-label="Close item actions"
                disabled={isWorkspaceBusy}
                onClick={() => setWorkspaceAction(null)}
              >
                ×
              </button>
            </div>
            <div className="workspace-action-list">
              {workspaceAction.kind !== "request" && (
                <button
                  type="button"
                  disabled={isWorkspaceBusy}
                  onClick={() => {
                    const target = workspaceAction;
                    setWorkspaceAction(null);
                    setAuthPolicyTarget(target);
                  }}
                >
                  <strong>Authentication</strong>
                  <span>Set a policy here or inherit from the nearest parent.</span>
                </button>
              )}
              <button
                type="button"
                disabled={isWorkspaceBusy}
                onClick={() => {
                  const target = workspaceAction;
                  setWorkspaceAction(null);
                  showModal({ kind: "renameItem", target });
                }}
              >
                <strong>Rename</strong>
                <span>Change the display name without changing its stable ID.</span>
              </button>
              {(workspaceAction.kind === "request" ||
                workspaceAction.kind === "folder") && (
                <button
                  type="button"
                  disabled={isWorkspaceBusy}
                  onClick={() => {
                    const target = workspaceAction;
                    setWorkspaceAction(null);
                    if (target.kind === "request") {
                      showModal({ kind: "moveRequest", target });
                    } else if (target.kind === "folder") {
                      showModal({ kind: "moveFolder", target });
                    }
                  }}
                >
                  <strong>Move</strong>
                  <span>Move this item to another collection or nested folder.</span>
                </button>
              )}
              <button
                type="button"
                disabled={isWorkspaceBusy}
                onClick={() => void handleReorderWorkspaceItem(workspaceAction, -1)}
              >
                <strong>Move up</strong>
                <span>Move this item earlier among its current siblings.</span>
              </button>
              <button
                type="button"
                disabled={isWorkspaceBusy}
                onClick={() => void handleReorderWorkspaceItem(workspaceAction, 1)}
              >
                <strong>Move down</strong>
                <span>Move this item later among its current siblings.</span>
              </button>
              <button
                type="button"
                disabled={isWorkspaceBusy}
                onClick={() => void handleDuplicateWorkspaceItem(workspaceAction)}
              >
                <strong>Duplicate</strong>
                <span>Copy it with new IDs so both versions remain editable.</span>
              </button>
              <button
                className="danger-action"
                type="button"
                disabled={isWorkspaceBusy}
                onClick={() => void handleDeleteWorkspaceItem(workspaceAction)}
              >
                <strong>Delete</strong>
                <span>Remove it from this local workspace.</span>
              </button>
            </div>
          </div>
        </div>
      )}

      {curlImportOpen && (
        <div className="modal-backdrop" role="presentation">
          <form
            className="modal-card curl-import-card"
            role="dialog"
            aria-modal="true"
            aria-labelledby="curl-import-title"
            onSubmit={(event) => void handleCurlModalSubmit(event)}
          >
            <div className="modal-heading">
              <div>
                <strong id="curl-import-title">Import cURL</strong>
                <span>
                  Paste a browser “Copy as cURL” command. File uploads and unsafe TLS
                  bypasses are never imported silently.
                </span>
              </div>
              <button
                type="button"
                aria-label="Close cURL import"
                disabled={isImportingCurl}
                onClick={() => {
                  setCurlImportError(null);
                  setCurlImportOpen(false);
                }}
              >
                ×
              </button>
            </div>
            <label className="modal-field">
              <span>cURL command</span>
              <textarea
                autoFocus
                aria-label="cURL command"
                spellCheck={false}
                value={curlText}
                placeholder={
                  "curl 'https://api.example.com/users' \\\n  -H 'Accept: application/json'"
                }
                onChange={(event) => {
                  setCurlText(event.target.value);
                  setCurlImportError(null);
                }}
              />
            </label>
            {curlImportError && (
              <div className="curl-import-error" role="alert">
                <strong>Could not import this command</strong>
                <span>{curlImportError}</span>
              </div>
            )}
            <div className="modal-actions">
              <button
                className="ghost-button"
                type="button"
                disabled={isImportingCurl}
                onClick={() => {
                  setCurlImportError(null);
                  setCurlImportOpen(false);
                }}
              >
                Cancel
              </button>
              <button
                className="primary-small"
                type="submit"
                disabled={!curlText.trim() || isImportingCurl}
              >
                {isImportingCurl ? "Importing…" : "Import request"}
              </button>
            </div>
          </form>
        </div>
      )}

      {pendingApiImport && (
        <div className="modal-backdrop" role="presentation">
          <div
            className="modal-card api-import-card"
            role="dialog"
            aria-modal="true"
            aria-labelledby="api-import-title"
          >
            <div className="modal-heading">
              <div>
                <strong id="api-import-title">Import {pendingApiImport.name}</strong>
                <span>
                  {apiDocumentKindLabel(pendingApiImport.kind)} ·{" "}
                  {pendingApiImport.collections.reduce(
                    (total, collection) => total + countRequests(collection),
                    0,
                  )}{" "}
                  requests · {pendingApiImport.environments.length} environments
                </span>
              </div>
              <button
                type="button"
                aria-label="Close API document import"
                disabled={isWorkspaceBusy}
                onClick={() => setPendingApiImport(null)}
              >
                ×
              </button>
            </div>
            <div className="api-import-summary">
              <strong>Ready to add to {session?.workspace.name}</strong>
              <span>
                Existing collections and environments will be preserved. Imported secret
                placeholders remain empty until you set them in Environments.
              </span>
            </div>
            {pendingApiImport.warnings.length > 0 && (
              <div className="api-import-warnings" role="status">
                <strong>
                  {pendingApiImport.warnings.length} migration note
                  {pendingApiImport.warnings.length === 1 ? "" : "s"}
                </strong>
                <ul>
                  {pendingApiImport.warnings.map((warning, index) => (
                    <li key={`${index}-${warning}`}>{warning}</li>
                  ))}
                </ul>
              </div>
            )}
            <div className="modal-actions">
              <button
                className="ghost-button"
                type="button"
                disabled={isWorkspaceBusy}
                onClick={() => setPendingApiImport(null)}
              >
                Cancel
              </button>
              <button
                className="primary-small"
                type="button"
                disabled={isWorkspaceBusy}
                onClick={() => void handleApiDocumentConfirm()}
              >
                {isWorkspaceBusy ? "Saving…" : "Import into workspace"}
              </button>
            </div>
          </div>
        </div>
      )}

      {environmentEditorOpen && session && (
        <EnvironmentEditor
          environments={session.workspace.environments}
          activeEnvironmentId={activeEnvironmentId}
          secretStatuses={secretStatuses}
          localValues={localEnvironmentValues}
          onCancel={() => setEnvironmentEditorOpen(false)}
          onActiveEnvironmentChange={setActiveEnvironmentId}
          onSave={handleSaveEnvironments}
        />
      )}

      {historyOpen && session && (
        <HistoryPanel
          workspaceId={session.workspace.id}
          revision={historyRevision}
          onClose={() => setHistoryOpen(false)}
          onRestore={restoreHistoryEntry}
          onError={handlePanelError}
        />
      )}

      {cookieManagerOpen && session && (
        <CookieManager
          workspaceId={session.workspace.id}
          initialUrl={composeRequestUrl(url, queryParams)}
          onClose={() => setCookieManagerOpen(false)}
          onError={handlePanelError}
        />
      )}

      {codeSnippetOpen && (
        <CodeSnippetModal
          request={buildRequest()}
          activeEnvironmentId={activeEnvironmentId}
          onClose={() => setCodeSnippetOpen(false)}
        />
      )}

      {commandPaletteOpen && (
        <CommandPalette
          requests={paletteRequests}
          actions={paletteActions}
          onSelectRequest={(id) => {
            const request = session ? findSavedRequest(session.workspace, id) : undefined;
            if (request) {
              loadRequest(request);
            }
          }}
          onClose={() => setCommandPaletteOpen(false)}
        />
      )}

      {networkSettingsOpen && session && (
        <NetworkSettingsEditor
          settings={session.workspace.network}
          onCancel={() => setNetworkSettingsOpen(false)}
          onSave={handleSaveNetworkSettings}
          onError={handlePanelError}
        />
      )}

      {authPolicyTarget && session && (
        <AuthPolicyEditor
          title={`Authentication — ${authPolicyTarget.name}`}
          description={
            authPolicyTarget.kind === "workspace"
              ? "Default authentication for inheriting collections, folders, and requests."
              : "Overrides its parent when set; nested items can continue to inherit it."
          }
          auth={getAuthPolicy(session.workspace, authPolicyTarget)}
          allowInherit={authPolicyTarget.kind !== "workspace"}
          onCancel={() => setAuthPolicyTarget(null)}
          onSave={handleSaveAuthPolicy}
        />
      )}

      {collectionRunnerOpen && session && (
        <CollectionRunner
          workspace={session.workspace}
          activeEnvironmentId={activeEnvironmentId}
          onClose={() => setCollectionRunnerOpen(false)}
          onError={handlePanelError}
        />
      )}

      {protocolWorkbenchOpen && session && (
        <ProtocolWorkbench
          workspace={session.workspace}
          activeEnvironmentId={activeEnvironmentId}
          onSave={handleSaveProtocolProfiles}
          onClose={() => setProtocolWorkbenchOpen(false)}
          onError={handlePanelError}
        />
      )}
    </div>
  );
}

function flattenWorkspaceRequests(workspace: WorkspaceDocument): PaletteRequestEntry[] {
  return workspace.collections.flatMap((collection) => [
    ...collection.requests.map((request) => ({
      id: request.id,
      name: request.name,
      method: request.request.method,
      path: collection.name,
    })),
    ...collection.folders.flatMap((folder) =>
      flattenFolderRequests(folder, [collection.name]),
    ),
  ]);
}

function flattenFolderRequests(
  folder: RequestFolder,
  parents: string[],
): PaletteRequestEntry[] {
  const path = [...parents, folder.name].join(" / ");
  return [
    ...folder.requests.map((request) => ({
      id: request.id,
      name: request.name,
      method: request.request.method,
      path,
    })),
    ...folder.folders.flatMap((child) =>
      flattenFolderRequests(child, [...parents, folder.name]),
    ),
  ];
}

function workspaceDestinations(workspace: WorkspaceDocument): DestinationOption[] {
  return workspace.collections.flatMap((collection) => {
    const collectionOption: DestinationOption = {
      key: destinationId({ collectionId: collection.id, folderId: null }),
      label: collection.name,
      location: { collectionId: collection.id, folderId: null },
    };
    return [
      collectionOption,
      ...collection.folders.flatMap((folder) =>
        folderDestinations(collection.id, folder, [collection.name]),
      ),
    ];
  });
}

function folderDestinations(
  collectionId: string,
  folder: RequestFolder,
  parents: string[],
): DestinationOption[] {
  const path = [...parents, folder.name];
  const location = { collectionId, folderId: folder.id };
  return [
    { key: destinationId(location), label: path.join(" / "), location },
    ...folder.folders.flatMap((child) =>
      folderDestinations(collectionId, child, path),
    ),
  ];
}

function destinationId(location: RequestLocation): string {
  return `${location.collectionId}:${location.folderId ?? "root"}`;
}

function closeDetailsMenu(element: HTMLElement) {
  element.closest("details")?.removeAttribute("open");
}

function modalTitle(modal: ModalState): string {
  switch (modal.kind) {
    case "newWorkspace":
      return "Create local workspace";
    case "newCollection":
      return "New collection";
    case "newFolder":
      return "New folder";
    case "saveRequest":
      return "Save request";
    case "renameItem":
      return `Rename ${modal.target.kind}`;
    case "moveRequest":
      return `Move ${modal.target.name}`;
    case "moveFolder":
      return `Move ${modal.target.name}`;
  }
}

function modalDescription(modal: ModalState): string {
  switch (modal.kind) {
    case "newWorkspace":
      return "Choose a parent folder next; the workspace will be created inside it.";
    case "newCollection":
      return "Collections group related requests and folders.";
    case "newFolder":
      return "Folders can be nested to match your API structure.";
    case "saveRequest":
      return "The request will be written to workspace.yaml.";
    case "renameItem":
      return "The stable ID and all nested content will be preserved.";
    case "moveRequest":
      return "Choose a collection or nested folder. The request ID is preserved.";
    case "moveFolder":
      return "Choose a collection or folder outside this folder. All contents and IDs are preserved.";
  }
}

function modalPlaceholder(modal: ModalState): string {
  switch (modal.kind) {
    case "newWorkspace":
      return "Payments API";
    case "newCollection":
      return "Public API";
    case "newFolder":
      return "Authentication";
    case "saveRequest":
      return "List customers";
    case "renameItem":
      return modal.target.name;
    case "moveRequest":
      return "";
    case "moveFolder":
      return "";
  }
}

function modalAction(modal: ModalState): string {
  if (modal.kind === "saveRequest") {
    return "Save";
  }
  if (modal.kind === "moveRequest" || modal.kind === "moveFolder") {
    return "Move";
  }
  return modal.kind === "renameItem" ? "Rename" : "Create";
}

function errorMessage(cause: unknown): string {
  if (cause instanceof Error) {
    return cause.message;
  }
  return typeof cause === "string" ? cause : "An unexpected error occurred.";
}

function apiDocumentKindLabel(kind: ApiDocumentImport["kind"]): string {
  switch (kind) {
    case "postmanCollection":
      return "Postman collection";
    case "postmanEnvironment":
      return "Postman environment";
    case "openApi":
      return "OpenAPI / Swagger";
  }
}
