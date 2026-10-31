import { describe, expect, it } from "vitest";
import type {
  Collection,
  RequestDefinition,
  RequestFolder,
  SavedRequest,
  WorkspaceDocument,
} from "./api";
import {
  addFolder,
  countRequests,
  duplicateWorkspaceItem,
  filterWorkspace,
  findFolderLocation,
  findRequestLocation,
  findSavedRequest,
  getAuthPolicy,
  mergeApiDocumentImport,
  moveFolder,
  removeWorkspaceItem,
  renameWorkspaceItem,
  reorderWorkspaceItem,
  setAuthPolicy,
  upsertRequest,
} from "./workspace-model";

const requestDefinition: RequestDefinition = {
  method: "GET",
  url: "https://example.com/payments",
  queryParams: [],
  headers: [],
  auth: { kind: "none" },
  body: { kind: "empty" },
  options: {
    timeoutMs: 30_000,
    maxRequestBytes: 100 * 1024 * 1024,
    maxResponseBytes: 10 * 1024 * 1024,
    followRedirects: true,
  },
};

function saved(id: string, name = "List payments"): SavedRequest {
  return {
    id,
    name,
    request: requestDefinition,
    scripts: { preRequest: "", postResponse: "" },
  };
}

function collection(): Collection {
  return { id: "collection", name: "Payments", folders: [], requests: [] };
}

function workspace(): WorkspaceDocument {
  return {
    schemaVersion: 7,
    id: "workspace",
    name: "Team APIs",
    collections: [collection()],
    environments: [],
  };
}

