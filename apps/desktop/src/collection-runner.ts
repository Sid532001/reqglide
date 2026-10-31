import type {
  CollectionRunTarget,
  RequestFolder,
  WorkspaceDocument,
} from "./api";

export interface CollectionRunTargetOption {
  key: string;
  label: string;
  requestCount: number;
  target: CollectionRunTarget;
}

export function collectionRunTargets(
  workspace: WorkspaceDocument,
): CollectionRunTargetOption[] {
  return workspace.collections.flatMap((collection) => [
    {
      key: `collection:${collection.id}`,
      label: collection.name,
      requestCount:
        collection.requests.length +
        collection.folders.reduce((total, folder) => total + countFolderRequests(folder), 0),
      target: { kind: "collection" as const, collectionId: collection.id },
    },
    ...collection.folders.flatMap((folder) => folderRunTargets(folder, collection.name, 1)),
  ]);
}

export function parseRunData(source: string): Array<Record<string, string>> {
  const trimmed = source.trim();
  if (!trimmed) {
    return [];
  }
  if (trimmed.startsWith("[") || trimmed.startsWith("{")) {
    return parseJsonRows(trimmed);
  }
  return parseCsvRows(trimmed);
}

function parseJsonRows(source: string): Array<Record<string, string>> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(source);
  } catch (cause) {
    throw new Error(`Run data is not valid JSON: ${errorMessage(cause)}`);
  }
  const rows = Array.isArray(parsed) ? parsed : [parsed];
  return rows.map((row, index) => normalizeRow(row, index + 1));
}

function normalizeRow(value: unknown, rowNumber: number): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`Run data row ${rowNumber} must be an object.`);
  }
  return Object.fromEntries(
    Object.entries(value).map(([key, cell]) => {
      if (cell === null || cell === undefined) {
        return [key, ""];
      }
      if (!["string", "number", "boolean"].includes(typeof cell)) {
        throw new Error(`Run data value '${key}' in row ${rowNumber} must be a scalar.`);
      }
      return [key, String(cell)];
    }),
  );
}

function parseCsvRows(source: string): Array<Record<string, string>> {
  const table = parseCsvTable(source);
  const headers = table.shift()?.map((header) => header.trim()) ?? [];
  if (!headers.length || headers.some((header) => !header)) {
    throw new Error("CSV run data must have a non-empty header row.");
  }
  if (new Set(headers).size !== headers.length) {
    throw new Error("CSV run data contains duplicate column names.");
  }
  return table
    .filter((row) => row.some((cell) => cell.length > 0))
    .map((row, index) => {
      if (row.length > headers.length) {
        throw new Error(`CSV row ${index + 2} has more values than the header row.`);
      }
      return Object.fromEntries(headers.map((header, column) => [header, row[column] ?? ""]));
    });
}

function parseCsvTable(source: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  for (let index = 0; index < source.length; index += 1) {
    const character = source[index]!;
    if (quoted) {
      if (character === '"' && source[index + 1] === '"') {
        cell += '"';
        index += 1;
      } else if (character === '"') {
        quoted = false;
      } else {
        cell += character;
      }
      continue;
    }
    if (character === '"' && cell.length === 0) {
      quoted = true;
    } else if (character === ",") {
      row.push(cell);
      cell = "";
    } else if (character === "\n") {
      row.push(cell.replace(/\r$/, ""));
      rows.push(row);
      row = [];
      cell = "";
    } else {
      cell += character;
    }
  }
  if (quoted) {
    throw new Error("CSV run data contains an unterminated quoted value.");
  }
  row.push(cell.replace(/\r$/, ""));
  rows.push(row);
  return rows;
}

function folderRunTargets(
  folder: RequestFolder,
  prefix: string,
  depth: number,
): CollectionRunTargetOption[] {
  const label = `${prefix} / ${folder.name}`;
  return [
    {
      key: `folder:${folder.id}`,
      label: `${"  ".repeat(depth)}${label}`,
      requestCount: countFolderRequests(folder),
      target: { kind: "folder", folderId: folder.id },
    },
    ...folder.folders.flatMap((child) => folderRunTargets(child, label, depth + 1)),
  ];
}

function countFolderRequests(folder: RequestFolder): number {
  return (
    folder.requests.length +
    folder.folders.reduce((total, child) => total + countFolderRequests(child), 0)
  );
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
