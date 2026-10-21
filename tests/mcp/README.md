# Official MCP SDK interoperability smoke

This is a **development-only client test**, not a server or runtime dependency. It runs against a disposable Rust service that is already migrated, bootstrapped and listening on loopback. It uses the official SDK for initialize/initialized, tools/list and all tool calls; ordinary HTTP only verifies auth discovery and streams export artifacts.

The official [Python SDK](https://github.com/modelcontextprotocol/python-sdk) / [PyPI release](https://pypi.org/project/mcp/2.3.0/) was checked on 2026-10-21. Latest release was **2.3.0**, published 2026-10-02, supporting Python 3.10+. This test selects Python 3.11–3.13 and pins `mcp==2.3.0`; `uv.lock` pins transitive versions and distribution hashes from public PyPI. `uv run --locked` refuses an out-of-date lock. Regenerate intentionally with `uv lock --project tests/mcp --python 3.13 --index-url https://pypi.org/simple`; do not update dependencies implicitly during CI.

The SDK documents [`Client` with a supplied HTTP transport](https://py.sdk.modelcontextprotocol.io/client/transports/) for protected bearer headers. Its [`mode="legacy"`](https://py.sdk.modelcontextprotocol.io/protocol-versions/) selects the initialize handshake for the service's explicitly pinned MCP **2025-11-25**. This is a supported compatibility mode in the latest SDK; no custom transport/protocol parser, fork or workaround server is installed. The SDK validates/deserializes the Rust server's protocol responses.

## Run

Build/start the disposable service using the integration stack first. Credentials are read from a mode-0600 bootstrap handoff. The token never appears in arguments, stdout, committed configuration or exception logs.

```sh
uv run --project tests/mcp --locked --python 3.13 python tests/mcp/interop.py \
  --url http://127.0.0.1:8080 \
  --credentials local/handoff/initial.json
```

The supplied owner agent credential remains valid and unchanged. The smoke creates then deletes one synthetic property; it issues and revokes a separate 15-minute read-only credential of the same principal. Reusing that principal allows optional downloads to exercise the existing export owner's permissions. Credential revocation records and sanitized audits remain in the disposable database.

To include the configured issuer and an existing small completed synthetic export:

```sh
uv run --project tests/mcp --locked --python 3.13 python tests/mcp/interop.py \
  --url http://127.0.0.1:8080 \
  --credentials local/handoff/initial.json \
  --expect-issuer http://127.0.0.1:8081 \
  --manifest-url http://127.0.0.1:8080/v1/exports/00000000-0000-4000-8000-000000000001/manifest
```

Replace the illustrative manifest UUID with the acceptance fixture's actual export. Each dataset is streamed, bounded to 16 MiB, and checked against manifest bytes/SHA-256. Redirects and off-origin artifact URLs are rejected so the test never forwards a bearer token to another host. After revocation both the manifest and a file must be denied. Omission is reported explicitly as `remote_artifact: "not_requested"`, not a successful artifact test.

Integration can invoke this command inside its existing `Service` lifecycle using the actual `.url`, `.directory/bootstrap.json`, fixture issuer and completed manifest URL. The executable exits nonzero on a failed assertion and emits one secret-free JSON result on success. It suppresses SDK/HTTP debug logging and exception payloads that could carry request contexts. No new root Python/Node dependencies are needed; the virtual environment is ignored under `tests/mcp/.venv/`.

## Assertions and recorded evidence

- SDK negotiates protocol 2025-11-25; observed requests include initialize, notifications/initialized, tools/list and tools/call.
- Required identity/status/admin tools are discoverable; identity matches the credential's principal.
- A property created through the official SDK can be read back with the selected minimal profile.
- The read-only temporary credential can inspect the property but receives a structured forbidden tool result for deletion.
- Unauthenticated SDK access receives HTTP 401. The held SDK connection and a new SDK connection both receive 401 after revocation.
- Unauthenticated `/mcp` returns a `WWW-Authenticate` protected-resource-metadata challenge; the metadata has the expected resource audience, bearer header support and authorization-server list. With `--expect-issuer`, the configured issuer must be present.
- Optional remote files match exact byte lengths and checksums, then become inaccessible to the revoked credential.

Initial run on 2026-10-21: actual `analytics-server` binary from foundation revision `932da03de4f1d762b2a345791cc19d0485903efa`, PostgreSQL 17.11 in a dedicated disposable container, Python 3.13.0, official `mcp` 2.3.0. All required protocol/admin/revocation/discovery checks passed. That isolated foundation run had no configured issuer or export extension, and reported `configured_issuer:false`, `remote_artifact:"not_requested"`. The integrating chat runs those optional assertions against its already-tested composed fixture service; they are not claimed from the foundation-only run. The external provider's PKCE/state/nonce/code-replay validation remains the separate provider acceptance test.