describe("workspace model", () => {
  it("merges imported collections and environments without mutating the workspace", () => {
    const original = workspace();
    const merged = mergeApiDocumentImport(original, {
      kind: "openApi",
      name: "Pet Store",
      collections: [
        { id: "imported-collection", name: "Pet Store", folders: [], requests: [] },
      ],
      environments: [
        { id: "imported-environment", name: "Pet Store", variables: [] },
      ],
      warnings: [],
    });

    expect(original.collections).toHaveLength(1);
    expect(original.environments).toHaveLength(0);
    expect(merged.collections.map((item) => item.id)).toEqual([
      "collection",
      "imported-collection",
    ]);
    expect(merged.environments[0]?.id).toBe("imported-environment");
    expect(merged.collections[1]).not.toBe(merged.collections[0]);
  });

  it("sets workspace, collection, and nested folder authentication policies immutably", () => {
    const withFolder = addFolder(workspace(), "collection", null, {
      id: "folder",
      name: "Admin",
      folders: [],
      requests: [],
    });
    const workspaceTarget = { kind: "workspace" as const, name: "Team APIs" };
    const collectionTarget = {
      kind: "collection" as const,
      collectionId: "collection",
      name: "Payments",
    };
    const folderTarget = {
      kind: "folder" as const,
      collectionId: "collection",
      folderId: "folder",
      name: "Admin",
    };

    const withWorkspaceAuth = setAuthPolicy(withFolder, workspaceTarget, {
      kind: "bearer",
      token: "{{workspaceToken}}",
    });
    const withCollectionAuth = setAuthPolicy(withWorkspaceAuth, collectionTarget, {
      kind: "none",
    });
    const withFolderAuth = setAuthPolicy(withCollectionAuth, folderTarget, {
      kind: "apiKey",
      key: "X-API-Key",
      value: "{{apiKey}}",
      placement: "header",
    });

    expect(getAuthPolicy(withFolderAuth, workspaceTarget)?.kind).toBe("bearer");
    expect(getAuthPolicy(withFolderAuth, collectionTarget)?.kind).toBe("none");
    expect(getAuthPolicy(withFolderAuth, folderTarget)?.kind).toBe("apiKey");
    expect(getAuthPolicy(withFolder, workspaceTarget)).toBeUndefined();
  });

  it("adds and finds a request in a nested folder", () => {
    const folder: RequestFolder = {
      id: "folder",
      name: "Admin",
      folders: [],
      requests: [],
    };
    const withFolder = addFolder(workspace(), "collection", null, folder);
    const withRequest = upsertRequest(withFolder, saved("request"), {
      collectionId: "collection",
      folderId: "folder",
    });

    expect(findSavedRequest(withRequest, "request")?.name).toBe("List payments");
    expect(findRequestLocation(withRequest, "request")).toEqual({
      collectionId: "collection",
      folderId: "folder",
    });
    expect(countRequests(withRequest.collections[0]!)).toBe(1);
  });

  it("updates a request without moving it", () => {
    const initial = upsertRequest(workspace(), saved("request"), {
      collectionId: "collection",
      folderId: null,
    });
    const updated = upsertRequest(initial, saved("request", "Updated name"));

    expect(updated.collections[0]?.requests).toHaveLength(1);
    expect(updated.collections[0]?.requests[0]?.name).toBe("Updated name");
  });

  it("filters by request URL while preserving its parents", () => {
    const populated = upsertRequest(workspace(), saved("request"), {
      collectionId: "collection",
      folderId: null,
    });

    expect(filterWorkspace(populated, "payments").collections).toHaveLength(1);
    expect(filterWorkspace(populated, "does-not-exist").collections).toHaveLength(0);
  });

  it("renames and removes nested workspace items immutably", () => {
    const folder: RequestFolder = {
      id: "folder",
      name: "Admin",
      folders: [],
      requests: [saved("request")],
    };
    const populated = addFolder(workspace(), "collection", null, folder);
    const renamed = renameWorkspaceItem(
      populated,
      { kind: "folder", collectionId: "collection", folderId: "folder", name: "Admin" },
      "Operations",
    );
    expect(renamed.collections[0]?.folders[0]?.name).toBe("Operations");
    expect(populated.collections[0]?.folders[0]?.name).toBe("Admin");

    const removed = removeWorkspaceItem(renamed, {
      kind: "request",
      requestId: "request",
      name: "List payments",
    });
    expect(findSavedRequest(removed, "request")).toBeUndefined();
  });

  it("duplicates folders and regenerates every descendant id", () => {
    const folder: RequestFolder = {
      id: "folder",
      name: "Admin",
      folders: [
        { id: "child", name: "Nested", folders: [], requests: [saved("nested-request")] },
      ],
      requests: [saved("request")],
    };
    const populated = addFolder(workspace(), "collection", null, folder);
    let sequence = 0;
    const duplicated = duplicateWorkspaceItem(
      populated,
      { kind: "folder", collectionId: "collection", folderId: "folder", name: "Admin" },
      "folder-copy",
      () => `generated-${sequence++}`,
    );

    const copy = duplicated.collections[0]?.folders[1];
    expect(copy?.name).toBe("Admin copy");
    expect(copy?.id).toBe("folder-copy");
    expect(copy?.requests[0]?.id).not.toBe("request");
    expect(copy?.folders[0]?.id).not.toBe("child");
    expect(copy?.folders[0]?.requests[0]?.id).not.toBe("nested-request");
  });

  it("moves folders across collections while rejecting cycles", () => {
    const parent: RequestFolder = {
      id: "parent",
      name: "Parent",
      folders: [
        {
          id: "child",
          name: "Child",
          folders: [],
          requests: [saved("nested-request")],
        },
      ],
      requests: [],
    };
    const initial: WorkspaceDocument = {
      ...workspace(),
      collections: [
        { ...collection(), folders: [parent] },
        { id: "secondary", name: "Secondary", folders: [], requests: [] },
      ],
    };

    expect(findFolderLocation(initial, "child")).toEqual({
      collectionId: "collection",
      folderId: "parent",
    });
    expect(
      moveFolder(initial, "parent", {
        collectionId: "collection",
        folderId: "child",
      }),
    ).toBe(initial);

    const moved = moveFolder(initial, "child", {
      collectionId: "secondary",
      folderId: null,
    });
    expect(moved.collections[0]?.folders[0]?.folders).toHaveLength(0);
    expect(moved.collections[1]?.folders[0]?.id).toBe("child");
    expect(findSavedRequest(moved, "nested-request")).toBeDefined();
  });

  it("reorders collections, folders, and requests within their siblings", () => {
    const initial: WorkspaceDocument = {
      ...workspace(),
      collections: [
        {
          ...collection(),
          folders: [
            { id: "first-folder", name: "First", folders: [], requests: [] },
            {
              id: "second-folder",
              name: "Second",
              folders: [],
              requests: [saved("first-request"), saved("second-request")],
            },
          ],
        },
        { id: "secondary", name: "Secondary", folders: [], requests: [] },
      ],
    };

    const collections = reorderWorkspaceItem(
      initial,
      { kind: "collection", collectionId: "secondary", name: "Secondary" },
      -1,
    );
    expect(collections.collections.map((item) => item.id)).toEqual([
      "secondary",
      "collection",
    ]);

    const folders = reorderWorkspaceItem(
      initial,
      {
        kind: "folder",
        collectionId: "collection",
        folderId: "second-folder",
        name: "Second",
      },
      -1,
    );
    expect(folders.collections[0]?.folders.map((item) => item.id)).toEqual([
      "second-folder",
      "first-folder",
    ]);

    const requests = reorderWorkspaceItem(
      initial,
      { kind: "request", requestId: "second-request", name: "Second request" },
      -1,
    );
    expect(
      requests.collections[0]?.folders[1]?.requests.map((item) => item.id),
    ).toEqual(["second-request", "first-request"]);
  });
});
