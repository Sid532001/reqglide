import type { NetworkSettings } from "./api";

export const DEFAULT_NETWORK_SETTINGS: Readonly<NetworkSettings> = Object.freeze({
  cookieJarEnabled: true,
  proxyMode: "system",
  proxyUrl: "",
  proxyUsername: "",
  proxyPassword: "",
  noProxy: "",
  caCertificatePath: "",
  clientCertificatePath: "",
  acceptInvalidCertificates: false,
});

export function normalizeNetworkSettings(
  settings?: Partial<NetworkSettings>,
): NetworkSettings {
  return {
    ...DEFAULT_NETWORK_SETTINGS,
    ...settings,
  };
}
