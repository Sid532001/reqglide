import type {
  ApiDocumentImport,
  Collection,
  RequestAuth,
  RequestFolder,
  SavedRequest,
  WorkspaceDocument,
} from "./api";

export function mergeApiDocumentImport(
  workspace: WorkspaceDocument,
  imported: ApiDocumentImport,
): WorkspaceDocument {
  return {
    ...workspace,
    collections: [...workspace.collections, ...structuredClone(imported.collections)],
    environments: [...workspace.environments, ...structuredClone(imported.environments)],
  };
}

export interface RequestLocation {
  collectionId: string;
  folderId: string | null;
}

export type WorkspaceItemTarget =
  | { kind: "collection"; collectionId: string; name: string }
  | { kind: "folder"; collectionId: string; folderId: string; name: string }
  | { kind: "request"; requestId: string; name: string };

export type AuthPolicyTarget =
  | { kind: "workspace"; name: string }
  | Extract<WorkspaceItemTarget, { kind: "collection" | "folder" }>;

export function getAuthPolicy(
  workspace: WorkspaceDocument,
  target: AuthPolicyTarget,
): RequestAuth | undefined {
  if (target.kind === "workspace") {
    return workspace.auth;
  }
  if (target.kind === "collection") {
    return workspace.collections.find(
      (collection) => collection.id === target.collectionId,
    )?.auth;
  }
  return findFolder(workspace, target.folderId)?.auth;
}

export function setAuthPolicy(
  workspace: WorkspaceDocument,
  target: AuthPolicyTarget,
  auth: RequestAuth | undefined,
): WorkspaceDocument {
  if (target.kind === "workspace") {
    return { ...workspace, auth };
  }
  if (target.kind === "collection") {
    return {
      ...workspace,
      collections: workspace.collections.map((collection) =>
        collection.id === target.collectionId ? { ...collection, auth } : collection,
      ),
    };
  }
  return {
    ...workspace,
    collections: workspace.collections.map((collection) =>
      collection.id === target.collectionId
        ? {
            ...collection,
            folders: collection.folders.map((folder) =>
              updateFolder(folder, target.folderId, (current) => ({
                ...current,
                auth,
              })),
            ),
          }
        : collection,
    ),
  };
}

export function findSavedRequest(
  workspace: WorkspaceDocument,
  requestId: string,
): SavedRequest | undefined {
  for (const collection of workspace.collections) {
    const direct = collection.requests.find((request) => request.id === requestId);
    if (direct) {
      return direct;
    }

    for (const folder of collection.folders) {
      const nested = findRequestInFolder(folder, requestId);
      if (nested) {
        return nested;
      }
    }
  }
  return undefined;
}

export function findRequestLocation(
  workspace: WorkspaceDocument,
  requestId: string,
): RequestLocation | undefined {
  for (const collection of workspace.collections) {
    if (collection.requests.some((request) => request.id === requestId)) {
      return { collectionId: collection.id, folderId: null };
    }
    for (const folder of collection.folders) {
      const folderId = findRequestFolderId(folder, requestId);
      if (folderId) {
        return { collectionId: collection.id, folderId };
      }
    }
  }
  return undefined;
}

export function findFolderLocation(
  workspace: WorkspaceDocument,
  folderId: string,
): RequestLocation | undefined {
  for (const collection of workspace.collections) {
    for (const folder of collection.folders) {
      if (folder.id === folderId) {
        return { collectionId: collection.id, folderId: null };
      }
      const parentFolderId = findFolderParentId(folder, folderId);
      if (parentFolderId) {
        return { collectionId: collection.id, folderId: parentFolderId };
      }
    }
  }
  return undefined;
}

export function addCollection(
  workspace: WorkspaceDocument,
  collection: Collection,
): WorkspaceDocument {
  return {
    ...workspace,
    collections: [...workspace.collections, collection],
  };
}

