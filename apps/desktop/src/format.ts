import type { ResponseSnapshot } from "./api";

export function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }

  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = units[0];

  for (let index = 1; index < units.length && value >= 1024; index += 1) {
    value /= 1024;
    unit = units[index];
  }

  return `${value.toFixed(value >= 10 ? 1 : 2)} ${unit}`;
}

export function displayResponseBody(response: ResponseSnapshot): string {
  if (response.body.encoding === "base64") {
    return `[Binary response encoded as base64]\n\n${response.body.content}`;
  }

  if (response.body.contentType?.toLowerCase().includes("json")) {
    try {
      return JSON.stringify(JSON.parse(response.body.content), null, 2);
    } catch {
      return response.body.content;
    }
  }

  return response.body.content;
}

export function rawResponseBody(response: ResponseSnapshot): string {
  return response.body.content;
}

export function countTextMatches(content: string, query: string): number {
  const needle = query.toLocaleLowerCase();
  if (!needle) {
    return 0;
  }
  const haystack = content.toLocaleLowerCase();
  let count = 0;
  let offset = 0;
  while ((offset = haystack.indexOf(needle, offset)) !== -1) {
    count += 1;
    offset += Math.max(needle.length, 1);
  }
  return count;
}

export function responseFileName(response: ResponseSnapshot): string {
  const mediaType = response.body.contentType?.split(";", 1)[0]?.trim().toLowerCase();
  const extension =
    mediaType?.includes("json")
      ? "json"
      : mediaType?.includes("xml")
        ? "xml"
        : mediaType === "text/html"
          ? "html"
          : mediaType === "text/css"
            ? "css"
            : mediaType?.startsWith("text/")
              ? "txt"
              : mediaType === "application/pdf"
                ? "pdf"
                : mediaType === "image/png"
                  ? "png"
                  : mediaType === "image/jpeg"
                    ? "jpg"
                    : response.body.encoding === "base64"
                      ? "bin"
                      : "txt";
  return `response.${extension}`;
}

export function canPreviewResponse(response: ResponseSnapshot): boolean {
  return (
    response.body.encoding === "utf8" &&
    response.body.contentType?.toLowerCase().includes("text/html") === true
  );
}

export function sandboxedHtmlPreview(content: string): string {
  const policy =
    "default-src 'none'; img-src data: blob:; style-src 'unsafe-inline'; font-src data:";
  return `<meta http-equiv="Content-Security-Policy" content="${policy}">${content}`;
}
