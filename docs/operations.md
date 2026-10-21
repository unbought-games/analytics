# Operations and local validation

## Disposable setup

Run `make fixtures` for independent fixture/client tests. Run `make integration` for disposable engine and external-source PostgreSQL databases, controlled HTTP fixtures and Rust tests. Each invocation has a random `analytics-test-` Compose project, generated database credentials, dynamically assigned loopback ports, explicit CPU/memory limits and no connection to another installation. Test data lives in per-project named volumes. `stack.py run` supplies `DATABASE_URL`, `TEST_DATABASE_URL`, `TEST_SOURCE_DATABASE_URL`, `TEST_FIXTURE_URL` and `ANALYTICS_STACK_STATE` in its child environment; it does not print them.

For debugging, `make local-up` prints a state-file path. Run a command using the retained resources:

```sh
python3 tests/acceptance/stack.py run --state local/acceptance/run-EXAMPLE/state.json -- python3 -m unittest discover -s tests/acceptance -p test_stack.py -v
make local-down STATE=local/acceptance/run-EXAMPLE/state.json
```

Use the actual generated path. Cleanup validates that the state belongs to this checkout and that its project matches the generated test-name format. It removes that project's containers, network and volumes and deletes its password files. It never invokes a global Docker prune. A machine crash can require the documented explicit cleanup. Do not upload `local/`, environment dumps, bootstrap handoffs or database dumps to CI artifacts.

The PostgreSQL superuser in this disposable setup is used only for fixture creation and migrations. `synthetic_reader` has access only to the manufactured `game.activity` source, defaults to read-only transactions, and has 500ms statement/100ms lock budgets. It cannot inspect the private fixture schema. This is not a production runtime-role configuration.

## Controlled external services

The Python fixture intentionally authenticates a manufactured OIDC subject automatically. Its authorization-code flow uses RS256/JWKS, S256 PKCE, state/nonce reflection, short-lived one-use codes and loopback callback validation. It is a test identity provider, never an operator sign-in service. The Docker-network issuer is `http://fixtures:8081`; host tests instantiate a fixture with a host-reachable issuer.

Prometheus fixtures implement instant/range queries, series and label discovery. Queries `fixture_empty`, `fixture_rate_limit` and `fixture_unavailable` produce successful empty results, HTTP 429/Retry-After and HTTP 503 respectively. Range results preserve a missing interval, `NaN` and `+Inf` as strings. [Prometheus HTTP API reference](https://prometheus.io/docs/prometheus/latest/querying/api/).

Sentry fixtures restrict organization `synthetic` and project `game`/ID `101`, require a bearer, return two issue pages with Link continuation, and include deliberately sensitive synthetic fields in event details so adapters can prove redaction. Vendor writes return 405. [Sentry issues reference](https://docs.sentry.io/api/events/list-an-organizations-issues/). OIDC behavior follows the relevant authorization-code and ID-token fields in [OpenID Connect Core](https://openid.net/specs/openid-connect-core-1_0.html).

## Service installation handoff

Foundation supplies these CLI contracts (execution against the completed binary remains pending at this checkpoint):

```sh
analytics-server migrate
analytics-server bootstrap --owner synthetic-owner --agent synthetic-agent --output local/protected/bootstrap.json --expires-days 30
analytics-server serve --bind 127.0.0.1:8080
```

`ANALYTICS_DATABASE_URL_FILE` references a regular, non-symlink mode-0600 file containing the DSN. The bootstrap output parent must already exist with mode 0700; bootstrap creates a new 0600 file with `{owner_id, agent_id, credential: {id, token, expires_at}}`. It must not overwrite an existing handoff or repeat initialization. Configure `ANALYTICS_PUBLIC_BASE_URL`, `ANALYTICS_SECRET_DIRECTORY`, `ANALYTICS_EXPORT_DIRECTORY` and optionally `ANALYTICS_OIDC_ISSUER`. No secret is passed in an argument. For container setup, first run `engine migrate`, then `engine bootstrap`, then the server; the Compose `engine` profile is opt-in until these steps have run.

The release installation sequence must continue through authenticated MCP initialization/identity, property/policy/source configuration, a test export and remote download. The independent client in `tests/acceptance/client.py` implements released protocol `2025-11-25`, JSON and SSE responses, protected token loading, same-origin downloads and checksum verification. Privileged configuration uses `admin_execute` through the same authorization layer as HTTP.

Human sign-in uses an external OIDC provider. Bind its exact issuer/subject explicitly to a principal using `admin_execute` on `oidc_bindings`; email matching does not establish identity. The resource audience is `<public-base-url>/mcp`. The service requires RS256 access tokens with JWT header `typ=at+jwt` and rejects OIDC ID tokens. The fixture issues both distinct token types and accepts the resource audience through the authorization request's `resource` parameter.

Normal rotation/revocation uses authorized credential administration. If administrative credentials are lost, the explicit local command is `analytics-server recover --principal <UUID> --output <new-protected-file> --revoke-all --expires-days 30`. It revokes prior credentials and OIDC bindings and creates a new protected handoff. Do not substitute SQL edits to authorization tables. Bootstrap concurrency, recovery and revocation still require integrated execution before acceptance signoff.

A production installation needs separate migration and runtime database roles, TLS at the public endpoint, protected operator-managed source secrets, persisted export storage, and explicit profile/retention settings. Deployment is outside this implementation task. No image or package is published by these commands.

## Backup and restore

The current `test_source_backup_restore_preserves_values` test takes an actual custom-format `pg_dump`, restores it into a separate disposable database with `pg_restore --exit-on-error`, compares lossless rows, and checks restricted-reader access afterward. It demonstrates the fixture database restore only; it is not evidence that engine migrations, principals, revocations or export files have been recovered.

For the engine, back up PostgreSQL and export storage at a documented consistent cutoff; keep source secrets separately in the deployment's secret backup mechanism. Restore to an isolated installation first. Verify schema version, object counts, identity/attribution facts, principal/grant/credential state, export metadata/checksums and source references. Check that revoked credentials remain denied, a new export downloads correctly, and retention/deletion policies still apply. Do not point a restore drill at production sources. This full engine restore drill remains an acceptance dependency.

## Diagnostics and release preparation

Health/readiness, structured service diagnostics and backlog/rejection/export-failure metrics must be checked on the integrated service. Avoid logging tokens, connection strings, query results or request bodies. Capture revision, schema version, policy version and source coverage when diagnosing missing evidence; successful HTTP delivery alone does not prove accepted events.

Before release, run the complete A01–A14 suite and benchmark workloads, review the evidence and limits, build/package the browser distribution and service image, verify license and schema regeneration, and inspect package contents for secrets. Publishing requires a separate authorized release step, package-name/access checks and immutable version tagging. No live collection, production migration, vendor mutation or package publication is performed here.
