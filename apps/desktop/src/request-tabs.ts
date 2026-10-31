import type {
  RequestDefinition,
  RequestScripts,
  ResponseSnapshot,
  ScriptReport,
  SavedRequest,
} from "./api";

export type RequestEditorTab =
  | "params"
  | "auth"
  | "headers"
  | "body"
  | "scripts"
  | "settings";
export type ResponseEditorTab = "body" | "headers" | "tests" | "console";
export type ScriptEditorTab = "preRequest" | "postResponse";

export interface OpenRequestTab {
  id: string;
  requestId: string | null;
  title: string;
  request: RequestDefinition;
  scripts: RequestScripts;
  dirty: boolean;
  requestTab: RequestEditorTab;
  responseTab: ResponseEditorTab;
  scriptTab: ScriptEditorTab;
  response: ResponseSnapshot | null;
  scriptRuns: ScriptReport[];
  requestError: string | null;
}

export interface RecoverySnapshot {
  schemaVersion: 1;
  workspaceId: string;
  activeTabId: string;
  activeEnvironmentId: string | null;
  tabs: RecoveryTab[];
}

export type RecoveryTab = Omit<
  OpenRequestTab,
  "response" | "scriptRuns" | "requestError"
>;

export function duplicateRequestTab(source: OpenRequestTab, id: string): OpenRequestTab {
  const { response: _response, scriptRuns: _scriptRuns, requestError: _requestError, ...draft } = source;
  return {
    ...structuredClone(draft),
    id,
    requestId: null,
    title: `${source.title} copy`,
    dirty: true,
    response: null,
    scriptRuns: [],
    requestError: null,
  };
}

export function markRequestTabSaved(tab: OpenRequestTab, saved: SavedRequest): OpenRequestTab {
  return {
    ...tab,
    requestId: saved.id,
    title: saved.name,
    dirty: JSON.stringify(tab.request) !== JSON.stringify(saved.request)
      || JSON.stringify(tab.scripts) !== JSON.stringify(saved.scripts),
  };
}

export function toRecoverySnapshot(
  workspaceId: string,
  tabs: OpenRequestTab[],
  activeTabId: string,
  activeEnvironmentId: string | null,
): RecoverySnapshot {
  return {
    schemaVersion: 1,
    workspaceId,
    activeTabId,
    activeEnvironmentId,
    tabs: tabs.map(
      ({ response: _response, scriptRuns: _scriptRuns, requestError: _requestError, ...tab }) =>
        structuredClone(tab),
    ),
  };
}

export function restoreRecoveryTabs(snapshot: RecoverySnapshot): OpenRequestTab[] {
  return snapshot.tabs.map((tab) => ({
    ...structuredClone(tab),
    response: null,
    scriptRuns: [],
    requestError: null,
  }));
}

export function requestTabAfterClose(
  tabs: OpenRequestTab[],
  closingId: string,
): OpenRequestTab | undefined {
  const index = tabs.findIndex((tab) => tab.id === closingId);
  if (index < 0) {
    return undefined;
  }
  return tabs[index + 1] ?? tabs[index - 1];
}
