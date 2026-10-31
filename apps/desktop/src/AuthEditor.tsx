import { useState } from "react";
import type { RequestAuth } from "./api";
import { KeyValueEditor } from "./KeyValueEditor";

interface AuthEditorProps {
  auth: RequestAuth;
  onChange: (auth: RequestAuth) => void;
  allowInherit?: boolean;
}

interface MaskedFieldProps {
  label: string;
  ariaLabel: string;
  value: string;
  placeholder?: string;
  autoComplete?: string;
  wide?: boolean;
  onChange: (value: string) => void;
}

// Masks the raw value by default but lets the user reveal it, since {{variable}} references
// (and typos in them) are otherwise invisible behind a password field.
function MaskedField({
  label,
  ariaLabel,
  value,
  placeholder,
  autoComplete,
  wide,
  onChange,
}: MaskedFieldProps) {
  const [visible, setVisible] = useState(false);
  return (
    <label className={wide ? "editor-field auth-wide-field" : "editor-field"}>
      <span>{label}</span>
      <div className="masked-field">
        <input
          aria-label={ariaLabel}
          type={visible ? "text" : "password"}
          value={value}
          autoComplete={autoComplete}
          placeholder={placeholder}
          onChange={(event) => onChange(event.target.value)}
        />
        <button
          type="button"
          className="masked-field-toggle"
          aria-label={visible ? `Hide ${label.toLowerCase()}` : `Show ${label.toLowerCase()}`}
          title={visible ? "Hide value" : "Show value"}
          onClick={() => setVisible((current) => !current)}
        >
          {visible ? "\u{1F648}" : "\u{1F441}\u{FE0F}"}
        </button>
      </div>
    </label>
  );
}

