import { useEffect, useState } from "react";
import {
  clearHistory,
  deleteHistoryEntry,
  listHistory,
  setHistoryPinned,
  type HistoryEntry,
} from "./api";
import { formatBytes } from "./format";

interface HistoryPanelProps {
  workspaceId: string;
  revision: number;
  onClose: () => void;
  onRestore: (entry: HistoryEntry) => void;
  onError: (message: string) => void;
}

const PAGE_SIZE = 50;

export function HistoryPanel({
  workspaceId,
  revision,
  onClose,
  onRestore,
  onError,
}: HistoryPanelProps) {
  const [search, setSearch] = useState("");
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [isLoadingMore, setIsLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);

  useEffect(() => {
    let cancelled = false;
    const timer = window.setTimeout(() => {
      setIsLoading(true);
      void listHistory({ workspaceId, search, limit: PAGE_SIZE, offset: 0 })
        .then((result) => {
          if (!cancelled) {
            setEntries(result);
            setHasMore(result.length === PAGE_SIZE);
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
    }, 220);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [onError, revision, search, workspaceId]);

  async function loadMore() {
    if (isLoadingMore || !hasMore) {
      return;
    }
    setIsLoadingMore(true);
    try {
      const result = await listHistory({
        workspaceId,
        search,
        limit: PAGE_SIZE,
        offset: entries.length,
      });
      setEntries((current) => [...current, ...result]);
      setHasMore(result.length === PAGE_SIZE);
    } catch (cause) {
      onError(errorMessage(cause));
    } finally {
      setIsLoadingMore(false);
    }
  }

  async function togglePinned(entry: HistoryEntry) {
    try {
      await setHistoryPinned(workspaceId, entry.id, !entry.pinned);
      setEntries((current) =>
        current.map((candidate) =>
          candidate.id === entry.id
            ? { ...candidate, pinned: !candidate.pinned }
            : candidate,
        ),
      );
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  async function remove(entry: HistoryEntry) {
    if (!window.confirm(`Delete history entry for "${entry.requestName}"?`)) {
      return;
    }
    try {
      await deleteHistoryEntry(workspaceId, entry.id);
      setEntries((current) => current.filter((candidate) => candidate.id !== entry.id));
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  async function clearUnpinned() {
    if (!window.confirm("Clear all unpinned request history for this workspace?")) {
      return;
    }
    try {
      await clearHistory(workspaceId, false);
      setEntries((current) => current.filter((entry) => entry.pinned));
      setHasMore(false);
    } catch (cause) {
      onError(errorMessage(cause));
    }
  }

  return (
    <div className="modal-backdrop history-backdrop" role="presentation">
      <section
        className="history-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="history-title"
      >
        <header className="history-heading">
          <div>
            <strong id="history-title">Request history</strong>
            <span>Local metadata and redacted request snapshots. Response bodies are not stored.</span>
          </div>
          <button type="button" aria-label="Close request history" onClick={onClose}>
            ×
          </button>
        </header>

        <div className="history-toolbar">
          <input
            aria-label="Search request history"
            placeholder="Search method, URL, name, status, or error"
            maxLength={256}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
          <button type="button" onClick={() => void clearUnpinned()}>
            Clear unpinned
          </button>
        </div>

        <div className="history-list">
          {isLoading ? (
            <div className="history-empty">Loading local history…</div>
          ) : entries.length ? (
            entries.map((entry) => (
              <article className="history-entry" key={entry.id}>
                <button
                  className="history-entry-main"
                  type="button"
                  onClick={() => onRestore(entry)}
                >
                  <span className={`method-badge ${entry.method.toLowerCase()}`}>
                    {entry.method}
                  </span>
                  <span className="history-entry-copy">
                    <strong>{entry.requestName}</strong>
                    <span>{entry.url}</span>
                    <small>
                      {new Date(entry.createdAtMs).toLocaleString()}
                      {entry.status !== null ? ` · ${entry.status} ${entry.statusText ?? ""}` : ""}
                      {entry.durationMs !== null ? ` · ${entry.durationMs} ms` : ""}
                      {entry.receivedBytes !== null ? ` · ${formatBytes(entry.receivedBytes)}` : ""}
                      {entry.requestBodyOmitted ? " · body omitted" : ""}
                    </small>
                    {entry.error && <em>{entry.error}</em>}
                  </span>
                </button>
                <button
                  className={entry.pinned ? "history-pin active" : "history-pin"}
                  type="button"
                  aria-label={entry.pinned ? "Unpin history entry" : "Pin history entry"}
                  title={entry.pinned ? "Unpin" : "Pin"}
                  onClick={() => void togglePinned(entry)}
                >
                  {entry.pinned ? "◆" : "◇"}
                </button>
                <button
                  className="history-delete"
                  type="button"
                  aria-label="Delete history entry"
                  title="Delete"
                  onClick={() => void remove(entry)}
                >
                  ×
                </button>
              </article>
            ))
          ) : (
            <div className="history-empty">
              {search ? "No history entries match this search." : "Send a request to create local history."}
            </div>
          )}
          {hasMore && (
            <button
              className="history-load-more"
              type="button"
              disabled={isLoadingMore}
              onClick={() => void loadMore()}
            >
              {isLoadingMore ? "Loading…" : "Load more"}
            </button>
          )}
        </div>
      </section>
    </div>
  );
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
