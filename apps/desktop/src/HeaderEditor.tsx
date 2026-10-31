import { useMemo, useState } from "react";
import type { Environment, HeaderEntry, RequestDefinition } from "./api";
import {
  buildGeneratedHeaderPreviews,
  generatedConflict,
  inspectVariableReferences,
} from "./request-headers";

interface HeaderEditorProps {
  request: RequestDefinition;
  environment: Environment | undefined;
  environmentName: string | null;
  secretStatuses: Record<string, boolean>;
  cookieJarEnabled: boolean;
  onChange: (headers: HeaderEntry[]) => void;
  onOpenEnvironments: () => void;
}

export function HeaderEditor({
  request,
  environment,
  environmentName,
  secretStatuses,
  cookieJarEnabled,
  onChange,
  onOpenEnvironments,
}: HeaderEditorProps) {
  const [showGenerated, setShowGenerated] = useState(true);
  const generated = useMemo(
    () => buildGeneratedHeaderPreviews(request, cookieJarEnabled),
    [cookieJarEnabled, request],
  );

  function update(index: number, patch: Partial<HeaderEntry>) {
    onChange(
      request.headers.map((header, headerIndex) =>
        headerIndex === index ? { ...header, ...patch } : header,
      ),
    );
  }

  return (
    <div className="header-editor">
      <div className="header-editor-toolbar">
        <div>
          <strong>Headers</strong>
          <span>
            {request.headers.length} saved · {generated.length} generated
          </span>
        </div>
        <span className={environment ? "environment-context active" : "environment-context"}>
          {environmentName ? `Environment: ${environmentName}` : "No active environment"}
        </span>
        <button type="button" onClick={onOpenEnvironments}>
          Manage environments
        </button>
        <button
          type="button"
          aria-pressed={showGenerated}
          onClick={() => setShowGenerated((current) => !current)}
        >
          {showGenerated ? "Hide" : "Show"} generated headers
        </button>
      </div>

      <div className="header-table">
        <div className="header-row header-heading">
          <span />
          <span>Header</span>
          <span>Value</span>
          <span>Details</span>
          <span />
        </div>

        {showGenerated &&
          generated.map((header) => (
            <div className="header-row generated-header-row" key={`generated-${header.name}`}>
              <input
                aria-label={`${header.name} is generated automatically`}
                type="checkbox"
                checked
                disabled
              />
              <code>{header.name}</code>
              <code>{header.value}</code>
              <span className="header-description">
                <span className="header-source-badge">Auto</span>
                {header.description}
              </span>
              <span />
            </div>
          ))}

        {request.headers.map((header, index) => {
          const references = inspectVariableReferences(
            [header.name, header.value],
            environment,
            secretStatuses,
          );
          const conflict = generatedConflict(header, generated);
          return (
            <div
              className={`header-row editable-header-row${
                references.some((reference) => reference.kind === "missing") || conflict
                  ? " has-warning"
                  : ""
              }`}
              key={index}
            >
              <input
                aria-label={`Enable ${header.name || "header"}`}
                type="checkbox"
                checked={header.enabled}
                onChange={(event) => update(index, { enabled: event.target.checked })}
              />
              <input
                aria-label="Header name"
                value={header.name}
                placeholder="Header"
                onChange={(event) => update(index, { name: event.target.value })}
              />
              <input
                aria-label="Header value"
                value={header.value}
                placeholder="Value or {{variable}}"
                spellCheck={false}
                onChange={(event) => update(index, { value: event.target.value })}
              />
              <div className="header-details">
                {conflict ? (
                  <span className="variable-reference missing" title={conflict.description}>
                    Overridden by {conflict.name}
                  </span>
                ) : references.length ? (
                  references.map((reference) => (
                    <span
                      className={`variable-reference ${reference.kind}`}
                      key={reference.key}
                      title={reference.label}
                    >
                      {reference.label}
                    </span>
                  ))
                ) : (
                  <span className="header-source-copy">Saved header</span>
                )}
              </div>
              <button
                type="button"
                aria-label={`Remove ${header.name || "header"}`}
                onClick={() =>
                  onChange(
                    request.headers.filter((_, headerIndex) => headerIndex !== index),
                  )
                }
              >
                ×
              </button>
            </div>
          );
        })}

        <button
          className="add-row"
          type="button"
          onClick={() =>
            onChange([...request.headers, { name: "", value: "", enabled: true }])
          }
        >
          + Add header
        </button>
      </div>
    </div>
  );
}
