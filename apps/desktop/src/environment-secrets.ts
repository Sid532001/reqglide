import type { Environment, EnvironmentVariable, SecretMutation } from "./api";

export type PendingSecretValues = Record<string, string>;

export function changeVariableStorage(
  variable: EnvironmentVariable,
  storage: "plain" | "local" | "secret",
  pending: PendingSecretValues,
): { variable: EnvironmentVariable; pending: PendingSecretValues } | null {
  const backed = Boolean(variable.secret || variable.local);
  const hasReplacement = Object.hasOwn(pending, variable.id);
  // An unreadable stored value cannot be copied into the shared document.
  if (storage === "plain" && backed && !hasReplacement) {
    return null;
  }
  const nextPending = { ...pending };
  if (storage === "plain") {
    delete nextPending[variable.id];
  } else if (!backed) {
    nextPending[variable.id] = variable.value;
  }
  return {
    variable: {
      ...variable,
      secret: storage === "secret",
      local: storage === "local",
      value: storage === "plain" ? (backed ? pending[variable.id] ?? "" : variable.value) : "",
    },
    pending: nextPending,
  };
}

function isSensitiveVariableName(key: string): boolean {
  const normalized = key.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();
  return /(?:^|[^a-z0-9])(?:token|secret|password|passwd|authorization|credential|api[_-]?key)(?:$|[^a-z0-9])/.test(normalized);
}

export function saveExtractedVariable(
  environments: Environment[],
  environmentId: string,
  key: string,
  value: string,
  createId: () => string,
): { environments: Environment[]; mutations: SecretMutation[] } {
  const trimmedKey = key.trim();
  const target = environments.find((environment) => environment.id === environmentId);
  if (!target || !trimmedKey) {
    throw new Error("Select an existing environment and enter a variable name.");
  }
  const existing = target.variables.find((variable) => variable.key.trim() === trimmedKey);
  const variable: EnvironmentVariable = existing
    ? { ...existing, enabled: true }
    : { id: createId(), key: trimmedKey, value: "", enabled: true, secret: isSensitiveVariableName(trimmedKey) };
  const backed = variable.secret || variable.local;
  variable.value = backed ? "" : value;
  return {
    environments: environments.map((environment) => environment.id === environmentId
      ? { ...environment, variables: existing
        ? environment.variables.map((entry) => entry.id === variable.id ? variable : entry)
        : [...environment.variables, variable] }
      : environment),
    mutations: backed ? [{ variableId: variable.id, value }] : [],
  };
}

export function applyScriptEnvironmentUpdates(
  environments: Environment[],
  environmentId: string,
  updates: Record<string, string>,
  createId: () => string,
): Environment[] {
  return environments.map((environment) => {
    if (environment.id !== environmentId) {
      return environment;
    }
    const remaining = new Map(Object.entries(updates));
    const variables = environment.variables.map((variable) => {
      const key = variable.key.trim();
      const value = remaining.get(key);
      remaining.delete(key);
      // Protected values are persisted natively; never place them in workspace.yaml.
      return value !== undefined && !variable.secret && !variable.local
        ? { ...variable, value }
        : variable;
    });
    return { ...environment, variables: [...variables, ...[...remaining].map(([key, value]) => ({
      id: createId(), key, value, enabled: true,
    }))] };
  });
}

export function buildSecretMutations(
  original: Environment[],
  drafts: Environment[],
  pending: PendingSecretValues,
): SecretMutation[] {
  const originalSecrets = new Set(
    original.flatMap((environment) =>
      environment.variables
        .filter((variable) => variable.secret || variable.local)
        .map((variable) => variable.id),
    ),
  );
  const draftVariables = new Map(
    drafts.flatMap((environment) =>
      environment.variables.map((variable) => [variable.id, variable] as const),
    ),
  );
  const changes = new Map<string, string | null>();

  for (const variableId of originalSecrets) {
    const draft = draftVariables.get(variableId);
    if (!draft || (!draft.secret && !draft.local)) {
      changes.set(variableId, null);
    }
  }

  for (const variable of draftVariables.values()) {
    if ((variable.secret || variable.local) && Object.hasOwn(pending, variable.id)) {
      changes.set(variable.id, pending[variable.id] ?? "");
    }
  }

  return [...changes].map(([variableId, value]) => ({ variableId, value }));
}

export function hasMissingSecretValues(
  drafts: Environment[],
  pending: PendingSecretValues,
  statuses: Record<string, boolean>,
): boolean {
  return drafts.some((environment) =>
    environment.variables.some(
      (variable) =>
        (variable.secret || variable.local) &&
        !Object.hasOwn(pending, variable.id) &&
        statuses[variable.id] !== true,
    ),
  );
}

export function sanitizeSecretReferences(environments: Environment[]): Environment[] {
  return environments.map((environment) => ({
    ...environment,
    variables: environment.variables.map((variable) => ({
      ...variable,
      value: variable.secret || variable.local ? "" : variable.value,
    })),
  }));
}
