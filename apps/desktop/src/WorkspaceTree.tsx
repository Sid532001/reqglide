import type { Collection, RequestFolder, SavedRequest, WorkspaceDocument } from "./api";
import { countRequests, type WorkspaceItemTarget } from "./workspace-model";

interface WorkspaceTreeProps {
  workspace: WorkspaceDocument;
  selectedRequestId: string | null;
  expandedIds: ReadonlySet<string>;
  forceExpanded: boolean;
  onToggle: (id: string) => void;
  onSelectRequest: (request: SavedRequest) => void;
  onAddRequest: (collectionId: string, parentFolderId: string | null) => void;
  onAddFolder: (collectionId: string, parentFolderId: string | null) => void;
  onManageItem: (target: WorkspaceItemTarget) => void;
}

export function WorkspaceTree({
  workspace,
  selectedRequestId,
  expandedIds,
  forceExpanded,
  onToggle,
  onSelectRequest,
  onAddRequest,
  onAddFolder,
  onManageItem,
}: WorkspaceTreeProps) {
  if (!workspace.collections.length) {
    return (
      <div className="tree-empty">
        <span>No collections yet</span>
        <small>Create one with the + button above.</small>
      </div>
    );
  }

  return (
    <div className="workspace-tree">
      {workspace.collections.map((collection) => (
        <CollectionNode
          key={collection.id}
          collection={collection}
          selectedRequestId={selectedRequestId}
          expandedIds={expandedIds}
          forceExpanded={forceExpanded}
          onToggle={onToggle}
          onSelectRequest={onSelectRequest}
          onAddRequest={onAddRequest}
          onAddFolder={onAddFolder}
          onManageItem={onManageItem}
        />
      ))}
    </div>
  );
}

interface CollectionNodeProps extends Omit<WorkspaceTreeProps, "workspace"> {
  collection: Collection;
}

function CollectionNode({
  collection,
  selectedRequestId,
  expandedIds,
  forceExpanded,
  onToggle,
  onSelectRequest,
  onAddRequest,
  onAddFolder,
  onManageItem,
}: CollectionNodeProps) {
  const expanded = forceExpanded || expandedIds.has(collection.id);

  return (
    <div className="tree-group">
      <div className="tree-node-row">
        <button
          className="tree-item collection"
          type="button"
          aria-expanded={expanded}
          onClick={() => onToggle(collection.id)}
        >
          <span className={`chevron ${expanded ? "expanded" : ""}`}>›</span>
          <span className="tree-label">{collection.name}</span>
          <span className="tree-count">{countRequests(collection)}</span>
        </button>
        <button
          className="tree-action tree-action-primary"
          type="button"
          title={`Add request to ${collection.name}`}
          aria-label={`Add request to ${collection.name}`}
          onClick={() => onAddRequest(collection.id, null)}
        >
          +
        </button>
        <button
          className="tree-action"
          type="button"
          title={`Add folder to ${collection.name}`}
          aria-label={`Add folder to ${collection.name}`}
          onClick={() => onAddFolder(collection.id, null)}
        >
          +
        </button>
        <button
          className="tree-action"
          type="button"
          title={`Manage ${collection.name}`}
          aria-label={`Manage collection ${collection.name}`}
          onClick={() =>
            onManageItem({
              kind: "collection",
              collectionId: collection.id,
              name: collection.name,
            })
          }
        >
          ⋯
        </button>
      </div>

      {expanded && (
        <div className="tree-children">
          {collection.folders.map((folder) => (
            <FolderNode
              key={folder.id}
              folder={folder}
              collectionId={collection.id}
              depth={1}
              selectedRequestId={selectedRequestId}
              expandedIds={expandedIds}
              forceExpanded={forceExpanded}
              onToggle={onToggle}
              onSelectRequest={onSelectRequest}
              onAddRequest={onAddRequest}
              onAddFolder={onAddFolder}
              onManageItem={onManageItem}
            />
          ))}
          {collection.requests.map((request) => (
            <RequestNode
              key={request.id}
              request={request}
              depth={1}
              selected={selectedRequestId === request.id}
              onSelect={onSelectRequest}
              onManage={onManageItem}
            />
          ))}
          {!collection.folders.length && !collection.requests.length && (
            <div className="tree-empty inline">No saved requests</div>
          )}
        </div>
      )}
    </div>
  );
}

