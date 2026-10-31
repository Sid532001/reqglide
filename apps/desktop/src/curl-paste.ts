/** Recognize clipboard wrappers without changing any request arguments or body bytes. */
export function curlCommandFromClipboard(value: string): string | null {
  let command = value.trim();
  const fenced = /^```[^\r\n]*\r?\n([\s\S]*?)\r?\n```$/.exec(command);
  if (fenced) command = fenced[1]!.trim();
  command = command.replace(/^(?:PS [^\r\n>]*>|\$)\s*/i, "");
  command = command.replace(/^&\s+(?=["'])/, "");
  const executable = /^(?:"([^"]+)"|'([^']+)'|(\S+))\s+/.exec(command);
  if (!executable) return null;
  const name = (executable[1] ?? executable[2] ?? executable[3] ?? "").split(/[\\/]/).at(-1);
  return /^curl(?:\.exe)?$/i.test(name ?? "") ? command : null;
}
