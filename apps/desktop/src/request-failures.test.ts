import { describe, expect, it } from "vitest";
import { describeNetworkFailure } from "./request-failures";

describe("network failure guidance", () => {
  it("makes legacy and staged timeouts actionable without showing secrets", () => {
    for (const error of ["request timed out", "connection timed out during DNS, proxy, TCP, or TLS setup", "the server responded, but reading the response body timed out"]) {
      const result = describeNetworkFailure(error, "https://user:password@api.example.com:8443/private?token=credential", 30000, "system");
      expect(result).toMatchObject({ destination: "api.example.com:8443", proxy: "System proxy", timeout: "30,000 ms" });
      expect(JSON.stringify(result)).not.toMatch(/password|credential|private|user:/);
    }
  });

  it("does not mislabel authentication or variable errors as network failures", () => {
    expect(describeNetworkFailure("HTTP 401 Unauthorized", "https://example.com", 1000, "disabled")).toBeNull();
    expect(describeNetworkFailure("Unresolved variables: {{token}}", "{{host}}", 1000, "system")).toBeNull();
    expect(describeNetworkFailure("Windows automatic proxy resolution failed (code 12166)", "https://example.com", 1000, "system")).not.toBeNull();
    expect(describeNetworkFailure("request timed out", "{{host}}/private?token=secret", 1000, "custom")).toMatchObject({ destination: "Current request", proxy: "Custom proxy" });
  });
});
