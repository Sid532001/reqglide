# ADR 0001: Tauri shell with a reusable Rust core

- Status: Accepted
- Date: 2026-09-15

## Context

The application must run locally without an account, handle credentials and
untrusted API responses safely, support multiple protocols, and expose exactly
the same request behavior through a desktop UI and a CI-friendly CLI.

## Decision

Use Tauri 2 as the desktop adapter, React and TypeScript for presentation, and a
Rust workspace for domain, transport, persistence, security, scripting, and CLI
capabilities.

The React application communicates through narrow typed commands. It does not
perform API requests directly. Core crates have no dependency on Tauri so they
can be embedded in the CLI, tests, and future adapters.

User-owned collection data will use versioned, deterministic text files. SQLite
will be reserved for local operational state such as history, recovery data,
indexes, and run results.

## Consequences

- Networking and business behavior can be tested without launching a webview.
- Desktop and CLI execution cannot silently diverge.
- Privileged capabilities remain outside the renderer.
- Rust increases initial implementation cost.
- The UI must be tested against each operating system's webview.
- Replacing the desktop shell remains possible because the core and React UI do
  not depend on Tauri internals outside a small adapter module.
