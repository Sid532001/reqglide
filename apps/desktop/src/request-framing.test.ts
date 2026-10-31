import { describe, expect, it } from "vitest";
import type { HeaderEntry, RequestDefinition } from "./api";
import { alignContentTypeHeaders, hasJsonFormContentTypeMismatch } from "./request-framing";

const request = (headers: HeaderEntry[] = [], content = '{"filters":[]}', contentType = "application/x-www-form-urlencoded"): RequestDefinition => ({
  method: "POST", url: "https://api.example.test/search", queryParams: [], headers,
  auth: { kind: "bearer", token: "{{access_token}}" },
  body: { kind: "text", content, contentType },
  options: { timeoutMs: 30_000, maxRequestBytes: 100_000, maxResponseBytes: 100_000, followRedirects: true },
});

describe("request body framing", () => {
  it("detects JSON sent with a form content type, including a manual override", () => {
    expect(hasJsonFormContentTypeMismatch(request())).toBe(true);
    expect(hasJsonFormContentTypeMismatch(request([
      { name: "content-type", value: "Application/X-Www-Form-Urlencoded; charset=utf-8", enabled: true },
    ], '[{"field":"id"}]', "application/json"))).toBe(true);
  });

  it("uses enabled manual headers and leaves form data and intentional text alone", () => {
    expect(hasJsonFormContentTypeMismatch(request([], "grant_type=client_credentials"))).toBe(false);
    expect(hasJsonFormContentTypeMismatch(request([], '{"filters":[]}', "text/plain"))).toBe(false);
    expect(hasJsonFormContentTypeMismatch(request([
      { name: "Content-Type", value: "application/x-www-form-urlencoded", enabled: false },
    ], '{"filters":[]}', "application/json"))).toBe(false);
    expect(hasJsonFormContentTypeMismatch(request([
      { name: "Content-Type", value: "application/json", enabled: true },
    ]))).toBe(false);
  });

  it("aligns a manual override without changing auth or disabled headers", () => {
    const authorization = { name: "Authorization", value: "Bearer {{access_token}}", enabled: true };
    const disabled = { name: "Content-Type", value: "text/plain", enabled: false };
    const headers = [
      authorization,
      { name: "Content-Type", value: "application/x-www-form-urlencoded", enabled: true },
      disabled,
      { name: " content-type ", value: "application/xml", enabled: true },
    ];
    const updated = alignContentTypeHeaders(headers, "application/json");
    expect(updated).toEqual([
      authorization, { name: "Content-Type", value: "application/json", enabled: true }, disabled,
    ]);
    expect(headers[1]?.value).toBe("application/x-www-form-urlencoded");
    expect(hasJsonFormContentTypeMismatch(request(updated, '{"filters":[]}', "application/json"))).toBe(false);
  });
});
