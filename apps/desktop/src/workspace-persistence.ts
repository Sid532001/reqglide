import type { SecretMutation, WorkspaceDocument, WorkspaceSession } from "./api";

export interface WorkspaceChange {
  workspace: WorkspaceDocument;
  mutations?: SecretMutation[];
}

export type WorkspaceUpdate = (workspace: WorkspaceDocument) => WorkspaceChange;

export function createWorkspacePersistence(options: {
  current: () => WorkspaceSession | null;
  save: (workspace: WorkspaceDocument, mutations?: SecretMutation[]) => Promise<WorkspaceSession>;
  onSaved: (session: WorkspaceSession) => void;
  onBusy: (busy: boolean) => void;
}) {
  let tail: Promise<unknown> = Promise.resolve();
  let pending = 0;
  return {
    get busy() { return pending > 0; },
    async idle(): Promise<void> {
      while (pending > 0) {
        await tail;
      }
    },
    enqueue(workspaceId: string, update: WorkspaceUpdate): Promise<WorkspaceSession> {
      pending++;
      options.onBusy(true);
      const result = tail.then(async () => {
        const current = options.current();
        if (!current || current.workspace.id !== workspaceId) {
          throw new Error("The workspace changed before this save could complete.");
        }
        const change = update(current.workspace);
        const saved = await options.save(change.workspace, change.mutations);
        options.onSaved(saved);
        return saved;
      }).finally(() => {
        pending--;
        options.onBusy(pending > 0);
      });
      // A failed save must not prevent later saves or leave the UI busy.
      tail = result.catch(() => undefined);
      return result;
    },
  };
}
