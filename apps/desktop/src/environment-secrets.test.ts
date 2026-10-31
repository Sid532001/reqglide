import { describe, expect, it } from "vitest";
import type { Environment } from "./api";
import {
  buildSecretMutations,
  changeVariableStorage,
  saveExtractedVariable,
  applyScriptEnvironmentUpdates,
  hasMissingSecretValues,
  sanitizeSecretReferences,
} from "./environment-secrets";

function environment(variables: Environment["variables"]): Environment {
  return { id: "environment", name: "Development", variables };
}

function variable(id: string, secret: boolean, value = "", local = false) {
  return { id, key: id, value, enabled: true, secret, local };
}

describe("environment secret changes", () => {
  it("updates new values and deletes secrets removed from the workspace", () => {
    const original = [environment([variable("existing", true), variable("public", false, "x")])];
    const drafts = [environment([variable("existing", true), variable("new", true)])];

    expect(buildSecretMutations(original, drafts, { new: "token" })).toEqual([
      { variableId: "new", value: "token" },
    ]);

    expect(buildSecretMutations(original, [], {})).toEqual([
      { variableId: "existing", value: null },
    ]);
  });

  it("requires either a stored value or an explicit pending value", () => {
    const drafts = [environment([variable("token", true)])];

    expect(hasMissingSecretValues(drafts, {}, {})).toBe(true);
    expect(hasMissingSecretValues(drafts, {}, { token: true })).toBe(false);
    expect(hasMissingSecretValues(drafts, { token: "" }, {})).toBe(false);
  });

  it("never leaves a secret value in the serializable document", () => {
    const result = sanitizeSecretReferences([
      environment([variable("token", true, "must disappear"), variable("host", false, "localhost")]),
    ]);

    expect(result[0]?.variables).toEqual([
      variable("token", true, ""),
      variable("host", false, "localhost"),
    ]);
  });

  it("stores local-only values outside the shareable document", () => {
    const original = [environment([])];
    const drafts = [environment([variable("host", false, "", true)])];

    expect(buildSecretMutations(original, drafts, { host: "localhost" })).toEqual([
      { variableId: "host", value: "localhost" },
    ]);
    expect(sanitizeSecretReferences([
      environment([variable("host", false, "must disappear", true)]),
    ])[0]?.variables[0]?.value).toBe("");
  });

  it("retains an unreadable credential when changing Secret to Local and back", () => {
    const original = variable("token", true);
    const local = changeVariableStorage(original, "local", {})!;
    expect(local.pending).toEqual({});
    expect(local.variable).toMatchObject({ secret: false, local: true, value: "" });
    expect(buildSecretMutations([environment([original])], [environment([local.variable])], local.pending)).toEqual([]);
    const secret = changeVariableStorage(local.variable, "secret", local.pending)!;
    expect(secret.pending).toEqual({});
    expect(secret.variable).toEqual(original);
  });

  it("requires an explicit replacement before changing an unreadable secret to Shared", () => {
    const original = variable("token", true);
    expect(changeVariableStorage(original, "plain", {})).toBeNull();
    expect(changeVariableStorage(original, "plain", { token: "replacement" })).toEqual({
      variable: variable("token", false, "replacement"), pending: {},
    });
    expect(changeVariableStorage(original, "local", { token: "replacement" })?.pending)
      .toEqual({ token: "replacement" });
  });

  it("retains a known local value when converting it to Shared", () => {
    expect(changeVariableStorage(variable("host", false, "", true), "plain", { host: "localhost" }))
      .toEqual({ variable: variable("host", false, "localhost"), pending: {} });
  });

  it.each(["access_token", "token", "refresh_token", "idToken", "clientSecret", "password", "apiKey"])(
    "stores a new extracted %s in the keychain", (key) => {
      const change = saveExtractedVariable([environment([])], "environment", key, "credential", () => "new");
      expect(change.environments[0]?.variables[0]).toMatchObject({ id: "new", key, value: "", secret: true });
      expect(change.mutations).toEqual([{ variableId: "new", value: "credential" }]);
      expect(JSON.stringify(change.environments)).not.toContain("credential");
    },
  );

  it.each([[true, false], [false, true], [false, false]])(
    "respects existing secret=%s local=%s storage when extracting a response value", (secret, local) => {
      const change = saveExtractedVariable(
        [environment([variable("token", secret, "", local)])], "environment", "token", "updated", () => "unused",
      );
      const backed = secret || local;
      expect(change.environments[0]?.variables[0]).toEqual(variable("token", secret, backed ? "" : "updated", local));
      expect(change.mutations).toEqual(backed ? [{ variableId: "token", value: "updated" }] : []);
    },
  );

  it("allows extracted non-sensitive values to be shared", () => {
    const change = saveExtractedVariable([environment([])], "environment", "expires_in", "3600", () => "new");
    expect(change.environments[0]?.variables[0]).toMatchObject({ value: "3600", secret: false });
    expect(change.mutations).toEqual([]);
  });

  it("applies script updates only to the originating environment without serializing protected values", () => {
    const environments = [environment([
      variable("token", true), variable("host", false, "", true), variable("plain", false, "before"),
    ]), { id: "other", name: "Other", variables: [] }];
    const updated = applyScriptEnvironmentUpdates(environments, "environment", {
      token: "secret", host: "local", plain: "after", added: "new value",
    }, () => "new");
    expect(updated[0]?.variables.map((entry) => entry.value)).toEqual(["", "", "after", "new value"]);
    expect(updated[1]).toBe(environments[1]);
  });
});
