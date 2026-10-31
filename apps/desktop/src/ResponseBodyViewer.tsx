import { useEffect, useMemo, useState, type ReactNode } from "react";
import type { ResponseSnapshot } from "./api";
import {
  canPreviewResponse,
  countTextMatches,
  displayResponseBody,
  rawResponseBody,
  sandboxedHtmlPreview,
} from "./format";

type ResponseViewMode = "pretty" | "raw" | "preview";

interface ResponseBodyViewerProps {
  response: ResponseSnapshot;
  isSaving: boolean;
  onCopy: (content: string) => Promise<void>;
  onSave: () => Promise<void>;
  activeEnvironmentName?: string | null;
  onSaveVariable?: (key: string, value: string) => Promise<boolean>;
}

const MAX_HIGHLIGHTS = 500;
const MAX_SYNTAX_CHARACTERS = 256 * 1024;
const MAX_SYNTAX_TOKENS = 5_000;

export function ResponseBodyViewer({
  response,
  isSaving,
  onCopy,
  onSave,
  activeEnvironmentName = null,
  onSaveVariable,
}: ResponseBodyViewerProps) {
  const [mode, setMode] = useState<ResponseViewMode>("pretty");
  const [query, setQuery] = useState("");
  const [wrap, setWrap] = useState(false);
  const [extractOpen, setExtractOpen] = useState(false);
  const [extractNames, setExtractNames] = useState<Record<string, string>>({});
  const previewAvailable = canPreviewResponse(response);
  const isJson = response.body.contentType?.toLowerCase().includes("json") === true;
  const displayed = useMemo(
    () =>
      mode === "pretty" ? displayResponseBody(response) : rawResponseBody(response),
    [mode, response],
  );
  const matchCount = useMemo(
    () => countTextMatches(displayed, query.trim()),
    [displayed, query],
  );
  const extractableFields = useMemo(
    () => (isJson ? extractFlatJsonFields(response.body.content) : []),
    [isJson, response.body.content],
  );
  const renderedBody = useMemo(() => {
    if (!displayed) return "(empty response body)";
    if (mode === "pretty" && isJson && !query.trim()) {
      return displayed.length <= MAX_SYNTAX_CHARACTERS ? highlightJson(displayed) : displayed;
    }
    return highlightMatches(displayed, query.trim());
  }, [displayed, mode, isJson, query]);

  useEffect(() => {
    setMode("pretty");
    setQuery("");
    setExtractOpen(false);
    setExtractNames({});
  }, [response]);

  return (
    <div className="response-body-viewer">
      <div className="response-view-toolbar">
        <div className="response-view-modes" role="tablist" aria-label="Response body view">
          {(["pretty", "raw", "preview"] as const).map((viewMode) => (
            <button
              key={viewMode}
              className={mode === viewMode ? "active" : undefined}
              type="button"
              role="tab"
              aria-selected={mode === viewMode}
              disabled={viewMode === "preview" && !previewAvailable}
              onClick={() => setMode(viewMode)}
            >
              {viewMode[0]?.toUpperCase() + viewMode.slice(1)}
            </button>
          ))}
        </div>
        {mode !== "preview" && (
          <label className="response-search">
            <span aria-hidden="true">⌕</span>
            <input
              aria-label="Search response body"
              value={query}
              placeholder="Find in response"
              onChange={(event) => setQuery(event.target.value)}
            />
            {query && <small>{matchCount} matches</small>}
          </label>
        )}
        <button
          className={wrap ? "active" : undefined}
          type="button"
          aria-pressed={wrap}
          onClick={() => setWrap((current) => !current)}
        >
          Wrap
        </button>
        {onSaveVariable && isJson && extractableFields.length > 0 && (
          <button
            className={extractOpen ? "active" : undefined}
            type="button"
            aria-pressed={extractOpen}
            onClick={() => setExtractOpen((current) => !current)}
          >
            Extract variable
          </button>
        )}
        <button type="button" onClick={() => void onCopy(displayed)}>
          Copy
        </button>
        <button type="button" disabled={isSaving} onClick={() => void onSave()}>
          {isSaving ? "Saving…" : "Save"}
        </button>
        {mode === "pretty" && isJson && displayed.length > MAX_SYNTAX_CHARACTERS && (
          <span className="response-render-note">Large response · syntax colors paused</span>
        )}
      </div>

      {extractOpen && onSaveVariable && (
        <div className="response-extract-panel">
          <div className="response-extract-header">
            <strong>Save a response value as a variable</strong>
            <span>
              {activeEnvironmentName
                ? `Saves into "${activeEnvironmentName}" — reference it elsewhere as {{name}}.`
                : "Select an active environment (bottom-right) first."}
            </span>
          </div>
          {extractableFields.map((field) => (
            <ExtractRow
              key={field.key}
              field={field}
              name={extractNames[field.key] ?? field.key}
              onNameChange={(name) =>
                setExtractNames((current) => ({ ...current, [field.key]: name }))
              }
              onSave={onSaveVariable}
            />
          ))}
        </div>
      )}

      {mode === "preview" && previewAvailable ? (
        <iframe
          className="response-preview"
          title="Sandboxed response preview"
          sandbox=""
          srcDoc={sandboxedHtmlPreview(response.body.content)}
        />
      ) : (
        <pre className={`response-body ${wrap ? "wrap" : ""}`}>
          {renderedBody}
        </pre>
      )}
    </div>
  );
}

