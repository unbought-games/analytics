# Headless installation and access

Build `cargo build --locked -p analytics-server`. Binary: `target/debug/analytics-server`. The PostgreSQL DSN is read only from `ANALYTICS_DATABASE_URL_FILE`, a regular file with mode 0600 or stricter. Keep it in ignored `secrets/` or an external secret mount. It is not accepted as a CLI argument. Migration and runtime database accounts should be distinct: migration account owns schema/DDL; runtime receives schema USAGE, table SELECT/INSERT/UPDATE/DELETE and sequence USAGE, with no CREATEDB/CREATEROLE, ownership, superuser or access to other databases. Database provisioning is a deployment responsibility; no principal/authentication SQL is required.

Create a mode-0700 handoff directory. Run commands with the selected secret file already provisioned by the deployment secret mechanism:

```sh
export ANALYTICS_DATABASE_URL_FILE="$PWD/secrets/migration-dsn"
./target/debug/analytics-server migrate
mkdir -m 700 -p local/handoff
./target/debug/analytics-server bootstrap --owner 'Synthetic Owner' --agent 'Synthetic Agent' --output local/handoff/initial.json --expires-days 30
export ANALYTICS_DATABASE_URL_FILE="$PWD/secrets/runtime-dsn"
export ANALYTICS_PUBLIC_BASE_URL=http://127.0.0.1:8080
export ANALYTICS_SECRET_DIRECTORY="$PWD/secrets"
export ANALYTICS_EXPORT_DIRECTORY="$PWD/exports"
./target/debug/analytics-server serve --bind 127.0.0.1:8080
```

Bootstrap creates separate human owner and agent principals with owner authority. The initial agent's credential is delivered once as `{owner_id, agent_id, credential:{id, token, expires_at}}`, atomically with initialization. Parent directory must preexist with mode 0700; the output must not exist and is created 0600. Files are synchronized before committing. Concurrent/repeated bootstrap returns `already_initialized` without extra owners, replacement credentials or secret disclosure. If a crash leaves an inaccessible handoff, local recovery is explicit; rerunning setup does not reveal secrets.

The default expiry is 30 days; allowed lifetime is 1–366 days. Source/admin/API tokens must never enter a browser bundle. Public property tokens are separate ingestion capabilities, issued by ingestion's property-token operation.

The included Python client reads the protected handoff and performs a real remote MCP call without printing or passing a bearer token as an argument:

```sh
python3 examples/admin/mcp_client.py --url http://127.0.0.1:8080 --credentials local/handoff/initial.json --tool identity
```

The response identifies principal, credential, delegation and effective resource grants. Standard MCP clients can use their protected bearer-token/header facility with this initial automation credential; it is already an access credential, not an OIDC identity token and not an OAuth client secret. Never commit a literal bearer header in client configuration. Clients supporting only interactive OAuth use the provider flow below. Host-authorized agents can run the same setup/recovery commands as humans.

## Programmatic administration

`POST /v1/admin/execute` and MCP `admin_execute` take `{operation,resource_kind,resource_id?,value?}`. All callers use identical permission checks. HTTP `/v1/me` maps to MCP `identity`, and `/v1/admin/status` maps to `status`. Sources/property/policy/schema discovery is the corresponding `list`/`get`; extension tools add domain-specific coverage/query/export discovery.

| Kind | Operations | ID/value |
| --- | --- | --- |
| properties | list/get/put/delete | property UUID / PropertyDefinition |
| policies | get/put | property UUID / CollectionPolicy; increment version |
| schemas | list/get/put/delete | `property:name:version` / EventSchemaDefinition; new version to change |
| sources | list/get/put/delete | source UUID / SourceDefinition; secrets only by filename reference |
| definitions | list/get/put/delete | definition UUID / SavedDefinition; one source/property scope |
| principals | list/get/put/delete | principal UUID / PrincipalDefinition; delete disables |
| grants | get/put/delete | principal UUID / Grant array; put replaces, delete removes |
| oidc_bindings | get/list/put/delete | principal UUID / OidcBinding; delete unbinds all external subjects |
| credentials | list/get/issue/rotate/revoke/delete | list ID is principal UUID; get/rotate/revoke/delete ID is credential UUID |
| audit | list/get | newest 1,000 sanitized audit records, installation audit:read |

Ordinary lists are capped at 1,000 entries; individual known IDs remain retrievable. For credentials `issue`/`rotate`, value is `{principal_id,name,expires_at,grants}`. Rotation requires the previous credential ID and atomically invalidates it. Both operations return `{id,token,expires_at}` once. `list`/`get` expose metadata and grant ceiling, never token/verifier. Set `--secret-output <new protected path>` in the Python client when issuing/rotating. A network failure after issuance cannot redisplay a raw token: list/revoke the orphan credential and issue another.

