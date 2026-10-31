# Workspace format

A Reqglide workspace is an ordinary directory containing a single
`workspace.yaml` manifest. It requires no account, service, daemon, or hidden
cloud identifier.

## Compatibility

- `schemaVersion` is required and currently equals `7`.
- Versions 1 through 6 are accepted, migrated in memory, and written as version
  7 on their next save. Defaults preserve older documents when newer
  environment, request, network, inherited-auth, and protocol-profile fields are
  absent.
- New application versions must explicitly migrate older schema versions.
- A newer, unsupported schema is rejected instead of being rewritten.
- Unknown fields are rejected so misspellings and partial migrations fail
  visibly.
- Collections, folders, requests, environments, variables, and protocol
  profiles use stable UUIDs so names and ordering can change without changing
  identity.
- Serialization follows model order and writes the complete file atomically. A
  failed or interrupted write preserves the previous valid document.
- Saves compare against the document last opened by the app. If another editor
  or Git changes workspace data, the save is rejected until the workspace is
  reopened, preventing silent lost updates. YAML comments are not preserved.

The loader limits manifests to 16 MiB, nesting to 32 folder levels, and the
document to 100,000 nodes before accepting it.

## Version 7 shape

The following is a representative subset. Empty/default fields may be omitted
by the serializer.

```yaml
schemaVersion: 7
id: 7b85bb25-2f43-4d7b-a606-df8f09a273a4
name: Payments API
auth:
  kind: bearer
  token: "{{accessToken}}"
network:
  cookieJarEnabled: true
  proxyMode: system
  noProxy: localhost,127.0.0.1
  caCertificatePath: ""
  clientCertificatePath: ""
  acceptInvalidCertificates: false
collections:
  - id: a1a62347-7867-43e2-b49d-1f18d3fd0402
    name: My Collection
    folders:
      - id: 1d967a38-3a76-433e-9df2-c33591662136
        name: Customers
        folders: []
        requests: []
    requests:
      - id: f2f8e7ba-040b-401c-85cb-408a177f1a1b
        name: Health check
        request:
          method: GET
          url: "{{baseUrl}}/health"
          queryParams:
            - name: verbose
              value: "true"
              enabled: true
          headers:
            - name: Accept
              value: application/json
              enabled: true
          auth:
            kind: inherit
          body:
            kind: empty
          options:
            timeoutMs: 30000
            maxRequestBytes: 104857600
            maxResponseBytes: 10485760
            followRedirects: true
        scripts:
          preRequest: ""
          postResponse: ""
environments:
  - id: 76be4a1b-2fee-4a9f-985a-a604ac9d3c25
    name: Development
    variables:
      - id: d420277f-c53c-421b-b7f5-621aa236628c
        key: baseUrl
        value: https://api.example.com
        enabled: true
      - id: d23dedbe-c8df-4594-93a5-b9e252e249b8
        key: localPort
        enabled: true
        local: true
      - id: 3bd9b127-1a48-4eb7-b8ac-4cf179c7f637
        key: accessToken
        enabled: true
        secret: true
protocolProfiles:
  - id: 04a7477e-c2aa-4333-897c-56771484fe47
    name: Countries GraphQL
    url: "{{graphqlUrl}}"
    headers: []
    auth:
      kind: inherit
    configuration:
      kind: graphQl
      query: "query Countries { countries { code name } }"
      variables: "{}"
```

Workspace, collection, and folder authentication can be inherited by saved HTTP
requests and protocol profiles. Network settings apply at workspace level.
GraphQL, WebSocket, Server-Sent Events, and gRPC profiles share the same header,
environment, authentication, and network policy boundary where their transports
support it.

## Security and local-state boundary

`workspace.yaml` is shareable configuration, not a secret vault. Request URLs,
raw headers, authentication fields, shared environment values, request bodies,
and local upload paths are plain text. File bytes are never copied into the
manifest.

A variable marked `secret: true` or `local: true` has no `value` field. Its
value is stored under the workspace and variable UUIDs in the operating-system
credential manager. Secret values are resolved natively and are not returned to
the normal editor; local values are returned only to the environment editor on
that device. The two flags are mutually exclusive. Deleting either kind of
variable, or changing it to Shared, removes its credential-store entry.

Interpolation, auth materialization, script execution, and transport execution
are composed inside the native command boundary. Normal execution returns only
responses and redacted reports to the webview. **Copy cURL** is an explicit
disclosure action: generated clipboard text can contain resolved credentials and
must be reviewed before sharing.

Request history, crash-recovery snapshots, collection-run reports, and cookie
jar state are operational local data and do not belong in the shareable
manifest. Credential values are not included in those files.
