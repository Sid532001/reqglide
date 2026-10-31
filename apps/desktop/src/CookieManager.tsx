import { useEffect, useState, type FormEvent } from "react";
import {
  addWorkspaceCookie,
  clearWorkspaceCookies,
  deleteWorkspaceCookie,
  listWorkspaceCookies,
  type CookieSnapshot,
} from "./api";

interface CookieManagerProps {
  workspaceId: string;
  initialUrl: string;
  onClose: () => void;
  onError: (message: string) => void;
}

export function CookieManager({
  workspaceId,
  initialUrl,
  onClose,
  onError,
}: CookieManagerProps) {
  const [cookies, setCookies] = useState<CookieSnapshot[]>([]);
  const [url, setUrl] = useState(initialUrl);
  const [setCookie, setSetCookie] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void listWorkspaceCookies(workspaceId)
      .then((result) => {
        if (!cancelled) {
          setCookies(result);
        }
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          onError(errorMessage(cause));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setIsLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [onError, workspaceId]);

  async function add(event: FormEvent) {
    event.preventDefault();
    if (!url.trim() || !setCookie.trim() || isSaving) {
      return;
    }
    setIsSaving(true);
    try {
      await addWorkspaceCookie(workspaceId, url.trim(), setCookie.trim());
      setCookies(await listWorkspaceCookies(workspaceId));
      setSetCookie("");
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setIsSaving(false);
    }
  }

  async function remove(cookie: CookieSnapshot) {
    try {
      await deleteWorkspaceCookie(workspaceId, cookie);
      setCookies((current) => current.filter((candidate) => candidate !== cookie));
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  async function clear() {
    if (!window.confirm("Clear all session cookies for this workspace?")) {
      return;
    }
    try {
      await clearWorkspaceCookies(workspaceId);
      setCookies([]);
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        className="cookie-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="cookie-title"
      >
        <header className="cookie-heading">
          <div>
            <strong id="cookie-title">Workspace cookies</strong>
            <span>RFC-aware cookies are isolated to this workspace and kept for this app session.</span>
          </div>
          <button type="button" aria-label="Close cookie manager" onClick={onClose}>
            ×
          </button>
        </header>

        <form className="cookie-add" onSubmit={(event) => void add(event)}>
          <input
            aria-label="Cookie origin URL"
            placeholder="https://api.example.com/path"
            spellCheck={false}
            value={url}
            onChange={(event) => setUrl(event.target.value)}
          />
          <input
            aria-label="Set-Cookie value"
            placeholder="session=value; Path=/; Secure; HttpOnly"
            spellCheck={false}
            value={setCookie}
            onChange={(event) => setSetCookie(event.target.value)}
          />
          <button type="submit" disabled={isSaving || !url.trim() || !setCookie.trim()}>
            {isSaving ? "Adding…" : "Add"}
          </button>
        </form>

        <div className="cookie-table">
          <div className="cookie-row cookie-header">
            <span>Name</span>
            <span>Value</span>
            <span>Domain / path</span>
            <span>Flags</span>
            <span />
          </div>
          {isLoading ? (
            <div className="cookie-empty">Loading cookies…</div>
          ) : cookies.length ? (
            cookies.map((cookie) => (
              <div className="cookie-row" key={`${cookie.domain}:${cookie.path}:${cookie.name}`}>
                <strong>{cookie.name}</strong>
                <code title={cookie.value}>{cookie.value}</code>
                <span title={`${cookie.domain}${cookie.path}`}>
                  {cookie.domain}
                  {cookie.path}
                </span>
                <small>
                  {cookie.secure ? "Secure " : ""}
                  {cookie.httpOnly ? "HttpOnly " : ""}
                  {cookie.sameSite ?? ""}
                  {cookie.expiresAtMs === null
                    ? " Session"
                    : ` ${new Date(cookie.expiresAtMs).toLocaleString()}`}
                </small>
                <button
                  type="button"
                  aria-label={`Delete cookie ${cookie.name}`}
                  onClick={() => void remove(cookie)}
                >
                  ×
                </button>
              </div>
            ))
          ) : (
            <div className="cookie-empty">No cookies are stored for this workspace session.</div>
          )}
        </div>

        <footer className="modal-actions">
          <button className="ghost-button" type="button" onClick={() => void clear()}>
            Clear all
          </button>
          <button className="primary-small" type="button" onClick={onClose}>
            Done
          </button>
        </footer>
      </section>
    </div>
  );
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
