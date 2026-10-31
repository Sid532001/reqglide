# Dependency advisory review — 2026-10-30

The production JavaScript audit (`pnpm audit --prod --audit-level high`) reported
no known vulnerabilities. RustSec `cargo-audit` 0.22.2 initially reported 11
vulnerability advisories and additional informational warnings against the
checked-in dependency resolution. Compatible updates were applied to the
lockfile; no advisory was added to an ignore list.

| Package | Previous | Updated |
| --- | --- | --- |
| ctutils | 0.4.2 | 0.4.7 |
| rustls | 0.23.45 | 0.23.46 |
| rustls-webpki | 0.103.15 | 0.103.16 |
| webpki-root-certs | 1.0.9 | 1.0.11 |
| tokio | 1.53.1 | 1.54.1 |
| zstd | 0.14.0 | 0.14.3 |
| zstd-safe | 8.0.0 | 8.2.0 |
| zstd-sys | 2.1.0+zstd.1.5.7 | 2.1.2+zstd.1.5.7 |

The subsequent RustSec scan reported **zero vulnerability advisories**. This
does not mean the dependency graph is warning-free or that the software has
been independently security certified.

## Remaining informational findings

- GLib 0.18.5 has `RUSTSEC-2024-0429`, an unsound iterator implementation fixed
  upstream in GLib 0.20. The Windows target graph contains no GLib package
  (`cargo tree --locked --target x86_64-pc-windows-msvc --invert glib` prints no
  dependency path). The older GTK binding is part of the non-Windows desktop
  dependency graph. Do not force a semver-incompatible replacement without
  checking the Tauri/GTK integration. Linux production distribution remains
  gated on resolving and testing that upstream dependency path.
- Ten maintenance warnings remain: `proc-macro-error`, `unic-char-property`,
  `unic-char-range`, `unic-common`, `unic-ucd-ident`, `unic-ucd-version`,
  `unsafe-libyaml`, `zbus_names`, `zvariant`, and `zvariant_derive`. Each requires
  an upstream replacement/upgrade assessment. These warnings are not silently
  ignored, and a zero vulnerability count must not be presented as their removal.
- `unsafe-libyaml` is relevant to the YAML parsing boundary. Bounded inputs and
  workspace validation remain necessary; parser maintenance, fuzzing, and a
  migration assessment belong in the release security review.

CI and Windows candidate workflows now run both advisory checks. Registry and
advisory-database access is a build-time requirement, not a runtime dependency
of the desktop client. Re-run scans before each release because advisory data
changes independently of the source. Remote workflow execution, third-party
notices, SBOM generation, license review, and independent security acceptance
remain outstanding.
