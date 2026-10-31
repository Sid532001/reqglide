import { useState, type FormEvent } from "react";
import { pickNetworkFile, type NetworkSettings } from "./api";
import { normalizeNetworkSettings } from "./network-settings";

interface NetworkSettingsEditorProps {
  settings?: NetworkSettings;
  onCancel: () => void;
  onSave: (settings: NetworkSettings) => Promise<boolean>;
  onError: (message: string) => void;
}

export function NetworkSettingsEditor({
  settings,
  onCancel,
  onSave,
  onError,
}: NetworkSettingsEditorProps) {
  const [draft, setDraft] = useState(() => normalizeNetworkSettings(settings));
  const [isSaving, setIsSaving] = useState(false);

  function update(patch: Partial<NetworkSettings>) {
    setDraft((current) => ({ ...current, ...patch }));
  }

  async function chooseFile(kind: "caCertificate" | "clientIdentity") {
    try {
      const path = await pickNetworkFile(kind);
      if (path) {
        update(
          kind === "caCertificate"
            ? { caCertificatePath: path }
            : { clientCertificatePath: path },
        );
      }
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (isSaving) {
      return;
    }
    if (draft.proxyMode === "custom" && !draft.proxyUrl.trim()) {
      onError("Enter a proxy URL when custom proxy mode is selected.");
      return;
    }

    setIsSaving(true);
    try {
      await onSave({
        ...draft,
        proxyUrl: draft.proxyUrl.trim(),
        proxyUsername: draft.proxyUsername.trim(),
        noProxy: draft.noProxy.trim(),
        caCertificatePath: draft.caCertificatePath.trim(),
        clientCertificatePath: draft.clientCertificatePath.trim(),
      });
    } finally {
      setIsSaving(false);
    }
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <form
        className="network-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="network-settings-title"
        onSubmit={(event) => void submit(event)}
      >
        <header className="network-heading">
          <div>
            <strong id="network-settings-title">Network &amp; TLS</strong>
            <span>Workspace-scoped transport settings used by the native HTTP engine.</span>
          </div>
          <button type="button" aria-label="Close network settings" onClick={onCancel}>
            ×
          </button>
        </header>

        <div className="network-settings-body">
          <section className="network-section">
            <h3>Session</h3>
            <label className="network-check">
              <input
                type="checkbox"
                checked={draft.cookieJarEnabled}
                onChange={(event) => update({ cookieJarEnabled: event.target.checked })}
              />
              <span>
                <strong>Send and capture workspace cookies</strong>
                <small>The cookie jar remains local and is cleared when the app exits.</small>
              </span>
            </label>

          </section>

          <section className="network-section">
            <h3>Proxy</h3>
            <label className="network-field">
              <span>Mode</span>
              <select
                value={draft.proxyMode}
                onChange={(event) =>
                  update({ proxyMode: event.target.value as NetworkSettings["proxyMode"] })
                }
              >
                <option value="system">Use system proxy</option>
                <option value="disabled">No proxy</option>
                <option value="custom">Custom proxy</option>
              </select>
            </label>

            {draft.proxyMode === "system" && (
              <p className="network-hint">Uses operating-system and environment proxy settings. HTTP requests on Windows also resolve automatic PAC/WPAD configuration. Postman-specific custom proxy settings are separate; choose Custom proxy to match them.</p>
            )}

            {draft.proxyMode === "custom" && (
              <div className="network-grid">
                <label className="network-field network-field-wide">
                  <span>Proxy URL</span>
                  <input
                    required
                    spellCheck={false}
                    placeholder="http://proxy.example.com:8080"
                    value={draft.proxyUrl}
                    onChange={(event) => update({ proxyUrl: event.target.value })}
                  />
                </label>
                <label className="network-field">
                  <span>Username</span>
                  <input
                    spellCheck={false}
                    placeholder="{{proxyUsername}}"
                    value={draft.proxyUsername}
                    onChange={(event) => update({ proxyUsername: event.target.value })}
                  />
                </label>
                <label className="network-field">
                  <span>Password / secret reference</span>
                  <input
                    type="password"
                    spellCheck={false}
                    placeholder="{{proxyPassword}}"
                    value={draft.proxyPassword}
                    onChange={(event) => update({ proxyPassword: event.target.value })}
                  />
                </label>
                <label className="network-field network-field-wide">
                  <span>Bypass proxy</span>
                  <input
                    spellCheck={false}
                    placeholder="localhost, 127.0.0.1, .internal.example.com"
                    value={draft.noProxy}
                    onChange={(event) => update({ noProxy: event.target.value })}
                  />
                </label>
                <p className="network-hint network-field-wide">
                  Use environment references for credentials. Secret environment values are
                  resolved inside Rust from the operating system credential store.
                </p>
              </div>
            )}
          </section>

          <section className="network-section">
            <h3>Certificates</h3>
            <label className="network-field">
              <span>Custom CA bundle (PEM)</span>
              <div className="network-path-row">
                <input
                  spellCheck={false}
                  placeholder="Optional PEM certificate bundle"
                  value={draft.caCertificatePath}
                  onChange={(event) => update({ caCertificatePath: event.target.value })}
                />
                <button type="button" onClick={() => void chooseFile("caCertificate")}>
                  Browse…
                </button>
              </div>
            </label>
            <label className="network-field">
              <span>Client identity (combined PEM)</span>
              <div className="network-path-row">
                <input
                  spellCheck={false}
                  placeholder="PEM containing client certificate and private key"
                  value={draft.clientCertificatePath}
                  onChange={(event) => update({ clientCertificatePath: event.target.value })}
                />
                <button type="button" onClick={() => void chooseFile("clientIdentity")}>
                  Browse…
                </button>
              </div>
            </label>
            <p className="network-hint">
              Certificate files are read only by the native process when a request is sent and
              are never copied into the workspace.
            </p>
          </section>

          <section className="network-section network-danger-zone">
            <label className="network-check">
              <input
                type="checkbox"
                checked={draft.acceptInvalidCertificates}
                onChange={(event) =>
                  update({ acceptInvalidCertificates: event.target.checked })
                }
              />
              <span>
                <strong>Disable TLS certificate verification</strong>
                <small>
                  Dangerous: this permits man-in-the-middle attacks. Use only for controlled
                  local testing.
                </small>
              </span>
            </label>
          </section>
        </div>

        <footer className="modal-actions">
          <button className="ghost-button" type="button" disabled={isSaving} onClick={onCancel}>
            Cancel
          </button>
          <button className="primary-small" type="submit" disabled={isSaving}>
            {isSaving ? "Saving…" : "Save settings"}
          </button>
        </footer>
      </form>
    </div>
  );
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
