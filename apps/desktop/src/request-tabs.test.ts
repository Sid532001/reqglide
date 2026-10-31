import { describe, expect, it } from "vitest";
import type { OpenRequestTab } from "./request-tabs";
import {
  duplicateRequestTab,
  markRequestTabSaved,
  requestTabAfterClose,
  restoreRecoveryTabs,
  toRecoverySnapshot,
} from "./request-tabs";

function tab(id: string): OpenRequestTab {
  return {
    id,
    requestId: id === "scratch" ? null : id,
    title: id,
    request: {
      method: "GET",
      url: "https://example.com",
      queryParams: [],
      headers: [],
      auth: { kind: "none" },
      body: { kind: "empty" },
      options: {
        timeoutMs: 30_000,
        maxRequestBytes: 1024,
        maxResponseBytes: 1024,
        followRedirects: true,
      },
    },
    scripts: { preRequest: "", postResponse: "" },
    dirty: false,
    requestTab: "headers",
    responseTab: "body",
    scriptTab: "preRequest",
    response: null,
    scriptRuns: [],
    requestError: null,
  };
}

describe("request tabs", () => {
  it("duplicates a draft without linking it to the saved request", () => {
    const source = tab("saved");
    source.request.headers.push({ name: "Accept", value: "application/json", enabled: true });
    const copy = duplicateRequestTab(source, "copy");

    expect(copy).toMatchObject({ id: "copy", requestId: null, title: "saved copy", dirty: true });
    copy.request.headers[0]!.value = "text/plain";
    expect(source.request.headers[0]?.value).toBe("application/json");
  });

  it("chooses the next tab, then the previous tab, after close", () => {
    const tabs = [tab("a"), tab("b"), tab("c")];
    expect(requestTabAfterClose(tabs, "b")?.id).toBe("c");
    expect(requestTabAfterClose(tabs, "c")?.id).toBe("b");
  });

  it("keeps edits made while a save was pending dirty", () => {
    const draft = tab("scratch");
    const saved = { id: "saved", name: "Saved request", request: structuredClone(draft.request), scripts: { ...draft.scripts } };
    draft.request.url = "https://example.com/edited-after-save-started";
    const updated = markRequestTabSaved(draft, saved);
    expect(updated).toMatchObject({ requestId: "saved", title: "Saved request", dirty: true });
    expect(updated.request.url).toBe(draft.request.url);
    expect(markRequestTabSaved({ ...draft, request: saved.request }, saved).dirty).toBe(false);
  });

  it("omits responses, script output, and errors from crash recovery", () => {
    const source = tab("a");
    source.requestError = "private diagnostic";
    source.scriptRuns = [
      { phase: "preRequest", logs: [], tests: [], error: null, durationMs: 1 },
    ];
    source.response = {
      status: 200,
      statusText: "OK",
      headers: [],
      body: { content: "private body", encoding: "utf8", contentType: null, truncated: false },
      timing: { totalMs: 1 },
      receivedBytes: 12,
    };

    const recovery = toRecoverySnapshot("workspace", [source], "a", null);
    expect(recovery.tabs[0]).not.toHaveProperty("response");
    const restored = restoreRecoveryTabs(recovery);
    expect(restored[0]).toMatchObject({ response: null, scriptRuns: [], requestError: null });
  });
});