function highlightMatches(content: string, query: string): ReactNode {
  if (!query) {
    return content;
  }
  const normalized = content.toLocaleLowerCase();
  const needle = query.toLocaleLowerCase();
  const result: ReactNode[] = [];
  let offset = 0;
  let matches = 0;

  while (matches < MAX_HIGHLIGHTS) {
    const index = normalized.indexOf(needle, offset);
    if (index === -1) {
      break;
    }
    result.push(content.slice(offset, index));
    result.push(<mark key={`${index}-${matches}`}>{content.slice(index, index + query.length)}</mark>);
    offset = index + query.length;
    matches += 1;
  }
  result.push(content.slice(offset));
  return result;
}

const JSON_TOKEN_PATTERN =
  /("(?:\\u[a-fA-F0-9]{4}|\\[^u]|[^\\"])*"(\s*:)?|\b(?:true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/g;

// Tokenizes pretty-printed JSON into colored React text nodes. Content always stays as plain
// text (never HTML), so a malicious response body cannot inject markup into the page.
function highlightJson(content: string): ReactNode {
  const nodes: ReactNode[] = [];
  let lastIndex = 0;
  let tokenKey = 0;
  let match: RegExpExecArray | null;

  JSON_TOKEN_PATTERN.lastIndex = 0;
  while (tokenKey < MAX_SYNTAX_TOKENS && (match = JSON_TOKEN_PATTERN.exec(content)) !== null) {
    if (match.index > lastIndex) {
      nodes.push(content.slice(lastIndex, match.index));
    }
    const token = match[0];
    let className = "json-number";
    if (token.startsWith('"')) {
      className = token.endsWith(":") ? "json-key" : "json-string";
    } else if (token === "true" || token === "false") {
      className = "json-boolean";
    } else if (token === "null") {
      className = "json-null";
    }
    nodes.push(
      <span key={tokenKey++} className={className}>
        {token}
      </span>,
    );
    lastIndex = JSON_TOKEN_PATTERN.lastIndex;
  }
  nodes.push(content.slice(lastIndex));
  return nodes;
}

interface ExtractableField {
  key: string;
  value: string;
}

// Only top-level primitive fields (the common shape for token endpoints, e.g. access_token,
// expires_in) are offered; nested objects/arrays aren't meaningful single variable values.
function extractFlatJsonFields(content: string): ExtractableField[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(content);
  } catch {
    return [];
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return [];
  }
  return Object.entries(parsed as Record<string, unknown>)
    .filter(([, value]) => typeof value === "string" || typeof value === "number" || typeof value === "boolean")
    .map(([key, value]) => ({ key, value: String(value) }));
}

interface ExtractRowProps {
  field: ExtractableField;
  name: string;
  onNameChange: (name: string) => void;
  onSave: (key: string, value: string) => Promise<boolean>;
}

function ExtractRow({ field, name, onNameChange, onSave }: ExtractRowProps) {
  const [status, setStatus] = useState<"idle" | "saving" | "saved">("idle");
  const preview = field.value.length > 60 ? `${field.value.slice(0, 60)}…` : field.value;

  async function handleSave() {
    setStatus("saving");
    const saved = await onSave(name, field.value);
    setStatus(saved ? "saved" : "idle");
  }

  return (
    <div className="response-extract-row">
      <code className="response-extract-preview" title={field.value}>
        {field.key}: {preview}
      </code>
      <input
        aria-label={`Variable name for ${field.key}`}
        value={name}
        onChange={(event) => {
          onNameChange(event.target.value);
          setStatus("idle");
        }}
      />
      <button type="button" disabled={!name.trim() || status === "saving"} onClick={() => void handleSave()}>
        {status === "saving" ? "Saving…" : status === "saved" ? "Saved ✓" : "Save"}
      </button>
    </div>
  );
}
