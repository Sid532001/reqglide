import { useMemo, useState } from "react";
import type { Environment, EnvironmentVariable, SecretMutation } from "./api";
import {
  buildSecretMutations,
  changeVariableStorage,
  hasMissingSecretValues,
  sanitizeSecretReferences,
  type PendingSecretValues,
} from "./environment-secrets";

interface EnvironmentEditorProps {
  environments: Environment[];
  activeEnvironmentId: string | null;
  secretStatuses: Record<string, boolean>;
  localValues: Record<string, string>;
  onCancel: () => void;
  onActiveEnvironmentChange: (environmentId: string | null) => void;
  onSave: (environments: Environment[], mutations: SecretMutation[]) => Promise<boolean>;
}

export function EnvironmentEditor({
  environments,
  activeEnvironmentId,
  secretStatuses,
  localValues,
  onCancel,
  onActiveEnvironmentChange,
  onSave,
}: EnvironmentEditorProps) {
  const [drafts, setDrafts] = useState<Environment[]>(() =>
    environments.map(cloneEnvironment),
  );
  const [selectedId, setSelectedId] = useState<string | null>(
    environments.some((environment) => environment.id === activeEnvironmentId)
      ? activeEnvironmentId
      : (environments[0]?.id ?? null),
  );
  const [pendingActiveId, setPendingActiveId] = useState<string | null>(activeEnvironmentId);
  const [newName, setNewName] = useState("");
  const [isSaving, setIsSaving] = useState(false);
  const [storageNotice, setStorageNotice] = useState<string | null>(null);
  const [pendingSecrets, setPendingSecrets] = useState<PendingSecretValues>(() => ({
    ...localValues,
  }));

  const selected = useMemo(
    () => drafts.find((environment) => environment.id === selectedId),
    [drafts, selectedId],
  );

  function addEnvironment() {
    const name = newName.trim();
    if (!name) {
      return;
    }
    const environment: Environment = {
      id: crypto.randomUUID(),
      name,
      variables: [],
    };
    setDrafts((current) => [...current, environment]);
    setSelectedId(environment.id);
    setNewName("");
  }

  function updateSelected(update: (environment: Environment) => Environment) {
    if (!selectedId) {
      return;
    }
    setDrafts((current) =>
      current.map((environment) =>
        environment.id === selectedId ? update(environment) : environment,
      ),
    );
  }

  function addVariable() {
    const variable: EnvironmentVariable = {
      id: crypto.randomUUID(),
      key: "",
      value: "",
      enabled: true,
      secret: false,
      local: false,
    };
    updateSelected((environment) => ({
      ...environment,
      variables: [...environment.variables, variable],
    }));
  }

  function updateVariable(id: string, patch: Partial<EnvironmentVariable>) {
    updateSelected((environment) => ({
      ...environment,
      variables: environment.variables.map((variable) =>
        variable.id === id ? { ...variable, ...patch } : variable,
      ),
    }));
  }

  function removeVariable(id: string) {
    setPendingSecrets((current) => {
      const next = { ...current };
      delete next[id];
      return next;
    });
    updateSelected((environment) => ({
      ...environment,
      variables: environment.variables.filter((variable) => variable.id !== id),
    }));
  }

  function setStorage(
    variable: EnvironmentVariable,
    storage: "plain" | "local" | "secret",
  ) {
    const change = changeVariableStorage(variable, storage, pendingSecrets);
    if (!change) {
      setStorageNotice(`Enter a replacement value for ${variable.key || "this variable"} before selecting Shared. Stored secrets cannot be displayed.`);
      return;
    }
    setStorageNotice(null);
    setPendingSecrets(change.pending);
    updateVariable(variable.id, change.variable);
  }

  function removeSelected() {
    if (!selected || !window.confirm(`Delete environment "${selected.name}"?`)) {
      return;
    }
    const remaining = drafts.filter((environment) => environment.id !== selected.id);
    setDrafts(remaining);
    setSelectedId(remaining[0]?.id ?? null);
    if (pendingActiveId === selected.id) {
      setPendingActiveId(remaining[0]?.id ?? null);
    }
  }

  async function save() {
    setIsSaving(true);
    try {
      const sanitized = sanitizeSecretReferences(drafts);
      const mutations = buildSecretMutations(environments, sanitized, pendingSecrets);
      if (await onSave(sanitized, mutations)) {
        onActiveEnvironmentChange(
          sanitized.some((environment) => environment.id === pendingActiveId)
            ? pendingActiveId
            : (sanitized[0]?.id ?? null),
        );
        onCancel();
      }
    } finally {
      setIsSaving(false);
    }
  }

  const hasInvalidVariables = drafts.some((environment) => {
    const keys = environment.variables.map((variable) => variable.key.trim());
    return keys.some((key) => !key) || new Set(keys).size !== keys.length;
  });
  const hasInvalidNames = drafts.some((environment) => !environment.name.trim());
  const hasMissingSecrets = hasMissingSecretValues(
    drafts,
    pendingSecrets,
    secretStatuses,
  );

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        className="environment-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="environment-title"
      >
        <header className="environment-heading">
          <div>
            <strong id="environment-title">Environments</strong>
            <span>Use variables such as {"{{baseUrl}}"} in URLs, headers, and bodies.</span>
          </div>
          <button type="button" aria-label="Close environments" onClick={onCancel}>
            ×
          </button>
        </header>

        <div className="environment-layout">
          <aside className="environment-list">
            <div className="environment-new-row">
              <input
                aria-label="New environment name"
                placeholder="Development"
                maxLength={120}
                value={newName}
                onChange={(event) => setNewName(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    addEnvironment();
                  }
                }}
              />
              <button type="button" disabled={!newName.trim()} onClick={addEnvironment}>
                +
              </button>
            </div>
            {drafts.map((environment) => (
              <button
                key={environment.id}
                className={environment.id === selectedId ? "active" : undefined}
                type="button"
                onClick={() => setSelectedId(environment.id)}
              >
                <span>{environment.name || "Untitled environment"}</span>
                <small>
                  {environment.id === pendingActiveId ? "Active · " : ""}
                  {environment.variables.length}
                </small>
              </button>
            ))}
            {!drafts.length && (
              <div className="environment-empty">Create your first environment above.</div>
            )}
          </aside>

          <div className="environment-detail">
            {selected ? (
              <>
                <div className="environment-name-row">
                  <label>
                    <span>Environment name</span>
                    <input
                      aria-label="Environment name"
                      maxLength={120}
                      value={selected.name}
                      onChange={(event) =>
                        updateSelected((environment) => ({
                          ...environment,
                          name: event.target.value,
                        }))
                      }
                    />
                  </label>
                  <button className="danger-button" type="button" onClick={removeSelected}>
                    Delete
                  </button>
                  <button
                    className={selected.id === pendingActiveId ? "active-environment-button" : "ghost-button"}
                    type="button"
                    disabled={selected.id === pendingActiveId}
                    onClick={() => setPendingActiveId(selected.id)}
                  >
                    {selected.id === pendingActiveId ? "✓ Active environment" : "Use environment"}
                  </button>
                </div>

                <div className="environment-variable-table">
                  <div className="environment-variable-heading">
                    <span />
                    <span>Variable</span>
                    <span>Value</span>
                    <span>Storage</span>
                    <span />
                  </div>
                  {selected.variables.map((variable) => (
                    <div className="environment-variable-row" key={variable.id}>
                      <input
                        aria-label={`Enable ${variable.key || "variable"}`}
                        type="checkbox"
                        checked={variable.enabled}
                        onChange={(event) =>
                          updateVariable(variable.id, { enabled: event.target.checked })
                        }
                      />
                      <input
                        aria-label="Variable name"
                        placeholder="baseUrl"
                        value={variable.key}
                        onChange={(event) =>
                          updateVariable(variable.id, { key: event.target.value })
                        }
                      />
                      <input
                        aria-label={`Value for ${variable.key || "variable"}`}
                        type={variable.secret ? "password" : "text"}
                        autoComplete="new-password"
                        spellCheck={false}
                        placeholder={
                          variable.secret
                            ? secretStatuses[variable.id]
                              ? "••••••••••••  Stored securely — enter to replace"
                              : "Enter a secret value"
                            : variable.local
                              ? "Stored on this device — enter to replace"
                            : "https://api.example.com"
                        }
                        value={
                          variable.secret || variable.local
                            ? (pendingSecrets[variable.id] ?? "")
                            : variable.value
                        }
                        onChange={(event) =>
                          variable.secret || variable.local
                            ? setPendingSecrets((current) => ({
                                ...current,
                                [variable.id]: event.target.value,
                              }))
                            : updateVariable(variable.id, { value: event.target.value })
                        }
                      />
                      <select
                        className="environment-storage-select"
                        aria-label={`Storage for ${variable.key || "variable"}`}
                        value={variable.secret ? "secret" : variable.local ? "local" : "plain"}
                        onChange={(event) =>
                          setStorage(
                            variable,
                            event.target.value as "plain" | "local" | "secret",
                          )
                        }
                      >
                        <option value="plain">Shared</option>
                        <option value="local">Local · device</option>
                        <option value="secret">Secret · keychain</option>
                      </select>
                      <button
                        type="button"
                        aria-label={`Remove ${variable.key || "variable"}`}
                        onClick={() => removeVariable(variable.id)}
                      >
                        ×
                      </button>
                    </div>
                  ))}
                  <button className="add-row" type="button" onClick={addVariable}>
                    + Add variable
                  </button>
                </div>
              </>
            ) : (
              <div className="environment-detail-empty">
                Select or create an environment to edit its variables.
              </div>
            )}
          </div>
        </div>

        {storageNotice && <div className="environment-warning" role="alert">{storageNotice}</div>}
        <div className="environment-warning">
          Shared values are saved in workspace.yaml. Local and Secret values are held only
          by this device's operating system credential manager. Local values are visible in
          this editor; Secret values are never returned to the interface after saving.
        </div>

        <footer className="modal-actions">
          <button className="ghost-button" type="button" disabled={isSaving} onClick={onCancel}>
            Cancel
          </button>
          <button
            className="primary-small"
            type="button"
            disabled={
              isSaving || hasInvalidNames || hasInvalidVariables || hasMissingSecrets
            }
            onClick={() => void save()}
            title={hasMissingSecrets ? "Enter a value for every new secret variable" : undefined}
          >
            {isSaving ? "Saving…" : "Save environments"}
          </button>
        </footer>
      </section>
    </div>
  );
}

function cloneEnvironment(environment: Environment): Environment {
  return {
    ...environment,
    variables: environment.variables.map((variable) => ({
      ...variable,
      secret: variable.secret ?? false,
      local: variable.local ?? false,
    })),
  };
}
