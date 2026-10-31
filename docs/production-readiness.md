# Windows release readiness

The current application is a V1 preview. Passing automated checks or producing
an installer does not establish production readiness. The Reqglide website,
MIT-licensed source, and clearly labeled unsigned community previews support
early evaluation and contribution. Production releases remain gated below.

## Hardening implemented

| Boundary | Failure addressed | Verification |
| --- | --- | --- |
| Variable resolution | A failed expansion budget could leave partially resolved request, network, protocol, or export values in use. | Desktop entry points reject resolver errors, cycles, and missing values. Local-server tests prove an expansion failure opens no HTTP connection. |
| Credential persistence | A credential read failing after writes could skip rollback. | Workspace validation and manifest persistence run inside the vault transaction. Tests cover post-write read failure, external manifest edits, rollback, successful persistence, and no secret values in YAML. |
| Credential identifiers | A deletion could target a UUID not present in workspace metadata. | Mutations must refer to protected variables in the previous or proposed workspace, as appropriate. A regression test verifies unrelated entries are preserved. |
| Command cancellation | A waiting save command could release its lock before its blocking worker completed. | The owned workspace lock and active-state update stay inside the blocking save worker. |
| HTTP deadlines | Each redirect could start a fresh timeout. | One deadline bounds redirects, proxy discovery, body preparation, sending, and response reads. A delayed redirect-chain test verifies the cumulative budget. |
| Redirect limits | Exhausting the redirect allowance could return a normal redirect response. | Exhaustion is an explicit error; a local-server test verifies the eleven-request bound for ten redirects. |
| Bearer input | Whitespace-only tokens could pass validation. | Validation rejects empty and whitespace-only tokens before transport. |
| First token generation | An unset protected variable blocked requests even when they did not use it. | Unset values stay unresolved. A token response can initialize the vault value; a request referencing a still-missing value is rejected before network I/O. |
| Backup/restore | A restore could overwrite an existing workspace or reconnect old device credentials. | Versioned, bounded backups restore into a new workspace identity; tests verify round trips, rejection of existing destinations, invalid/protected-value data, and oversized inputs. |
| Support exports | Serializing workspace data into diagnostics could disclose credentials or request data. | A typed field allowlist contains only app information, counts, and configuration flags. Sentinel-data tests verify sensitive strings are absent; no vault access or upload occurs. |

Threat model: imported manifests, request variables and scripts are untrusted
inputs. They must not bypass expansion bounds or cause partial requests to be
sent. Vault operations can fail at any read or write. Disk contents may change
outside the app. Cancelling a waiting future must not allow overlapping vault
and manifest mutations. Remote servers can redirect indefinitely or delay each
hop to prolong execution. All regression fixtures use temporary workspaces,
an in-memory credential store, and local HTTP servers, never real credentials.

Rollback covers handled failures; it is not a power-loss transaction spanning
the OS credential store and filesystem. Process termination and rollback
backend failures remain part of recovery and operational acceptance testing.

Backup exports are private, unencrypted copies of saved workspace configuration,
including Shared values and literal credentials. They exclude Secret/Local
values, history, cookies, unsaved tabs, uploads, and certificate files. See the
[backup and support guide](backup-and-support.md) for the supported scope and
restore steps. Browser tests cover export disclosures, cancellation, unsaved-tab
preservation, failed/successful restores, and diagnostics without a workspace;
native file dialogs still require packaged-app acceptance.

## Build an internal Windows candidate

Local verification on 2026-10-30 passed 144 Rust tests, 83 frontend unit tests,
and 22 browser workflows. Rust formatting, strict Clippy, TypeScript checks,
and the production frontend build passed. Both NSIS and MSI packages were
built locally and their manifest checksums verified. The browser suite uses
simulated native IPC; this is not packaged-app or clean-machine acceptance.
That initial local verification predates the source publication. Community
preview builds now require a clean, committed source checkout.

```powershell
pnpm install --frozen-lockfile
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
pnpm test
pnpm test:e2e
./scripts/build-windows.ps1 -Preview
```

The script builds NSIS and MSI installers, checks version consistency, records
Authenticode status, and creates `.release-artifacts/windows/0.1.2/` containing
the installers, `manifest.json`, and `SHA256SUMS.txt`. Artifacts are explicitly
classified as `internal-preview`. They are not automatically uploaded or
published. The manifest records source commit/cleanliness and whether a build
was performed. `-Preview -SkipBuild` only inspects existing packages; it must not
be represented as a fresh build.

## Rename compatibility

