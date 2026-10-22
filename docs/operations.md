# Operations and local validation

For installation and everyday production operation, start with the
[self-hosting guide](deployment.md). This page describes the disposable validation
stack and backup/restore responsibilities.

## Disposable setup

Run `make fixtures` for independent fixture/client tests. Run `make integration` for disposable engine and external-source PostgreSQL databases, controlled HTTP fixtures and Rust tests. Each invocation has a random `analytics-test-` Compose project, generated database credentials, dynamically assigned loopback ports, explicit CPU/memory limits and no connection to another installation. Test data lives in per-project named volumes. `stack.py run` supplies `DATABASE_URL`, `TEST_DATABASE_URL`, `TEST_SOURCE_DATABASE_URL`, `TEST_FIXTURE_URL` and `ANALYTICS_STACK_STATE` in its child environment; it does not print them.

For debugging, `make local-up` prints a state-file path. Run a command using the retained resources:

```sh
python3 tests/acceptance/stack.py run --state local/acceptance/run-EXAMPLE/state.json -- python3 -m unittest discover -s tests/acceptance -p test_stack.py -v
make local-down STATE=local/acceptance/run-EXAMPLE/state.json
```

Use the actual generated path. Cleanup validates that the state belongs to this checkout and that its project matches the generated test-name format. It removes that project's containers, network and volumes and deletes its password files. It never invokes a global Docker prune. A machine crash can require the documented explicit cleanup. Do not upload `local/`, environment dumps, bootstrap handoffs or database dumps to CI artifacts.

The PostgreSQL fixture administrator creates disposable databases and performs backup/restore. The service uses one generated `analytics_runtime` account for automatic migrations, bootstrap and serving. It owns the analytics database/schema and has no superuser or cluster role/database creation privileges. `synthetic_reader` has access only to explicitly granted manufactured source tables, defaults to read-only transactions, and has 500ms statement/100ms lock budgets. It cannot inspect the private fixture schema. Manufactured analytical rows from Task 5 are also loaded into `cohort.synthetic_acquisition`, `cohort.synthetic_identity_map`, `cohort.synthetic_memberships` and `cohort.synthetic_activity`, using typed text/boolean/JSONB columns and an explicit stable fixture ordinal. These source rows are compared with the original files in real PostgreSQL tests. This is not a production runtime-role configuration.

## Controlled external services

The Python fixture intentionally authenticates a manufactured OIDC subject automatically. Its authorization-code flow uses RS256/JWKS, S256 PKCE, state/nonce reflection, short-lived one-use codes and loopback callback validation. It is a test identity provider, never an operator sign-in service. The container fixture issuer is `http://127.0.0.1:8081` inside the engine/fixture shared network namespace; host tests instantiate their own fixture with a host-reachable issuer.

