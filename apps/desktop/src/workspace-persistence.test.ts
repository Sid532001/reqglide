import { describe, expect, it, vi } from "vitest";
import type { SecretMutation, WorkspaceDocument, WorkspaceSession } from "./api";
import { applyScriptEnvironmentUpdates, saveExtractedVariable } from "./environment-secrets";
import { createWorkspacePersistence } from "./workspace-persistence";

function session(): WorkspaceSession {
  return { rootPath: "/workspace", workspace: {
    schemaVersion: 1, id: "workspace", name: "Workspace", collections: [],
    environments: [{ id: "env", name: "Development", variables: [] }],
  } };
}

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

describe("workspace persistence", () => {
  it("serializes saves against the latest workspace and keeps Send waiting for script values", async () => {
    let current = session();
    const gate = deferred();
    const saves: WorkspaceDocument[] = [];
    const onBusy = vi.fn();
    const queue = createWorkspacePersistence({
      current: () => current,
      save: async (workspace) => {
        saves.push(workspace);
        if (saves.length === 1) await gate.promise;
        return { ...current, workspace };
      },
      onSaved: (saved) => { current = saved; }, onBusy,
    });
    const first = queue.enqueue("workspace", (workspace) => ({ workspace: { ...workspace, name: "Renamed" } }));
    const scripts = queue.enqueue("workspace", (workspace) => ({ workspace: {
      ...workspace, environments: applyScriptEnvironmentUpdates(workspace.environments, "env", { token: "new" }, () => "token"),
    } }));
    const nextSend = vi.fn();
    const send = queue.idle().then(nextSend);
    await Promise.resolve();
    expect(saves).toHaveLength(1);
    expect(nextSend).not.toHaveBeenCalled();
    gate.resolve();
    await Promise.all([first, scripts, send]);
    expect(saves[1]?.name).toBe("Renamed");
    expect(current.workspace.environments[0]?.variables[0]?.value).toBe("new");
    expect(nextSend).toHaveBeenCalledOnce();
    expect(onBusy).toHaveBeenLastCalledWith(false);
  });

  it("merges simultaneous extracted values and supplies secrets only as mutations", async () => {
    let current = session();
    const mutations: (SecretMutation[] | undefined)[] = [];
    const queue = createWorkspacePersistence({
      current: () => current,
      save: async (workspace, changes) => { mutations.push(changes); return { ...current, workspace }; },
      onSaved: (saved) => { current = saved; }, onBusy: () => {},
    });
    await Promise.all(["access_token", "expires_in"].map((key) => queue.enqueue("workspace", (workspace) => {
      const change = saveExtractedVariable(workspace.environments, "env", key, key === "access_token" ? "credential" : "3600", () => key);
      return { workspace: { ...workspace, environments: change.environments }, mutations: change.mutations };
    })));
    expect(current.workspace.environments[0]?.variables).toHaveLength(2);
    expect(JSON.stringify(current.workspace)).not.toContain("credential");
    expect(mutations).toEqual([[{ variableId: "access_token", value: "credential" }], []]);
  });

  it("recovers after failure and rejects queued saves after a workspace switch", async () => {
    let current = session();
    const save = vi.fn(async (workspace: WorkspaceDocument) => ({ ...current, workspace }));
    save.mockRejectedValueOnce(new Error("disk full"));
    const queue = createWorkspacePersistence({ current: () => current, save, onSaved: (saved) => { current = saved; }, onBusy: () => {} });
    await expect(queue.enqueue("workspace", (workspace) => ({ workspace }))).rejects.toThrow("disk full");
    await expect(queue.enqueue("workspace", (workspace) => ({ workspace }))).resolves.toBeDefined();
    current = { ...current, workspace: { ...current.workspace, id: "other" } };
    await expect(queue.enqueue("workspace", (workspace) => ({ workspace }))).rejects.toThrow("workspace changed");
    expect(queue.busy).toBe(false);
    expect(save).toHaveBeenCalledTimes(2);
  });
});
