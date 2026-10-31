import { type KeyboardEvent, useEffect, useMemo, useState } from "react";

export interface PaletteRequestEntry {
  id: string;
  name: string;
  method: string;
  path: string;
}

export interface PaletteAction {
  id: string;
  label: string;
  hint?: string;
  run: () => void;
}

interface CommandPaletteProps {
  requests: PaletteRequestEntry[];
  actions: PaletteAction[];
  onSelectRequest: (id: string) => void;
  onClose: () => void;
}

type PaletteEntry =
  | { kind: "action"; key: string; action: PaletteAction }
  | { kind: "request"; key: string; request: PaletteRequestEntry };

export function CommandPalette({ requests, actions, onSelectRequest, onClose }: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);

  const normalizedQuery = query.trim().toLowerCase();

  const entries: PaletteEntry[] = useMemo(() => {
    const matchedActions = normalizedQuery
      ? actions.filter((action) => action.label.toLowerCase().includes(normalizedQuery))
      : actions;
    const matchedRequests = (
      normalizedQuery
        ? requests.filter(
            (entry) =>
              entry.name.toLowerCase().includes(normalizedQuery) ||
              entry.path.toLowerCase().includes(normalizedQuery),
          )
        : requests
    ).slice(0, 30);
    return [
      ...matchedActions.map((action) => ({ kind: "action" as const, key: `action-${action.id}`, action })),
      ...matchedRequests.map((request) => ({
        kind: "request" as const,
        key: `request-${request.id}`,
        request,
      })),
    ];
  }, [actions, requests, normalizedQuery]);

  useEffect(() => {
    setActiveIndex(0);
  }, [normalizedQuery]);

  function runEntry(entry: PaletteEntry | undefined) {
    if (!entry) {
      return;
    }
    if (entry.kind === "action") {
      entry.action.run();
    } else {
      onSelectRequest(entry.request.id);
    }
    onClose();
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((current) => Math.min(current + 1, entries.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((current) => Math.max(current - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      runEntry(entries[activeIndex]);
    }
  }

  return (
    <div className="modal-backdrop" role="presentation" onClick={onClose}>
      <div
        className="command-palette"
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onClick={(event) => event.stopPropagation()}
      >
        <input
          autoFocus
          className="command-palette-input"
          aria-label="Search commands and requests"
          placeholder="Search requests or run a command…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={handleKeyDown}
        />
        <div className="command-palette-list" role="listbox">
          {entries.length === 0 && <div className="command-palette-empty">No matches.</div>}
          {entries.map((entry, index) => (
            <button
              key={entry.key}
              type="button"
              role="option"
              aria-selected={index === activeIndex}
              className={index === activeIndex ? "active" : undefined}
              onMouseEnter={() => setActiveIndex(index)}
              onClick={() => runEntry(entry)}
            >
              {entry.kind === "action" ? (
                <>
                  <strong>{entry.action.label}</strong>
                  {entry.action.hint && <span>{entry.action.hint}</span>}
                </>
              ) : (
                <>
                  <span className={`method-badge ${entry.request.method.toLowerCase()}`}>
                    {entry.request.method}
                  </span>
                  <strong>{entry.request.name}</strong>
                  <span>{entry.request.path}</span>
                </>
              )}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