Prometheus fixtures implement instant/range queries, series and label discovery. Queries `fixture_empty`, `fixture_rate_limit` and `fixture_unavailable` produce successful empty results, HTTP 429/Retry-After and HTTP 503 respectively. Range results preserve a missing interval, `NaN` and `+Inf` as strings. [Prometheus HTTP API reference](https://prometheus.io/docs/prometheus/latest/querying/api/).

Sentry fixtures restrict organization `synthetic` and project `game`/ID `101`, require a bearer, return two issue pages with Link continuation, and include deliberately sensitive synthetic fields in event details so adapters can prove redaction. Event detail requires the explicit project-bound route `/api/0/projects/synthetic/game/events/0123456789abcdef0123456789abcdef/` and returns numeric `projectID: 101`, distinct from the configured `game` slug. Issue-scoped detail, `latest`, and other organization/project routes are denied. Vendor writes return 405. [Sentry project event reference](https://docs.sentry.io/api/events/retrieve-an-event-for-a-project/). [Sentry issues reference](https://docs.sentry.io/api/events/list-an-organizations-issues/). OIDC behavior follows the relevant authorization-code and ID-token fields in [OpenID Connect Core](https://openid.net/specs/openid-connect-core-1_0.html).

## Service installation handoff

Use the [Docker deployment guide](deployment.md) for a normal installation. The CLI sequence is:

```sh
analytics-server bootstrap --owner synthetic-owner --agent synthetic-agent --output local/protected/bootstrap.json --expires-days 30
analytics-server serve --bind 127.0.0.1:8080
```

`DATABASE_URL` supplies the connection string directly. Alternatively, `ANALYTICS_DATABASE_URL_FILE` references a regular, non-symlink mode-0600 file containing the DSN. The bootstrap output parent must already exist with mode 0700; bootstrap creates a new 0600 file with `{owner_id, agent_id, credential: {id, token, expires_at}}`. It must not overwrite an existing handoff or repeat initialization. Configure `ANALYTICS_PUBLIC_BASE_URL`, `ANALYTICS_SECRET_DIRECTORY`, `ANALYTICS_EXPORT_DIRECTORY` and optionally `ANALYTICS_OIDC_ISSUER`. No secret is passed in an argument. After `make local-up`, run `python3 tests/acceptance/stack.py engine-up --state <generated-state-path>` to build the image, start the service with automatic migrations and bootstrap to the protected handoff using the same database account. It finalizes dependency containers before reading the dynamic host binding, starts the engine without recreating dependencies, verifies the actual host readiness endpoint, and prints that loopback URL. The engine shares the controlled fixture network namespace so the OIDC issuer is loopback HTTP; validation is not weakened for arbitrary HTTP hosts.

The release installation sequence must continue through authenticated MCP initialization/identity, property/policy/source configuration, a test export and remote download. The independent client in `tests/acceptance/client.py` implements released protocol `2025-11-25`, JSON and SSE responses, protected token loading, same-origin downloads and checksum verification. Privileged configuration uses `admin_execute` through the same authorization layer as HTTP.

Human sign-in uses an external OIDC provider. Bind its exact issuer/subject explicitly to a principal using `admin_execute` on `oidc_bindings`; email matching does not establish identity. The resource audience is `<public-base-url>/mcp`. The service requires RS256 access tokens with JWT header `typ=at+jwt` and rejects OIDC ID tokens. The fixture issues both distinct token types and accepts the resource audience through the authorization request's `resource` parameter.

Normal rotation/revocation uses authorized credential administration. If administrative credentials are lost, the explicit local command is `analytics-server recover --principal <UUID> --output <new-protected-file> --revoke-all --expires-days 30`. It revokes prior credentials and OIDC bindings and creates a new protected handoff. Do not substitute SQL edits to authorization tables. Foundation database tests and the CLI lifecycle test cover bootstrap concurrency, rotation, revocation and recovery; the live composed service also verifies human OIDC/agent parity and denied wrong-token/audience/expiry access.

A production installation uses one database account with schema ownership, TLS at the public endpoint, protected operator-managed source secrets, persisted export storage, and explicit profile/retention settings. Deployment is outside this implementation task. No image or package is published by these commands.

## Backup and restore

Use your database provider's backup tooling or `pg_dump`/`pg_restore` with
credentials loaded from protected files or your secret manager. Keep passwords
out of command arguments. Store backups outside the service host with restricted
access; they contain event data and authorization state, and the data volume can
contain the initial raw credential handoff.

For a straightforward single-instance backup:

1. Pause incoming producers or let their durable outboxes retain pending delivery,
   stop the analytics service with `docker compose -f compose.deploy.yaml stop`,
   and prevent other service instances/maintenance jobs from writing. Browser
   observations during the interruption remain best effort and can be lost.
2. Back up the dedicated PostgreSQL database and the complete `analytics_data`
   volume while the service remains stopped. Record a common cutoff, the deployed
   revision, schema version and volume/project identity. Back up deployment
   configuration and connector secrets through your protected secret backup process.
3. Restart with `docker compose -f compose.deploy.yaml up -d --wait` (including any
   local overrides), verify readiness and authenticated access, and resume producers.

To rehearse recovery, provision a separate empty database and volume, restore the
matched database/files, preserve file ownership for UID/GID 65532, and initially
use the backed-up service revision. Configure an isolated test URL and block access
to live external sources and producers. Restoring credentials also restores their
recorded authority: protect the drill as an installation containing sensitive data.
Check the items below before reconnecting a recovered installation. Account for
credential revocations and privacy deletions made after the backup cutoff; an old
backup must not silently reinstate revoked access or deleted data.

The current `test_source_backup_restore_preserves_values` test takes an actual custom-format `pg_dump`, restores it into a separate disposable database with `pg_restore --exit-on-error`, compares lossless rows, and checks restricted-reader access afterward. It demonstrates the fixture database restore only; it is not evidence that engine migrations, principals, revocations or export files have been recovered.

For the engine, back up PostgreSQL and export storage at a documented consistent cutoff; keep source secrets separately in the deployment's secret backup mechanism. Restore to an isolated installation first. Verify schema version, object counts, identity/attribution facts, principal/grant/credential state, export metadata/checksums and source references. Check that revoked credentials remain denied, a new export downloads correctly, and retention/deletion policies still apply. Do not point a restore drill at production sources. The live engine restore drill now quiesces the service, takes a real custom-format dump, restores into a separate disposable database, and copies export storage plus source-secret files. It compares migration/principal/grant/credential/audit/config/ingestion/mapping/export row counts, restarts the runtime-role service at the same public URL, verifies an existing manifest/download checksum, denies a revoked credential and duplicate bootstrap, then creates new exports and a report. No production sources are contacted.

## Diagnostics and release preparation

Use `GET /healthz` for process liveness, `GET /readyz` for database readiness, MCP `status` for installation state, `ingest_status` for durable accepted/rejected/conflict/reported-loss counters, and `export_status` for explicit queued/running/partial/failed/cancelled/expired outcomes. The source-owned `producer_delivery_coverage` view exposes pending journal, projected queue, quarantines and explicit unconfirmed retention gaps through an authorized read-only source. Avoid logging tokens, connection strings, query results or request bodies. Capture revision, schema version, policy version and source coverage when diagnosing missing evidence; successful HTTP delivery alone does not prove accepted events.

Before release, run the complete A01–A14 suite and benchmark workloads, review the evidence and limits, build/package the browser distribution and service image, verify license and schema regeneration, and inspect package contents for secrets. Publishing requires a separate authorized release step, package-name/access checks and immutable version tagging. No live collection, production migration, vendor mutation or package publication is performed here.

## Public build and package boundary

The root Docker context allows only Cargo manifests, crate sources, migrations and the exact synthetic Rust producer/SQL build inputs, with nested secret/cache exclusions. The ignored original local handoff file is explicitly excluded. The fixture context allows only `server.py` and its Dockerfile. Docker build-context tests use manufactured private-file markers in temporary directories and inspect the actual exported Docker context; they never open the operator's local handoff. The service Dockerfile copies those explicit build inputs and the final binary, not the repository wholesale.

Before packaging browser code, build it and inspect `npm pack --dry-run --json --ignore-scripts` from `packages/browser`; only intended package metadata, license/readme and browser distribution files belong in the tarball. The automated package check currently verifies all 16 distribution entries. Git ignore rules do not by themselves control Docker or npm packaging. CI uploads only the explicit command-evidence and service-benchmark JSON files, never a broad workspace directory or unacknowledged raw request files.
