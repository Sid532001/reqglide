import { useEffect, useMemo, useState } from "react";
import {
  executeGraphqlProfile,
  invokeGrpcProfile,
  pickGrpcDescriptorSet,
  pollProtocolSession,
  reflectGrpcProfile,
  sendWebsocketMessage,
  startSseSession,
  startWebsocketSession,
  stopProtocolSession,
  type GrpcInvokeResult,
  type GrpcMethodInfo,
  type ProtocolEvent,
  type ProtocolProfile,
  type ProtocolProfileCommandInput,
  type ResponseSnapshot,
  type WebSocketOutbound,
  type WorkspaceDocument,
} from "./api";
import { AuthEditor } from "./AuthEditor";
import { KeyValueEditor } from "./KeyValueEditor";
import {
  GRAPHQL_INTROSPECTION_QUERY,
  createProtocolProfile,
  parseSubprotocols,
  protocolLabel,
  validateProtocolProfiles,
  type ProtocolKind,
} from "./protocol-profiles";

interface ProtocolWorkbenchProps {
  workspace: WorkspaceDocument;
  activeEnvironmentId: string | null;
  onSave: (profiles: ProtocolProfile[]) => Promise<boolean>;
  onClose: () => void;
  onError: (message: string) => void;
}

interface ActiveSession {
  id: string;
  profileId: string;
}