export function addFolder(
  workspace: WorkspaceDocument,
  collectionId: string,
  parentFolderId: string | null,
  folder: RequestFolder,
): WorkspaceDocument {
  return {
    ...workspace,
    collections: workspace.collections.map((collection) => {
      if (collection.id !== collectionId) {
        return collection;
      }

      if (!parentFolderId) {
        return { ...collection, folders: [...collection.folders, folder] };
      }

      return {
        ...collection,
        folders: collection.folders.map((candidate) =>
          updateFolder(candidate, parentFolderId, (parent) => ({
            ...parent,
            folders: [...parent.folders, folder],
          })),
        ),
      };
    }),
  };
}

export function renameWorkspaceItem(
  workspace: WorkspaceDocument,
  target: WorkspaceItemTarget,
  name: string,
): WorkspaceDocument {
  if (target.kind === "collection") {
    return {
      ...workspace,
      collections: workspace.collections.map((collection) =>
        collection.id === target.collectionId ? { ...collection, name } : collection,
      ),
    };
  }
  if (target.kind === "folder") {
    return {
      ...workspace,
      collections: workspace.collections.map((collection) =>
        collection.id === target.collectionId
          ? {
              ...collection,
              folders: collection.folders.map((folder) =>
                updateFolder(folder, target.folderId, (current) => ({
                  ...current,
                  name,
                })),
              ),
            }
          : collection,
      ),
    };
  }
  const request = findSavedRequest(workspace, target.requestId);
  return request
    ? replaceRequestInPlace(workspace, { ...request, name })
    : workspace;
}

export function removeWorkspaceItem(
  workspace: WorkspaceDocument,
  target: WorkspaceItemTarget,
): WorkspaceDocument {
  if (target.kind === "collection") {
    return {
      ...workspace,
      collections: workspace.collections.filter(
        (collection) => collection.id !== target.collectionId,
      ),
    };
  }
  if (target.kind === "folder") {
    return {
      ...workspace,
      collections: workspace.collections.map((collection) =>
        collection.id === target.collectionId
          ? {
              ...collection,
              folders: removeFolderFromTree(collection.folders, target.folderId),
            }
          : collection,
      ),
    };
  }
  return removeRequest(workspace, target.requestId);
}

export function canMoveFolderTo(
  workspace: WorkspaceDocument,
  folderId: string,
  destination: RequestLocation,
): boolean {
  const source = findFolderLocation(workspace, folderId);
  const folder = findFolder(workspace, folderId);
  if (!source || !folder || sameLocation(source, destination)) {
    return false;
  }
  if (!locationExists(workspace, destination)) {
    return false;
  }
  return (
    destination.folderId !== folderId &&
    !folderContains(folder, destination.folderId)
  );
}

export function moveFolder(
  workspace: WorkspaceDocument,
  folderId: string,
  destination: RequestLocation,
): WorkspaceDocument {
  const source = findFolderLocation(workspace, folderId);
  const folder = findFolder(workspace, folderId);
  if (!source || !folder || !canMoveFolderTo(workspace, folderId, destination)) {
    return workspace;
  }
  const withoutFolder = removeWorkspaceItem(workspace, {
    kind: "folder",
    collectionId: source.collectionId,
    folderId,
    name: folder.name,
  });
  return addFolder(
    withoutFolder,
    destination.collectionId,
    destination.folderId,
    folder,
  );
}

export function reorderWorkspaceItem(
  workspace: WorkspaceDocument,
  target: WorkspaceItemTarget,
  offset: -1 | 1,
): WorkspaceDocument {
  if (target.kind === "collection") {
    const collections = moveById(workspace.collections, target.collectionId, offset);
    return collections === workspace.collections ? workspace : { ...workspace, collections };
  }

  if (target.kind === "folder") {
    let changed = false;
    const collections = workspace.collections.map((collection) => {
      if (collection.id !== target.collectionId) {
        return collection;
      }
      const folders = reorderFolderInTree(collection.folders, target.folderId, offset);
      if (folders === collection.folders) {
        return collection;
      }
      changed = true;
      return { ...collection, folders };
    });
    return changed ? { ...workspace, collections } : workspace;
  }

  let changed = false;
  const collections = workspace.collections.map((collection) => {
    const requests = moveById(collection.requests, target.requestId, offset);
    const folders = reorderRequestInFolders(
      collection.folders,
      target.requestId,
      offset,
    );
    if (requests === collection.requests && folders === collection.folders) {
      return collection;
    }
    changed = true;
    return { ...collection, requests, folders };
  });
  return changed ? { ...workspace, collections } : workspace;
}

