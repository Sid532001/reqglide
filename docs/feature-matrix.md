# Product feature matrix

This matrix prevents “Postman parity” from becoming an ambiguous claim. A
feature is marked complete only when it is usable in the desktop app, covered
by tests, persisted where applicable, and available to the shared engine or
CLI when that feature participates in automation.

The detailed, requirement-level inventory is maintained in
[`capability-coverage.md`](capability-coverage.md). It includes current Postman
protocols, auth schemes, runners, lifecycle tooling, and optional enterprise
capabilities; this table is only the release summary.

| Area | Available now | Remaining |
| --- | --- | --- |
| HTTP execution | Common/custom methods, URL, structured query parameters, raw, URL-encoded, multipart, and streamed binary/file bodies, headers, cancellation, redirects/limits, isolated cookie jars, system/custom proxy and no-proxy rules, custom CA, PEM mTLS identity, explicit insecure TLS mode | Path-parameter inspector, generated-header/TLS trace, PFX/PKCS#12 identity, per-request network override |
| cURL | Paste into URL or workspace search, explicit request importer, common browser-copy flags, `--data-urlencode`, multipart/file/upload flags, Markdown-copy cleanup, safe warnings, PowerShell/POSIX export | Multi-URL commands, cmd.exe-specific export |
| Workspaces | Local folders, schema-v7 YAML, atomic writes, migrations, conflict detection, multiple tabs, debounced machine-local crash recovery | Recent-workspace picker, external file watching and merge UX |
| Organization | Collections, nested folders, request saving, in-memory search, rename, deep duplicate, confirmed delete, request/folder move, sibling reorder, searchable/pinnable SQLite history | Undo/trash, tags, favorites, saved examples, SQLite FTS ranking |
| Environments | Shareable, machine-local, and secret values; active selector; nested native `{{variable}}` resolution; OS credential-store persistence; redaction | Global/collection/folder/dynamic scopes, resolution trace, vault lock/backup policy |
| Scripts | Persisted pre/post editors, resource-limited native QuickJS execution, initial `pm` API, sync tests/assertions, console/result views, collection-run execution | Broader `pm`/Chai compatibility, async APIs, package allowlist, workflow control |
| Authentication | No Auth, Basic, bearer, API key, workspace/collection/folder inheritance, OAuth 2.0 client credentials with optional OIDC discovery, mTLS via network profile | OAuth authorization-code/PKCE, device/refresh flows, digest, AWS SigV4, OAuth 1.0, JWT, Hawk, NTLM/Kerberos, EdgeGrid, ASAP |
| Testing | Per-request assertions; collection/folder runner; iterations, delay and stop-on-failure; CSV/JSON data; retained local reports; JSON/JUnit export | CLI runner, async tests, workflow control, HTML/console report export |
| Protocols | HTTP/HTTPS REST; GraphQL query/variables/introspection; WebSocket lifecycle and text/binary/ping; SSE stream viewer; dynamic gRPC reflection/descriptors and all RPC cardinalities | GraphQL autocomplete/subscriptions, persisted message/event history, SSE reconnect/search/save, Socket.IO, MQTT, SOAP, MCP |
| API lifecycle | OpenAPI/Swagger JSON/YAML import and request generation | Contract validation, specification editor, examples, local mocks, documentation export |
| Migration | cURL, Postman v2.x collection/environment, and OpenAPI/Swagger import | Postman export/examples/globals, Insomnia, Bruno, Hoppscotch, HAR, SoapUI |
| Distribution | Development launch plus verified unsigned Windows NSIS and MSI installers | Code signing, macOS/Linux packages, updater, notarization, SBOM/provenance, managed deployment |
| Enterprise | Local operation without an account | Policy packs, audit controls, optional self-hosted sync, organization administration |

Cloud services remain optional adapters. None of the request execution,
workspace, environment, scripting, testing, or CLI paths may require a cloud
account.
