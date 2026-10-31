import { describe, expect, it } from "vitest";
import {
  createProtocolProfile,
  parseSubprotocols,
  protocolLabel,
  validateProtocolProfiles,
} from "./protocol-profiles";

describe("protocol profiles", () => {
  it("creates each supported local protocol", () => {
    expect(createProtocolProfile("graphQl", "graphql").configuration.kind).toBe("graphQl");
    expect(createProtocolProfile("webSocket", "ws").configuration.kind).toBe("webSocket");
    expect(createProtocolProfile("serverSentEvents", "sse").configuration.kind).toBe(
      "serverSentEvents",
    );
    expect(createProtocolProfile("grpc", "grpc").configuration.kind).toBe("grpc");
    expect(protocolLabel("grpc")).toBe("gRPC");
  });

  it("validates JSON and descriptor requirements", () => {
    const graphql = createProtocolProfile("graphQl", "graphql");
    if (graphql.configuration.kind !== "graphQl") {
      throw new Error("expected GraphQL profile");
    }
    graphql.configuration = { ...graphql.configuration, variables: "[]" };
    expect(validateProtocolProfiles([graphql])).toContain("JSON object");

    const grpc = createProtocolProfile("grpc", "grpc");
    if (grpc.configuration.kind !== "grpc") {
      throw new Error("expected gRPC profile");
    }
    grpc.configuration = {
      ...grpc.configuration,
      useReflection: false,
      descriptorSetPath: "",
    };
    expect(validateProtocolProfiles([grpc])).toContain("descriptor-set");
  });

  it("normalizes comma-separated WebSocket subprotocols", () => {
    expect(parseSubprotocols(" graphql-ws, , chat ")).toEqual(["graphql-ws", "chat"]);
  });
});