Reqglide retains the original application identifier
`dev.localfirst.apiclient`, credential-service namespace, local storage keys,
workspace paths/format, backup format, Rust crate names, `@local-api` package
names, and the `apic` CLI command. New workspace files use a Reqglide comment banner; older manifests
continue to load. The MSI upgrade code is pinned to the code emitted by the
previous API Client preview (`8f473f52-f229-57b6-a782-976bea1737de`).

Installer labels, shortcuts, output filenames, and the release manifest now use
Reqglide. Packaged upgrades across the rename still require acceptance testing.
NSIS registry detection uses the product name; until a migration is validated,
uninstall the old API Client preview before installing the Reqglide NSIS preview
and preserve its application data. Existing workspaces can be opened directly.

## Signing configuration

A publisher certificate or signing service has not been obtained. This remains
a production-release blocker. Unsigned community previews must be disclosed
as evaluation builds. Do not embed signing passwords, private keys,
or certificates in this repository or pass them through chat.

For a certificate already available in the Windows current-user certificate
store, run from a clean, committed checkout:

```powershell
./scripts/build-windows.ps1 -CertificateThumbprint '<publisher certificate thumbprint>' -TimestampUrl '<provider timestamp URL>'
```

This path refuses missing signing configuration, skipped builds, dirty or
uncommitted sources, invalid signatures, unexpected signers, and missing
timestamps. Successful artifacts are `signed-candidate`, not automatically
approved production releases. Signing-service integrations may need a
provider-specific Tauri signing command rather than a certificate-store key.

The manually triggered `Windows release candidate` workflow builds either an
internal preview or a signed candidate and retains workflow artifacts under
the repository's access policy. For its certificate-store signing path,
configure `WINDOWS_CERTIFICATE` (base64 PFX) and
`WINDOWS_CERTIFICATE_PASSWORD` as repository secrets, and
`WINDOWS_TIMESTAMP_URL` as a repository variable. The PFX import runs on the
ephemeral Windows runner; its temporary file is removed. The workflow does not
create a public GitHub Release. GitHub execution remains unverified until this
source is committed and pushed to a configured repository.

Configuration reference: [Tauri Windows code signing](https://v2.tauri.app/distribute/sign/windows/).

## Community preview and website

The repository's existing MIT license is preserved, and workspace package
metadata and installer license displays use MIT. To prepare an unsigned
evaluation release from clean, committed source:

```powershell
./scripts/build-windows.ps1 -CommunityPreview
node scripts/site.mjs capture-release
pnpm site:build
```

The artifact manifest identifies this as `community-preview` and records its
source commit. It cannot skip the build or use dirty source. Community previews
are published as GitHub prereleases with checksums and an unsigned-build notice.
The website uses versioned release links; its GitHub Pages workflow publishes
only static website files, without installer binaries in Git history. Local
website builds include verified installers when available.

## Gates still required before a production release

The current dependency findings and patched versions are recorded in
[`dependency-audit.md`](dependency-audit.md). The latest advisory scan has zero
vulnerability findings, with maintenance and non-Windows GLib warnings still
requiring review; it is not a blanket security certification.

- [ ] Publisher signing identity and valid, timestamped Windows signatures.
- [ ] Reviewed release source/version/tag and successful remote CI tied to the
      packaged commit. Local checks do not replace remote or packaged acceptance.
- [ ] Clean Windows 10/11 install, launch, upgrade, uninstall, and reinstall
      acceptance using the packaged application and a non-developer account.
- [ ] Real Windows Credential Manager success/failure/prompt tests; organization
      VPN, PAC/custom proxy, CA, mTLS, and OAuth acceptance with test accounts.
- [ ] Dependency advisory audit, third-party notices/SBOM, and license/commercial
      terms review. The project is MIT licensed; dependency licenses retain their
      own terms and require a separate notices/SBOM review.
- [ ] Backup/restore and recovery acceptance, including interrupted writes,
      missing credentials, disk-full cases, and rollback failures.
- [ ] Packaged-app diagnostics acceptance, a private vulnerability-reporting
      route, and a documented support/rollback process. Local diagnostics export
      and its privacy boundary are implemented and covered by automated tests.
- [ ] Accessibility audit and longer performance/memory soak tests across
      representative workspaces and large responses.

The feature coverage ledger remains authoritative for unfinished capabilities.
An initial public release needs a documented supported scope rather than a
claim of full Postman compatibility. A signed updater, broader auth flows,
CLI collection runs, and optional organization policies are subsequent work;
none should introduce a cloud dependency into local request execution.
