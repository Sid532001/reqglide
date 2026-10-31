import { useState } from "react";
import type { RequestAuth } from "./api";
import { AuthEditor } from "./AuthEditor";

interface AuthPolicyEditorProps {
  title: string;
  description: string;
  auth?: RequestAuth;
  allowInherit: boolean;
  onCancel: () => void;
  onSave: (auth: RequestAuth | undefined) => Promise<boolean>;
}

export function AuthPolicyEditor({
  title,
  description,
  auth,
  allowInherit,
  onCancel,
  onSave,
}: AuthPolicyEditorProps) {
  const [draft, setDraft] = useState<RequestAuth>(() =>
    auth ? structuredClone(auth) : allowInherit ? { kind: "inherit" } : { kind: "none" },
  );
  const [isSaving, setIsSaving] = useState(false);

  async function save() {
    if (isSaving) {
      return;
    }
    setIsSaving(true);
    try {
      await onSave(draft.kind === "inherit" ? undefined : draft);
    } finally {
      setIsSaving(false);
    }
  }

  return (
    <div className="modal-backdrop" role="presentation">
      <section
        className="auth-policy-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="auth-policy-title"
      >
        <header className="network-heading">
          <div>
            <strong id="auth-policy-title">{title}</strong>
            <span>{description}</span>
          </div>
          <button type="button" aria-label="Close authentication policy" onClick={onCancel}>
            ×
          </button>
        </header>
        <div className="auth-policy-body">
          <AuthEditor auth={draft} allowInherit={allowInherit} onChange={setDraft} />
        </div>
        <footer className="modal-actions">
          <button className="ghost-button" type="button" disabled={isSaving} onClick={onCancel}>
            Cancel
          </button>
          <button
            className="primary-small"
            type="button"
            disabled={isSaving}
            onClick={() => void save()}
          >
            {isSaving ? "Saving…" : "Save policy"}
          </button>
        </footer>
      </section>
    </div>
  );
}
