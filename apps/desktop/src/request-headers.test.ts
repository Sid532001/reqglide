import { describe, expect, it } from "vitest";
import type { Environment, RequestDefinition } from "./api";
import {
  buildGeneratedHeaderPreviews,
  generatedConflict,
  inspectVariableReferences,
} from "./request-headers";

const request: RequestDefinition = {
  method: "POST",
  url: "https://auth.example.test/oauth2/v1/token",
  queryParams: [],
  headers: [
    {
      name: "Authorization",
      value: "{{basic_auth_stage}}",
      enabled: true,
    },
  ],
  auth: { kind: "none" },
  body: {
    kind: "formUrlEncoded",
    fields: [{ name: "grant_type", value: "client_credentials", enabled: true }],
  },
  options: {
    timeoutMs: 30_000,
    maxRequestBytes: 1024,
    maxResponseBytes: 1024,
    followRedirects: true,
  },
};

describe("request header previews", () => {
  it("shows runtime headers without exposing environment credentials", () => {
    const generated = buildGeneratedHeaderPreviews(request, true);

    expect(generated).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ name: "Host", value: "auth.example.test" }),
        expect.objectContaining({
          name: "Content-Type",
          value: "application/x-www-form-urlencoded",
        }),
        expect.objectContaining({ name: "Content-Length" }),
        expect.objectContaining({ name: "Cookie" }),
      ]),
    );
    expect(JSON.stringify(generated)).not.toContain("client-secret");
  });

  it("reports secret, shared, disabled, and missing references", () => {
    const environment: Environment = {
      id: "stage",
      name: "Stage",
      variables: [
        {
          id: "secret-id",
          key: "basic_auth_stage",
          value: "",
          enabled: true,
          secret: true,
        },
        { id: "host-id", key: "host", value: "example.com", enabled: true },
        { id: "off-id", key: "off", value: "x", enabled: false },
      ],
    };

    expect(
      inspectVariableReferences(
        ["{{basic_auth_stage}} {{host}} {{off}} {{unknown}}"],
        environment,
        { "secret-id": true },
      ),
    ).toEqual([
      { key: "basic_auth_stage", kind: "secret", label: "basic_auth_stage: secret stored" },
      { key: "host", kind: "shared", label: "host: shared" },
      { key: "off", kind: "disabled", label: "off: disabled" },
      { key: "unknown", kind: "missing", label: "unknown: missing" },
    ]);
  });

  it("warns when configured authentication overrides a manual header", () => {
    const authenticated = {
      ...request,
      auth: { kind: "bearer", token: "{{access_token}}" } as const,
    };
    const generated = buildGeneratedHeaderPreviews(authenticated);

    expect(generatedConflict(request.headers[0]!, generated)?.name).toBe("Authorization");
  });
});
