import { describe, expect, it } from "vitest";
import type { WorkspaceDocument } from "./api";
import { collectionRunTargets, parseRunData } from "./collection-runner";

describe("collection runner helpers", () => {
  it("lists collection and nested folder targets with request counts", () => {
    const workspace: WorkspaceDocument = {
      schemaVersion: 7,
      id: "workspace",
      name: "APIs",
      environments: [],
      collections: [
        {
          id: "collection",
          name: "Payments",
          requests: [],
          folders: [
            {
              id: "folder",
              name: "Admin",
              requests: [request("request")],
              folders: [
                {
                  id: "child",
                  name: "Audit",
                  requests: [request("nested")],
                  folders: [],
                },
              ],
            },
          ],
        },
      ],
    };

    expect(collectionRunTargets(workspace).map(({ key, requestCount }) => [key, requestCount]))
      .toEqual([
        ["collection:collection", 2],
        ["folder:folder", 2],
        ["folder:child", 1],
      ]);
  });

  it("parses JSON objects and RFC-style quoted CSV cells", () => {
    expect(parseRunData('{"user":"Ada","active":true}')).toEqual([
      { user: "Ada", active: "true" },
    ]);
    expect(parseRunData('user,note\r\nAda,"hello, world"\r\nGrace,"said ""hi"""')).toEqual([
      { user: "Ada", note: "hello, world" },
      { user: "Grace", note: 'said "hi"' },
    ]);
  });

  it("rejects nested JSON values and malformed CSV", () => {
    expect(() => parseRunData('[{"nested":{"value":1}}]')).toThrow("must be a scalar");
    expect(() => parseRunData('name,name\nAda,Lovelace')).toThrow("duplicate");
  });
});

function request(id: string) {
  return {
    id,
    name: id,
    request: {
      method: "GET",
      url: "https://example.com",
      queryParams: [],
      headers: [],
      auth: { kind: "none" as const },
      body: { kind: "empty" as const },
      options: {
        timeoutMs: 30_000,
        maxRequestBytes: 1024,
        maxResponseBytes: 1024,
        followRedirects: true,
      },
    },
    scripts: { preRequest: "", postResponse: "" },
  };
}
