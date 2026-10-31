import { test, expect } from "@playwright/test";
import { writeFile } from "node:fs/promises";

const pageErrors = new WeakMap();

// The real React app runs in Chromium. Only native dialogs, storage, and network IPC
// are substituted; native transport and scripting have separate local-server tests.
async function installDesktopFixture(page) {
  await page.addInitScript(() => {
    window.isTauri = true;
    const workspace = {
      schemaVersion: 7,
      id: "1e9a4d81-a41e-4fb4-9347-414674d25f82",
      name: "Workflow checks",
      collections: [{ id: "03459c45-ed10-4527-af5a-928c17bc58f6", name: "APIs", folders: [], requests: [] }],
      environments: [{
        id: "843e482b-09b7-44ce-9b80-a269ef2e90dc", name: "Stage",
        variables: [{
          id: "992ee4f3-9587-4d35-a3a7-ded9e6d4ef1e", key: "access_token",
          value: "", enabled: true, secret: true, local: false,
        }],
      }],
      protocolProfiles: [],
    };
    const fixture = window.desktopFixture = {
      workspace: JSON.parse(localStorage.getItem("desktopFixture.workspace") || "null") || workspace,
      secrets: { "992ee4f3-9587-4d35-a3a7-ded9e6d4ef1e": "original-test-token" },
      calls: [],
      saveDelay: 0,
      sendDelay: 0,
      pendingSend: null,
      nextError: null,
    };
    const session = () => ({ workspace: structuredClone(fixture.workspace), rootPath: "C:/workflow-fixture" });
    window.__TAURI_INTERNALS__ = {
      invoke: async (command, args = {}) => {
        fixture.calls.push({ command, args: structuredClone(args) });
        switch (command) {
          case "reopen_last_workspace": {
            const error = localStorage.getItem("desktopFixture.startupError");
            if (error) throw new Error(error);
            return localStorage.getItem("desktopFixture.lastWorkspace") ? session() : null;
          }
          case "remember_open_workspace":
            if (args.workspaceId !== fixture.workspace.id) return false;
            localStorage.setItem("desktopFixture.workspace", JSON.stringify(fixture.workspace));
            localStorage.setItem("desktopFixture.lastWorkspace", fixture.workspace.id);
            return true;
          case "open_workspace":
            localStorage.removeItem("desktopFixture.startupError");
            localStorage.setItem("desktopFixture.workspace", JSON.stringify(fixture.workspace));
            return session();
          case "export_workspace_backup":
          case "export_support_diagnostics":
            if (fixture.cancelMaintenance) return null;
            return command === "export_workspace_backup" ? "C:/backups/workspace.apic-backup.json" : "C:/backups/support.apic-diagnostics.json";
          case "restore_workspace_backup":
            if (fixture.cancelMaintenance) return null;
            if (fixture.restoreError) throw new Error(fixture.restoreError);
            fixture.workspace = structuredClone(fixture.workspace);
            fixture.workspace.id = "791dc8e9-24f8-44fa-944d-dd8faac7ee36";
            fixture.workspace.name = "Restored workspace";
            fixture.secrets = {};
            return session();
          case "load_recovery_snapshot":
            return JSON.parse(localStorage.getItem(`desktopFixture.recovery.${args.workspaceId}`) || "null");
          case "save_recovery_snapshot":
            localStorage.setItem(`desktopFixture.recovery.${args.workspaceId}`, JSON.stringify(args.snapshot));
            return;
          case "clear_recovery_snapshot":
            localStorage.removeItem(`desktopFixture.recovery.${args.workspaceId}`);
            return;
          case "close_workspace":
            localStorage.removeItem("desktopFixture.lastWorkspace");
            return;
          case "environment_secret_statuses":
            return Object.fromEntries(Object.keys(fixture.secrets).map(id => [id, true]));
          case "environment_local_values":
            return Object.fromEntries(fixture.workspace.environments.flatMap(env => env.variables)
              .filter(variable => variable.local).map(variable => [variable.id, fixture.secrets[variable.id]]));
          case "save_workspace":
          case "save_workspace_with_secrets": {
            if (fixture.saveDelay) await new Promise(resolve => setTimeout(resolve, fixture.saveDelay));
            for (const env of args.workspace.environments) {
              if (env.variables.some(variable => (variable.secret || variable.local) && variable.value)) {
                throw new Error("Protected variable plaintext reached workspace storage");
              }
            }
            for (const mutation of args.mutations ?? []) {
              if (mutation.value === null) delete fixture.secrets[mutation.variableId];
              else fixture.secrets[mutation.variableId] = mutation.value;
            }
            fixture.workspace = structuredClone(args.workspace);
            localStorage.setItem("desktopFixture.workspace", JSON.stringify(fixture.workspace));
            return session();
          }
          case "execute_request_lifecycle": {
            if (fixture.nextError) return { response: null, scriptRuns: [], environmentUpdates: {}, error: fixture.nextError };
            if (fixture.sendDelay) {
              await new Promise((resolve, reject) => {
                fixture.pendingSend = { reject };
                setTimeout(resolve, fixture.sendDelay);
              });
              fixture.pendingSend = null;
            }
            const content = fixture.nextResponseContent ?? '{"access_token":"fresh-test-token","expires_in":3600}';
            return {
              response: { status: 200, statusText: "OK", headers: [],
                body: { content, contentType: "application/json", encoding: "utf8", truncated: false },
                timing: { totalMs: 12 }, receivedBytes: content.length },
              scriptRuns: [], environmentUpdates: {}, error: null,
            };
          }
          case "cancel_http_request":
            fixture.pendingSend?.reject("request cancelled");
            fixture.pendingSend = null;
            return true;
          case "record_history": return {};
          case "list_history": return [];
          case "import_curl":
            if (fixture.importDelay) await new Promise(resolve => setTimeout(resolve, fixture.importDelay));
            if (fixture.nextCurlImport) return structuredClone(fixture.nextCurlImport);
            if (!args.command.includes("oauth2/v1/token")) throw new Error("Paste a curl command containing an HTTP URL.");
            return {
              request: {
                method: "POST", url: "https://auth.example.test/oauth2/v1/token", queryParams: [],
                headers: [
                  { name: "Content-Type", value: "application/x-www-form-urlencoded", enabled: true },
                  { name: "Authorization", value: "{{basic_auth_stage}}", enabled: true },
                ],
                auth: { kind: "none" },
                body: { kind: "formUrlEncoded", fields: [{ name: "grant_type", value: "client_credentials", enabled: true }] },
                options: { timeoutMs: 30_000, maxRequestBytes: 104857600, maxResponseBytes: 10485760, followRedirects: true },
              }, warnings: [],
            };
          default: throw new Error(`Unexpected native command: ${command}`);
        }
      },
    };
  });
}

