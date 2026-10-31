import { describe, expect, it } from "vitest";
import { curlCommandFromClipboard } from "./curl-paste";

describe("cURL clipboard detection", () => {
  it("recognizes shell and Markdown wrappers while preserving the body and variables", () => {
    const command = "curl.exe `\r\n --request POST `\r\n --header 'Authorization: Bearer {{access_token}}' `\r\n --data-raw '{\"message\":\"hello\"}' https://api.example.test";
    for (const wrapped of [command, `$ ${command}`, `PS C:\\work> ${command}`, `\uFEFF\n\`\`\`powershell\n${command}\n\`\`\``]) {
      expect(curlCommandFromClipboard(wrapped)).toBe(command);
    }
    const quoted = "'C:\\Program Files\\curl.exe' https://api.example.test";
    expect(curlCommandFromClipboard(`& ${quoted}`)).toBe(quoted);
    expect(curlCommandFromClipboard('/usr/bin/curl -H "Accept: application/json" https://example.test')).not.toBeNull();
  });

  it("leaves ordinary URLs, text, and commands other than curl untouched", () => {
    for (const value of ["https://curl.example.test", "curl", "curling https://example.test", "echo curl https://example.test", "{{base_url}}/users", "", "node script.js"]) {
      expect(curlCommandFromClipboard(value)).toBeNull();
    }
  });
});
