# Foundation validation record

Date: 2026-10-21. Implementation revision: `204baac59798dfe27190e2036c591ad9670a4598`, following contract checkpoint `3b752ee2d24819444dc7195c47c90fb44410eb3b` and functional checkpoint `2d42c2903f0a59b5965f5fdc30bb8884d67bae68`. The final validation commit adds this record and a tampered-signature negative assertion; production code remains at the implementation revision.

Environment: Rust `1.99.0 (b940084d7 2026-09-28)`, PostgreSQL **17.11**, Docker image `postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73`. All principals, endpoint names, keys and records are manufactured. RSA signing keys and invalid-inline-credential canaries are generated at test runtime. No external account, production query, deployment, push or publication was used.

## Commands and outcomes

Disposable database launch (local loopback only, no production credential):

```sh
docker run --detach --rm --name analytics-foundation-test -e POSTGRES_HOST_AUTH_METHOD=trust -e POSTGRES_DB=analytics_test -p 127.0.0.1:55439:5432 postgres:17-alpine
```

Compilation, database-backed security tests and lint:

```sh
cargo check --workspace
TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:55439/analytics_test cargo test --locked --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Passed. The Rust integration test `postgres_foundation_security_and_protocol` is one composed scenario with real SQL transactions and real HTTP requests, not one isolated assertion. It creates a random schema, applies all migrations and drops that schema on successful completion. It requires `TEST_DATABASE_URL` and never silently skips database validation. Runtime on the final recorded runs was roughly 2–6 seconds (5.54 seconds on the final tampered-signature run), including generated RSA keys; this is test duration, not a performance benchmark. Initial Clippy runs flagged collapsible conditionals; those were corrected and warnings-denied reruns passed.

Executable CLI and shipped Python remote client:

```sh
cargo build --locked -p analytics-server
docker exec analytics-foundation-test createdb -U postgres foundation_cli
TEST_DATABASE_URL=postgresql://postgres@127.0.0.1:55439/foundation_cli python3 crates/analytics-server/tests/cli_smoke.py
```

Passed with: `PASS: migrate, bootstrap, repeat rejection, remote MCP initialize/identity, protected rotation, revocation and recovery`. On reruns the dedicated `foundation_cli` database was dropped/recreated using `docker exec ... dropdb/createdb`; no non-test database was touched. The script creates only temporary mode-0700/0600 handoff files, starts/stops its own loopback service and cleans temporary credentials. It never prints tokens. The rotated protected credential file is reused successfully by the example client; old credentials receive denied access.

Contract stability:

```sh
cargo run -q -p analytics-core --example openapi > /tmp/analytics-foundation-openapi.json
cmp contracts/openapi.json /tmp/analytics-foundation-openapi.json
```

Passed byte-for-byte. The contract remains the foundation schema; ingestion/query additive routes are integrated by their module owners.

Cleanup:

```sh
docker stop analytics-foundation-test
```

The container uses `--rm`; its disposable database storage is removed on stop.

## Behavior demonstrated

- Empty schema migrations; existing handoff path fails without initializing or changing its content; simultaneous bootstrap creates exactly one owner/agent pair; repeated bootstrap cannot reveal/replace credentials.
- Mode-0600 handoff file, separately identifiable owner/agent, verifier-only database credentials, one-time issue/rotation delivery, explicit local recovery revoking all previous opaque/JWT credentials and external bindings.
- Human and agent principals with equivalent grants have equal scoped visibility; unauthorized property IDs, capability elevation and external-owner identity binding are denied.
- Effective authority intersects principal grants, token ceilings and delegated-parent grants; action and resource dimensions intersect independently. Parent changes deny previously held delegated actors. Audits preserve actor and delegated identity.
- Source-scoped administrators cannot create a connection or change endpoint, secret reference or kind. They can change noncredential metadata. Inline source credentials are rejected. Property policy versions increase; schema and saved-definition versions cannot be overwritten or reassigned across deletion.
- Source delete/recreate receives a strictly newer database revision. A database trigger enforces generation assignment even for other module writers.
- HTTP and direct Rust MCP use the same administration implementation; an MCP policy update is visible through HTTP. Unauthorized MCP receives a resource-metadata challenge; hostile Origin receives 403. Network responses never return existing token verifiers or secrets through ordinary credential lists/audits.
- Controlled discovery/JWKS fixture and real RS256 tokens exercise explicit subject binding; issuer/audience/expiry/nbf/unbound-subject/tampered-signature and ID-token confusion failures. Local JWT revocation and binding deletion invalidate both reauthentication and held job actors.
- Local credential expiry/revocation, atomic rotation, and loss of source/property grants are enforced at subsequent authorization checks. No secret token/JWT appears in the audit data inspected by tests.

## Integration boundaries and remaining evidence

The foundation binary initially registers an empty extension list; the integrating chat registers the implemented ingestion/query modules. Foundation does not claim a query/export/browser implementation from that empty list. The shared `Extension`, `AppState` and DTO signatures remain unchanged from the contract checkpoint.

Query ownership must validate the complete saved-definition lifecycle across admin-created versions and query-save/reproduce/delete, using canonical `admin_objects` current/history records and the agreed definition advisory lock. Its artifact tests must prove captured revision checks deny queued work and file downloads after source/property deletion/recreation. Foundation tests prove generation assignment and canonical storage; they do not manufacture a query job implementation.

A full OAuth authorization-code/PKCE/state/nonce/replay integration test belongs to the selected provider fixture and assembled-service acceptance. Foundation is a resource server and validates the resulting access token; it does not issue OIDC identity tokens or implement an identity provider. The provider fixture used here proves discovery and JWT verification. The shipped Python MCP client is exercised remotely; official SDK client interoperability is the integration task's separate evidence.

A01/A06/A14 foundation evidence is implemented and tested here. A02–A13 and full A14 cross-module artifact/revocation behavior require the other tasks' integration and acceptance runs. No six-month live result, benchmark guarantee, deployment or real-data collection is claimed. Ordinary administrative lists are bounded to 1,000 entries; individual known IDs remain accessible. Detailed benchmark, backup/restore and production release procedures belong to integration/operations handoff.
