import type { RequestOptions } from "./api";

interface RequestSettingsProps {
  options: RequestOptions;
  onChange: (options: RequestOptions) => void;
}

const MEBIBYTE = 1024 * 1024;

export function RequestSettings({ options, onChange }: RequestSettingsProps) {
  return (
    <div className="request-settings">
      <div className="settings-grid">
        <label className="editor-field">
          Timeout (milliseconds)
          <input
            aria-label="Request timeout in milliseconds"
            type="number"
            min={1}
            step={100}
            value={options.timeoutMs}
            onChange={(event) =>
              onChange({ ...options, timeoutMs: Number(event.target.value) })
            }
          />
        </label>
        <label className="editor-field">
          Maximum upload (MiB)
          <input
            aria-label="Maximum request body size in MiB"
            type="number"
            min={1}
            step={1}
            value={options.maxRequestBytes / MEBIBYTE}
            onChange={(event) =>
              onChange({
                ...options,
                maxRequestBytes: Math.round(Number(event.target.value) * MEBIBYTE),
              })
            }
          />
        </label>
        <label className="editor-field">
          Maximum displayed response (MiB)
          <input
            aria-label="Maximum response size in MiB"
            type="number"
            min={1}
            step={1}
            value={options.maxResponseBytes / MEBIBYTE}
            onChange={(event) =>
              onChange({
                ...options,
                maxResponseBytes: Math.round(Number(event.target.value) * MEBIBYTE),
              })
            }
          />
        </label>
      </div>
      <label className="settings-toggle">
        <input
          type="checkbox"
          checked={options.followRedirects}
          onChange={(event) =>
            onChange({ ...options, followRedirects: event.target.checked })
          }
        />
        <span>
          <strong>Follow redirects</strong>
          <small>Follow at most 10 HTTP redirects for this request.</small>
        </span>
      </label>
      <p className="editor-help inline-help">
        Limits are enforced by the shared Rust engine for desktop and CLI requests.
        Multipart limits count field and file payload bytes; protocol framing is excluded.
      </p>
    </div>
  );
}