Grant-management and credential-management permissions do not confer authority to grant arbitrary capabilities: requested grants must be a subset of the caller's effective grants. Token grants must also fit the target principal's grants. Changing/removing grants requires authority over the prior set. Binding an external subject requires authority over all target grants, preventing a limited principal manager from attaching their identity to an owner. Source readers cannot inspect principal/credential/audit tables through core APIs.

A human can receive exactly the same grants as an agent; `kind` never adds authority. A delegated agent names an existing nondelegated principal in immutable `delegated_by`. Its grants and credentials intersect with its parent's live authority. Disabling the parent or revoking their grants affects subsequent delegated operations. Record delegated identity in audits.

## Human sign-in and remote OAuth

Set `ANALYTICS_OIDC_ISSUER` to the chosen provider's issuer URL before serving. Configure that provider to mint **RS256 access tokens** with `typ: at+jwt` (or `application/at+jwt`) and audience exactly `<ANALYTICS_PUBLIC_BASE_URL>/mcp`; this is the resource identifier for this service, including its HTTP administrative surfaces. The provider must publish OIDC discovery, authorization/token endpoints and JWKS. HTTPS is required, with loopback HTTP supported only for disposable local fixtures.

Through authorized MCP, explicitly bind `{principal_id,issuer,subject}` using `oidc_bindings put`. Issuer and subject are exact, case-sensitive identifiers; they are never inferred from email or display name. Ordinary OIDC ID tokens (`typ: JWT`) are not accepted as access tokens. A valid access token with an unbound subject gets 401. JWT permissions come solely from local principal grants; arbitrary provider role claims cannot escalate them.

Clients discover `/.well-known/oauth-protected-resource/mcp` from the 401 `WWW-Authenticate` challenge (also available at `/.well-known/oauth-protected-resource`). Metadata advertises the external authorization server. Clients retrieve provider metadata, use a preregistered public client, and perform authorization-code + S256 PKCE with the provider. Include `resource=<base>/mcp`; provider validates registered redirects, PKCE, one-time codes and any OIDC state/nonce. Request a token for this resource and send it as `Authorization: Bearer`. Unattended first-agent access uses the protected opaque credential directly. Dynamic registration is optional and delegated to the selected provider, not implemented here. The `/oauth/authorize` convenience route forwards to the configured provider; it does not create a second authorization server.

`/.well-known/oauth-authorization-server` mirrors configured provider metadata for operators; the issuer remains the external provider. Resource metadata remains authoritative for interoperable client discovery. Signature, issuer, audience, expiry and nbf are enforced. Unknown signing keys trigger throttled JWKS refresh. Existing external access tokens acquire local credential IDs; revoke those through normal credentials administration, or unbind/disable a principal for all their sessions. Held job actors also revalidate the live binding and credential.

## Local recovery and revocation

With authorized host access and database secret access, select an existing nondelegated owner/principal UUID:

```sh
./target/debug/analytics-server recover --principal '<principal-uuid>' --output local/handoff/recovery.json --revoke-all --expires-days 30
```

Recovery is never exposed over HTTP or MCP. It enables that principal, restores owner grant, revokes **all installation credentials and OIDC bindings**, issues one protected replacement, and records `local_recovery:revoke_all` with the target. The explicit flag is required; omitted/false is rejected. Rebind intended OIDC identities and reissue intended agent credentials afterward. `--output` cannot overwrite an existing handoff. This deliberate revocation scope avoids preserving access after complete administrative credential loss.

Revocation, expiry, disabled principal, removed identity binding and narrowed grants apply on subsequent HTTP, MCP and extension artifact/job checks. They cannot retract bytes already downloaded. Extensions must check both export ownership/access and underlying source/property permission for every manifest/file read.

Source creation and changes to connection kind, endpoint or secret reference require **installation-level** `sources:manage`. A scoped source manager can edit noncredential settings but cannot borrow another secret or redirect credentials. Saved definitions and schema versions retain immutable history; deleting their active configuration does not make a version identifier reusable with different content. Definition reproduction must require the active current definition before consulting history.

The service also accepts `ANALYTICS_MAX_IN_FLIGHT_REQUESTS` (default 64, valid range 1–4096). This shared fail-fast limit includes HTTP, MCP and streaming response lifetimes. Saturation returns HTTP 503 with `Retry-After: 1`; only cheap GET/HEAD liveness bypasses it. See [request admission](request-admission.md) for lifecycle and deployment boundaries.

Admitted body/handler work also has an absolute deadline: `ANALYTICS_REQUEST_TIMEOUT_MS` defaults to 30000 (valid 1–600000). Expiry returns HTTP 408 and closes an unread HTTP/1.x request. `ANALYTICS_STREAM_TIMEOUT_MS` defaults to 300000 (valid 1–3600000); its independent watchdog drops a stalled response producer and releases capacity even without body polling. Expired streams fail explicitly; retained completed export files can be downloaded again. Both limits apply to existing default router compositions.
