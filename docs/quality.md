# Verification and release quality

The application is a local-first V1 under active development. Passing checks
establishes coverage for the scenarios below, not complete Postman parity or
the absence of bugs. The remaining product scope lives in
[capability-coverage.md](capability-coverage.md).

## Repeatable checks

```shell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm check
pnpm test
pnpm build
pnpm test:e2e
```

Before the first browser run, install its browser with
`pnpm --filter @local-api/desktop exec playwright install chromium --only-shell`.
The browser suite uses port 1431, separate from the desktop development port
1420, and starts its own development server. Failed checks preserve a
screenshot and trace under `apps/desktop/test-results`.

## Coverage added in the October 2026 reliability pass

- Chromium exercises multipart typing, environment storage changes, existing
  and new secret-token extraction, overlapping variable saves, cancellation
  followed by a new send, sidebar folder creation, cURL paste, and import-error
  recovery. These run the real React app with simulated native dialogs,
  credentials, disk storage, and HTTP responses.
- Native local-server tests cover token-response scripting followed by a
  separate authenticated send, same-origin credential retention, stripping
  credentials on cross-origin redirects, redirect-body handling, bounded
  execution time, redirect limits, uploads, response limits, and protocol
  behavior. No company API or real credentials are needed for these tests.
- Request-framing checks import a PowerShell cURL JSON search and capture the
  actual HTTP method, path, Host, single Authorization header, Content-Type,
  Content-Length, and body bytes. Browser checks expose imported content types
  accurately and repair a JSON/form mismatch through either the format selector
  or the suggested correction, preserving the URL and token reference.
- URL-bar paste checks cover multiline commands, shell prompts, code fences,
  import progress/errors, token references, query parameters, draft preservation,
  and the requirement that paste never sends a request automatically.
- A large-response workflow checks complete response content, editable URLs,
  debounced recovery, and records typing duration and input-to-frame latency.
  Response syntax rendering is bounded, immutable response snapshots are reused,
  and recovery serialization runs after typing pauses. Desktop/compact layout
  checks exercise command search, both splitters, overflow, and reduced motion.
- Secret-store tests simulate read/write failures, partial writes, failed
  workspace commits, and failed rollbacks. They check restoration and error
  reporting without changing the developer's actual keychain.
- Deferred-save tests check that queued changes use the latest workspace,
  preserve newer request edits, recover after save errors, and wait before the
  next Send. Variable resolution checks distinguish enabled and disabled rows.
- Nested variable resolution stops at 64 levels, 100,000 substitutions, or
  16 MiB cumulative expansion work. It returns an error before network calls or
  exports; large literal bodies remain governed by the request-size limit.
- SSE checks cover response-header deadlines, unfinished lines and events,
  split UTF-8, all line-ending forms, and EOF. Incomplete lines are buffered
  within the event limit before parsing, avoiding repeated rescans.

The local Windows verification on 2026-10-26 passed 134 Rust tests, 76 frontend
tests, and 9 Chromium workflows, plus strict Clippy, Rust formatting,
TypeScript, frontend production build, and an unsigned Tauri release binary.

The CI workflow runs Rust and frontend checks on Windows, macOS, and Linux,
and browser workflow checks on Linux. A local Windows pass does not establish
that the other platforms have passed; CI must run on each platform.

## Windows corporate proxy fix (2026-10-30)

The HTTP executor now resolves Windows PAC/WPAD proxy configuration when System
proxy mode is selected. This addresses corporate destinations that time out on
direct connections while succeeding in clients configured with a corporate
proxy. Explicit custom/disabled settings and environment proxy overrides remain
authoritative. Discovery uses the destination origin, runs on a bounded
blocking worker, and releases WinHTTP allocations and handles.

Regression checks cover raw bearer tokens, environment references, and manual
Authorization headers through a local proxy, plus timeout-stage reporting,
proxy settings access, token reuse in a new request, and editable header focus.
Success notifications no longer intercept clicks on the New request control.
See the README's corporate network troubleshooting section for configuration.

Verification for this fix passed 22 HTTP/PAC tests, 78 frontend tests, and all
12 Chromium workflows, with strict HTTP Clippy, formatting, and TypeScript
checks. A live unauthenticated HEAD check through Windows' selected PAC route
reached the previously timing-out destination in approximately 1.8 seconds.
No real token or saved API request body was sent during that connectivity check.

## Network-error clarification (2026-10-31)

Connection-setup failures, response-header deadlines, response-body failures,
and proxy-discovery failures have distinct user-facing guidance. They do not
claim that a bearer token was rejected without an HTTP response. Native timeout
text and network settings no longer reference a competing client; known legacy
messages in recovered tabs/history are normalized for display. Authentication
wire tests continue to cover raw tokens, environment references, duplicate
Authorization headers, and redirects using synthetic credentials and local
servers. Restoring an unavailable destination's network route is separate from
changing request credentials.

## Workspace startup recovery (2026-10-31)

Reqglide stores only the active workspace's native path and UUID in
`last-workspace.json` under its device-local application-data directory. Opening,
creating, or restoring a workspace remembers that location. Startup validates
the workspace document and remembered identity before activating it; it never
sends a request, runs a script, or starts a protocol session. Existing recovery
snapshots restore available tabs and the active environment automatically.
Recovery snapshots use the existing debounced persistence and redaction rules.

Explicit **Close workspace** atomically clears the startup preference while
leaving workspace files intact. An unavailable, invalid, or replaced workspace
shows a non-blocking notice and leaves manual Open available. Startup waits at
most five seconds for its operation lock and workspace read; a late read worker
cannot change the active workspace. A location-save failure does not prevent use
of an already-open workspace. The startup screen prevents editing while the
previous workspace is being loaded.

Native temporary-directory checks cover metadata-only persistence, restart
loading, latest-workspace replacement, explicit close, missing/replaced folders,
corrupt/oversized settings, and relative-path rejection. Browser checks cover
tab/environment restoration without a picker or execution, missing-folder
fallback, explicit close, and a single startup command under React StrictMode.

## Release acceptance still required

Actual OS credential-manager prompts and access failures, native file dialogs,
packaged-app lifecycle, installer upgrades/uninstall, organization-specific
proxies and certificates, and corporate OAuth providers need testing on the
target machines. The automated browser suite cannot replace those native
acceptance checks. Backup/restore and private diagnostics now have native and
browser regression coverage; their packaged-app and interruption acceptance
remain. Signed distribution and the feature gaps listed in the capability
ledger also remain release work. See [backup and support](backup-and-support.md).

cURL export currently rejects a custom content type on literal multipart text
and file-part metadata/path characters that curl would interpret as form
syntax. HTTP sending still supports those body types; export reports a clear
error rather than changing which local file is uploaded.
