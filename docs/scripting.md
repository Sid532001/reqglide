# Request scripting

Pre-request and post-response scripts are stored with each saved request and
run locally when **Send** is selected. They never execute in the desktop
webview. Each phase receives a fresh embedded QuickJS runtime with:

- a 2-second execution deadline enforced by the engine interrupt handler;
- a 64 MiB JavaScript heap limit and 512 KiB stack limit;
- a 256 KiB source limit and 12 MiB serialized context limit;
- no filesystem, network, process, native module, dynamic module loader, or
  Tauri desktop bridge;
- at most 200 captured console entries, each truncated to 4,096 characters.

Pre-request changes are applied before `{{variable}}` resolution and HTTP
validation. Post-response scripts receive the final resolved request and the
bounded response snapshot. `pm.variables` changes are per-run local scope only
and never leave that one Send lifecycle. `pm.environment.set()` changes to
plain (non-secret, non-local) variables are written back to the active
environment and saved to `workspace.yaml` after the send completes, so a
later, separately-sent request can resolve the new value with `{{variable}}`.
Updates to existing Secret and Local variables are saved natively to the OS
credential store before Send completes. Their values are excluded from the
environment updates returned to the interface and from workspace.yaml. A value
edited in the environment editor while a request was running is preserved,
and the script save reports the conflict. Create a Secret `access_token`
variable before using the following token-response script:

```javascript
const token = pm.response.json().access_token;
pm.environment.set("access_token", token);
```

New variables created by scripts use Shared storage; pre-create sensitive
variables as Secret, or use **Extract variable** in the response viewer, which
defaults new token/credential variables to Secret storage. During collection
runs, updates to existing Secret and Local variables are also saved after each
response. Shared variable changes remain scoped to that collection run.

`pm.environment.unset()` still
only affects the current Send lifecycle; it does not remove the variable from
the saved environment.

## Available API

The initial compatibility surface includes:

```javascript
pm.environment.get("baseUrl");
pm.environment.set("token", "value");
pm.environment.unset("token");
pm.environment.replaceIn("{{baseUrl}}/health");

pm.variables.get("name");
pm.variables.set("requestOnly", "value");
pm.collectionVariables.get("name"); // currently aliases run-local variables

pm.request.method = "POST";
pm.request.url = "{{baseUrl}}/users";
pm.request.headers.get("Content-Type");
pm.request.headers.add({ key: "X-Trace", value: "local" });
pm.request.headers.upsert({ key: "Authorization", value: "Bearer {{token}}" });
pm.request.headers.remove("X-Trace");

console.log("message", { structured: true });

pm.test("created", () => {
  pm.response.to.have.status(201);
  pm.expect(pm.response.json()).to.have.property("id");
  pm.expect(pm.response.headers.get("Content-Type")).to.include("json");
});
```

`pm.expect` currently supports chain words plus `not`, `ok`, `true`, `false`,
`null`, `undefined`, `equal`/`eq`, `eql`, `include`, `match`, `above`, `below`,
and `property`. Test failures are isolated and reported without hiding the HTTP
response.

## Deliberate current limits

- Top-level promises and asynchronous `pm.test` callbacks are rejected.
- `pm.sendRequest`, cookies, `pm.execution`, package imports, and external
  libraries are not enabled yet.
- Collection variables currently use the per-run local scope rather than a
  separately persisted collection scope.
- Pressing Cancel during the HTTP phase aborts the native network task. Script
  phases are deadline-interrupted but do not yet react to the Cancel button.
- This is an intentionally scoped compatibility layer, not a claim of complete
  Postman Sandbox or Chai compatibility.
