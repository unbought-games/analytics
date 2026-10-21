# Analytics Engine Implementation Plan

**Goal:** Deliver the complete generic analytics engine and reproducible synthetic acceptance evidence for A01–A14.

**Architecture:** One Rust service owns PostgreSQL persistence, authorization, ingestion, direct MCP and authenticated streaming artifacts. A small browser package captures permitted observations; restricted external adapters and an external analysis script preserve source-specific meanings.

**Tech Stack:** Rust, PostgreSQL, TypeScript, JSON/JSONL, OpenAPI, containers, fixture HTTP services and browser automation. Foundation records and pins maintained library versions and a released MCP protocol version.

## Global Constraints

- Product requirements: `docs/SPECIFICATION.md`; all A01–A14 are required.
- No references or configuration from the operator's other properties. Synthetic producers, identities and datasets only.
- No production operations or package publication. Public code and documentation contain no secrets.
- One authorization layer, complete human/agent parity, resource-scoped access on every operation and download.
- No dashboard, built-in LLM, telemetry collector, distributed SQL or unrestricted source proxy.
- Per-task worktree and commits; the orchestrator integrates and coordinates shared contract changes.
- MIT license. Conservative default collection; operator-configurable depth and retention with explicit analytical limitations.

---

### Task 1: Foundation, contracts and headless administration

**Files:** Create `Cargo.toml`, `crates/analytics-core/`, `crates/analytics-server/`, `migrations/0001_foundation.sql`, `contracts/openapi.json`, `docs/architecture.md`, `docs/contracts.md`, `docs/authentication.md`, `LICENSE`.

**Signatures:** Pin exact Rust DTOs, shared application state and module interfaces in `docs/contracts.md` before dependent Rust tasks begin. HTTP surfaces: `GET /healthz`, `GET /readyz`, `POST /mcp`, protected `/v1/admin/*`, OAuth discovery/authorization surfaces, and CLI `migrate`, `bootstrap`, `recover`, `serve`. Foundation owns shared DTOs and router/MCP composition. All other tasks consume the committed OpenAPI contract.

**Behavior:**
- Implement persistence, migrations, owner/agent bootstrap and explicit local recovery; atomic concurrent bootstrap, verifier-only credentials and protected one-time delivery.
- Shared resource-scoped authorization for humans, delegated agents and service principals; OIDC integration, MCP protocol authorization interoperability, expiry/rotation/revocation, escalation prevention and auditable administration.
- Programmatic and MCP CRUD for properties, profiles/retention, schemas, sources, principals/grants and credentials; saved definitions, status and deletion must have clear extension interfaces.
- Freeze event, handoff, identity, query and export DTOs early. Record dependency/protocol choices and security boundaries; do not leave permissive auth substitutes.

**Tests must prove:** Empty database setup; concurrent/repeated bootstrap safety; human/agent parity; issuer/audience/expiry validation; denied escalation and resource leakage; credential revocation; secret-free ordinary responses and audits. A01/A06/A14.

### Task 2: Ingestion, policy, attribution and identity

**Files:** Create `crates/analytics-ingest/`, `migrations/0002_ingestion.sql`, `examples/producers/`, `docs/ingestion.md`, `docs/privacy.md`.

**Signatures:** Consume foundation DTOs for `POST /v1/ingest/browser`, `POST /v1/ingest/server`, journey handoff/redemption, verified identity mapping/reset, withdrawal and deletion. Export a router and MCP operations through foundation's exact interfaces.

**Behavior:**
- Validate typed allowlisted events under effective server policy; browser credentials cannot assert trusted outcomes/accounts or escalate collection. Bound requests, batches, quotas and queues.
- Commit before acknowledgment, scoped stable-ID deduplication and conflict detection; receipt ordering retains late events. Expose rejection/loss indicators.
- Immutable acquisition from first eligible tagged touch within configurable 30 days, separate last touch, auditable corrections, unknown attribution and verified namespaced mappings.
- Audience-bound expiring cross-property handoffs with trusted redemption; enforce logout/shared-device, opt-out, retention and deletion. Synthetic transactional outbox producers demonstrate recoverable outcomes without coupling application success to analytics availability.

**Tests must prove:** Retry/conflict/rollback/outage recovery; late and out-of-order events; campaign cancellation/retry/expiry; login versus signup; differing IDs and multiple memberships; profile and withdrawal enforcement; prohibited field rejection. A03–A06.

### Task 3: Browser capture and package

**Files:** Create `packages/browser/`, package workspace files, `examples/static-site/`, `docs/browser.md`.

**Signatures:** `init(options): AnalyticsClient`; client exposes `track`, `identify`, `reset`, `setCollectionChoice`, `flush`, `shutdown` with exact types aligned to foundation's OpenAPI. Identity association requires trusted evidence, never arbitrary authoritative account IDs.

