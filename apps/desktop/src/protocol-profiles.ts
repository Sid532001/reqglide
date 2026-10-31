import type { ProtocolConfiguration, ProtocolProfile } from "./api";

export type ProtocolKind = ProtocolConfiguration["kind"];

export const GRAPHQL_INTROSPECTION_QUERY = `query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types {
      kind
      name
      description
      fields(includeDeprecated: true) {
        name
        description
        args { name description type { kind name ofType { kind name } } defaultValue }
        type { kind name ofType { kind name } }
        isDeprecated
        deprecationReason
      }
    }
  }
}`;

export function protocolLabel(kind: ProtocolKind): string {
  switch (kind) {
    case "graphQl":
      return "GraphQL";
    case "webSocket":
      return "WebSocket";
    case "serverSentEvents":
      return "SSE";
    case "grpc":
      return "gRPC";
  }
}

export function createProtocolProfile(
  kind: ProtocolKind,
  id: string = crypto.randomUUID(),
): ProtocolProfile {
  const shared = {
    id,
    headers: [],
    auth: { kind: "inherit" } as const,
  };
  switch (kind) {
    case "graphQl":
      return {
        ...shared,
        name: "GraphQL API",
        url: "https://example.com/graphql",
        configuration: {
          kind,
          query: "query { __typename }",
          variables: "{}",
          operationName: "",
        },
      };
    case "webSocket":
      return {
        ...shared,
        name: "WebSocket",
        url: "wss://echo.websocket.org",
        configuration: { kind, subprotocols: [] },
      };
    case "serverSentEvents":
      return {
        ...shared,
        name: "Event stream",
        url: "https://example.com/events",
        configuration: { kind, lastEventId: "" },
      };
    case "grpc":
      return {
        ...shared,
        name: "gRPC API",
        url: "https://localhost:50051",
        configuration: {
          kind,
          service: "package.Service",
          method: "Method",
          message: "{}",
          descriptorSetPath: "",
          useReflection: true,
        },
      };
  }
}

export function validateProtocolProfiles(profiles: ProtocolProfile[]): string | null {
  const ids = new Set<string>();
  for (const profile of profiles) {
    if (ids.has(profile.id)) {
      return "Protocol profiles contain a duplicate ID.";
    }
    ids.add(profile.id);
    if (!profile.name.trim()) {
      return "Every protocol profile needs a name.";
    }
    if (!profile.url.trim()) {
      return `${profile.name} needs an endpoint URL.`;
    }
    if (profile.configuration.kind === "graphQl") {
      if (!profile.configuration.query.trim()) {
        return `${profile.name} needs a GraphQL query.`;
      }
      try {
        const variables = JSON.parse(profile.configuration.variables);
        if (variables !== null && (Array.isArray(variables) || typeof variables !== "object")) {
          return `${profile.name} variables must be a JSON object or null.`;
        }
      } catch {
        return `${profile.name} variables are not valid JSON.`;
      }
    }
    if (profile.configuration.kind === "grpc") {
      if (!profile.configuration.service.trim() || !profile.configuration.method.trim()) {
        return `${profile.name} needs a gRPC service and method.`;
      }
      if (!profile.configuration.useReflection && !profile.configuration.descriptorSetPath.trim()) {
        return `${profile.name} needs a descriptor-set file when reflection is disabled.`;
      }
      try {
        JSON.parse(profile.configuration.message);
      } catch {
        return `${profile.name} message is not valid JSON.`;
      }
    }
  }
  return null;
}

export function parseSubprotocols(value: string): string[] {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}
