import { describe, expect, it } from "vitest";
import { DEFAULT_NETWORK_SETTINGS, normalizeNetworkSettings } from "./network-settings";

describe("network settings", () => {
  it("normalizes workspaces created before network settings existed", () => {
    expect(normalizeNetworkSettings()).toEqual(DEFAULT_NETWORK_SETTINGS);
  });

  it("preserves defaults while applying persisted values", () => {
    expect(
      normalizeNetworkSettings({
        proxyMode: "custom",
        proxyUrl: "http://localhost:8080",
        cookieJarEnabled: false,
      }),
    ).toMatchObject({
      proxyMode: "custom",
      proxyUrl: "http://localhost:8080",
      cookieJarEnabled: false,
      acceptInvalidCertificates: false,
    });
  });
});