export function ProtocolWorkbench({
  workspace,
  activeEnvironmentId,
  onSave,
  onClose,
  onError,
}: ProtocolWorkbenchProps) {
  const initialProfiles = useMemo(
    () => structuredClone(workspace.protocolProfiles ?? []),
    [workspace.id],
  );
  const [profiles, setProfiles] = useState<ProtocolProfile[]>(initialProfiles);
  const [baseline, setBaseline] = useState(() => JSON.stringify(initialProfiles));
  const [selectedId, setSelectedId] = useState(initialProfiles[0]?.id ?? "");
  const [newKind, setNewKind] = useState<ProtocolKind>("graphQl");
  const [environmentId, setEnvironmentId] = useState(activeEnvironmentId ?? "");
  const [isSaving, setIsSaving] = useState(false);
  const [isRunning, setIsRunning] = useState(false);
  const [response, setResponse] = useState<ResponseSnapshot | null>(null);
  const [grpcResult, setGrpcResult] = useState<GrpcInvokeResult | null>(null);
  const [services, setServices] = useState<string[]>([]);
  const [methods, setMethods] = useState<GrpcMethodInfo[]>([]);
  const [events, setEvents] = useState<ProtocolEvent[]>([]);
  const [activeSession, setActiveSession] = useState<ActiveSession | null>(null);
  const [outboundKind, setOutboundKind] = useState<"text" | "binary" | "ping">(
    "text",
  );
  const [outboundData, setOutboundData] = useState("");

  const selected = profiles.find((profile) => profile.id === selectedId);
  const dirty = JSON.stringify(profiles) !== baseline;

  useEffect(() => {
    const sessionId = activeSession?.id;
    if (!sessionId) {
      return;
    }
    const currentSessionId: string = sessionId;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;

    async function poll() {
      try {
        const result = await pollProtocolSession(currentSessionId, 250);
        if (cancelled) {
          return;
        }
        if (result.events.length) {
          setEvents((current) => [...current, ...result.events].slice(-1_000));
        }
        if (result.finished) {
          setActiveSession((current) => (current?.id === currentSessionId ? null : current));
          return;
        }
        timer = setTimeout(() => void poll(), 250);
      } catch (cause) {
        if (!cancelled) {
          setActiveSession((current) => (current?.id === currentSessionId ? null : current));
          onError(errorMessage(cause));
        }
      }
    }

    void poll();
    return () => {
      cancelled = true;
      if (timer) {
        clearTimeout(timer);
      }
    };
  }, [activeSession?.id, onError]);

  function updateSelected(next: ProtocolProfile) {
    setProfiles((current) =>
      current.map((profile) => (profile.id === next.id ? next : profile)),
    );
  }

  function addProfile() {
    const profile = createProtocolProfile(newKind);
    setProfiles((current) => [...current, profile]);
    setSelectedId(profile.id);
    resetResults();
  }

  async function selectProfile(profileId: string) {
    if (profileId === selectedId) {
      return;
    }
    await disconnect();
    setSelectedId(profileId);
    resetResults();
  }

  async function removeProfile() {
    if (!selected || !window.confirm(`Delete protocol profile “${selected.name}”?`)) {
      return;
    }
    await disconnect();
    const next = profiles.filter((profile) => profile.id !== selected.id);
    setProfiles(next);
    setSelectedId(next[0]?.id ?? "");
    resetResults();
  }

  async function saveProfiles() {
    const validation = validateProtocolProfiles(profiles);
    if (validation) {
      onError(validation);
      return;
    }
    setIsSaving(true);
    try {
      if (await onSave(profiles)) {
        setBaseline(JSON.stringify(profiles));
      }
    } finally {
      setIsSaving(false);
    }
  }

  async function closeWorkbench() {
    if (dirty && !window.confirm("Discard unsaved protocol profile changes?")) {
      return;
    }
    await disconnect();
    onClose();
  }

  function commandInput(profile = selected): ProtocolProfileCommandInput {
    if (!profile) {
      throw new Error("Select a protocol profile first.");
    }
    return {
      profile: structuredClone(profile),
      environmentId: environmentId || null,
    };
  }

  async function runGraphql(introspection = false) {
    if (!selected || selected.configuration.kind !== "graphQl" || isRunning) {
      return;
    }
    setIsRunning(true);
    setGrpcResult(null);
    try {
      const profile = introspection
        ? {
            ...selected,
            configuration: {
              ...selected.configuration,
              query: GRAPHQL_INTROSPECTION_QUERY,
              variables: "{}",
              operationName: "IntrospectionQuery",
            },
          }
        : selected;
      setResponse(await executeGraphqlProfile(commandInput(profile)));
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setIsRunning(false);
    }
  }

  async function connectRealtime() {
    if (!selected || activeSession || isRunning) {
      return;
    }
    if (
      selected.configuration.kind !== "webSocket" &&
      selected.configuration.kind !== "serverSentEvents"
    ) {
      return;
    }
    setIsRunning(true);
    setEvents([]);
    try {
      const id =
        selected.configuration.kind === "webSocket"
          ? await startWebsocketSession(commandInput())
          : await startSseSession(commandInput());
      setActiveSession({ id, profileId: selected.id });
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setIsRunning(false);
    }
  }

  async function disconnect() {
    const current = activeSession;
    setActiveSession(null);
    if (current) {
      try {
        await stopProtocolSession(current.id);
      } catch {
        // A session that completed between polls is already gone.
      }
    }
  }

  async function sendWebsocket() {
    if (!activeSession || !selected || selected.configuration.kind !== "webSocket") {
      return;
    }
    let message: WebSocketOutbound;
    if (outboundKind === "binary") {
      message = { kind: "binary", dataBase64: outboundData };
    } else if (outboundKind === "ping") {
      message = { kind: "ping", data: outboundData };
    } else {
      message = { kind: "text", data: outboundData };
    }
    try {
      await sendWebsocketMessage(activeSession.id, message);
      if (outboundKind === "text") {
        setOutboundData("");
      }
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  async function discoverServices() {
    if (!selected || selected.configuration.kind !== "grpc" || isRunning) {
      return;
    }
    setIsRunning(true);
    try {
      const profile: ProtocolProfile = {
        ...selected,
        configuration: { ...selected.configuration, service: "" },
      };
      const result = await reflectGrpcProfile(commandInput(profile));
      setServices(result.services);
      setMethods([]);
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setIsRunning(false);
    }
  }

  async function discoverMethods() {
    if (!selected || selected.configuration.kind !== "grpc" || isRunning) {
      return;
    }
    const configuration = selected.configuration;
    setIsRunning(true);
    try {
      const result = await reflectGrpcProfile(commandInput());
      setMethods(result.methods);
      if (
        result.methods.length > 0 &&
        !result.methods.some((method) => method.name === configuration.method)
      ) {
        updateSelected({
          ...selected,
          configuration: {
            ...configuration,
            method: result.methods[0]!.name,
          },
        });
      }
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setIsRunning(false);
    }
  }

  async function invokeGrpc() {
    if (!selected || selected.configuration.kind !== "grpc" || isRunning) {
      return;
    }
    setIsRunning(true);
    setResponse(null);
    try {
      setGrpcResult(await invokeGrpcProfile(commandInput()));
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setIsRunning(false);
    }
  }

  async function chooseDescriptorSet() {
    if (!selected || selected.configuration.kind !== "grpc") {
      return;
    }
    try {
      const path = await pickGrpcDescriptorSet();
      if (path) {
        updateSelected({
          ...selected,
          configuration: { ...selected.configuration, descriptorSetPath: path },
        });
      }
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  function resetResults() {
    setResponse(null);
    setGrpcResult(null);
    setServices([]);
    setMethods([]);
    setEvents([]);
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        className="protocol-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="protocol-workbench-title"
      >
        <header className="protocol-heading">
          <div>
            <strong id="protocol-workbench-title">Protocol workbench</strong>
            <span>GraphQL, WebSocket, Server-Sent Events, and reflection-driven gRPC.</span>
          </div>
          <label className="protocol-environment">
            <span>Environment</span>
            <select value={environmentId} onChange={(event) => setEnvironmentId(event.target.value)}>
              <option value="">None</option>
              {workspace.environments.map((environment) => (
                <option key={environment.id} value={environment.id}>
                  {environment.name}
                </option>
              ))}
            </select>
          </label>
          <button type="button" aria-label="Close protocol workbench" onClick={() => void closeWorkbench()}>
            ×
          </button>
        </header>

        <div className="protocol-layout">
          <aside className="protocol-sidebar">
            <div className="protocol-add-row">
              <select value={newKind} onChange={(event) => setNewKind(event.target.value as ProtocolKind)}>
                <option value="graphQl">GraphQL</option>
                <option value="webSocket">WebSocket</option>
                <option value="serverSentEvents">SSE</option>
                <option value="grpc">gRPC</option>
              </select>
              <button type="button" onClick={addProfile}>+ Add</button>
            </div>
            <div className="protocol-profile-list">
              {profiles.map((profile) => (
                <button
                  className={profile.id === selectedId ? "active" : ""}
                  type="button"
                  key={profile.id}
                  onClick={() => void selectProfile(profile.id)}
                >
                  <strong>{profile.name}</strong>
                  <span>{protocolLabel(profile.configuration.kind)}</span>
                </button>
              ))}
              {!profiles.length && (
                <div className="protocol-empty-list">Add a protocol profile to begin.</div>
              )}
            </div>
            <div className="protocol-save-row">
              <button
                className="primary-small"
                type="button"
                disabled={!dirty || isSaving}
                onClick={() => void saveProfiles()}
              >
                {isSaving ? "Saving…" : "Save profiles"}
              </button>
              <button type="button" disabled={!selected} onClick={() => void removeProfile()}>
                Delete
              </button>
            </div>
          </aside>

          {selected ? (
            <div className="protocol-main">
              <ProtocolEditor
                profile={selected}
                update={updateSelected}
                services={services}
                methods={methods}
                isRunning={isRunning}
                activeSession={activeSession?.profileId === selected.id}
                outboundKind={outboundKind}
                outboundData={outboundData}
                onOutboundKind={setOutboundKind}
                onOutboundData={setOutboundData}
                onGraphql={runGraphql}
                onConnect={connectRealtime}
                onDisconnect={disconnect}
                onSend={sendWebsocket}
                onDiscoverServices={discoverServices}
                onDiscoverMethods={discoverMethods}
                onInvokeGrpc={invokeGrpc}
                onChooseDescriptor={chooseDescriptorSet}
              />
              <ProtocolOutput
                profile={selected}
                response={response}
                grpcResult={grpcResult}
                events={events}
                onClearEvents={() => setEvents([])}
              />
            </div>
          ) : (
            <div className="protocol-no-selection">
              <strong>No protocol profile selected</strong>
              <span>Create a GraphQL, WebSocket, SSE, or gRPC profile from the sidebar.</span>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

interface ProtocolEditorProps {
  profile: ProtocolProfile;
  update: (profile: ProtocolProfile) => void;
  services: string[];
  methods: GrpcMethodInfo[];
  isRunning: boolean;
  activeSession: boolean;
  outboundKind: "text" | "binary" | "ping";
  outboundData: string;
  onOutboundKind: (kind: "text" | "binary" | "ping") => void;
  onOutboundData: (data: string) => void;
  onGraphql: (introspection?: boolean) => Promise<void>;
  onConnect: () => Promise<void>;
  onDisconnect: () => Promise<void>;
  onSend: () => Promise<void>;
  onDiscoverServices: () => Promise<void>;
  onDiscoverMethods: () => Promise<void>;
  onInvokeGrpc: () => Promise<void>;
  onChooseDescriptor: () => Promise<void>;
}

function ProtocolEditor({
  profile,
  update,
  services,
  methods,
  isRunning,
  activeSession,
  outboundKind,
  outboundData,
  onOutboundKind,
  onOutboundData,
  onGraphql,
  onConnect,
  onDisconnect,
  onSend,
  onDiscoverServices,
  onDiscoverMethods,
  onInvokeGrpc,
  onChooseDescriptor,
}: ProtocolEditorProps) {
  const configuration = profile.configuration;
  const selectedMethod =
    configuration.kind === "grpc"
      ? methods.find((method) => method.name === configuration.method)
      : undefined;

  return (
    <div className="protocol-editor">
      <div className="protocol-common-grid">
        <label className="network-field">
          <span>Name</span>
          <input value={profile.name} onChange={(event) => update({ ...profile, name: event.target.value })} />
        </label>
        <label className="network-field protocol-url-field">
          <span>Endpoint</span>
          <input
            value={profile.url}
            spellCheck={false}
            onChange={(event) => update({ ...profile, url: event.target.value })}
          />
        </label>
      </div>

      {configuration.kind === "graphQl" && (
        <section className="protocol-specific-editor">
          <label className="protocol-code-field protocol-code-large">
            <span>Query or mutation</span>
            <textarea
              value={configuration.query}
              spellCheck={false}
              onChange={(event) =>
                update({ ...profile, configuration: { ...configuration, query: event.target.value } })
              }
            />
          </label>
          <div className="protocol-split-fields">
            <label className="protocol-code-field">
              <span>Variables (JSON)</span>
              <textarea
                value={configuration.variables}
                spellCheck={false}
                onChange={(event) =>
                  update({ ...profile, configuration: { ...configuration, variables: event.target.value } })
                }
              />
            </label>
            <label className="network-field">
              <span>Operation name</span>
              <input
                value={configuration.operationName}
                onChange={(event) =>
                  update({ ...profile, configuration: { ...configuration, operationName: event.target.value } })
                }
              />
            </label>
          </div>
          <div className="protocol-action-row">
            <button type="button" disabled={isRunning} onClick={() => void onGraphql(true)}>
              Introspect schema
            </button>
            <button className="primary-small" type="button" disabled={isRunning} onClick={() => void onGraphql()}>
              {isRunning ? "Sending…" : "Send GraphQL"}
            </button>
          </div>
        </section>
      )}

      {configuration.kind === "webSocket" && (
        <section className="protocol-specific-editor">
          <label className="network-field">
            <span>Subprotocols (comma separated)</span>
            <input
              value={configuration.subprotocols.join(", ")}
              spellCheck={false}
              placeholder="graphql-transport-ws"
              onChange={(event) =>
                update({
                  ...profile,
                  configuration: {
                    ...configuration,
                    subprotocols: parseSubprotocols(event.target.value),
                  },
                })
              }
            />
          </label>
          <div className="protocol-action-row">
            {activeSession ? (
              <button className="danger-button" type="button" onClick={() => void onDisconnect()}>
                Disconnect
              </button>
            ) : (
              <button className="primary-small" type="button" disabled={isRunning} onClick={() => void onConnect()}>
                {isRunning ? "Connecting…" : "Connect"}
              </button>
            )}
          </div>
          <div className="protocol-send-row">
            <select value={outboundKind} onChange={(event) => onOutboundKind(event.target.value as typeof outboundKind)}>
              <option value="text">Text</option>
              <option value="binary">Binary (base64)</option>
              <option value="ping">Ping</option>
            </select>
            <textarea
              value={outboundData}
              spellCheck={false}
              placeholder={outboundKind === "binary" ? "Base64 payload" : "Message payload"}
              onChange={(event) => onOutboundData(event.target.value)}
            />
            <button type="button" disabled={!activeSession} onClick={() => void onSend()}>
              Send
            </button>
          </div>
        </section>
      )}

      {configuration.kind === "serverSentEvents" && (
        <section className="protocol-specific-editor">
          <label className="network-field">
            <span>Last event ID</span>
            <input
              value={configuration.lastEventId}
              spellCheck={false}
              placeholder="Optional resume cursor"
              onChange={(event) =>
                update({ ...profile, configuration: { ...configuration, lastEventId: event.target.value } })
              }
            />
          </label>
          <div className="protocol-action-row">
            {activeSession ? (
              <button className="danger-button" type="button" onClick={() => void onDisconnect()}>
                Stop stream
              </button>
            ) : (
              <button className="primary-small" type="button" disabled={isRunning} onClick={() => void onConnect()}>
                {isRunning ? "Connecting…" : "Connect stream"}
              </button>
            )}
          </div>
        </section>
      )}

      {configuration.kind === "grpc" && (
        <section className="protocol-specific-editor">
          <label className="network-check">
            <input
              type="checkbox"
              checked={configuration.useReflection}
              onChange={(event) =>
                update({ ...profile, configuration: { ...configuration, useReflection: event.target.checked } })
              }
            />
            <span>
              <strong>Use server reflection</strong>
              <small>Disable this to load a local compiled FileDescriptorSet.</small>
            </span>
          </label>
          {!configuration.useReflection && (
            <label className="network-field">
              <span>Descriptor set</span>
              <div className="network-path-row">
                <input value={configuration.descriptorSetPath} readOnly placeholder="service.protoset" />
                <button type="button" onClick={() => void onChooseDescriptor()}>Browse</button>
              </div>
            </label>
          )}
          <div className="grpc-discovery-row">
            <label className="network-field">
              <span>Service</span>
              <input
                list={`grpc-services-${profile.id}`}
                value={configuration.service}
                spellCheck={false}
                onChange={(event) =>
                  update({ ...profile, configuration: { ...configuration, service: event.target.value } })
                }
              />
              <datalist id={`grpc-services-${profile.id}`}>
                {services.map((service) => <option value={service} key={service} />)}
              </datalist>
            </label>
            <button type="button" disabled={isRunning} onClick={() => void onDiscoverServices()}>
              Discover services
            </button>
            <button type="button" disabled={isRunning || !configuration.service.trim()} onClick={() => void onDiscoverMethods()}>
              Load methods
            </button>
          </div>
          <label className="network-field">
            <span>Method</span>
            <select
              value={configuration.method}
              onChange={(event) =>
                update({ ...profile, configuration: { ...configuration, method: event.target.value } })
              }
            >
              {!methods.some((method) => method.name === configuration.method) && (
                <option value={configuration.method}>{configuration.method}</option>
              )}
              {methods.map((method) => (
                <option value={method.name} key={method.fullName}>
                  {method.name} · {cardinality(method)}
                </option>
              ))}
            </select>
          </label>
          {selectedMethod && (
            <div className="grpc-method-hint">
              {selectedMethod.inputType} → {selectedMethod.outputType} · {cardinality(selectedMethod)}
            </div>
          )}
          <label className="protocol-code-field protocol-code-large">
            <span>
              Request JSON{selectedMethod?.clientStreaming ? " array (one object per client message)" : ""}
            </span>
            <textarea
              value={configuration.message}
              spellCheck={false}
              onChange={(event) =>
                update({ ...profile, configuration: { ...configuration, message: event.target.value } })
              }
            />
          </label>
          <div className="protocol-action-row">
            <button className="primary-small" type="button" disabled={isRunning} onClick={() => void onInvokeGrpc()}>
              {isRunning ? "Invoking…" : "Invoke gRPC"}
            </button>
          </div>
        </section>
      )}

      <details className="protocol-details">
        <summary>Headers / metadata</summary>
        <KeyValueEditor
          entries={profile.headers}
          nameLabel="Header"
          addLabel="Add header"
          onChange={(headers) => update({ ...profile, headers })}
        />
      </details>
      <details className="protocol-details">
        <summary>Authentication</summary>
        <AuthEditor auth={profile.auth} onChange={(auth) => update({ ...profile, auth })} />
        {profile.auth.kind === "inherit" && (
          <div className="editor-help">Protocol profiles inherit the workspace authentication policy.</div>
        )}
      </details>
    </div>
  );
}

function ProtocolOutput({
  profile,
  response,
  grpcResult,
  events,
  onClearEvents,
}: {
  profile: ProtocolProfile;
  response: ResponseSnapshot | null;
  grpcResult: GrpcInvokeResult | null;
  events: ProtocolEvent[];
  onClearEvents: () => void;
}) {
  if (
    profile.configuration.kind === "webSocket" ||
    profile.configuration.kind === "serverSentEvents"
  ) {
    return (
      <section className="protocol-output">
        <header>
          <strong>Messages</strong>
          <span>{events.length} retained locally</span>
          <button type="button" onClick={onClearEvents}>Clear</button>
        </header>
        <div className="protocol-event-list">
          {events.map((event) => (
            <div className={`protocol-event ${event.direction} ${event.kind}`} key={event.id}>
              <time>{new Date(event.timestampMs).toLocaleTimeString()}</time>
              <span>{event.direction}</span>
              <strong>{event.eventName || event.kind}</strong>
              <pre>{event.data || "(empty)"}</pre>
              <small>
                {event.byteLength} B
                {event.eventId ? ` · id ${event.eventId}` : ""}
                {event.retryMs ? ` · retry ${event.retryMs} ms` : ""}
              </small>
            </div>
          ))}
          {!events.length && <div className="protocol-output-empty">Connect to view protocol events.</div>}
        </div>
      </section>
    );
  }

  if (profile.configuration.kind === "grpc") {
    return (
      <section className="protocol-output">
        <header>
          <strong>gRPC response</strong>
          {grpcResult && <span>{grpcResult.code} · {grpcResult.durationMs} ms</span>}
        </header>
        {grpcResult ? (
          <div className="grpc-output-body">
            {grpcResult.message && <div className="protocol-call-error">{grpcResult.message}</div>}
            <pre>{JSON.stringify(grpcResult.responses, null, 2)}</pre>
            <MetadataBlock title="Initial metadata" entries={grpcResult.metadata} />
            <MetadataBlock title="Trailers" entries={grpcResult.trailers} />
          </div>
        ) : (
          <div className="protocol-output-empty">Invoke a method to inspect messages and metadata.</div>
        )}
      </section>
    );
  }

  return (
    <section className="protocol-output">
      <header>
        <strong>GraphQL response</strong>
        {response && <span>HTTP {response.status} · {response.timing.totalMs} ms</span>}
      </header>
      {response ? (
        <div className="graphql-output-body">
          <pre>{responseText(response)}</pre>
        </div>
      ) : (
        <div className="protocol-output-empty">Send a query or introspection request to view its response.</div>
      )}
    </section>
  );
}

function MetadataBlock({ title, entries }: { title: string; entries: ProtocolProfile["headers"] }) {
  if (!entries.length) {
    return null;
  }
  return (
    <details className="grpc-metadata">
      <summary>{title} ({entries.length})</summary>
      {entries.map((entry, index) => (
        <div key={`${entry.name}:${index}`}><strong>{entry.name}</strong><span>{entry.value}</span></div>
      ))}
    </details>
  );
}

function cardinality(method: GrpcMethodInfo): string {
  if (method.clientStreaming && method.serverStreaming) {
    return "bidirectional stream";
  }
  if (method.clientStreaming) {
    return "client stream";
  }
  if (method.serverStreaming) {
    return "server stream";
  }
  return "unary";
}

function responseText(response: ResponseSnapshot): string {
  if (response.body.encoding === "base64") {
    return `[base64 response]\n${response.body.content}`;
  }
  try {
    return JSON.stringify(JSON.parse(response.body.content), null, 2);
  } catch {
    return response.body.content;
  }
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
