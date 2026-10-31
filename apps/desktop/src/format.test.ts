import { describe, expect, it } from "vitest";
import type { ResponseSnapshot } from "./api";
import {
  canPreviewResponse,
  countTextMatches,
  displayResponseBody,
  formatBytes,
  responseFileName,
  sandboxedHtmlPreview,
} from "./format";

function response(content: string, contentType: string): ResponseSnapshot {
  return {
    status: 200,
    statusText: "OK",
    headers: [],
    body: {
      content,
      contentType,
      encoding: "utf8",
      truncated: false,
    },
    timing: { totalMs: 12 },
    receivedBytes: content.length,
  };
}

describe("response formatting", () => {
  it("pretty prints JSON without changing non-JSON text", () => {
    expect(displayResponseBody(response('{"ready":true}', "application/json"))).toBe(
      '{\n  "ready": true\n}',
    );
    expect(displayResponseBody(response("hello", "text/plain"))).toBe("hello");
  });

  it("formats byte counts", () => {
    expect(formatBytes(800)).toBe("800 B");
    expect(formatBytes(2048)).toBe("2.00 KB");
  });

  it("counts non-overlapping response search matches", () => {
    expect(countTextMatches("One one ONE", "one")).toBe(3);
    expect(countTextMatches("aaaa", "aa")).toBe(2);
    expect(countTextMatches("text", "")).toBe(0);
  });

  it("chooses a safe response filename and preview policy", () => {
    const html = response("<h1>Hello</h1>", "text/html; charset=utf-8");
    expect(responseFileName(html)).toBe("response.html");
    expect(canPreviewResponse(html)).toBe(true);
    expect(sandboxedHtmlPreview(html.body.content)).toContain("default-src 'none'");
    expect(canPreviewResponse(response("{}", "application/json"))).toBe(false);
  });
});