export function duplicateWorkspaceItem(
  workspace: WorkspaceDocument,
  target: WorkspaceItemTarget,
  duplicateId: string,
  createId: () => string,
): WorkspaceDocument {
  if (target.kind === "collection") {
    return {
      ...workspace,
      collections: insertDuplicate(
        workspace.collections,
        target.collectionId,
        (collection) => cloneCollection(collection, duplicateId, createId),
      ),
    };
  }
  if (target.kind === "folder") {
    return {
      ...workspace,
      collections: workspace.collections.map((collection) =>
        collection.id === target.collectionId
          ? {
              ...collection,
              folders: duplicateFolderInTree(
                collection.folders,
                target.folderId,
                duplicateId,
                createId,
              ),
            }
          : collection,
      ),
    };
  }
  return duplicateRequestInWorkspace(
    workspace,
    target.requestId,
    duplicateId,
  );
}

export function upsertRequest(
  workspace: WorkspaceDocument,
  request: SavedRequest,
  destination?: RequestLocation,
): WorkspaceDocument {
  const withoutExisting = removeRequest(workspace, request.id);
  if (!destination) {
    return replaceRequestInPlace(workspace, request);
  }

  return {
    ...withoutExisting,
    collections: withoutExisting.collections.map((collection) => {
      if (collection.id !== destination.collectionId) {
        return collection;
      }

      if (!destination.folderId) {
        return { ...collection, requests: [...collection.requests, request] };
      }

      return {
        ...collection,
        folders: collection.folders.map((folder) =>
          updateFolder(folder, destination.folderId as string, (target) => ({
            ...target,
            requests: [...target.requests, request],
          })),
        ),
      };
    }),
  };
}

export function countRequests(collection: Collection): number {
  return (
    collection.requests.length +
    collection.folders.reduce((total, folder) => total + countFolderRequests(folder), 0)
  );
}

export function filterWorkspace(
  workspace: WorkspaceDocument,
  query: string,
): WorkspaceDocument {
  const normalized = query.trim().toLocaleLowerCase();
  if (!normalized) {
    return workspace;
  }

  return {
    ...workspace,
    collections: workspace.collections.flatMap((collection) => {
      if (collection.name.toLocaleLowerCase().includes(normalized)) {
        return [collection];
      }

      const requests = collection.requests.filter((request) =>
        requestMatches(request, normalized),
      );
      const folders = collection.folders.flatMap((folder) => {
        const filtered = filterFolder(folder, normalized);
        return filtered ? [filtered] : [];
      });

      return requests.length || folders.length
        ? [{ ...collection, requests, folders }]
        : [];
    }),
  };
}

function findRequestInFolder(
  folder: RequestFolder,
  requestId: string,
): SavedRequest | undefined {
  const direct = folder.requests.find((request) => request.id === requestId);
  if (direct) {
    return direct;
  }
  for (const child of folder.folders) {
    const nested = findRequestInFolder(child, requestId);
    if (nested) {
      return nested;
    }
  }
  return undefined;
}

function findRequestFolderId(
  folder: RequestFolder,
  requestId: string,
): string | undefined {
  if (folder.requests.some((request) => request.id === requestId)) {
    return folder.id;
  }
  for (const child of folder.folders) {
    const nested = findRequestFolderId(child, requestId);
    if (nested) {
      return nested;
    }
  }
  return undefined;
}

function findFolderParentId(
  parent: RequestFolder,
  folderId: string,
): string | undefined {
  if (parent.folders.some((folder) => folder.id === folderId)) {
    return parent.id;
  }
  for (const child of parent.folders) {
    const nested = findFolderParentId(child, folderId);
    if (nested) {
      return nested;
    }
  }
  return undefined;
}

function findFolder(
  workspace: WorkspaceDocument,
  folderId: string,
): RequestFolder | undefined {
  for (const collection of workspace.collections) {
    for (const folder of collection.folders) {
      const found = findFolderInTree(folder, folderId);
      if (found) {
        return found;
      }
    }
  }
  return undefined;
}

