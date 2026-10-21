# Read-only sources

Source configuration uses foundation `SourceDefinition` and `admin_execute`. Query clients supply a configured source UUID, never an endpoint, filesystem path, or connection string. Creating a source or changing its kind, endpoint, or secret reference requires installation-level source administration. Ordinary discovery returns approved schemas/capabilities without the endpoint or secret reference. A source configuration revision change invalidates existing exports; recreating an ID must use foundation's monotonic revision migration.

## PostgreSQL

Use a dedicated login role that owns no exported relations, has no role memberships, and lacks superuser, role/database creation, replication and row-security bypass. Grant schema USAGE and SELECT only on the approved columns. Revoke access to private tables and execution of application functions, including PUBLIC grants. Apply these changes on a dedicated reporting database/replica when production workload isolation requires it. Never reuse the analytics service's administrative database credential.

```sql
CREATE ROLE reporting_reader LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE
  NOINHERIT NOREPLICATION NOBYPASSRLS;
GRANT USAGE ON SCHEMA reporting TO reporting_reader;
GRANT SELECT (id, occurred_at, amount) ON reporting.activity TO reporting_reader;
REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA reporting FROM PUBLIC;
```

Provision a strong password outside source control. A mode-0600 regular file under `ServiceConfig.secret_directory` contains the complete PostgreSQL DSN. `secret_ref` is its basename (no directories or symlinks). Configure `allowed_schemas:["reporting"]`, `allowed_tables:["reporting.activity"]`, and `allowed_columns:{"reporting.activity":["id","occurred_at","amount"]}`. The endpoint is descriptive for PostgreSQL; the protected file is the connection authority. HTTPS vendor sources instead use their configured endpoint and a protected file containing the bearer token. Do not put secrets in endpoint strings or query parameters.

Each connection checks the role and every configured column grant/type; unexpectedly broad column grants fail closed. Queries parse as one SELECT/CTE/union/join/aggregate/window query. The validator tracks lexical CTE scope, accepts only allowlisted relations, built-in value types/operators and a narrow `pg_catalog` analytical function set. It rejects writes, modifying CTEs, SELECT INTO, locks, table functions, recursion, custom casts/operators and unsafe functions. Execution uses a dedicated repeatable-read, read-only transaction, `pg_catalog` search path, UTC, lock/statement/idle budgets, row/byte ceilings and cancellation by connection close. PostgreSQL's client-connection check detects abandoned work within one second; statement timeout independently bounds it. Row size is checked in SQL before transfer. Duplicate output column names are rejected to prevent JSON key loss.

SQL requires an explicit ORDER BY; include a unique final tie-breaker (for example occurrence time then event ID). Queries may contain `$1`…`$N` parameters; `parameters` is `{ "p1":"123", "p2":null }`. Values are bound as nullable text, never substituted into SQL. Cast explicitly, for example `WHERE id=CAST($1 AS bigint)`. IDs, arbitrary precision numeric values, nested JSON numeric values and nonfinite values retain their lossless encodings. Data-dependent PostgreSQL errors are exposed as stable codes, never raw error/detail strings.

PostgreSQL discovery inspects only the approved relations/columns. It uses the same role verification and a maximum 30-second total deadline. Configuration must expose actual documented data, not security/credential tables. A query can aggregate near the source before exporting; the role and SQL surface do not permit database-wide dumps through catalogs.

## Prometheus

An administratively configured HTTPS endpoint ending in `/` is a complete tenant/source trust boundary. Its credentials must be limited to that tenant. Arbitrary PromQL is permitted within that endpoint; this adapter does **not** claim to impose metric- or label-level authorization within a shared Prometheus tenant. Define separate source endpoints/credentials when those boundaries differ. HTTP is accepted only for loopback fixture services; redirects are disabled.

`kind` is `prometheus_instant`, `prometheus_range`, or `prometheus_discovery`. Instant queries use `query:{"expression":"sum(requests_total)"}` and the request cutoff as evaluation time. Range adds `query.step_seconds` and requires `window.start/end`; Prometheus evaluates both endpoint times according to its API, unlike an analytics half-open event window. Discovery uses `query.operation` `metrics`, `labels`, or `label_values`, a window, and a selector/label where applicable. Query expressions, time and step remain in the manifest.

Bounded source warnings are preserved with the configured token redacted. Results preserve vector/matrix/scalar/string shapes, metric labels, histogram data, original sample value strings (`NaN`, `+Inf`, `-Inf`) and decimal-string timestamps. Absent series/samples remain absent; they are never zero-filled. Series/sample/page-byte ceilings are checked. Source failures cannot become successful empty results. The adapter uses `/api/v1/query`, `/query_range`, `/labels`, and `/label/{label}/values` as specified by the [Prometheus HTTP API](https://prometheus.io/docs/prometheus/latest/querying/api/).

## Sentry

Configure `kind:"sentry"`, the HTTPS API origin ending in `/`, `semantics.organization`, and explicit `allowed_projects` (IDs or slugs). Use a token restricted to the selected organization/projects with `event:read` for issues and `project:read` for project event endpoints; no vendor mutation is implemented. Queries never accept an organization or URL override, and `project:-1` is denied.

- `sentry_issues`: `query:{"project":"manufactured","filter":"","cursor":null}` and a window. Uses the organization issues route with an explicit project and `shortIdLookup=0`; returned project scope is verified. Empty filter requests all issues instead of Sentry's default unresolved filter.
- `sentry_events`: `query:{"project":"manufactured","cursor":null}` and a window. Uses the project error-event route.
- `sentry_event`: `query:{"project":"manufactured","issue":"123","event":"0123456789abcdef0123456789abcdef"}`. Uses the project-bound event-detail route, which safely supports a configured slug even when the response contains numeric `projectID`. Explicit event IDs are required; latest/oldest aliases are rejected. The issue ID remains investigation provenance; the event detail itself is selected by project and event ID.

Pagination reads the `Link` header's next/results state but extracts **only** the cursor; it never follows its URL or accepts changed project filters. Page limits yield a partial manifest with continuation. HTTP 429 records a retry delay from `Retry-After` or Sentry's reset header and terminates without retry storms. Source errors/redirects fail explicitly. Issue counts are not event counts or player counts. Pages have independent read boundaries, so local analyses should deduplicate stable IDs.

Redaction uses a positive output-field allowlist: identifiers, wall times, counts, project identifiers, release/environment, platform/level/type, and approved correlation tags. Raw messages/titles, users, requests/headers, breadcrumbs, stack variables and arbitrary tags are not exported. No secret-bearing vendor error body is returned. The implementation follows [issues](https://docs.sentry.io/api/events/list-an-organizations-issues/), [project events](https://docs.sentry.io/api/events/list-a-projects-error-events/), [project event detail](https://docs.sentry.io/api/events/retrieve-an-event-for-a-project/), [pagination](https://docs.sentry.io/api/pagination/) and [rate limits](https://docs.sentry.io/api/ratelimits/).

## History and join semantics

`semantics.coverage` may contain the exact versioned `Coverage` object: complete, history_start, history_end, limitations, sample_rate and policy_versions. Only an operator/source that can establish retained history should declare it complete. Missing bounds or sampling other than 1 make it incomplete; a requested window beyond declared history is also incomplete. Retrieval success by itself never establishes complete history. Partial execution always makes coverage incomplete. `semantics.columns` maps output column names to optional `namespace` and `unit`, which are copied into manifest column metadata. Sources have independent snapshots; never imply a cross-database atomic snapshot.
