import type { ProxyMode } from "./api";

export function describeNetworkFailure(error: string, url: string, timeoutMs: number, proxyMode: ProxyMode) {
  if (!/timed out|could not connect|connection failed|response stream ended|automatic proxy|proxy configuration/i.test(error)) {
    return null;
  }
  let destination = "Current request";
  try {
    const parsed = new URL(url);
    if (/^https?:$/.test(parsed.protocol) && !parsed.host.includes("{{")) {
      destination = parsed.host;
    }
  } catch { /* Variable URLs are resolved inside the native engine. */ }
  return {
    destination,
    timeout: `${timeoutMs.toLocaleString("en-US")} ms`,
    proxy: proxyMode === "disabled" ? "No proxy" : proxyMode === "custom" ? "Custom proxy" : "System proxy",
    help: "A timeout is different from an HTTP 401/403 authentication response. Match the proxy and VPN route used by Postman before changing the token.",
  };
}