function findFolderInTree(
  folder: RequestFolder,
  folderId: string,
): RequestFolder | undefined {
  if (folder.id === folderId) {
    return folder;
  }
  for (const child of folder.folders) {
    const found = findFolderInTree(child, folderId);
    if (found) {
      return found;
    }
  }
  return undefined;
}

function sameLocation(left: RequestLocation, right: RequestLocation): boolean {
  return left.collectionId === right.collectionId && left.folderId === right.folderId;
}

function locationExists(
  workspace: WorkspaceDocument,
  location: RequestLocation,
): boolean {
  const collection = workspace.collections.find(
    (candidate) => candidate.id === location.collectionId,
  );
  if (!collection) {
    return false;
  }
  return (
    location.folderId === null ||
    collection.folders.some((folder) =>
      Boolean(findFolderInTree(folder, location.folderId as string)),
    )
  );
}

function folderContains(folder: RequestFolder, folderId: string | null): boolean {
  return (
    folderId !== null &&
    folder.folders.some(
      (child) => child.id === folderId || folderContains(child, folderId),
    )
  );
}

function updateFolder(
  folder: RequestFolder,
  folderId: string,
  update: (folder: RequestFolder) => RequestFolder,
): RequestFolder {
  if (folder.id === folderId) {
    return update(folder);
  }
  return {
    ...folder,
    folders: folder.folders.map((child) => updateFolder(child, folderId, update)),
  };
}

function removeRequest(
  workspace: WorkspaceDocument,
  requestId: string,
): WorkspaceDocument {
  return {
    ...workspace,
    collections: workspace.collections.map((collection) => ({
      ...collection,
      requests: collection.requests.filter((request) => request.id !== requestId),
      folders: collection.folders.map((folder) => removeRequestFromFolder(folder, requestId)),
    })),
  };
}

function removeFolderFromTree(
  folders: RequestFolder[],
  folderId: string,
): RequestFolder[] {
  return folders
    .filter((folder) => folder.id !== folderId)
    .map((folder) => ({
      ...folder,
      folders: removeFolderFromTree(folder.folders, folderId),
    }));
}

function insertDuplicate<T extends { id: string }>(
  items: T[],
  sourceId: string,
  duplicate: (source: T) => T,
): T[] {
  return items.flatMap((item) =>
    item.id === sourceId ? [item, duplicate(item)] : [item],
  );
}

function moveById<T extends { id: string }>(
  items: T[],
  id: string,
  offset: -1 | 1,
): T[] {
  const index = items.findIndex((item) => item.id === id);
  const destination = index + offset;
  if (index < 0 || destination < 0 || destination >= items.length) {
    return items;
  }
  const reordered = [...items];
  const [item] = reordered.splice(index, 1);
  if (!item) {
    return items;
  }
  reordered.splice(destination, 0, item);
  return reordered;
}

function reorderFolderInTree(
  folders: RequestFolder[],
  folderId: string,
  offset: -1 | 1,
): RequestFolder[] {
  const reordered = moveById(folders, folderId, offset);
  if (reordered !== folders) {
    return reordered;
  }
  let changed = false;
  const nested = folders.map((folder) => {
    const children = reorderFolderInTree(folder.folders, folderId, offset);
    if (children === folder.folders) {
      return folder;
    }
    changed = true;
    return { ...folder, folders: children };
  });
  return changed ? nested : folders;
}

function reorderRequestInFolders(
  folders: RequestFolder[],
  requestId: string,
  offset: -1 | 1,
): RequestFolder[] {
  let changed = false;
  const reordered = folders.map((folder) => {
    const requests = moveById(folder.requests, requestId, offset);
    const children = reorderRequestInFolders(folder.folders, requestId, offset);
    if (requests === folder.requests && children === folder.folders) {
      return folder;
    }
    changed = true;
    return { ...folder, requests, folders: children };
  });
  return changed ? reordered : folders;
}

function cloneCollection(
  collection: Collection,
  id: string,
  createId: () => string,
): Collection {
  return {
    ...collection,
    id,
    name: `${collection.name} copy`,
    folders: collection.folders.map((folder) => cloneFolder(folder, createId(), createId)),
    requests: collection.requests.map((request) =>
      cloneSavedRequest(request, createId()),
    ),
  };
}

