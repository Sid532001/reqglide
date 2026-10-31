import { describe, expect, it } from "vitest";
import type { Environment, RequestDefinition } from "./api";
import { resolveRequestVariables } from "./variables";

const request: RequestDefinition = {
  method: "POST",
  url: "{{baseUrl}}/users/{{userId}}",
  queryParams: [
    { name: "owner", value: "{{userId}}", enabled: true },
  ],
  headers: [
    { name: "X-{{headerName}}", value: "{{token}}", enabled: true },
  ],
  auth: { kind: "bearer", token: "{{token}}" },
  body: { kind: "text", content: "{\"id\":\"{{userId}}\"}" },
  options: {
    timeoutMs: 30_000,
    maxRequestBytes: 100 * 1024 * 1024,
    maxResponseBytes: 10 * 1024 * 1024,
    followRedirects: true,
  },
};

function environment(variables: Environment["variables"]): Environment {
  return { id: "environment", name: "Development", variables };
}

function variable(id: string, key: string, value: string, enabled = true) {
  return { id, key, value, enabled };
}

const optionalEntryKinds: {
  kind: string;
  add: (draft: RequestDefinition, enabled: boolean) => void;
}[] = [
  { kind: "header", add: (draft, enabled) => { draft.headers.push(optionalEntry(enabled)); } },
  { kind: "query", add: (draft, enabled) => { draft.queryParams.push(optionalEntry(enabled)); } },
  { kind: "form", add: (draft, enabled) => { draft.body = { kind: "formUrlEncoded", fields: [optionalEntry(enabled)] }; } },
  { kind: "multipart text", add: (draft, enabled) => {
    draft.body = { kind: "multipart", parts: [{ name: entryTemplate, enabled,
      value: { kind: "text", content: entryTemplate, contentType: entryTemplate } }] };
  } },
  { kind: "multipart file", add: (draft, enabled) => {
    draft.body = { kind: "multipart", parts: [{ name: entryTemplate, enabled,
      value: { kind: "file", path: entryTemplate, fileName: entryTemplate, contentType: entryTemplate } }] };
  } },
  { kind: "OAuth extra parameter", add: (draft, enabled) => {
    draft.auth = { kind: "oauth2ClientCredentials", issuerUrl: "", tokenUrl: "https://id.example.com/token",
      clientId: "client", clientSecret: "secret", scope: "", audience: "", clientAuth: "basic",
      extraParameters: [optionalEntry(enabled)] };
  } },
];

const entryTemplate = "{{known}}/{{missing}}/{{first}}";
function optionalEntry(enabled: boolean) {
  return { name: entryTemplate, value: entryTemplate, enabled };
}

function requestWithOptionalEntry(add: (draft: RequestDefinition, enabled: boolean) => void, enabled: boolean) {
  const draft: RequestDefinition = {
    ...request, url: "https://example.com", queryParams: [], headers: [], auth: { kind: "none" }, body: { kind: "empty" },
  };
  add(draft, enabled);
  return draft;
}

function optionalEntryEnvironment() {
  return environment([
    variable("known", "known", "resolved"),
    variable("first", "first", "{{second}}"),
    variable("second", "second", "{{first}}"),
  ]);
}

function nestedVariables(depth: number, duplicate: boolean, leaf = "x") {
  const variables = [variable("v0", "v0", leaf)];
  for (let index = 1; index <= depth; index++) {
    variables.push(variable(`v${index}`, `v${index}`, `{{v${index - 1}}}`.repeat(duplicate ? 2 : 1)));
  }
  return environment(variables);
}

function resolveBodyTemplate(content: string, variables: Environment | undefined) {
  return resolveRequestVariables({
    ...request, url: "https://example.com", headers: [], queryParams: [], auth: { kind: "none" }, body: { kind: "text", content },
  }, variables);
}