test.beforeEach(async ({ page }) => {
  pageErrors.set(page, []);
  page.on("pageerror", error => pageErrors.get(page).push(error.message));
  await installDesktopFixture(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Open", exact: true }).click();
  await expect(page.getByLabel("Active environment")).toHaveValue("843e482b-09b7-44ce-9b80-a269ef2e90dc");
});

async function pasteIntoUrl(page, command) {
  await page.getByLabel("Request URL", { exact: true }).evaluate((input, command) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData("text/plain", command);
    input.dispatchEvent(new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData }));
  }, command);
}

test("private backups explain their contents and preserve unsaved request tabs", async ({ page }) => {
  const draft = "https://api.example.test/unsaved-draft";
  await page.getByLabel("Request URL", { exact: true }).fill(draft);
  await page.getByText("Tools", { exact: true }).click();
  await page.getByRole("button", { name: /Back up workspace/ }).click();
  const dialog = page.getByRole("dialog", { name: "Back up workspace", exact: true });
  await expect(dialog).toContainText("unencrypted JSON");
  await expect(dialog).toContainText("Secret and Local variable values stay in the OS credential store");
  await page.screenshot({ path: test.info().outputPath("workspace-backup-dialog.png") });
  await page.keyboard.press("Control+Enter");
  expect(await page.evaluate(() => window.desktopFixture.calls.some(call => call.command === "execute_request_lifecycle"))).toBe(false);
  await dialog.getByRole("button", { name: "Choose backup location" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText("Private backup saved.", { exact: false })).toBeVisible();
  await expect(page.getByLabel("Request URL", { exact: true })).toHaveValue(draft);
  const calls = await page.evaluate(() => window.desktopFixture.calls.filter(call => call.command === "export_workspace_backup"));
  expect(calls).toEqual([{ command: "export_workspace_backup", args: {} }]);
});

test("relaunch reopens the last workspace, tabs, and environment without a picker or request execution", async ({ page }) => {
  const productionId = "9d699421-c02f-40e9-8dfb-9f82d18a6a53";
  await page.evaluate((id) => {
    window.desktopFixture.workspace.environments.push({ id, name: "Production", variables: [] });
  }, productionId);
  await page.getByRole("button", { name: "Open", exact: true }).click();
  await page.getByLabel("Active environment").selectOption(productionId);
  await page.getByLabel("Request URL", { exact: true }).fill("https://api.example.test/first-draft");
  await page.getByRole("button", { name: "+New", exact: true }).click();
  await page.getByLabel("Request URL", { exact: true }).fill("https://api.example.test/second-draft");
  await expect.poll(() => page.evaluate(() => {
    const snapshot = JSON.parse(localStorage.getItem(`desktopFixture.recovery.${window.desktopFixture.workspace.id}`) || "null");
    return snapshot?.tabs.find(tab => tab.id === snapshot.activeTabId)?.request.url;
  })).toBe("https://api.example.test/second-draft");
  const prompts = [];
  page.on("dialog", async dialog => {
    if (dialog.type() !== "beforeunload") prompts.push(dialog.message());
    await dialog.accept();
  });
  await page.reload();
  await expect(page.getByLabel("Request URL", { exact: true })).toHaveValue("https://api.example.test/second-draft");
  await expect(page.locator(".open-request-tab")).toHaveCount(2);
  await expect(page.getByLabel("Active environment")).toHaveValue(productionId);
  const calls = await page.evaluate(() => window.desktopFixture.calls);
  expect(calls.filter(call => call.command === "reopen_last_workspace")).toHaveLength(1);
  expect(calls.some(call => ["open_workspace", "execute_request_lifecycle"].includes(call.command))).toBe(false);
  expect(prompts).toEqual([]);
});

test("an unavailable previous workspace leaves Open usable for recovery", async ({ page }) => {
  await page.evaluate(() => localStorage.setItem("desktopFixture.startupError", "Could not reopen the last workspace. Its folder may have moved or be unavailable. Use Open to choose a workspace."));
  await page.reload();
  await expect(page.getByText("Its folder may have moved", { exact: false })).toBeVisible();
  const open = page.getByRole("button", { name: "Open", exact: true });
  await expect(open).toBeEnabled();
  await open.click();
  await expect(page.getByLabel("Active environment")).toHaveValue("843e482b-09b7-44ce-9b80-a269ef2e90dc");
});

test("explicit Close workspace clears automatic reopening without deleting saved data", async ({ page }) => {
  await expect.poll(() => page.evaluate(() => localStorage.getItem("desktopFixture.lastWorkspace"))).not.toBeNull();
  await page.getByRole("button", { name: "Close workspace", exact: true }).click();
  await expect(page.getByText("Workspace closed. Files remain on disk.")).toBeVisible();
  expect(await page.evaluate(() => localStorage.getItem("desktopFixture.lastWorkspace"))).toBeNull();
  await page.reload();
  await page.getByText("Tools", { exact: true }).click();
  await expect(page.getByRole("button", { name: /Back up workspace/ })).toBeDisabled();
  expect(await page.evaluate(() => window.desktopFixture.calls.some(call => call.command === "open_workspace"))).toBe(false);
});

test("invalid restore preserves the current workspace and valid restore opens a separate one", async ({ page }) => {
  const before = await page.evaluate(() => window.desktopFixture.workspace.id);
  await page.evaluate(() => { window.desktopFixture.restoreError = "This file is not a valid workspace backup."; });
  await page.getByText("Tools", { exact: true }).click();
  await page.getByRole("button", { name: /Restore workspace backup/ }).click();
  const dialog = page.getByRole("dialog", { name: "Restore workspace backup", exact: true });
  await dialog.getByRole("button", { name: "Choose backup file" }).click();
  await expect(dialog.getByRole("alert")).toHaveText("This file is not a valid workspace backup.");
  expect(await page.evaluate(() => window.desktopFixture.workspace.id)).toBe(before);
  await page.evaluate(() => { window.desktopFixture.restoreError = null; });
  await dialog.getByRole("button", { name: "Choose backup file" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText("Restored Restored workspace as a separate workspace.", { exact: false })).toBeVisible();
  expect(await page.evaluate(() => window.desktopFixture.workspace.id)).not.toBe(before);
  expect(await page.evaluate(() => window.desktopFixture.secrets)).toEqual({});
  expect(await page.evaluate(() => window.desktopFixture.calls.some(call => call.command === "execute_request_lifecycle"))).toBe(false);
});

test("diagnostics work without a workspace and cancelled file dialogs report no export", async ({ page }) => {
  await page.evaluate(() => localStorage.removeItem("desktopFixture.lastWorkspace"));
  await page.reload();
  await page.getByText("Tools", { exact: true }).click();
  await expect(page.getByRole("button", { name: /Back up workspace/ })).toBeDisabled();
  await page.getByRole("button", { name: /Export support diagnostics/ }).click();
  let dialog = page.getByRole("dialog", { name: "Export support diagnostics", exact: true });
  await expect(dialog).toContainText("Nothing is uploaded automatically");
  await page.evaluate(() => { window.desktopFixture.cancelMaintenance = true; });
  await dialog.getByRole("button", { name: "Choose diagnostics location" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText("Diagnostics saved locally.", { exact: false })).toBeHidden();
  await page.evaluate(() => { window.desktopFixture.cancelMaintenance = false; });
  await page.getByText("Tools", { exact: true }).click();
  await page.getByRole("button", { name: /Export support diagnostics/ }).click();
  dialog = page.getByRole("dialog", { name: "Export support diagnostics", exact: true });
  await dialog.getByRole("button", { name: "Choose diagnostics location" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText("Diagnostics saved locally.", { exact: false })).toBeVisible();
  expect(await page.evaluate(() => window.desktopFixture.calls.filter(call => call.command === "export_support_diagnostics").every(call => Object.keys(call.args).length === 0))).toBe(true);
});

test("pasting multiline cURL in the URL bar imports without sending or replacing an edited tab", async ({ page }) => {
  const original = "https://api.example.test/keep-this-draft";
  await page.getByLabel("Request URL", { exact: true }).fill(original);
  await pasteIntoUrl(page, `\`\`\`powershell\nPS C:\\work> ${tokenCurl}\n\`\`\``);
  await expect(page.getByLabel("Request URL", { exact: true })).toHaveValue("https://auth.example.test/oauth2/v1/token");
  await expect(page.getByLabel("HTTP method")).toHaveValue("POST");
  await expect(page.getByLabel("Field", { exact: true })).toHaveValue("grant_type");
  await expect(page.getByLabel("Value", { exact: true })).toHaveValue("client_credentials");
  expect(await page.evaluate(() => window.desktopFixture.calls.filter(call => call.command === "execute_request_lifecycle").length)).toBe(0);
  await expect(page.locator(".open-request-tab")).toHaveCount(2);
  await page.locator(".open-request-tab-main").first().click();
  await expect(page.getByLabel("Request URL")).toHaveValue(original);
});

test("URL-bar JSON cURL import preserves headers, auth variables and query parameters", async ({ page }) => {
  await page.evaluate(() => {
    window.desktopFixture.importDelay = 200;
    window.desktopFixture.nextCurlImport = {
      request: {
        method: "PATCH", url: "https://api.example.test/users?active=true", queryParams: [],
        headers: [{ name: "Content-Type", value: "application/json", enabled: true }, { name: "X-Team", value: "{{team}}", enabled: true }],
        auth: { kind: "bearer", token: "{{access_token}}" },
        body: { kind: "text", content: '{"name":"Ada"}', contentType: "application/json" },
        options: { timeoutMs: 30_000, maxRequestBytes: 104857600, maxResponseBytes: 10485760, followRedirects: true },
      }, warnings: [],
    };
  });
  await pasteIntoUrl(page, 'curl -X PATCH "https://api.example.test/users?active=true" -H "Content-Type: application/json" -d \'{"name":"Ada"}\'');
  await expect(page.getByRole("button", { name: /Importing/ })).toBeDisabled();
  await expect(page.getByLabel("Request URL")).toHaveValue("https://api.example.test/users?active=true");
  await expect(page.getByLabel("HTTP method")).toHaveValue("PATCH");
  await expect(page.getByLabel("Request body", { exact: true })).toHaveValue('{"name":"Ada"}');
  await expect(page.getByLabel("Raw body content type")).toHaveValue("application/json");
  await page.getByRole("tab", { name: "Auth", exact: true }).click();
  await expect(page.getByLabel("Bearer token")).toHaveValue("{{access_token}}");
  await page.getByRole("tab", { name: /^Headers/ }).first().click();
  await expect(page.getByLabel("Header value", { exact: true }).last()).toHaveValue("{{team}}");
  expect(await page.evaluate(() => window.desktopFixture.calls.filter(call => call.command === "execute_request_lifecycle").length)).toBe(0);
});

test("invalid cURL in the URL bar keeps the current request usable", async ({ page }) => {
  const previous = await page.getByLabel("Request URL").inputValue();
  await pasteIntoUrl(page, "curl --unsupported https://bad.example.test");
  await expect(page.getByRole("alert")).toContainText("Paste a curl command");
  await expect(page.getByLabel("Request URL")).toHaveValue(previous);
  await expect(page.getByRole("button", { name: /Send/ })).toBeEnabled();
  await expect(page.locator(".open-request-tab")).toHaveCount(1);
  await pasteIntoUrl(page, tokenCurl);
  await expect(page.getByLabel("HTTP method")).toHaveValue("POST");
});

test("large responses keep URL typing and draft recovery responsive", async ({ page }) => {
  await page.evaluate(() => {
    window.desktopFixture.nextResponseContent = JSON.stringify({ items: Array.from({ length: 12_000 }, (_, id) => ({ id, message: "local response sample" })) });
  });
  await page.getByRole("button", { name: /Send/ }).click();
  await expect(page.getByText("Large response · syntax colors paused")).toBeVisible();
  await expect(page.locator(".response-body span")).toHaveCount(0);
  const url = page.getByLabel("Request URL");
  await url.fill("https://api.example.test/");
  await page.evaluate(() => {
    window.typingLatencySamples = [];
    document.querySelector('[aria-label="Request URL"]').addEventListener("input", () => {
      const start = performance.now();
      requestAnimationFrame(() => window.typingLatencySamples.push(performance.now() - start));
    });
  });
  const began = Date.now();
  await url.pressSequentially("this-editor-stays-responsive");
  const elapsed = Date.now() - began;
  await expect(url).toHaveValue("https://api.example.test/this-editor-stays-responsive");
  await expect.poll(() => page.evaluate(() => window.desktopFixture.calls.filter(call => call.command === "save_recovery_snapshot").at(-1)?.args.snapshot.tabs.find(tab => tab.id === window.desktopFixture.calls.filter(call => call.command === "save_recovery_snapshot").at(-1)?.args.snapshot.activeTabId)?.request.url)).toBe("https://api.example.test/this-editor-stays-responsive");
  const content = await page.locator(".response-body").textContent();
  expect(JSON.parse(content).items.at(-1).id).toBe(11_999);
  const samples = await page.evaluate(() => window.typingLatencySamples.toSorted((a,b) => a-b));
  const metrics = { responseCharacters: content.length, typingMilliseconds: elapsed, inputToFrameP95Ms: samples[Math.floor(samples.length * .95)] };
  await writeFile(test.info().outputPath("typing-metrics.json"), JSON.stringify(metrics, null, 2));
  await test.info().attach("large-response-typing", { body: JSON.stringify(metrics), contentType: "application/json" });
  expect(elapsed).toBeLessThan(5000);
});

test("workspace layout, quick actions and splitters work at desktop and compact sizes", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = window.desktopFixture;
    fixture.workspace.name = "Atlas Engineering";
    const request = (id, name, method, path) => ({
      id, name, scripts: { preRequest: "", postResponse: "" }, request: {
        method, url: `https://api.example.test/v1/${path}`, queryParams: [],
        headers: [{ name: "Content-Type", value: "application/json", enabled: true }],
        auth: { kind: "bearer", token: "{{access_token}}" },
        body: method === "GET" ? { kind: "empty" } : { kind: "text", content: '{\n  "filters": [{ "field": "status", "operator": "eq", "value": "active" }]\n}', contentType: "application/json" },
        options: { timeoutMs: 30_000, maxRequestBytes: 104857600, maxResponseBytes: 10485760, followRedirects: true },
      },
    });
    fixture.workspace.collections = [
      { id: "03459c45-ed10-4527-af5a-928c17bc58f6", name: "Device platform", folders: [], requests: [
        request("02d35c4d-e555-42d1-a1cc-3194e0fc77ed", "Search events", "POST", "events/search"),
        request("d4b90914-c413-40ac-9ff1-8ab812ad9214", "Get device details", "GET", "devices"),
      ] },
      { id: "2283a1b9-b831-4201-8647-cf37ed3eb553", name: "Identity & access", folders: [], requests: [
        request("3fd11afd-8fa0-4cd3-90d7-ad6561db2de5", "Create access token", "POST", "token"),
      ] },
      { id: "26e25724-d828-4f1c-aed9-12a340f94be8", name: "Customer services", folders: [], requests: [] },
    ];
    fixture.nextResponseContent = JSON.stringify({ data: [{ id: "evt_1209", status: "active", type: "device.connected", receivedAt: "2026-10-30T15:30:00Z" }], pagination: { total: 1, limit: 25 } });
  });
  await page.getByRole("button", { name: "Open", exact: true }).click();
  await page.locator(".tree-item.request").filter({ hasText: "Search events" }).click();
  await page.getByRole("tab", { name: "Body", exact: true }).first().click();
  await page.getByRole("button", { name: /Send/ }).click();
  await expect(page.locator(".response-metrics")).toContainText("200 OK");
  if (await page.getByLabel("Dismiss notification").isVisible()) await page.getByLabel("Dismiss notification").click();
  await page.screenshot({ path: test.info().outputPath("workspace-desktop.png") });
  await page.getByRole("button", { name: "Open command palette" }).click();
  await expect(page.getByRole("dialog", { name: "Command palette", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");

  const side = page.getByRole("separator", { name: "Resize sidebar" });
  const before = await page.locator(".sidebar").boundingBox();
  const grip = await side.boundingBox();
  await page.mouse.move(grip.x + 3, grip.y + 100);
  await page.mouse.down();
  await page.mouse.move(grip.x + 63, grip.y + 100, { steps: 12 });
  await page.mouse.up();
  expect((await page.locator(".sidebar").boundingBox()).width).toBeGreaterThan(before.width + 40);

  const split = page.getByRole("separator", { name: "Resize request panel" });
  const splitBounds = await split.boundingBox();
  const panelBefore = await page.locator(".request-panel").boundingBox();
  await page.mouse.move(splitBounds.x + splitBounds.width / 2, splitBounds.y + 4);
  await page.mouse.down();
  await page.mouse.move(splitBounds.x + splitBounds.width / 2, splitBounds.y + 54, { steps: 12 });
  await page.mouse.up();
  expect((await page.locator(".request-panel").boundingBox()).height).toBeGreaterThan(panelBefore.height + 30);

  for (const width of [1024, 720]) {
    await page.setViewportSize({ width, height: 820 });
    if (width === 720) await page.getByRole("button", { name: "Hide sidebar" }).click();
    await expect(page.getByLabel("Request URL")).toBeVisible();
    await expect(page.getByRole("button", { name: /Send/ })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: test.info().outputPath(`workspace-${width}.png`) });
  }
  await page.emulateMedia({ reducedMotion: "reduce" });
  expect(await page.getByRole("button", { name: /Send/ }).evaluate(button => getComputedStyle(button).transitionDuration)).toBe("0s");
});

test.afterEach(async ({ page }) => {
  expect(pageErrors.get(page), "Unhandled browser exceptions").toEqual([]);
});

test("multipart field names remain focused while typing", async ({ page }) => {
  await page.getByRole("tab", { name: "Body", exact: true }).first().click();
  await page.getByRole("tab", { name: "form-data", exact: true }).click();
  await page.getByRole("button", { name: "Add multipart field" }).click();
  const name = page.getByLabel("Multipart field name", { exact: true });
  await name.click();
  await name.pressSequentially("description");
  await expect(name).toBeFocused();
  await expect(name).toHaveValue("description");
});

test("changing Secret to Local preserves the existing credential", async ({ page }) => {
  await page.getByRole("button", { name: "Manage environments" }).click();
  const dialog = page.getByRole("dialog", { name: "Environments", exact: true });
  await test.info().attach("environment-editor", {
    body: await page.screenshot({ path: test.info().outputPath("environment-editor.png") }),
    contentType: "image/png",
  });
  await dialog.getByLabel("Storage for access_token", { exact: true }).selectOption("local");
  await dialog.getByRole("button", { name: "Save environments", exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(await page.evaluate(() => window.desktopFixture.secrets["992ee4f3-9587-4d35-a3a7-ded9e6d4ef1e"])).toBe("original-test-token");
  expect(await page.evaluate(() => window.desktopFixture.workspace.environments[0].variables[0].value)).toBe("");
});

test("response extraction updates an existing secret through the vault", async ({ page }) => {
  await page.getByRole("button", { name: /Send/ }).click();
  await page.getByRole("button", { name: "Extract variable", exact: true }).click();
  const row = page.locator(".response-extract-row").filter({ has: page.getByLabel("Variable name for access_token", { exact: true }) });
  await row.getByRole("button", { name: "Save", exact: true }).click();
  await expect(row.getByRole("button", { name: /Saved/ })).toBeVisible();
  expect(await page.evaluate(() => window.desktopFixture.secrets["992ee4f3-9587-4d35-a3a7-ded9e6d4ef1e"])).toBe("fresh-test-token");
  expect(await page.evaluate(() => window.desktopFixture.workspace.environments[0].variables[0].value)).toBe("");
});

test("a new extracted token defaults to secret storage", async ({ page }) => {
  await page.getByRole("button", { name: /Send/ }).click();
  await page.getByRole("button", { name: "Extract variable", exact: true }).click();
  const row = page.locator(".response-extract-row").filter({ has: page.getByLabel("Variable name for access_token", { exact: true }) });
  await row.getByRole("textbox").fill("service_token");
  await row.getByRole("button", { name: "Save", exact: true }).click();
  await expect(row.getByRole("button", { name: /Saved/ })).toBeVisible();
  const variable = await page.evaluate(() => window.desktopFixture.workspace.environments[0].variables.find(variable => variable.key === "service_token"));
  expect(variable.secret).toBe(true);
  expect(variable.value).toBe("");
  expect(await page.evaluate(id => window.desktopFixture.secrets[id], variable.id)).toBe("fresh-test-token");
});

test("Cancel returns the editor to a usable state", async ({ page }) => {
  await page.evaluate(() => { window.desktopFixture.sendDelay = 10_000; });
  await page.getByRole("button", { name: /Send/ }).click();
  await page.getByRole("button", { name: /Cancel/ }).click();
  await expect(page.getByRole("button", { name: /Send/ })).toBeEnabled();
  await expect(page.getByText("Request cancelled.", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.desktopFixture.calls.filter(call => call.command === "execute_request_lifecycle").length)).toBe(1);
  await page.evaluate(() => { window.desktopFixture.sendDelay = 0; });
  await page.getByRole("button", { name: /Send/ }).click();
  await expect(page.getByRole("button", { name: "Extract variable", exact: true })).toBeVisible();
});

test("new folders persist through the workspace sidebar", async ({ page }) => {
  await page.getByRole("button", { name: "Add folder to APIs", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "New folder", exact: true });
  await dialog.getByRole("textbox").fill("Authentication");
  await dialog.getByRole("button", { name: "Create", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.locator(".tree-item.folder").filter({ hasText: "Authentication" })).toBeVisible();
  expect(await page.evaluate(() => window.desktopFixture.workspace.collections[0].folders[0].name)).toBe("Authentication");
});

const tokenCurl = String.raw`curl --location '[https://auth.example.test/oauth2/v1/token](https://auth.example.test/oauth2/v1/token)' \
\--header 'Content-Type: application/x-www-form-urlencoded' \
\--header 'Authorization: {{basic_auth_stage}}' \
\--data-urlencode 'grant\_type=client\_credentials'`;

test("pasting cURL in workspace search populates the request editor", async ({ page }) => {
  await page.getByLabel("Search workspace", { exact: true }).evaluate((input, command) => {
    const clipboardData = new DataTransfer();
    clipboardData.setData("text/plain", command);
    input.dispatchEvent(new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData }));
  }, tokenCurl);
  await expect(page.getByLabel("HTTP method", { exact: true })).toHaveValue("POST");
  await expect(page.getByLabel("Request URL", { exact: true })).toHaveValue("https://auth.example.test/oauth2/v1/token");
  await expect(page.getByLabel("Search workspace", { exact: true })).toHaveValue("");
  await expect(page.getByLabel("Field", { exact: true })).toHaveValue("grant_type");
  await expect(page.getByLabel("Value", { exact: true })).toHaveValue("client_credentials");
  expect(await page.evaluate(() => window.desktopFixture.calls.find(call => call.command === "import_curl").args.command)).toBe(tokenCurl);
});

test("cURL dialog shows errors and lets the user correct the command", async ({ page }) => {
  await page.getByRole("button", { name: "Import cURL", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import cURL", exact: true });
  await dialog.getByLabel("cURL command", { exact: true }).fill("not a curl command");
  await dialog.getByRole("button", { name: "Import request", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText("Could not import this command");
  await dialog.getByLabel("cURL command", { exact: true }).fill(tokenCurl);
  await dialog.getByRole("button", { name: "Import request", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByLabel("HTTP method", { exact: true })).toHaveValue("POST");
});

test("simultaneous extraction saves preserve both values", async ({ page }) => {
  await page.getByRole("button", { name: /Send/ }).click();
  await page.getByRole("button", { name: "Extract variable", exact: true }).click();
  await page.evaluate(() => { window.desktopFixture.saveDelay = 250; });
  const token = page.locator(".response-extract-row").filter({ has: page.getByLabel("Variable name for access_token", { exact: true }) });
  const expires = page.locator(".response-extract-row").filter({ has: page.getByLabel("Variable name for expires_in", { exact: true }) });
  await token.getByRole("button", { name: "Save", exact: true }).click();
  await expires.getByRole("button", { name: "Save", exact: true }).click();
  await expect(token.getByRole("button", { name: /Saved/ })).toBeVisible();
  await expect(expires.getByRole("button", { name: /Saved/ })).toBeVisible();
  const values = await page.evaluate(() => ({
    variables: window.desktopFixture.workspace.environments[0].variables,
    secret: window.desktopFixture.secrets["992ee4f3-9587-4d35-a3a7-ded9e6d4ef1e"],
  }));
  expect(values.variables.find(variable => variable.key === "expires_in").value).toBe("3600");
  expect(values.secret).toBe("fresh-test-token");
});

test("network timeouts expose proxy and timeout settings instead of implying bad auth", async ({ page }) => {
  await page.evaluate(() => { window.desktopFixture.nextError = "connection timed out during DNS, proxy, TCP, or TLS setup; check the VPN and proxy settings used by Postman\nTransport route: system settings (no PAC override selected)"; });
  await page.getByRole("button", { name: /Send/ }).click();
  const failure = page.getByRole("alert");
  await expect(failure).toContainText("Connection timed out");
  await expect(failure).toContainText("No HTTP response was received");
  await expect(failure).not.toContainText("Postman");
  await expect(failure).not.toContainText("no PAC override selected");
  await expect(failure).toContainText("System proxy");
  await failure.getByRole("button", { name: "Network & TLS settings", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Network & TLS", exact: true });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("combobox", { name: "Mode", exact: true }).selectOption("custom");
  await dialog.getByLabel("Proxy URL", { exact: true }).fill("http://proxy.example.com:8080");
  await dialog.getByRole("button", { name: "Save settings", exact: true }).click();
  await expect(dialog).toBeHidden();
  expect(await page.evaluate(() => window.desktopFixture.workspace.network.proxyUrl)).toBe("http://proxy.example.com:8080");
});

test("a generated token can be used on a new request as a variable or raw bearer token", async ({ page }) => {
  await page.getByRole("button", { name: /Send/ }).click();
  await page.getByRole("button", { name: "Extract variable", exact: true }).click();
  const row = page.locator(".response-extract-row").filter({ has: page.getByLabel("Variable name for access_token", { exact: true }) });
  await row.getByRole("button", { name: "Save", exact: true }).click();
  await expect(row.getByRole("button", { name: /Saved/ })).toBeVisible();
  await page.getByRole("button", { name: "+New", exact: true }).click();
  await page.getByRole("tab", { name: "Auth", exact: true }).click();
  await page.getByLabel("Authentication type", { exact: true }).selectOption("bearer");
  await page.getByLabel("Bearer token", { exact: true }).fill("{{access_token}}");
  await page.getByRole("button", { name: /Send/ }).click();
  await expect(page.getByRole("button", { name: "Extract variable", exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.desktopFixture.calls.filter(call => call.command === "execute_request_lifecycle").at(-1).args.input.request.auth)).toEqual({ kind: "bearer", token: "{{access_token}}" });
  expect(await page.evaluate(() => window.desktopFixture.calls.filter(call => call.command === "execute_request_lifecycle").at(-1).args.input.environmentId)).toBe("843e482b-09b7-44ce-9b80-a269ef2e90dc");
  await page.getByLabel("Bearer token", { exact: true }).fill("fresh-test-token");
  await page.getByRole("button", { name: /Send/ }).click();
  await expect(page.getByRole("button", { name: "Extract variable", exact: true })).toBeVisible();
  expect(await page.evaluate(() => window.desktopFixture.calls.filter(call => call.command === "execute_request_lifecycle").at(-1).args.input.request.auth)).toEqual({ kind: "bearer", token: "fresh-test-token" });
});

test("manual Authorization header names remain focused while typing", async ({ page }) => {
  await page.getByRole("button", { name: "Add header", exact: false }).click();
  const header = page.getByLabel("Header name", { exact: true }).last();
  await header.click();
  await header.pressSequentially("Authorization");
  await expect(header).toHaveValue("Authorization");
  await expect(header).toBeFocused();
});

for (const repair of ["button", "selector"]) {
test(`an imported JSON body repairs its form content type with the ${repair} without changing auth or URL`, async ({ page }) => {
  const url = "https://api.example.test/v1/events/search";
  await page.evaluate((url) => {
    window.desktopFixture.nextCurlImport = {
      request: {
        method: "POST", url, queryParams: [],
        headers: [{ name: "Content-Type", value: "application/x-www-form-urlencoded", enabled: true }],
        auth: { kind: "bearer", token: "{{access_token}}" },
        body: { kind: "text", content: '{"filters":[{"field":"id","operator":"eq","value":"example-device"}]}', contentType: "application/x-www-form-urlencoded" },
        options: { timeoutMs: 30_000, maxRequestBytes: 104857600, maxResponseBytes: 10485760, followRedirects: true },
      }, warnings: [],
    };
  }, url);
  await page.getByRole("button", { name: "Import cURL", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Import cURL", exact: true });
  await dialog.getByLabel("cURL command", { exact: true }).fill("curl https://api.example.test/search");
  await dialog.getByRole("button", { name: "Import request", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByLabel("Raw body content type")).toHaveValue("application/x-www-form-urlencoded");
  await expect(page.locator(".request-framing-warning")).toContainText("JSON body");
  await test.info().attach("content-type-mismatch", {
    body: await page.screenshot({ path: test.info().outputPath("content-type-mismatch.png") }),
    contentType: "image/png",
  });
  if (repair === "button") {
    await page.getByRole("button", { name: "Use application/json", exact: true }).click();
  } else {
    await page.getByLabel("Raw body content type").selectOption("application/json");
  }
  await expect(page.getByLabel("Raw body content type")).toHaveValue("application/json");
  await expect(page.locator(".request-framing-warning")).toBeHidden();
  await page.getByRole("button", { name: /Send/ }).click();
  const sent = await page.evaluate(() => window.desktopFixture.calls.filter(call => call.command === "execute_request_lifecycle").at(-1).args.input);
  expect(sent.request.url).toBe(url);
  expect(sent.request.method).toBe("POST");
  expect(sent.request.auth).toEqual({ kind: "bearer", token: "{{access_token}}" });
  expect(sent.request.body.contentType).toBe("application/json");
  expect(sent.request.headers).toEqual([{ name: "Content-Type", value: "application/json", enabled: true }]);
  expect(JSON.parse(sent.request.body.content).filters[0].value).toBe("example-device");
  expect(sent.environmentId).toBe("843e482b-09b7-44ce-9b80-a269ef2e90dc");
});
}
