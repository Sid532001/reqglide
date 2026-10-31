import { useEffect, useRef, useState } from "react";

export type WorkspaceMaintenanceAction = "backup" | "restore" | "diagnostics";

const CONTENT = {
  backup: {
    title: "Back up workspace",
    description: "Save a private copy of your saved workspace.",
    details: "The unencrypted JSON file includes saved requests, scripts, Shared variables, and any credentials entered directly in requests or network settings. Keep it in a trusted location.",
    limits: "Secret and Local variable values stay in the OS credential store. Unsaved tabs, history, cookies, uploaded files, and certificate files are excluded.",
    action: "Choose backup location",
  },
  restore: {
    title: "Restore workspace backup",
    description: "Open a backup as a separate workspace.",
    details: "Choose your backup file, then a parent folder. Restore creates a new workspace and never replaces an existing workspace folder. Review restored requests and scripts before sending.",
    limits: "Re-enter Secret and Local variable values after restoring. Referenced upload and certificate files must be available on this device.",
    action: "Choose backup file",
  },
  diagnostics: {
    title: "Export support diagnostics",
    description: "Save a small diagnostic summary on your device.",
    details: "Includes the app version, operating system, architecture, workspace counts, proxy mode, and TLS configuration flags. Nothing is uploaded automatically.",
    limits: "Request data, names, URLs, file paths, scripts, variable values, credentials, history, and cookies are excluded.",
    action: "Choose diagnostics location",
  },
} as const;

export function WorkspaceMaintenanceDialog({ action, onRun, onClose }: {
  action: WorkspaceMaintenanceAction;
  onRun: (action: WorkspaceMaintenanceAction) => Promise<void>;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const running = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const content = CONTENT[action];

  useEffect(() => {
    const element = dialog.current!;
    element.showModal();
    return () => element.close();
  }, []);

  async function run() {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setError(null);
    try {
      await onRun(action);
      onClose();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      running.current = false;
      setBusy(false);
    }
  }

  return (
    <dialog
      ref={dialog}
      className="modal-card workspace-maintenance-card"
      aria-labelledby="maintenance-title"
      aria-describedby="maintenance-details maintenance-limits"
      aria-busy={busy}
      onKeyDown={(event) => event.stopPropagation()}
      onCancel={(event) => {
        event.preventDefault();
        if (!running.current) onClose();
      }}
    >
      <div className="modal-heading">
        <div>
          <strong id="maintenance-title">{content.title}</strong>
          <span>{content.description}</span>
        </div>
      </div>
      <p id="maintenance-details">{content.details}</p>
      <p id="maintenance-limits">{content.limits}</p>
      {error && <p role="alert">{error}</p>}
      <div className="modal-actions">
        <button className="ghost-button" type="button" disabled={busy} onClick={onClose}>Cancel</button>
        <button className="primary-small" type="button" disabled={busy} onClick={() => void run()}>
          {busy ? "Working…" : content.action}
        </button>
      </div>
    </dialog>
  );
}
