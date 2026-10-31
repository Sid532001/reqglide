# Capability coverage ledger

This is the product-level acceptance ledger for Postman-class capability. It is
deliberately broader than the current implementation. A feature is not “covered”
because a screen or placeholder exists: it must execute correctly, persist and
migrate where applicable, have automated tests, expose useful errors, and work
without an account or mandatory cloud service.

Status legend:

- **Done** — usable end to end and covered by automated tests.
- **Partial** — a useful slice exists, but listed acceptance work remains.
- **Planned** — accepted product scope, not implemented yet.
- **Optional** — future adapter or commercial capability; it may never become a
  dependency of the local core.

Priorities are **P0** (local V1), **P1** (team-ready V1.x), **P2** (broader API
platform), and **P3** (optional ecosystem parity).

The inventory was refreshed on 2026-09-17 against the official Postman docs for
[request protocols](https://learning.postman.com/docs/use/send-requests/create-requests/request-basics),
[authorization](https://learning.postman.com/v11/docs/use/send-requests/authorization/authorization-types),
[responses and cookies](https://learning.postman.com/docs/use/send-requests/response-data/response-data),
[collection runs](https://learning.postman.com/docs/tests-and-scripts/running-collections/intro-to-collection-runs),
[imports](https://learning.postman.com/docs/getting-started/importing-and-exporting/importing-data),
and [enterprise roles](https://learning.postman.com/docs/administration/roles-and-permissions).

## Request construction and HTTP transport

| Capability | Priority | Status | Remaining acceptance work |
| --- | --- | --- | --- |
| HTTP/HTTPS methods and custom method | P0 | Done | Common methods are suggested and any valid custom HTTP token is editable in desktop and CLI. |
| URL, path variables, query parameters, duplicate query keys | P0 | Partial | Structured query editor is done; add a dedicated path-variable inspector. |
| Request headers and generated/temporary headers | P0 | Partial | Saved/generated-header preview, active-environment indicators, secret/missing variable badges, and override explanations work; add editable header descriptions and exact sent-request inspection. |
| Body: none and raw text/JSON/XML/HTML/JavaScript | P0 | Partial | Execution/persistence work; add syntax highlighting and format/validation. |
| Body: `application/x-www-form-urlencoded` | P0 | Done | Structured fields execute, persist, migrate, and resolve variables. |
| Body: multipart form data and file parts | P0 | Done | Text/file parts, per-part MIME and filename, native selection, variable resolution, bounded streaming, persistence, CLI, and cURL import are covered. |
| Body: binary/file upload | P0 | Done | Local files stream with a configurable payload limit, native selection, MIME controls, persistence, CLI, cURL import, and non-leaking diagnostics. |
| Body: GraphQL query and variables | P1 | Partial | Persisted profiles, separate query/variables/operation fields, execution, and schema introspection work; add autocomplete, schema-file import, subscriptions, and runner support. |
| Cookies and per-workspace cookie jar | P0 | Partial | Native isolated session jars, automatic send/capture, enable/disable, inspect, add from Set-Cookie, delete, and clear work; add persistence and import/export. |
| Redirect policy, timeout, maximum request/response size | P0 | Done | Shared enforcement and per-request desktop controls are persisted; CLI flags use the same engine. |
| Compression and content decoding | P0 | Partial | Engine supports common encodings; expose diagnostics and raw transfer view. |
| TLS validation and CA bundles | P0 | Done | Per-workspace custom CA trust and an explicit insecure-mode warning ship with validation enabled by default. |
| Client certificates / mTLS | P0 | Partial | PEM identity/mTLS works for HTTP, GraphQL, WebSocket, SSE, and gRPC; add encrypted-key/PFX support, passphrase storage, and host-specific selection. |
| HTTP proxy, SOCKS proxy, no-proxy rules | P1 | Partial | System/disabled/custom modes, HTTP or SOCKS URLs, basic proxy auth, variable resolution, and no-proxy rules work for HTTP-family transports; add per-request override and gRPC proxy support. |
| HTTP/1.1 and HTTP/2 controls | P1 | Planned | Protocol preference, negotiated-version diagnostics; evaluate HTTP/3. |
| Unix sockets and Windows named pipes | P2 | Planned | Local endpoint syntax and permission-safe transport adapters. |
| Request cancellation | P0 | Done | The active native network task and response stream are aborted from the Cancel control. |
| Multiple request tabs and duplication | P0 | Done | Tabs isolate editor/response/script state, support duplication and close protection, and restore debounced native crash snapshots. |
| Request history | P0 | Done | Bounded SQLite metadata/request snapshots, FTS5 search, 2,000-entry retention, pin, restore, delete, and clear are implemented. |
| Code generation and “Copy as cURL” | P1 | Partial | Deterministic, shell-safe PowerShell/POSIX cURL includes auth, query, body, files, timeout, and redirect policy; add cmd.exe and language snippets. |

## Authentication and secrets

| Capability | Priority | Status | Remaining acceptance work |
| --- | --- | --- | --- |
| No Auth | P0 | Done | — |
| Basic Auth | P0 | Partial | Execution/persistence and keychain variable references work; add guarded reveal control. |
| Bearer Token | P0 | Partial | Execution/persistence and keychain variable references work; add guarded reveal/copy. |
| API Key in header or query | P0 | Partial | Execution/persistence and keychain variable references work; add generated-value preview. |
| Inherit auth from collection/folder | P0 | Done | Requests resolve the nearest folder, collection, or workspace auth; parent scopes inherit by omission. |
| OAuth 2.0 and OpenID Connect | P0 | Partial | Client credentials, basic/body client authentication, scopes/audience/extra parameters, native token cache, and OIDC discovery work; add authorization code + PKCE, device code, refresh, and interactive browser UX. |
| OAuth 1.0 | P1 | Planned | HMAC/RSA signing, nonce/timestamp controls, header/query placement. |
| Digest | P1 | Planned | Challenge handling, qop/auth-int coverage, replay tests. |
| AWS Signature v4/v4a | P1 | Planned | Service/region inference, session tokens, payload signing, clock skew. |
| JWT generation | P1 | Planned | HS/RS/ES/PS algorithms, key files, claims, header/query placement. |
| NTLM / Negotiate / Kerberos | P2 | Planned | OS-specific secure credential integration and enterprise policy gates. |
| Hawk | P2 | Planned | Standards-compatible signing and payload hash tests. |
| Akamai EdgeGrid | P2 | Planned | Signing configuration and timestamp/nonce support. |
| Atlassian ASAP | P2 | Planned | JWT claims, private-key storage, audience and issuer validation. |
| Custom authentication plugins | P2 | Planned | Signed, permissioned local extension API; never arbitrary privileged scripts. |
| Local secret vault | P0 | Partial | OS credential-store references, transactional save, native resolution, machine-local non-secret values, and redaction work; add lock/unlock and backup/recovery policy. |
| External vault adapters | P3 | Optional | 1Password, AWS Secrets Manager, Azure Key Vault, HashiCorp Vault. |

## Response inspection and debugging

| Capability | Priority | Status | Remaining acceptance work |
| --- | --- | --- | --- |
| Status, headers, body, elapsed time, received bytes | P0 | Done | — |
| Text and binary-safe bounded response capture | P0 | Done | — |
| Pretty/raw/preview modes | P0 | Partial | JSON pretty/raw and CSP-locked sandboxed HTML preview work; add XML/YAML/JS/Markdown and hex viewers. |
| Search, filter, copy, line wrap, large-body virtualization | P0 | Partial | Find/count, bounded highlighting, copy, and wrapping work; add match navigation and virtualized rendering. |
| Response cookies and network/TLS details | P1 | Partial | Workspace cookie inspection and management work; add per-response cookie diffs, remote address, negotiated protocol, certificate chain, and timing phases. |
| Save response to file | P0 | Partial | Native save preserves UTF-8 or decoded binary bytes; add direct-to-disk streaming beyond the display limit. |
| Saved examples | P0 | Planned | Named request/response pairs, matching metadata, redaction. |
| Response visualizer | P2 | Planned | Sandboxed HTML templates with CSP and no privileged bridge access. |
| Network/request console | P1 | Partial | Script logs are visible; add request lifecycle, redirect chain, and variable/auth trace. |

## Workspaces, collections, and variables

| Capability | Priority | Status | Remaining acceptance work |
| --- | --- | --- | --- |
| Local workspaces with no account | P0 | Done | — |
| Collections, nested folders, saved requests | P0 | Done | — |
| Create folders from sidebar/tree | P0 | Done | — |
| Rename, move, duplicate, reorder, and delete | P0 | Partial | Rename, deep duplicate, confirmed delete, cycle-safe folder/request moves, and sibling reorder are conflict-safe; add recoverable undo/trash. |
| Request/collection descriptions and Markdown docs | P1 | Planned | Editor, preview, links, export. |
| Tags, favorites, and filtering | P1 | Planned | Persisted metadata and compound filters. |
| Search | P0 | Partial | Workspace-tree search and FTS5-backed request-history search work; add unified ranked search across scripts, examples, and protocol profiles. |
| Import/export portable workspace | P0 | Partial | YAML is portable; add packaged archive with referenced assets. |
| Atomic save and concurrent-edit detection | P0 | Done | — |
| Autosave, crash recovery, recent workspaces, file watching | P0 | Partial | Debounced native snapshots recover tabs and unsaved request edits; add recent-workspace UX and external file-watch/merge handling. |
| Git-friendly workflows | P0 | Partial | Human-readable YAML exists; add semantic diff/merge assistance. |
| Environments | P0 | Partial | Editor, active selection, Postman environment import, shared/local/secret storage, and YAML portability work; add duplicate/rename refinements and standalone export. |
| Global, collection, folder, environment, data, and local scopes | P0 | Partial | Environment, machine-local, secret, and collection-run data scopes work; add global/collection/folder scopes and a resolution trace. |
| Nested and dynamic variables | P0 | Partial | Nested values work; add random/date/data dynamic variables. |
| Secret/local-only variables | P0 | Partial | Values stay in the OS credential store and out of shareable YAML; storage changes preserve existing credentials, response extraction respects storage, and Send scripts update existing protected values natively. Add backup/recovery and organization policy. |
| Configuration inheritance | P0 | Partial | Workspace/collection/folder authentication inheritance works; add headers, scripts, variables, certificates/settings, and an override trace. |

## Scripts, tests, runners, and observability

| Capability | Priority | Status | Remaining acceptance work |
| --- | --- | --- | --- |
| Pre-request and post-response script source | P0 | Done | Persisted editors execute at the correct lifecycle points and feed request/response state. |
| Isolated JavaScript runtime | P0 | Partial | Fresh native QuickJS runtime, 2 s interrupt, 64 MiB heap, stack/context/source caps, and no host capabilities ship; add user cancellation, fuzzing, and async-job policy. |
| Postman-compatible `pm` API | P0 | Partial | Environment/local variables, replaceIn, request headers/method/URL/body/auth, response status/headers/body/JSON, info, console, tests, and expectations work; add cookies, execution control, scoped collection globals, and guarded sendRequest. |
| Assertions (`pm.test`, `pm.expect`) | P0 | Partial | Structured sync tests and common equality, deep equality, include, regex, range, truthiness, and property assertions work; complete Chai compatibility and source locations. |
| Script console and test-results view | P0 | Done | Phase-labelled bounded logs, pass/fail counts, errors, and duration are shown with each response. |
| Reusable scripts and approved packages | P1 | Planned | Workspace modules and locked allowlist; no implicit network install. |
| Collection/folder runner | P0 | Done | Deterministic tree order, iterations, delay, stop-on-failure, environment selection, inherited auth, cookies, scripts, and bounded execution are implemented. |
| CSV/JSON data-driven runs | P0 | Done | Native file selection, bounded parsing, row-to-iteration data scope, diagnostics, and secret-aware redaction are implemented. |
| Workflow control | P1 | Planned | Skip/next/repeat with loop and total-run safety limits. |
| CLI runner | P0 | Planned | Headless parity, stable exit codes, secret injection, cancellation. |
| JUnit, JSON, HTML, and console reports | P0 | Partial | Versioned local reports plus JSON and JUnit export work with redaction; add HTML and standalone console reporters. |
| Scheduled local monitors | P1 | Planned | OS scheduler/service adapter, retention, desktop notifications. |
| Self-hosted/private-network runners | P2 | Optional | Admin-managed runner with signed jobs and outbound-only control channel. |
| Performance/load testing | P2 | Planned | VUs, duration/ramp, thresholds, percentiles, bounded resource controls. |
| OpenTelemetry/CI result export | P2 | Planned | Opt-in local export; no product telemetry by default. |

## Protocols

| Capability | Priority | Status | Remaining acceptance work |
| --- | --- | --- | --- |
| REST over HTTP/HTTPS | P0 | Partial | The production-oriented core flow works; remaining partial HTTP rows above cover richer inspection and editor parity. |
| GraphQL | P1 | Partial | Persisted query/variables/operation profiles, native execution, shared auth/network policy, and introspection work; add autocomplete, schema files, subscriptions, and runner support. |
| WebSocket | P1 | Partial | Native connection lifecycle, shared headers/auth/cookies/proxy/TLS policy, text/base64-binary/ping/pong/close, cancellation, and bounded event logs work; add persisted message history and richer frame tooling. |
| Socket.IO | P2 | Planned | Namespace, event listener/emitter, acknowledgements, transports. |
| Server-Sent Events | P1 | Partial | Native incremental parsing, event IDs/names/retry fields, shared auth/network policy, bounded viewer, and cancellation work; add reconnect policy, search, and save. |
| gRPC | P1 | Partial | Reflection or FileDescriptorSet loading, JSON/protobuf conversion, metadata/trailers, unary and all streaming cardinalities, custom CA, and mTLS work; add interactive client-stream input, custom proxy/insecure-test mode, and persistence of results. |
| MQTT | P2 | Planned | Connect settings, pub/sub, QoS, retained messages, TLS. |
| SOAP | P1 | Planned | XML request templates, SOAPAction, WSDL import, auth integration. |
| MCP client requests | P3 | Planned | Local/remote transports, tool/resource/prompt inspection, permission UX. |
| AI model requests | P3 | Optional | Provider-neutral local request type only if it serves the core audience. |

## API lifecycle, migration, and developer output

| Capability | Priority | Status | Remaining acceptance work |
| --- | --- | --- | --- |
| cURL import | P0 | Partial | Common flags, URL-encoded data, auth, multipart/file references, uploads, and Markdown cleanup work; add multi-URL commands and a full unsupported-option report. |
| cURL export | P0 | Partial | Environment-resolved POSIX and PowerShell commands have round-trip coverage; literal multipart text uses form-string and unsupported per-part text content types fail explicitly. Add typed-text support, cmd.exe quoting, and an explicit shell chooser. |
| Postman collections/environments/globals | P0 | Partial | v2.x collection and environment import preserves nested folders, requests, common auth, variables, and pre/post scripts while scrubbing credential values; add export, saved examples, globals, and a fuller loss report. |
| Insomnia, Bruno, Hoppscotch, Thunder Client, SoapUI | P1 | Planned | Versioned adapters with migration reports and unsupported-field warnings. |
| HAR | P1 | Planned | Import/export requests, responses, cookies, and timing where representable. |
| OpenAPI/Swagger | P1 | Partial | OpenAPI 3.x and Swagger 2 JSON/YAML import generate organized requests with servers/base paths, parameters, request bodies, and common security mapping; add export, multi-file references, and exhaustive 3.1/schema coverage. |
| AsyncAPI, RAML, WSDL, WADL, GraphQL schema, protobuf, Smithy | P2 | Planned | Format-specific parsers and explicit loss reports. |
| API specification editor and linting | P2 | Planned | Local language services, Spectral-compatible rules, multi-file references. |
| Contract validation | P1 | Planned | Validate request and response against selected operation/schema. |
| Request generation from specs | P1 | Planned | Example/schema-derived values with deterministic regeneration. |
| Local mock server | P1 | Planned | Example matching, latency/errors, TLS, logs, CLI operation. |
| Static documentation export | P1 | Planned | Searchable HTML/Markdown with examples and auth guidance. |
| SDK and typed client generation | P3 | Optional | Reproducible local generators and Git output; separate from request core. |

## Team, enterprise, platform, and ecosystem

| Capability | Priority | Status | Remaining acceptance work |
| --- | --- | --- | --- |
| Local multi-user workflow through Git | P0 | Partial | Files are reviewable; add semantic merge and secret scanning. |
| Optional self-hosted synchronization | P2 | Optional | End-to-end encryption, offline queue, conflict model; core remains independent. |
| Comments, presence, live sessions, forks, pull requests | P3 | Optional | Adapter-level collaboration, not required for local operation. |
| Roles and permissions | P2 | Optional | Workspace/element roles for self-hosted organization mode. |
| SSO/SAML, OIDC login, SCIM, domain capture | P3 | Optional | Enterprise administration only; never required for standalone use. |
| Audit logs and policy packs | P2 | Optional | Signed, exportable local events and centrally distributed policies. |
| Secret scanning | P1 | Planned | Pre-save and CI scanner with custom patterns and suppressions. |
| API catalog, scorecards, and governance | P3 | Optional | Local catalog first; optional organization aggregation. |
| Public/private API network | P3 | Optional | Product decision; registry adapter must not couple the editor to a cloud. |
| Integrations and webhooks | P3 | Optional | Permissioned extension/event system before vendor-specific connectors. |
| Visual workflow builder / Flows | P3 | Optional | Product decision after runner and scripting foundations are stable. |
| Desktop installers and code signing | P0 | Partial | Unsigned Windows x64 NSIS and MSI bundles are verified; add organization-owned signing, macOS/Linux builds, provenance, SBOM, and reproducible release automation. |
| Secure updater and managed deployment | P1 | Planned | Signed channels, rollback, offline installer, admin configuration. |
| Backup/restore and diagnostics bundle | P0 | Partial | Tools exports a private, versioned saved-workspace backup and restores into a new identity without overwriting folders. Vault values and auxiliary files are excluded. Separate allowlisted diagnostics contain counts/configuration flags with no request data, credentials, or automatic upload. Packaged-app, interruption, and disk-full acceptance remain. |
| Accessibility and keyboard navigation | P0 | Partial | Semantic controls exist; complete WCAG audit, focus flow, contrast, shortcuts. |
| Localization, themes, editor preferences | P2 | Planned | Keep defaults compact and developer-focused. |
| Product telemetry | P3 | Optional | Off by default; explicit opt-in and inspectable payload if ever added. |

## Release gate

Local V1 is not complete until every **P0** row is either **Done** or has an
explicitly approved scope change. P1 and later work can ship incrementally, but
the application must never label a partial capability as Postman-compatible.
Every release updates this ledger and the smaller summary in
`docs/feature-matrix.md`.
