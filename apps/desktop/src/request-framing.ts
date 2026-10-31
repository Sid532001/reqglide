import type { HeaderEntry, RequestDefinition } from "./api";

export const DEFAULT_RAW_CONTENT_TYPE = "application/json";

export function hasJsonFormContentTypeMismatch(request: RequestDefinition): boolean {
  if (request.body.kind !== "text") return false;
  const contentTypes = request.headers
    .filter((header) => header.enabled && header.name.trim().toLowerCase() === "content-type")
    .map((header) => header.value);
  if (contentTypes.length === 0) contentTypes.push(request.body.contentType ?? "");
  if (!contentTypes.some((value) =>
    value.split(";", 1)[0]?.trim().toLowerCase() === "application/x-www-form-urlencoded",
  )) return false;
  try {
    const body: unknown = JSON.parse(request.body.content);
    return body !== null && typeof body === "object";
  } catch {
    return false;
  }
}

// An explicit format change also updates the manual header that would override it.
export function alignContentTypeHeaders(headers: HeaderEntry[], contentType: string): HeaderEntry[] {
  let updated = false;
  return headers.flatMap((header) => {
    if (!header.enabled || header.name.trim().toLowerCase() !== "content-type") return [header];
    if (updated) return [];
    updated = true;
    return [{ ...header, value: contentType }];
  });
}