describe("environment variable resolution", () => {
  it("resolves URL, headers, body, and nested values", () => {
    const result = resolveRequestVariables(
      request,
      environment([
        variable("1", "host", "api.example.com"),
        variable("2", "baseUrl", "https://{{host}}"),
        variable("3", "userId", "42"),
        variable("4", "headerName", "Token"),
        variable("5", "token", "local-value"),
      ]),
    );

    expect(result.request.url).toBe("https://api.example.com/users/42");
    expect(result.request.headers[0]).toMatchObject({
      name: "X-Token",
      value: "local-value",
    });
    expect(result.request.queryParams[0]?.value).toBe("42");
    expect(result.request.auth).toEqual({ kind: "bearer", token: "local-value" });
    expect(result.request.body).toMatchObject({ content: '{"id":"42"}' });
    expect(result.unresolved).toEqual([]);
  });

  it("reports missing and disabled variables without deleting their tokens", () => {
    const result = resolveRequestVariables(
      request,
      environment([variable("1", "baseUrl", "https://example.com", false)]),
    );

    expect(result.request.url).toContain("{{baseUrl}}");
    expect(result.unresolved).toEqual([
      "baseUrl",
      "headerName",
      "token",
      "userId",
    ]);
  });

  it("detects cycles", () => {
    const result = resolveRequestVariables(
      {
        ...request,
        url: "{{a}}",
        queryParams: [],
        headers: [],
        auth: { kind: "none" },
        body: { kind: "empty" },
      },
      environment([variable("1", "a", "{{b}}"), variable("2", "b", "{{a}}")]),
    );

    expect(result.cycles).toEqual(["a → b → a"]);
    expect(result.errors).toEqual([]);
  });

  it.each(optionalEntryKinds)("preserves disabled $kind templates without diagnostics", ({ add }) => {
    const draft = requestWithOptionalEntry(add, false);
    const result = resolveRequestVariables(draft, optionalEntryEnvironment());

    expect(result.request).toEqual(draft);
    expect(result.unresolved).toEqual([]);
    expect(result.cycles).toEqual([]);
    expect(result.errors).toEqual([]);
  });

  it.each(optionalEntryKinds)("resolves enabled $kind and retains missing/cycle diagnostics", ({ add }) => {
    const draft = requestWithOptionalEntry(add, true);
    const before = structuredClone(draft);
    const result = resolveRequestVariables(draft, optionalEntryEnvironment());

    expect(result.unresolved).toEqual(["missing"]);
    expect(result.cycles).toEqual(["first → second → first"]);
    expect(JSON.stringify(result.request)).not.toContain("{{known}}");
    expect(JSON.stringify(result.request)).toContain("{{missing}}");
    expect(JSON.stringify(result.request)).toContain("{{first}}");
    expect(draft).toEqual(before);
  });

  it("does not suppress enabled diagnostics when a disabled entry has invalid variables", () => {
    const draft = requestWithOptionalEntry(optionalEntryKinds[0]!.add, false);
    draft.queryParams.push({ name: "active", value: "{{activeMissing}}", enabled: true });
    const result = resolveRequestVariables(draft, optionalEntryEnvironment());

    expect(result.request.headers).toEqual(draft.headers);
    expect(result.unresolved).toEqual(["activeMissing"]);
    expect(result.cycles).toEqual([]);
  });

  it("allows depth 64 and rejects deeper acyclic variables with a static diagnostic", () => {
    const allowed = resolveBodyTemplate("{{v63}}", nestedVariables(63, false));
    expect(allowed.errors).toEqual([]);
    expect(allowed.request.body).toMatchObject({ content: "x" });
    const rejected = resolveBodyTemplate("{{v64}}", nestedVariables(64, false));
    expect(rejected.errors).toEqual(["Variable expansion exceeded the nesting depth limit (64)."]);
    expect(rejected.request.body).toMatchObject({ content: "{{v64}}" });
    expect(rejected.cycles).toEqual([]);
    expect(rejected.unresolved).toEqual([]);
  });

  it("short-circuits exponential expansion at the substitution work budget", () => {
    const result = resolveBodyTemplate("{{v40}}", nestedVariables(40, true));
    expect(result.errors).toEqual(["Variable expansion exceeded the substitution work limit (100000)."]);
    expect(result.request.body).toMatchObject({ content: "{{v40}}" });
    expect(result.cycles).toEqual([]);
  });

  it("bounds cumulative UTF-8 expansion bytes without constructing the exponential result", () => {
    const result = resolveBodyTemplate("{{v12}}", nestedVariables(12, true, "🔑".repeat(1024)));
    expect(result.errors).toEqual(["Variable expansion exceeded the cumulative byte limit (16 MiB)."]);
    expect(result.request.body).toMatchObject({ content: "{{v12}}" });
    expect(result.cycles).toEqual([]);
    const oversized = resolveBodyTemplate("{{v0}}", nestedVariables(0, false, "x".repeat(16 * 1024 * 1024 + 1)));
    expect(oversized.errors).toEqual(result.errors);
    expect(oversized.request.body).toMatchObject({ content: "{{v0}}" });
  });

  it("shares the byte budget across request fields", () => {
    const result = resolveRequestVariables({
      ...request, url: "https://example.com", queryParams: [], auth: { kind: "none" },
      headers: ["First", "Second"].map((name) => ({ name, value: "{{large}}", enabled: true })),
      body: { kind: "text", content: "{{small}}" },
    }, environment([variable("large", "large", "x".repeat(8 * 1024 * 1024)), variable("small", "small", "x")]));
    expect(result.errors).toEqual(["Variable expansion exceeded the cumulative byte limit (16 MiB)."]);
    expect(result.request.headers.map((header) => header.value.length)).toEqual([8 * 1024 * 1024, 8 * 1024 * 1024]);
    expect(result.request.body).toMatchObject({ content: "{{small}}" });
  });

  it("shares the 100000 substitution work budget across fields, including empty replacements", () => {
    const result = resolveRequestVariables({
      ...request, url: "https://example.com", queryParams: [], auth: { kind: "none" },
      headers: ["First", "Second"].map((name) => ({ name, value: "{{empty}}".repeat(50_000), enabled: true })),
      body: { kind: "text", content: "{{empty}}" },
    }, environment([variable("empty", "empty", "")]));
    expect(result.errors).toEqual(["Variable expansion exceeded the substitution work limit (100000)."]);
    expect(result.request.headers.map((header) => header.value)).toEqual(["", ""]);
    expect(result.request.body).toMatchObject({ content: "{{empty}}" });
  });

  it("leaves raw no-token bodies larger than the expansion budget intact", () => {
    const content = "x".repeat(16 * 1024 * 1024 + 1);
    const result = resolveBodyTemplate(content, undefined);
    expect(result.errors).toEqual([]);
    expect(result.request.body).toMatchObject({ content });
  });

  it("resolves multipart metadata and local file references", () => {
    const result = resolveRequestVariables(
      {
        ...request,
        body: {
          kind: "multipart",
          parts: [
            {
              name: "{{field}}",
              enabled: true,
              value: {
                kind: "file",
                path: "{{folder}}/report.pdf",
                fileName: "{{name}}.pdf",
                contentType: "application/{{subtype}}",
              },
            },
          ],
        },
      },
      environment([
        variable("1", "baseUrl", "https://example.com"),
        variable("2", "userId", "42"),
        variable("3", "headerName", "Token"),
        variable("4", "token", "secret"),
        variable("5", "field", "document"),
        variable("6", "folder", "C:/files"),
        variable("7", "name", "annual"),
        variable("8", "subtype", "pdf"),
      ]),
    );

    expect(result.request.body).toEqual({
      kind: "multipart",
      parts: [
        {
          name: "document",
          enabled: true,
          value: {
            kind: "file",
            path: "C:/files/report.pdf",
            fileName: "annual.pdf",
            contentType: "application/pdf",
          },
        },
      ],
    });
    expect(result.unresolved).toEqual([]);
  });
});
