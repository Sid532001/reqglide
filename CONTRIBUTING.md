# Contributing to Reqglide

Reqglide is a local-first API client, licensed under MIT. Contributions to code,
documentation, accessibility, and developer workflows are welcome.

## Start with the workflow

Check existing [issues](https://github.com/Sid532001/reqglide/issues), the
[roadmap](docs/roadmap.md), and the [capability ledger](docs/capability-coverage.md).
For a substantial change, open a feature proposal describing the problem and
intended behavior before investing in an implementation. Small fixes and
documentation improvements can go straight to a pull request.

Bug reports should include the app version, OS, reproduction steps, expected
behavior, and a minimal example. Remove credentials, cookies, private endpoint
details, and personal data. Report security vulnerabilities according to
[SECURITY.md](SECURITY.md), rather than posting exploit details publicly.

## Run the desktop app

Install Node.js 24+, pnpm 11.19, stable Rust, and your platform's Tauri 2 build
prerequisites. Windows development uses MSVC build tools and WebView2.

```shell
git clone https://github.com/Sid532001/reqglide.git
cd reqglide
pnpm install --frozen-lockfile
pnpm dev
```

The desktop app uses port 1420. The separate product website uses port 4173:

```shell
pnpm site:dev
```

Website source is in `apps/site`. It uses HTML, CSS, and browser JavaScript,
without a frontend framework, external fonts, analytics, or a backend. Build
with `pnpm site:build`; output is `apps/site/dist`. On a fresh checkout it links
to the versioned GitHub release. Local verified installer artifacts, when
available, are copied into the generated site for local download checks.

## Validate your change

Run the checks relevant to the affected boundary:

```shell
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
pnpm check
pnpm test
```

For interface changes, install the test browser and run the workflow suite:

```shell
pnpm --filter @local-api/desktop exec playwright install chromium --only-shell
pnpm test:e2e
```

Use synthetic credentials and local test servers. Browser workflows simulate
native IPC and do not replace packaged-app acceptance. For website changes,
check desktop/mobile navigation, keyboard access, examples, docs, and download
links. See [quality notes](docs/quality.md) and [release gates](docs/production-readiness.md).

## Pull requests

Keep the scope focused. Explain the problem, resulting behavior, and relevant
validation. Document compatibility changes and limitations. Avoid committing
local workspaces, backups, generated output, installer binaries, or credentials.
Preserve the local-first execution boundary: core request execution must not
depend on a hosted account, telemetry service, or cloud workspace.

Be respectful, describe concrete behavior, and assume good intent in review.
By contributing, you agree to license your contribution under this project's
MIT license. Third-party dependencies retain their own licenses.
