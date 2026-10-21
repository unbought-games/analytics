# Durable analytical files

Register `analytics_query::QueryExtension` with the server's shared extension list and start `analytics_query::start_worker(state.clone())` once after migrations. The worker claims PostgreSQL jobs under a database lock: at most four exports run cluster-wide, further bounded by each source's and request's concurrency. Jobs persist before acknowledgment. A process interruption leaves running jobs with a 310-second lease; expired leases become explicit `failed/worker_interrupted`, while queued jobs remain runnable. Queries have a maximum five-minute execution budget. Failed source configuration at the head of the queue is terminalized rather than blocking unrelated work.

HTTP and MCP use the same domain operations and foundation authorization. Create requires `query:read` plus `exports:create` on the underlying property/source. Status/manifest/download require current underlying query access and exports read on the export or its underlying scope. Files are private to their creating principal unless the caller has installation-level artifact authority. Every operation revalidates the creating principal/credential as well as the caller; revoking the creator also invalidates queued jobs and later retrieval through replacement credentials. Disabling, deleting or revising a source/property invalidates old artifacts. Ingestion privacy deletion marks property exports expired transactionally. Download rechecks access and lifecycle at every 64-KiB chunk.

## API

| MCP tool | HTTP equivalent |
| --- | --- |
| `export_create` | `POST /v1/exports` |
| `export_status {id}` | `GET /v1/exports/{id}` |
| `export_manifest {id}` | `GET /v1/exports/{id}/manifest` |
| `export_cancel {id}` | `POST /v1/exports/{id}/cancel` |
| `export_delete {id}` | `DELETE /v1/exports/{id}` |
| `query_source_discover {id}` | `GET /v1/sources/{id}/discover` |
| `query_analytics_discover {id}` | `GET /v1/properties/{id}/analytics` |
| `query_save` | `POST /v1/query/definitions` |
| `query_reproduce` | `POST /v1/query/reproduce` |

Create takes the core `ExportRequest`: `{query:QueryRequest,format:"jsonl",retention_hours:24}`. Retention is 1–168 hours. Source example query:

```json
{
  "source_id":"11111111-1111-4111-8111-111111111111",
  "property_id":null,
  "kind":"postgres",
  "query":{"sql":"SELECT id, occurred_at, amount FROM reporting.activity ORDER BY occurred_at, id","dataset":"activity"},
  "parameters":{},
  "window":null,
  "cutoff":"2026-07-01T00:00:00Z",
  "definition_version":"activity/1",
  "budget":{"timeout_ms":30000,"max_rows":100000,"max_bytes":67108864,"max_concurrency":2}
}
```

External SQL is reproduced exactly; its cutoff/window are provenance, not silently injected SQL predicates. Include explicit SQL predicates (or typed bound parameters) when a source query needs a cutoff. Analytics event queries enforce the cutoff and half-open window themselves. Vendor adapters map cutoff/window to their documented parameters. Requested source budgets are capped by the administrator's source budgets.

A completed status has an authenticated `manifest_url`. The manifest contains an authenticated dataset `download_url`; a remote client saves that URL with its Bearer credential. Reconnecting with a new HTTP/MCP session works until expiry. No secret is placed in a download URL and no server-local absolute path is required. Responses are private/no-store. A client can download again after a connection loss; byte-range resumption is not currently offered.

## Files and lifecycle

Files are UTF-8 JSONL, with one object per newline. All numeric tokens, including nested JSON numbers, are decimal strings; column metadata retains PostgreSQL types. IDs remain strings. Null is distinct from absent and zero. Wall timestamps use UTC; source logical time is a separate documented string. PostgreSQL query ordering must include a unique tie-breaker. Analytics adds the complete canonical JSON row as a final deterministic tie-breaker. Vendor order and read boundaries are documented rather than represented as an atomic snapshot.

The manifest uses foundation format version `1` and records query/version/parameters, source/property/environment, read timestamps, a PostgreSQL snapshot or analytics receipt watermark, columns/namespaces/units, file bytes/rows, SHA-256, coverage, continuation and expiry. SQL values are decoded with arbitrary-precision JSON, never through JavaScript floating-point numbers. Unique column names are mandatory. Empty successful queries still produce a valid empty file and SHA-256; unavailable sources produce failure, not empty success.

Rows stream to a mode-0600 temporary file under a mode-0700 generated UUID directory. The writer checks row/byte ceilings before accepting a row, updates SHA-256 incrementally, fsyncs the file, renames it and fsyncs its directory before publishing the durable manifest. No request can select a storage path. A PostgreSQL row is capped before transfer; HTTP pages are capped while receiving. Memory is bounded by a row/page, not total export size.

States are queued, running, complete, partial, failed, cancelled and expired. Limit exhaustion after output yields explicit partial data with continuation/reproduction guidance; failure before any row yields a failed manifest without a data file. Vendor pagination retains a cursor when stopped at a page boundary. A failure in the middle of a page requires rerunning and deduplicating stable IDs. Cancelled/expired files cannot be retrieved. Cleanup removes terminal temporary files and expired files. Already downloaded copies remain the recipient's responsibility under the applicable deletion/retention policy.