**Behavior:**
- Initial/SPA navigation and delegated semantic interaction/form/validation capture, idempotent init, exclusions, no arbitrary DOM text or field values.
- Sanitized URLs/referrers/campaign fields; bounded stable event IDs, queue/retries/batches, visible loss; static-site build and npm metadata.
- Effective profiles and visitor choice, defined visibility/session boundaries, permitted campaign persistence/handoff, logout/account switching.

**Tests must prove:** Real-browser automatic capture once; sanitized transmission; no forbidden contents; choice changes; bounded failure behavior and queue memory; navigation/campaign continuity; measured bundle size, initialization and interaction costs. A02/A06/A11.

### Task 4: Queries, exports and external source adapters

**Files:** Create `crates/analytics-query/`, `migrations/0003_exports.sql`, `docs/sources.md`, `docs/exports.md`, `tests/fixtures/sources/`.

**Signatures:** Foundation-pinned DTOs for analytics/source discovery and queries; export create/status/cancel/manifest/download; saved-query reproduction. Expose HTTP and MCP through shared authorization interfaces.

**Behavior:**
- Flexible bounded PostgreSQL analysis with narrow DB roles, allowed schemas/columns, safe function surface, read-only transactions and cancellation; no arbitrary connection strings or source secrets in responses.
- Prometheus instant/range/discovery and Sentry issue/event/detail queries with configured endpoint/scope, pagination, rate limits, redaction and explicit unavailable/partial states.
- Stream JSONL with lossless numbers, nulls, time/identity namespaces and stable ordering; durable manifests, checksums, provenance, coverage, cutoffs and continuation.
- Remote authenticated retrieval survives reconnect; expiry, cancellation, deletion and revoked/resource-scoped access apply to queued jobs and every artifact. Repeatability records source snapshot semantics.

**Tests must prove:** Real restricted PostgreSQL rejects unsafe/write/access paths; bounded memory and cancellation; source scopes and hostile input; fixture vendor semantics; actual remote file retrieval and checksum/type fidelity; partial/failure/expiry/revocation. A07/A08/A13.

### Task 5: Synthetic cohorts and external analysis

**Files:** Create `examples/analysis/`, `tests/fixtures/cohorts/`, `docs/measurement.md`.

**Signatures:** CLI `analyze --manifest <path> --cutoff <UTC> --output <directory>` reads acquisition, mapping, memberships and activity datasets through the versioned manifest contract and emits JSON plus a readable report.

**Behavior:**
- Manufacture deterministic clocks, different identity namespaces, multiple players/worlds, unmapped/deleted identities and unknown/partial historical coverage. No existing company property names or copied schema.
- Join outside the engine; distinguish visitors/accounts/memberships, authoritative human actions from scheduled/background events, acquired versus revisiting accounts.
- Versioned signup/activation/D1/D7/D30/D180 metrics with exact elapsed windows, mature denominators, exclusions, null unavailable rates, source references and retention limitations.

**Tests must prove:** Exact boundary maturity; zero denominator; duplicate/multiple membership handling; unmapped/deleted and missing-history limitations; scheduled effects excluded; reproducible reports with input provenance. A09/A10.

### Task 6: Integration, CI, performance and operating handoff

**Files:** Create `compose.yaml`, `Dockerfile`, `Makefile`, `.github/workflows/ci.yml`, `tests/acceptance/`, `benchmarks/`, `docs/operations.md`, `docs/acceptance.md`, `README.md`.

**Signatures:** Reproducible commands for build/check/unit/integration/acceptance/benchmark; disposable local stack starts engine, PostgreSQL, synthetic producer properties, OIDC and controlled vendor fixtures. Commands and cleanup scope are documented.

**Behavior:**
- Assemble a real browser-to-signup-to-enrollment-to-action-to-MCP-download-to-local-report journey across synthetic properties. Test negative paths across real module boundaries.
- CI runs meaningful Rust/TypeScript/database/browser checks and secret checks with minimal permissions; release preparation documents package/image publication without performing it.
- Record browser bytes/cost/volume, ingestion throughput and latency percentiles, streaming export memory/latency and external query impact under reproducible normal/burst/failure workloads.
- Document installation, migrations, first owner/agent, source setup, retention/deletion, backup/restore, diagnostics, release and exact remaining deployment steps. Review A01–A14 against evidence, fix gaps, record tested revision.

**Tests must prove:** Independent clean setup, complete real service journey, protocol-client interoperability and remote retrieval; restore validity; bounded queues/backpressure/memory; complete acceptance evidence without claims of live production collection. A01–A14.

## Execution order

Foundation publishes shared contracts first. Browser and analysis can begin independent synthetic work concurrently; ingestion and queries begin after contract integration. Integration/CI may prepare its harness while components are being implemented, then owns cross-component validation. Every task is executed in a project chat with its own Git worktree. The orchestrator reviews results, integrates commits and dispatches corrections.
