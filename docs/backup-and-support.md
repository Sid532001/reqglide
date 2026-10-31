# Workspace backups and support diagnostics

These features work locally from the **Tools** menu. No account or cloud
service is required, and exports are never uploaded automatically.

## Back up a workspace

1. Save any request edits you want included. Unsaved request tabs are excluded.
2. Choose **Tools → Back up workspace** and review the contents description.
3. Choose a trusted destination for `workspace.apic-backup.json`.

The backup includes the saved workspace configuration: collections, nested
folders, requests, authentication configuration, scripts, environment metadata,
Shared variable values, protocol profiles, and network settings.

**This is a private, unencrypted file.** Credentials entered directly into
request fields or network settings, and credentials placed in Shared variables,
are part of that saved configuration. Keep the backup under your organization's
access and storage rules. Use Secret or Local variables for values that should
remain device-only.

Secret and Local variable **values** stay in the OS credential store. The backup
retains their variable definitions without their values. Request history,
cookies, unsaved/recovery tabs, saved response files, uploaded files, and
certificate files are not included. Referenced file paths are retained.

The export uses an atomic file replacement. The backup format is versioned and
limited to 32 MiB. This feature complements a managed device backup; it does not
back up the credential store or guarantee recovery after hardware failure.

## Restore a backup

1. Choose **Tools → Restore workspace backup**.
2. Select the backup, then select a parent folder for the restored workspace.
3. Re-enter Secret and Local variable values through **Tools → Environments**.
4. Check upload/certificate paths and review restored requests and scripts
   before sending anything.

Restore validates the backup before creating a workspace. It creates a folder
using the workspace's name inside the chosen parent. An existing folder at that
location is rejected; select a different parent instead. A restored workspace
receives a new identity so it cannot silently attach the original workspace's
device credentials, cookies, or history. Requests are not sent during restore.

Invalid, oversized, unsupported-version, or invalid-workspace backups are
rejected. If validation or creation fails, the currently open workspace remains
selected. Cancelling either file dialog leaves the workspace unchanged.

Interrupted writes, disk-full failures, OS credential-store recovery, and
process termination still need acceptance testing on deployment machines.

## Export support diagnostics

Choose **Tools → Export support diagnostics**, review the description, and save
`support.apic-diagnostics.json`. This works even when no workspace is open.

The export is built from a fixed field allowlist:

- App version, OS family, architecture, and debug/release build flag.
- Counts of collections, environments, protected variables, and protocol profiles.
- Proxy mode, whether CA/client identity files are configured, and whether TLS
  certificate verification is enabled.

It excludes workspace/request/environment names, URLs, file paths, headers,
bodies, scripts, variable values, credentials, history, cookies, and logs. It
does not read the credential store. Review the JSON before sharing it through
your organization's approved support channel. The application sends nothing.

For a request failure, separately report the status/error code and reproduction
steps with sensitive identifiers removed. Never attach a private workspace
backup as if it were a diagnostics file.
