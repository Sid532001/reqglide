import type { KeyValueEntry } from "./api";

export interface SplitRequestUrl {
  url: string;
  queryParams: KeyValueEntry[];
}

export function splitRequestUrl(value: string): SplitRequestUrl {
  const hashIndex = value.indexOf("#");
  const beforeHash = hashIndex >= 0 ? value.slice(0, hashIndex) : value;
  const fragment = hashIndex >= 0 ? value.slice(hashIndex) : "";
  const queryIndex = beforeHash.indexOf("?");
  if (queryIndex < 0) {
    return { url: value, queryParams: [] };
  }

  const query = beforeHash.slice(queryIndex + 1);
  return {
    url: `${beforeHash.slice(0, queryIndex)}${fragment}`,
    queryParams: [...new URLSearchParams(query)].map(([name, parameterValue]) => ({
      name,
      value: parameterValue,
      enabled: true,
    })),
  };
}

export function composeRequestUrl(url: string, queryParams: KeyValueEntry[]): string {
  const hashIndex = url.indexOf("#");
  const base = hashIndex >= 0 ? url.slice(0, hashIndex) : url;
  const fragment = hashIndex >= 0 ? url.slice(hashIndex) : "";
  const query = queryParams
    .filter((parameter) => parameter.enabled)
    .filter((parameter) => parameter.name || parameter.value)
    .map(
      (parameter) =>
        `${encodeQueryComponent(parameter.name)}=${encodeQueryComponent(parameter.value)}`,
    )
    .join("&");
  return `${base}${query ? `?${query}` : ""}${fragment}`;
}

function encodeQueryComponent(value: string): string {
  return encodeURIComponent(value)
    .replaceAll("%7B", "{")
    .replaceAll("%7D", "}");
}