## Composable analytics

Use `kind:"analytics"`, exactly one `property_id`, and `query.dataset` `events`, `acquisition`, `identity_map`, `visits` or `touches`. Queries support `select`, `filters`, `group_by`, `aggregates` and `order_by`. Field paths such as `properties.amount` or `context.environment` allow up to four safe identifier segments.

```json
{
  "dataset":"events",
  "filters":[{"field":"category","op":"eq","value":"human_action"}],
  "group_by":["context.environment"],
  "aggregates":[{"op":"count","alias":"events"},{"op":"sum","field":"properties.amount","alias":"amount"}],
  "order_by":[{"field":"context.environment","descending":false}]
}
```

Filters are eq/ne/lt/le/gt/ge/is_null over typed JSON values; is_null matches explicit JSON null, not an absent key. Count without a field counts rows; count(field) excludes absent and JSON-null values. Sum/avg/min/max operate on numeric values and exclude nonnumeric/null values. Queries always filter property and expiry. Receipt-watermark snapshots retain late-arriving occurrence-time facts; a rerun at the same occurrence cutoff can legitimately incorporate newly received history.

Acquisition preserves immutable signup campaign/touch/rule and the original signup property/visit. It does not fetch another property's origin data. `visits` returns the earliest retained navigation for each property/visit, with original landing campaign and explicit linkage permission; null-visit observations remain unlinked. `touches` returns retained selected-touch evidence including global touch ID, original property/visit, campaign and linkage permission. Separate authorized manifests can be joined locally by touch ID and scoped visit key, as documented in measurement guidance. Multiple tagged navigations are touches, not extra visits. Retention may remove earlier visit facts, so observed first navigation is not guaranteed first-ever history.

Saved definitions use the same canonical `admin_objects` current pointer and immutable `definition_versions` records as foundation administration. `query_save` cannot change a version's contents or move a definition to another scope, even after deletion. Reproduction requires the current pointer, `definitions:read`, and fresh query/export permissions. Deleting via administration gates all retained versions. Reproduction changes only the supplied cutoff; it does not silently shift a saved explicit window.

## Verification

Run the manufactured PostgreSQL/HTTP suite with `tests/fixtures/sources/run.sh`. It creates and removes its own loopback-only disposable PostgreSQL container. Tests require `QUERY_TEST_DATABASE_URL` and fail when it is absent; no database tests silently skip. Test-created databases, roles and reader passwords are independent per test. A configured admin DSN with a username/password is parsed safely; no static reader password is required.

Coverage includes actual column/write/function denial, lexical SQL escape attempts, precision/null/nested-value checks, 200,000-row streaming, cancellation/timeouts, vendor scope/redaction/pagination/rate-limit/no-data states, remote download/checksum/reconnect, partial/failure/cancel/expiry/revocation/configuration changes, analytics property/retention filtering, null count and nested aggregation, and saved-definition lifecycle. Authorization in crate HTTP tests is a controlled fixture; composed service acceptance separately validates the production authorization implementation.

### Recorded local evidence (2026-10-21)

The disposable `tests/fixtures/sources/run.sh` run passed five tests (one parser unit test, one analytics database test, one HTTP/export lifecycle test and two source tests), with zero skipped database tests. `cargo clippy -p analytics-query --all-targets -- -D warnings` passed. `cargo run -q -p analytics-query --example openapi` generated nine additive paths with all 16 schema references resolving; merge this fragment into the foundation contract rather than replacing it.

On Apple M2 Pro, macOS 26.6.2, Rust 1.99.0 debug builds and Docker PostgreSQL 17, the counted 200,000-row PostgreSQL stream took 0.917–2.299 seconds across recorded runs. `/usr/bin/time -l target/debug/deps/sources-ad15bec02451c507 --exact postgres_restricted_streaming_lossless_and_cancellation --nocapture` (with the disposable `QUERY_TEST_DATABASE_URL` in the environment) reported 15,515,648 bytes maximum RSS for the complete source test. The corresponding export lifecycle binary command, `/usr/bin/time -l target/debug/deps/exports-0127742ce3dc7d01 --exact durable_remote_export_reconnect_scope_partial_failure_cancel_expiry_revocation --nocapture`, reported 21,135,360 bytes maximum RSS and 9.25 seconds wall time while creating, durably writing, remotely downloading and incrementally checking a 200,000-row export, plus its negative lifecycle checks. The source rows contain a 144-character manufactured value; their JSONL payload exceeds either measured process RSS. Neither client nor server collected the large result in memory. Executable hashes are build-specific; `cargo test --no-run` prints the current paths.

These are local debug measurements with concurrent development activity, not production latency guarantees. They exclude the PostgreSQL container's memory and do not establish a six-month real-world history. The composed acceptance suite owns end-to-end production authorization, the browser journey and agent-side report validation.