interface FolderNodeProps
  extends Omit<WorkspaceTreeProps, "workspace"> {
  collectionId: string;
  folder: RequestFolder;
  depth: number;
}

function FolderNode({
  collectionId,
  folder,
  depth,
  selectedRequestId,
  expandedIds,
  forceExpanded,
  onToggle,
  onSelectRequest,
  onAddRequest,
  onAddFolder,
  onManageItem,
}: FolderNodeProps) {
  const expanded = forceExpanded || expandedIds.has(folder.id);

  return (
    <div className="tree-group">
      <div className="tree-node-row" style={{ paddingLeft: `${depth * 10}px` }}>
        <button
          className="tree-item folder"
          type="button"
          aria-expanded={expanded}
          onClick={() => onToggle(folder.id)}
        >
          <span className={`chevron ${expanded ? "expanded" : ""}`}>›</span>
          <span className="folder-icon" aria-hidden="true" />
          <span className="tree-label">{folder.name}</span>
        </button>
        <button
          className="tree-action tree-action-primary"
          type="button"
          title={`Add request to ${folder.name}`}
          aria-label={`Add request to ${folder.name}`}
          onClick={() => onAddRequest(collectionId, folder.id)}
        >
          +
        </button>
        <button
          className="tree-action"
          type="button"
          title={`Add nested folder to ${folder.name}`}
          aria-label={`Add nested folder to ${folder.name}`}
          onClick={() => onAddFolder(collectionId, folder.id)}
        >
          +
        </button>
        <button
          className="tree-action"
          type="button"
          title={`Manage ${folder.name}`}
          aria-label={`Manage folder ${folder.name}`}
          onClick={() =>
            onManageItem({
              kind: "folder",
              collectionId,
              folderId: folder.id,
              name: folder.name,
            })
          }
        >
          ⋯
        </button>
      </div>

      {expanded && (
        <div className="tree-children">
          {folder.folders.map((child) => (
            <FolderNode
              key={child.id}
              folder={child}
              collectionId={collectionId}
              depth={depth + 1}
              selectedRequestId={selectedRequestId}
              expandedIds={expandedIds}
              forceExpanded={forceExpanded}
              onToggle={onToggle}
              onSelectRequest={onSelectRequest}
              onAddRequest={onAddRequest}
              onAddFolder={onAddFolder}
              onManageItem={onManageItem}
            />
          ))}
          {folder.requests.map((request) => (
            <RequestNode
              key={request.id}
              request={request}
              depth={depth + 1}
              selected={selectedRequestId === request.id}
              onSelect={onSelectRequest}
              onManage={onManageItem}
            />
          ))}
        </div>
      )}
    </div>
  );
}

interface RequestNodeProps {
  request: SavedRequest;
  depth: number;
  selected: boolean;
  onSelect: (request: SavedRequest) => void;
  onManage: (target: WorkspaceItemTarget) => void;
}

function RequestNode({ request, depth, selected, onSelect, onManage }: RequestNodeProps) {
  return (
    <div className="tree-node-row request-node-row">
      <button
        className={`tree-item request ${selected ? "active" : ""}`}
        style={{ paddingLeft: `${10 + depth * 10}px` }}
        type="button"
        onClick={() => onSelect(request)}
        title={request.request.url || request.name}
      >
        <span className={`method-badge ${request.request.method.toLowerCase()}`}>
          {request.request.method}
        </span>
        <span className="tree-label">{request.name}</span>
      </button>
      <button
        className="tree-action"
        type="button"
        title={`Manage ${request.name}`}
        aria-label={`Manage request ${request.name}`}
        onClick={() =>
          onManage({ kind: "request", requestId: request.id, name: request.name })
        }
      >
        ⋯
      </button>
    </div>
  );
}
