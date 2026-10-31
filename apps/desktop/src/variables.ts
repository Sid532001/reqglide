import type { Environment, RequestDefinition } from "./api";

export interface VariableResolution {
  request: RequestDefinition;
  unresolved: string[];
  cycles: string[];
  errors: string[];
}

const MAX_VARIABLE_DEPTH = 64;
const MAX_VARIABLE_SUBSTITUTIONS = 100_000;
const MAX_VARIABLE_EXPANSION_BYTES = 16 * 1024 * 1024;
const VARIABLE_DEPTH_ERROR = "Variable expansion exceeded the nesting depth limit (64).";
const VARIABLE_WORK_ERROR = "Variable expansion exceeded the substitution work limit (100000).";
const VARIABLE_BYTES_ERROR = "Variable expansion exceeded the cumulative byte limit (16 MiB).";

export function resolveRequestVariables(
  request: RequestDefinition,
  environment: Environment | undefined,
): VariableResolution {
  const values = new Map(
    (environment?.variables ?? [])
      .filter((variable) => variable.enabled && variable.key.trim())
      .map((variable) => [variable.key.trim(), variable.value]),
  );
  const unresolved = new Set<string>();
  const cycles = new Set<string>();
  const errors: string[] = [];
  let substitutions = 0;
  let expansionBytes = 0;

  function fail(message: string): false {
    if (!errors.length) errors.push(message);
    return false;
  }

  function consumeBytes(value: string, start: number, end: number): boolean {
    const remaining = MAX_VARIABLE_EXPANSION_BYTES - expansionBytes;
    // Reject large strings before slicing or encoding them into another allocation.
    if (end - start > remaining) return fail(VARIABLE_BYTES_ERROR);
    let bytes = 0;
    for (let index = start; index < end; index++) {
      const code = value.charCodeAt(index);
      if (code < 0x80) bytes++;
      else if (code < 0x800) bytes += 2;
      else if (code >= 0xd800 && code <= 0xdbff && index + 1 < end
        && value.charCodeAt(index + 1) >= 0xdc00 && value.charCodeAt(index + 1) <= 0xdfff) {
        bytes += 4;
        index++;
      } else bytes += 3;
      if (bytes > remaining) return fail(VARIABLE_BYTES_ERROR);
    }
    expansionBytes += bytes;
    return true;
  }

  function resolveText(input: string, stack: string[] = []): string {
    if (errors.length || !input.includes("{{")) return input;
    const pattern = /{{\s*([^{}]+?)\s*}}/g;
    const output: string[] = [];
    let cursor = 0;
    function append(value: string, start: number, end: number, expanded: boolean): boolean {
      if (expanded && !consumeBytes(value, start, end)) return false;
      output.push(value.slice(start, end));
      return true;
    }
    let match: RegExpExecArray | null;
    while ((match = pattern.exec(input)) !== null) {
      if (!append(input, cursor, match.index, stack.length > 0)) return input;
      if (substitutions >= MAX_VARIABLE_SUBSTITUTIONS) {
        fail(VARIABLE_WORK_ERROR);
        return input;
      }
      substitutions++;
      const token = match[0];
      const key = match[1]!.trim();
      const value = values.get(key);
      let replacement = token;
      if (value === undefined) {
        unresolved.add(key);
      } else if (stack.includes(key)) {
        cycles.add([...stack, key].join(" → "));
      } else {
        if (stack.length >= MAX_VARIABLE_DEPTH) {
          fail(VARIABLE_DEPTH_ERROR);
          return input;
        }
        stack.push(key);
        replacement = resolveText(value, stack);
        stack.pop();
        if (errors.length) return input;
      }
      if (!append(replacement, 0, replacement.length, true)) return input;
      cursor = pattern.lastIndex;
    }
    // Raw no-token input, including large bodies, remains governed by the HTTP limit.
    if (cursor === 0) return input;
    if (!append(input, cursor, input.length, stack.length > 0)) return input;
    return output.join("");
  }

  return {
    request: {
      ...request,
      url: resolveText(request.url),
      queryParams: request.queryParams.map((parameter) => parameter.enabled ? ({
        ...parameter,
        name: resolveText(parameter.name),
        value: resolveText(parameter.value),
      }) : parameter),
      headers: request.headers.map((header) => header.enabled ? ({
        ...header,
        name: resolveText(header.name),
        value: resolveText(header.value),
      }) : header),
      auth: resolveAuth(request.auth, resolveText),
      body: resolveBody(request.body, resolveText),
    },
    unresolved: [...unresolved].sort(),
    cycles: [...cycles].sort(),
    errors,
  };
}

function resolveAuth(
  auth: RequestDefinition["auth"],
  resolveText: (value: string) => string,
): RequestDefinition["auth"] {
  switch (auth.kind) {
    case "none":
    case "inherit":
      return auth;
    case "basic":
      return {
        ...auth,
        username: resolveText(auth.username),
        password: resolveText(auth.password),
      };
    case "bearer":
      return { ...auth, token: resolveText(auth.token) };
    case "apiKey":
      return {
        ...auth,
        key: resolveText(auth.key),
        value: resolveText(auth.value),
      };
    case "oauth2ClientCredentials":
      return {
        ...auth,
        issuerUrl: resolveText(auth.issuerUrl),
        tokenUrl: resolveText(auth.tokenUrl),
        clientId: resolveText(auth.clientId),
        clientSecret: resolveText(auth.clientSecret),
        scope: resolveText(auth.scope),
        audience: resolveText(auth.audience),
        extraParameters: auth.extraParameters.map((parameter) => parameter.enabled ? ({
          ...parameter,
          name: resolveText(parameter.name),
          value: resolveText(parameter.value),
        }) : parameter),
      };
  }
}

function resolveBody(
  body: RequestDefinition["body"],
  resolveText: (value: string) => string,
): RequestDefinition["body"] {
  switch (body.kind) {
    case "empty":
      return body;
    case "text":
      return {
        ...body,
        content: resolveText(body.content),
        contentType: body.contentType ? resolveText(body.contentType) : undefined,
      };
    case "formUrlEncoded":
      return {
        ...body,
        fields: body.fields.map((field) => field.enabled ? ({
          ...field,
          name: resolveText(field.name),
          value: resolveText(field.value),
        }) : field),
      };
    case "multipart":
      return {
        ...body,
        parts: body.parts.map((part) => part.enabled ? ({
          ...part,
          name: resolveText(part.name),
          value:
            part.value.kind === "text"
              ? {
                  ...part.value,
                  content: resolveText(part.value.content),
                  contentType: part.value.contentType
                    ? resolveText(part.value.contentType)
                    : undefined,
                }
              : {
                  ...part.value,
                  path: resolveText(part.value.path),
                  fileName: part.value.fileName
                    ? resolveText(part.value.fileName)
                    : undefined,
                  contentType: part.value.contentType
                    ? resolveText(part.value.contentType)
                    : undefined,
                },
        }) : part),
      };
    case "binary":
      return {
        ...body,
        path: resolveText(body.path),
        contentType: body.contentType ? resolveText(body.contentType) : undefined,
      };
  }
}
