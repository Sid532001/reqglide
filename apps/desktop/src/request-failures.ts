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
  // Recovered tabs and history may contain the old native troubleshooting copy.
  // Normalize only that known phrase; keep useful transport-route details.
  const message = error.replace(
    /; check the VPN and proxy settings used by Postman/gi,
    "; check destination reachability, VPN, and proxy settings",
  ).replace(/system settings \(no PAC override selected\)/gi, "operating-system proxy settings");
  let title = "Network request failed";
  let help = "Check the destination, VPN, DNS, and proxy settings. A connection failure is different from an HTTP authentication response.";
  if (/automatic proxy|proxy configuration/i.test(error)) {
    title = "Proxy configuration failed";
    help = "Reqglide could not resolve the configured proxy route. Check Network & TLS settings before retrying the request.";
  } else if (/reading the response body timed out|response stream ended/i.test(error)) {
    title = /timed out/i.test(error) ? "Response body timed out" : "Response interrupted";
    help = "The server responded, but the response body could not be read completely. Check the connection and request timeout; this error alone does not establish a token-validation failure.";
  } else if (/before response headers|request timed out/i.test(error)) {
    title = "Response timed out";
    help = "No HTTP response was received before the deadline. The server may have received the request; check its availability, the proxy route, and the request timeout. Token validation cannot be assessed from this timeout.";
  } else if (/connection timed out|could not connect|connection failed/i.test(error)) {
    title = /timed out/i.test(error) ? "Connection timed out" : "Connection failed";
    help = "No HTTP response was received. Check destination availability, DNS, VPN, and proxy settings. The connection failed before an HTTP 401/403 response could establish whether the token was accepted.";
  }
  return {
    title,
    message,
    destination,
    timeout: `${timeoutMs.toLocaleString("en-US")} ms`,
    proxy: proxyMode === "disabled" ? "No proxy" : proxyMode === "custom" ? "Custom proxy" : "System proxy",
    help,
  };
}