export function AuthEditor({ auth, onChange, allowInherit = true }: AuthEditorProps) {
  function selectKind(kind: RequestAuth["kind"]) {
    switch (kind) {
      case "none":
        onChange({ kind: "none" });
        break;
      case "inherit":
        onChange({ kind: "inherit" });
        break;
      case "basic":
        onChange({ kind: "basic", username: "", password: "" });
        break;
      case "bearer":
        onChange({ kind: "bearer", token: "" });
        break;
      case "apiKey":
        onChange({ kind: "apiKey", key: "", value: "", placement: "header" });
        break;
      case "oauth2ClientCredentials":
        onChange({
          kind: "oauth2ClientCredentials",
          issuerUrl: "",
          tokenUrl: "",
          clientId: "",
          clientSecret: "",
          scope: "",
          audience: "",
          clientAuth: "basic",
          extraParameters: [],
        });
        break;
    }
  }

  return (
    <div className="auth-editor">
      <label className="editor-field auth-type-field">
        <span>Auth type</span>
        <select
          aria-label="Authentication type"
          value={auth.kind}
          onChange={(event) => selectKind(event.target.value as RequestAuth["kind"])}
        >
          <option value="none">No Auth</option>
          {allowInherit && <option value="inherit">Inherit from parent</option>}
          <option value="basic">Basic Auth</option>
          <option value="bearer">Bearer Token</option>
          <option value="apiKey">API Key</option>
          <option value="oauth2ClientCredentials">OAuth 2.0 — Client Credentials</option>
        </select>
      </label>

      {auth.kind === "none" && (
        <div className="editor-help">This request won’t add authentication data.</div>
      )}

      {auth.kind === "inherit" && (
        <div className="editor-help">
          Uses the nearest folder, collection, or workspace authentication policy.
        </div>
      )}

      {auth.kind === "basic" && (
        <div className="auth-fields">
          <label className="editor-field">
            <span>Username</span>
            <input
              aria-label="Basic auth username"
              value={auth.username}
              autoComplete="off"
              onChange={(event) => onChange({ ...auth, username: event.target.value })}
            />
          </label>
          <MaskedField
            ariaLabel="Basic auth password"
            label="Password"
            value={auth.password}
            autoComplete="new-password"
            onChange={(value) => onChange({ ...auth, password: value })}
          />
        </div>
      )}

      {auth.kind === "bearer" && (
        <div>
        <MaskedField
          ariaLabel="Bearer token"
          label="Token"
          value={auth.token}
          autoComplete="off"
          placeholder="{{access_token}}"
          wide
          onChange={(value) => onChange({ ...auth, token: value })}
        />
        <div className="editor-help">Sends Authorization: Bearer &lt;token&gt;. Enter the raw token or {"{{access_token}}"}; the Bearer prefix is added automatically. Select the environment that stores the variable.</div>
        </div>
      )}

      {auth.kind === "apiKey" && (
        <div className="auth-fields api-key-fields">
          <label className="editor-field">
            <span>Key</span>
            <input
              aria-label="API key name"
              value={auth.key}
              placeholder="X-API-Key"
              onChange={(event) => onChange({ ...auth, key: event.target.value })}
            />
          </label>
          <MaskedField
            ariaLabel="API key value"
            label="Value"
            value={auth.value}
            autoComplete="off"
            placeholder="{{apiKey}}"
            onChange={(value) => onChange({ ...auth, value })}
          />
          <label className="editor-field">
            <span>Add to</span>
            <select
              aria-label="API key placement"
              value={auth.placement}
              onChange={(event) =>
                onChange({
                  ...auth,
                  placement: event.target.value as "header" | "query",
                })
              }
            >
              <option value="header">Header</option>
              <option value="query">Query params</option>
            </select>
          </label>
        </div>
      )}

      {auth.kind === "oauth2ClientCredentials" && (
        <div className="oauth-auth-fields">
          <div className="auth-fields">
            <label className="editor-field">
              <span>Token URL</span>
              <input
                aria-label="OAuth token URL"
                value={auth.tokenUrl}
                spellCheck={false}
                placeholder="https://id.example.com/oauth2/token"
                onChange={(event) => onChange({ ...auth, tokenUrl: event.target.value })}
              />
            </label>
            <label className="editor-field">
              <span>OIDC issuer URL</span>
              <input
                aria-label="OpenID Connect issuer URL"
                value={auth.issuerUrl}
                spellCheck={false}
                placeholder="Used for discovery when Token URL is empty"
                onChange={(event) => onChange({ ...auth, issuerUrl: event.target.value })}
              />
            </label>
            <label className="editor-field">
              <span>Client ID</span>
              <input
                aria-label="OAuth client ID"
                value={auth.clientId}
                spellCheck={false}
                placeholder="{{clientId}}"
                onChange={(event) => onChange({ ...auth, clientId: event.target.value })}
              />
            </label>
            <MaskedField
              ariaLabel="OAuth client secret"
              label="Client secret"
              value={auth.clientSecret}
              autoComplete="new-password"
              placeholder="{{clientSecret}}"
              onChange={(value) => onChange({ ...auth, clientSecret: value })}
            />
            <label className="editor-field">
              <span>Client authentication</span>
              <select
                aria-label="OAuth client authentication method"
                value={auth.clientAuth}
                onChange={(event) =>
                  onChange({
                    ...auth,
                    clientAuth: event.target.value as "basic" | "requestBody",
                  })
                }
              >
                <option value="basic">Basic header (recommended)</option>
                <option value="requestBody">Request body</option>
              </select>
            </label>
            <label className="editor-field">
              <span>Scope</span>
              <input
                aria-label="OAuth scopes"
                value={auth.scope}
                spellCheck={false}
                placeholder="read write"
                onChange={(event) => onChange({ ...auth, scope: event.target.value })}
              />
            </label>
            <label className="editor-field auth-wide-field">
              <span>Audience</span>
              <input
                aria-label="OAuth audience"
                value={auth.audience}
                spellCheck={false}
                placeholder="Optional API audience"
                onChange={(event) => onChange({ ...auth, audience: event.target.value })}
              />
            </label>
          </div>
          <div className="oauth-extra-parameters">
            <span>Extra token parameters</span>
            <KeyValueEditor
              entries={auth.extraParameters}
              nameLabel="Parameter"
              addLabel="Add token parameter"
              onChange={(extraParameters) => onChange({ ...auth, extraParameters })}
            />
          </div>
          <div className="editor-help">
            Tokens are acquired by the native process, cached only in memory until shortly before
            expiry, and never written to the workspace or returned to the UI.
          </div>
        </div>
      )}

      {auth.kind !== "none" && auth.kind !== "inherit" && (
        <div className="credential-warning">
          Authentication values are saved in workspace.yaml. Use environment references such
          as {"{{accessToken}}"} instead of storing secrets directly.
        </div>
      )}
    </div>
  );
}