function cloneFolder(
  folder: RequestFolder,
  id: string,
  createId: () => string,
  rename = false,
): RequestFolder {
  return {
    ...folder,
    id,
    name: rename ? `${folder.name} copy` : folder.name,
    folders: folder.folders.map((child) => cloneFolder(child, createId(), createId)),
    requests: folder.requests.map((request) => cloneSavedRequest(request, createId())),
  };
}

function cloneSavedRequest(
  request: SavedRequest,
  id: string,
  rename = false,
): SavedRequest {
  return {
    ...structuredClone(request),
    id,
    name: rename ? `${request.name} copy` : request.name,
  };
}

function duplicateFolderInTree(
  folders: RequestFolder[],
  folderId: string,
  duplicateId: string,
  createId: () => string,
): RequestFolder[] {
  return folders.flatMap((folder) => {
    if (folder.id === folderId) {
      return [folder, cloneFolder(folder, duplicateId, createId, true)];
    }
    return [
      {
        ...folder,
        folders: duplicateFolderInTree(
          folder.folders,
          folderId,
          duplicateId,
          createId,
        ),
      },
    ];
  });
}

function duplicateRequestInWorkspace(
  workspace: WorkspaceDocument,
  requestId: string,
  duplicateId: string,
): WorkspaceDocument {
  return {
    ...workspace,
    collections: workspace.collections.map((collection) => ({
      ...collection,
      requests: insertDuplicate(collection.requests, requestId, (request) =>
        cloneSavedRequest(request, duplicateId, true),
      ),
      folders: collection.folders.map((folder) =>
        duplicateRequestInFolder(folder, requestId, duplicateId),
      ),
    })),
  };
}

function duplicateRequestInFolder(
  folder: RequestFolder,
  requestId: string,
  duplicateId: string,
): RequestFolder {
  return {
    ...folder,
    requests: insertDuplicate(folder.requests, requestId, (request) =>
      cloneSavedRequest(request, duplicateId, true),
    ),
    folders: folder.folders.map((child) =>
      duplicateRequestInFolder(child, requestId, duplicateId),
    ),
  };
}

function removeRequestFromFolder(
  folder: RequestFolder,
  requestId: string,
): RequestFolder {
  return {
    ...folder,
    requests: folder.requests.filter((request) => request.id !== requestId),
    folders: folder.folders.map((child) => removeRequestFromFolder(child, requestId)),
  };
}

function replaceRequestInPlace(
  workspace: WorkspaceDocument,
  request: SavedRequest,
): WorkspaceDocument {
  return {
    ...workspace,
    collections: workspace.collections.map((collection) => ({
      ...collection,
      requests: collection.requests.map((candidate) =>
        candidate.id === request.id ? request : candidate,
      ),
      folders: collection.folders.map((folder) => replaceRequestInFolder(folder, request)),
    })),
  };
}

function replaceRequestInFolder(
  folder: RequestFolder,
  request: SavedRequest,
): RequestFolder {
  return {
    ...folder,
    requests: folder.requests.map((candidate) =>
      candidate.id === request.id ? request : candidate,
    ),
    folders: folder.folders.map((child) => replaceRequestInFolder(child, request)),
  };
}

function countFolderRequests(folder: RequestFolder): number {
  return (
    folder.requests.length +
    folder.folders.reduce((total, child) => total + countFolderRequests(child), 0)
  );
}

function requestMatches(request: SavedRequest, query: string): boolean {
  return (
    request.name.toLocaleLowerCase().includes(query) ||
    request.request.url.toLocaleLowerCase().includes(query) ||
    request.request.method.toLocaleLowerCase().includes(query)
  );
}

function filterFolder(folder: RequestFolder, query: string): RequestFolder | null {
  if (folder.name.toLocaleLowerCase().includes(query)) {
    return folder;
  }

  const requests = folder.requests.filter((request) => requestMatches(request, query));
  const folders = folder.folders.flatMap((child) => {
    const filtered = filterFolder(child, query);
    return filtered ? [filtered] : [];
  });

  return requests.length || folders.length ? { ...folder, requests, folders } : null;
}
