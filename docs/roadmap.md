# Delivery roadmap

The product is built as tested, local-first, end-to-end slices. “Complete” below
means the stated slice works; it does not mean complete Postman parity. The
requirement-level truth is maintained in
[`capability-coverage.md`](capability-coverage.md).

## Milestone 1: HTTP vertical slice — complete

- Request method, URL, headers, query parameters, and all core body modes
- Shared bounded Rust HTTP execution engine and CLI request command
- Cancellation, redirects, timeout, request/response limits, and response views
- Typed Tauri boundary with no mandatory account or cloud dependency

## Milestone 2: Local workspace foundation — complete

- Schema-v7 Git-friendly YAML with v1–v6 in-memory migration
- Collections, nested folders, saved requests, creation, rename, deep duplicate,
  move, reorder, confirmed delete, and search
- Atomic writes, conflict detection, bounded validation, and native dialogs
- Multiple request tabs and debounced machine-local crash recovery
- SQLite request history with search, pin, restore, delete, and clear

Remaining refinements: recent-workspace UX, external file watching/merge, undo or
trash, tags/favorites, saved examples, and ranked full-text search.

## Milestone 3: Environments, authentication, and networking — V1 complete

- Shared, machine-local, and secret environment values with nested native
  `{{variable}}` resolution
- OS-credential-store persistence for local/secret values and output redaction
- No Auth, Basic, bearer, API key, and hierarchical auth inheritance
- OAuth 2.0 client credentials with token caching and optional OIDC discovery
- Isolated cookie jars, system/custom/disabled proxy, no-proxy rules, custom CA,
  PEM client identity/mTLS, and explicit insecure-certificate mode

Remaining parity work: global/collection/folder/dynamic variable scopes and
resolution trace; OAuth authorization-code + PKCE, device and refresh flows;
PFX identities; digest, AWS SigV4, OAuth 1.0, JWT and enterprise auth schemes.

## Milestone 4: Testing and automation — desktop slice complete

- Persisted pre-request and post-response scripts
- Native resource-limited QuickJS runtime and initial Postman-style `pm` API
- Synchronous tests/assertions with console and result views
- Collection/folder runs with environment, iterations, delay, stop-on-failure,
  CSV/JSON data rows, persistent reports, and JSON/JUnit export

Remaining automation work: broader `pm`/Chai compatibility, async APIs,
workflow control, HTML reports, and a CI-grade collection runner in the CLI.

## Milestone 5: Protocols and migration — initial slice complete

- cURL import/export, including cleanup of common Markdown copy corruption
- Postman collection/environment import and OpenAPI/Swagger JSON/YAML import
- GraphQL query/variables and schema introspection
- WebSocket session lifecycle with text, binary, ping/pong, and bounded event log
- Server-Sent Events streaming and cancellation
- Dynamic gRPC via reflection or descriptor set, metadata/trailers, TLS, and
  unary/server-stream/client-stream/bidirectional-stream methods

Remaining protocol/lifecycle work: richer editors and persisted histories,
GraphQL subscriptions/autocomplete, SSE reconnect/search/save, custom-proxy
support for gRPC, additional import/export formats, contract validation, saved
examples, local mocks, and static documentation export.

## Milestone 6: Release and organization hardening — next

- [x] Verified unsigned Windows NSIS installer
- [x] Verified unsigned Windows MSI installer
- [ ] Organization-owned Windows code signing and release provenance
- [ ] macOS signing/notarization and Linux packages
- [ ] SBOM, reproducible release pipeline, secure updater, rollback, and offline
      deployment documentation
- [x] Local saved-workspace backup/restore and allowlisted support diagnostics;
      see [supported scope](backup-and-support.md)
- [ ] Backup/restore interruption and packaged-app acceptance, accessibility/
      keyboard audit, secret scanning, performance soak tests, and support playbooks

Optional self-hosted synchronization, policy distribution, organization
administration, and enterprise identity remain adapters around the local core.
They must never become dependencies of request execution or local workspaces.
