import type { Environment, HeaderEntry, RequestDefinition } from "./api";

export interface GeneratedHeaderPreview {
  name: string;
  value: string;
  description: string;
  overridesManual: boolean;
}

export type VariableReferenceKind =
  | "shared"
  | "local"
  | "secret"
  | "missing"
  | "disabled";

export interface VariableReferenceStatus {
  key: string;
  kind: VariableReferenceKind;
  label: string;
}

const VARIABLE_PATTERN = /{{\s*([^{}]+?)\s*}}/g;

export function buildGeneratedHeaderPreviews(
  request: RequestDefinition,
  cookieJarEnabled = false,
): GeneratedHeaderPreview[] {
  const generated: GeneratedHeaderPreview[] = [];
  const hasHeader = (name: string) =>
    request.headers.some(
      (header) => header.enabled && header.name.trim().toLowerCase() === name.toLowerCase(),
    );

  if (!hasHeader("host")) {
    generated.push({
      name: "Host",
      value: requestHost(request.url) ?? "<calculated from URL when sent>",
      description: "Calculated from the request URL by the HTTP runtime.",
      overridesManual: false,
    });
  }

  if (!hasHeader("accept-encoding")) {
    generated.push({
      name: "Accept-Encoding",
      value: "<managed by HTTP runtime>",
      description: "Negotiated automatically for supported response compression.",
      overridesManual: false,
    });
  }

  if (request.body.kind !== "empty") {
    generated.push({
      name: "Content-Length",
      value: "<calculated when request is sent>",
      description: "Calculated from the final encoded or streamed request body.",
      overridesManual: true,
    });
  }

  const manualContentType = hasHeader("content-type");
  const generatedContentType = contentTypeForBody(request);
  if (generatedContentType && (request.body.kind === "multipart" || !manualContentType)) {
    generated.push({
      name: "Content-Type",
      value: generatedContentType,
      description:
        request.body.kind === "multipart"
          ? "Generated with the multipart boundary when the request is sent."
          : "Generated from the selected request body type.",
      overridesManual: request.body.kind === "multipart",
    });
  }

  const authHeader = authenticationHeader(request);
  if (authHeader) {
    generated.push(authHeader);
  }

  if (cookieJarEnabled && !hasHeader("cookie")) {
    generated.push({
      name: "Cookie",
      value: "<matching workspace cookies, if any>",
      description: "Attached from this workspace's isolated cookie jar.",
      overridesManual: false,
    });
  }

  return generated;
}

export function inspectVariableReferences(
  values: string[],
  environment: Environment | undefined,
  vaultStatuses: Record<string, boolean>,
): VariableReferenceStatus[] {
  const keys = new Set<string>();
  for (const value of values) {
    for (const match of value.matchAll(VARIABLE_PATTERN)) {
      const key = match[1]?.trim();
      if (key) {
        keys.add(key);
      }
    }
  }

  return [...keys].map((key) => {
    const variable = environment?.variables.find((candidate) => candidate.key.trim() === key);
    if (!variable) {
      return { key, kind: "missing", label: `${key}: missing` };
    }
    if (!variable.enabled) {
      return { key, kind: "disabled", label: `${key}: disabled` };
    }
    if (variable.secret) {
      const stored = vaultStatuses[variable.id];
      return {
        key,
        kind: stored === false ? "missing" : "secret",
        label:
          stored === false
            ? `${key}: secret not set`
            : stored === true
              ? `${key}: secret stored`
              : `${key}: secret`,
      };
    }
    if (variable.local) {
      const stored = vaultStatuses[variable.id];
      return {
        key,
        kind: stored === false ? "missing" : "local",
        label:
          stored === false
            ? `${key}: local value not set`
            : stored === true
              ? `${key}: local value stored`
              : `${key}: local`,
      };
    }
    return { key, kind: "shared", label: `${key}: shared` };
  });
}

export function generatedConflict(
  header: HeaderEntry,
  generated: GeneratedHeaderPreview[],
): GeneratedHeaderPreview | undefined {
  const name = header.name.trim().toLowerCase();
  if (!header.enabled || !name) {
    return undefined;
  }
  return generated.find(
    (candidate) => candidate.overridesManual && candidate.name.toLowerCase() === name,
  );
}

function requestHost(value: string): string | null {
  try {
    return new URL(value).host || null;
  } catch {
    return null;
  }
}

function contentTypeForBody(request: RequestDefinition): string | null {
  switch (request.body.kind) {
    case "empty":
      return null;
    case "text":
      return request.body.contentType?.trim() || null;
    case "formUrlEncoded":
      return "application/x-www-form-urlencoded";
    case "multipart":
      return "multipart/form-data; boundary=<calculated when sent>";
    case "binary":
      return request.body.contentType?.trim() || null;
  }
}

function authenticationHeader(
  request: RequestDefinition,
): GeneratedHeaderPreview | null {
  switch (request.auth.kind) {
    case "none":
    case "inherit":
      return null;
    case "basic":
      return generatedCredentialHeader("Authorization", "Basic ••••••••••••");
    case "bearer":
    case "oauth2ClientCredentials":
      return generatedCredentialHeader("Authorization", "Bearer ••••••••••••");
    case "apiKey":
      return request.auth.placement === "header"
        ? generatedCredentialHeader(request.auth.key || "<API key header>", "••••••••••••")
        : null;
  }
}

function generatedCredentialHeader(name: string, value: string): GeneratedHeaderPreview {
  return {
    name,
    value,
    description: "Generated from the request's Authorization configuration.",
    overridesManual: true,
  };
}
