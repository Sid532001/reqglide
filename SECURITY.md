# Security policy

This project handles credentials, private API traffic, certificates, scripts,
and files. Security changes therefore require explicit threat modelling and
tests around the affected trust boundary.

## Baseline rules

- The webview cannot directly access the filesystem, process APIs, secrets, or
  the network execution engine.
- Tauri commands expose narrow operations and validate every input in Rust.
- Workspace files can contain request authentication fields and **Shared**
  environment values as plain text. Sensitive values must be marked **Secret**;
  visible device-specific values can be marked **Local**. Both are stored in the
  operating-system credential manager and only UUID-bound metadata remains in
  `workspace.yaml`.
- Credential lookup, interpolation, scripts, HTTP/protocol execution, OAuth token
  acquisition, and TLS identity loading stay inside the native boundary. Script
  logs, test failures, protocol events, and transport errors are redacted.
  Explicit export actions such as **Copy cURL** may place resolved credentials on
  the clipboard and must show a disclosure warning.
- Logs and errors must redact authorization, cookie, and configured secret
  values.
- Response HTML is data and must never execute in the privileged application
  context.
- User scripts run in a fresh embedded QuickJS runtime with memory, stack,
  source, context, and execution-time limits and no host capabilities. Process
  isolation remains a defense-in-depth item for the production security audit.
- Certificate validation cannot be disabled without a persistent visual
  warning in the request and response views.
- OAuth tokens, cookie jars, crash snapshots, history, and runner reports are
  machine-local operational state and must not be placed in shareable workspace
  manifests. History and reports must omit or redact credential material.

## Reporting vulnerabilities

Until a private disclosure address is configured, do not publish a suspected
credential-exposure or remote-code-execution issue. Contact the repository
owner privately.
