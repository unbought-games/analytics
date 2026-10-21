# Independent implementation review

Status: final code-review disposition at integrated checkpoint `c497a33`, including
the corrective ingestion batch checkpoint `92734ca` (integrated as `4cdfa18`).
All 14 reported code findings are resolved by inspected corrections and the
explicit evidence below. No unresolved code defect was found in this review.
**A11 performance acceptance remains open at this checkpoint:** the original
normal-load benchmark missed its budgets and the unchanged-workload rerun is
still required. This is not a blanket A01–A14 release signoff.

## Reported findings (historical)

| ID | Priority | Area | Reproduction and expected correction |
| --- | --- | --- | --- |
| R01 | P1 | Campaign capture | A fresh tagged browser landing supplies its own event ID as `touch_id`; ingestion checks existing touch access before creating the touch. Verify real SDK landing is accepted and reaches acquisition. |
| R02 | P2 | Scoped event IDs | Two properties submit a tagged event with the same UUID. The draft global touch primary key collides although event IDs are scoped by property. Verify independent acceptance and isolation. |
| R03 | P1 | Source secrets | A principal managing only source A changes its endpoint and secret reference to source B's protected file, then triggers an HTTP export to a controlled fixture. Require deployment or installation authority for connection/secret binding; verify fixture receives no secret. |
| R04 | P2 | Analytics null counts | `count(data->'subject')` counts explicit JSON null as non-null. One null and one identified event must yield `count(subject)=1`, while `count(*)=2`. |
| R05 | P2 | Export column fidelity | `SELECT 1 AS x, 2 AS x ORDER BY 1` produces duplicate JSON keys; parsing silently drops the first value. Reject duplicate output names before writing an export. |
| R06 | P2 | Receipt retention | Receipt rows have expiry but draft retention never sweeps them and dedupe ignores expiry. Verify bounded cleanup and documented retry semantics after expiry. |
| R07 | P2 | Saved definitions | Administration writes `admin_objects`, while save/reproduce use `query_definition_versions`. Verify create/get/delete/reproduce share one authorized lifecycle. |
| R08 | P2 | Sentry detail | A configured project slug cannot match a documented detail response containing only numeric-string `projectID`. Verify a project-bound route or verified slug/ID mapping without weakening scope enforcement. |
| R09 | P1 | Export queue availability | Delete a source after queueing its export. Draft claim propagates the missing source error and repeatedly blocks subsequent jobs. Verify the bad job becomes terminal and an unrelated valid job completes. |
| R10 | P2 | Analysis scoped IDs | Analysis deduplicates acquisition rows by event ID alone. Duplicate a valid visit row under another property: the draft raises a conflicting-row error. Use the property/event pair. |
| R11 | P2 | Configuration generations | An export pins `admin_objects.revision`, but delete/recreate resets that revision to 1. Recreating the same source/property UUID must not reauthorize an old artifact or queued job against a different configuration generation. |
| R12 | P1 | Application outage independence | The Rust producer rejects application commands when its analytics outbox reaches 10,000 undelivered rows. Analytics saturation must not halt gameplay; use an independently retained application journal/projection or another explicitly recoverable design. |
| R13 | P2 | Terminal artifact cleanup | The maintenance sweep repeatedly selects the same first 1,000 terminal jobs without advancing or recording cleanup. Later cancelled/failed/privacy-expired files can remain indefinitely. Persist cleanup progress and prove eventual deletion beyond the first batch. |
| R14 | P1 | Slow-body admission starvation | New shared admission in `16c5ed7` acquires capacity before body extraction without a request/body deadline. Unauthenticated incomplete POST bodies can hold every permit indefinitely. Bound transport/request time and test real slow bodies. |

Previously reported by the coordinator: SQL CTE lexical scope leakage and
cross-dimensional grant intersection. Both fixes and regression cases were
observed in module drafts; integrated verification remains pending.

## Independent evidence collected

- `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s examples/analysis -v`
  ran against the analysis draft: 32 tests passed. This was an initial read-only
  review baseline, not an integrated acceptance run.
- Disposable PostgreSQL 17.7 Alpine reproduced R02 using the actual draft
  ingestion migration: first property insertion succeeded; second returned
  `duplicate key value violates unique constraint "ingest_touches_pkey"`.
- Disposable PostgreSQL reproduced R04 with two JSONB rows: draft expression
  returned 2; `count(NULLIF(data->'subject','null'::jsonb))` returned 1.
- Disposable PostgreSQL reproduced R05: `row_to_json` returned
  `{"x":1,"x":2}` for the duplicate-name projection.
- A targeted Python probe loaded the manufactured cohort, copied one visit row
  with a different synthetic property but the same event ID, then called
  `calculate`: it raised `conflicting rows for the same scoped identifier` (R10).
- Local `initdb` could not start because the installed client package lacks the
  `postgres` executable. A separate disposable Docker container was used instead.

Official API references for R08:
[Sentry issue event detail](https://docs.sentry.io/api/events/retrieve-an-issue-event/),
[Sentry organization issues](https://docs.sentry.io/api/events/list-an-organizations-issues/).

## Fix verification checkpoint

The composed regression in
`crates/analytics-server/tests/independent_review.rs` uses the actual HTTP router,
database authorization, query worker, disposable PostgreSQL, two synthetic secret
files and a controlled vendor HTTP fixture. It is explicitly opt-in:

```sh
REVIEW_DATABASE_URL=postgresql://postgres@127.0.0.1:54742/postgres \
  cargo test -p analytics-server --test independent_review -- --ignored --nocapture
```

The port above was the dynamically published loopback port of this review's
disposable container. Use the current disposable port when repeating. Each test
creates/drops its own database and uses a protected temporary directory.

| Finding | Disposition and evidence |
| --- | --- |
| R01 | Browser fix omits a self-reference on a newly tagged event. Browser owner documents a real Chromium → Rust → PostgreSQL regression in `npm run test:browser:service`; independent code inspection agrees. Full journey remains integration-owned. |
| R02 | Ingestion now generates property/event-derived global touch IDs and resolves authorized aliases. Its real-PostgreSQL test `durable_ack_dedup_conflict_late_arrival_and_scoped_touch_ids` explicitly accepts the same event UUID in two properties and asserts different touch IDs. |
| R03 | Independently passed on integrated `e03780f`: scoped endpoint/kind/secret-reference changes and scoped source creation return 403; the permitted A export sends only A's canary to the safe fixture. B's canary is never transmitted by the restricted actor. |
| R04 | Inspected correction to `count(NULLIF(..., 'null'::jsonb))` and committed targeted real-PostgreSQL analytics test in query checkpoint `7e913d0`; owner reports passing results. Original defect was independently reproduced. |
| R05 | Inspected duplicate output-column rejection before serialization and committed adapter regression in `7e913d0`; owner reports passing results. Original duplicate JSON output was independently reproduced. |
| R06 | Inspected finite occurrence-time horizon, expired-receipt cleanup and deletion suppression in ingestion checkpoints. Final `5909478` also retains ended identity-binding provenance and removes matching raw receipts on deletion. |
| R07 | Independently passed: HTTP admin put → query reproduce → completed export → admin delete → reproduction denied. Current authority and immutable history now share the administration store. |
| R08 | Inspected project-bound detail route, preserving the project authorization boundary for slug configuration. Vendor-fixture regression is query-owner evidence. |
| R09 | Independently passed: pause worker, queue A, delete A, queue B, restart. A becomes failed and B completes. |
| R10 | Targeted probe passed against analysis `330bd2d`: copied visit/event UUID under another property becomes a distinct visit without manufacturing a conversion. A 50%-sampled origin-touch input also correctly makes mature conversion unavailable. |
| R11 | Independently reproduced before the generation migration; independently passed after `d3dc1a5`, included in `e03780f`. Delete/recreate no longer restores the old manifest. |
| R12 | Closed by inspected `6afc788` (integrated as `cd0b3eb`) and owner regressions. Both languages commit beyond a saturated 10,000-row queue, lose/rebuild the projection, and recover all 10,025 facts with ACK uncertainty. Rust's 2 unit/11 PostgreSQL suites pass in 104 seconds; TypeScript strict types/runtime recovery passes in 38.5 seconds, maximum batch 50, maximum wire payload 30,262 bytes and measured heap growth 32 MB. Independent retention records an explicit gap, and public/server quotas are separated. |
| R13 | Closed by inspected `0f35b7f` and owner regression: durable `cleaned_at`, writer-lease gating, ordered uncleaned batches and successful-removal marking. The real-PostgreSQL test inserts 1,001 old terminal rows plus a later directory, then asserts directory removal and its cleanup marker. |
| R14 | Closed by `ff384c65e026abcf7d682d6f3b47b4fe0476adaa` (integrated as `3dc9516`) and independent targeted execution. Request timeout covers body/auth/pool/handler; HTTP/1.x timeout closes the unread connection. A separate watchdog drops response producer/body resources and the permit without relying on body polling. Defaults are 30-second request and 300-second stream lifetimes with bounded overrides. |

The first composed run on `15d42ed` plus the regression failed solely at R11's
final assertion (old artifact revived). The same test passed after the generation
migration in 1.09 seconds. Extending it with R09's queue scenario passed in 1.16
seconds. No broad owner suite was rerun after its owner reported passing results.
The final composed check on integrated `c6ab39e` plus this review documentation
passed in 5.22 seconds, using a fresh database including migrations 0005/0006 and
the final query/ingestion lifecycle checkpoints. Compilation took 14.85 seconds.

Independent R14 commands after importing the deadline correction:

```sh
cargo test -p analytics-server --lib deadline -- --nocapture
cargo test -p analytics-server --lib headers_only -- --nocapture
```

The first passed two tests in 0.16 seconds (never-polled response and real TCP
nonreading download); the second passed one test in 1.02 seconds (64 incomplete
MCP request bodies receive 408/connection close, then valid traffic regains
capacity). These checks need no database and did not repeat the broader suite.

## Follow-up batching review

The first integrated normal-load benchmark accepted all 5,000 events but missed
its proposed latency/throughput budgets (approximately 184 events/second, p95
807 ms and p99 1,262 ms). That failed measurement remains evidence; it is not
converted to success by changing the budget.

The corrective ingestion draft was inspected for the boundaries affected by
batching. Property/schema caches are transaction-local and retain the existing
`FOR SHARE` configuration locks. Keys include property/name/version. The global
ingestion/privacy lock remains; receipts are keyed by property/event and include
pending ordinary writes, so same-batch duplicates/conflicts are detected before
the bulk insert. Complex touch/signup events flush preceding ordinary writes;
per-event validation and rejection savepoints remain. Ordered inserts and final
ACK cursor patching preserve committed receipt ordering. Counters are grouped
within the same transaction.

The new owner regressions exercise ordinary event → duplicate → conflict → touch
→ signup → stale-policy rejection → linked action, asserting exact statuses,
cursor equality/order, acquisition and counters. Another starts with only two
quota slots left, accepts exactly two of five events, then rejects all next 100.
No new semantic defect was found in the focused review of final owner checkpoint
`92734ca`. Its existing 11 PostgreSQL suites and two unit tests passed, followed
by a targeted 14-PostgreSQL-suite run adding mixed ordering, partial quotas,
forced bulk-write rollback and shared one-connection authorization. The long
10,025-fact recovery suite skipped in the targeted run had already passed the
full run. Clippy passed. The unchanged-workload benchmark rerun remains required.

## Remaining acceptance audit

This matrix distinguishes reviewed evidence from independent execution. It does
not replace the integration owner's final revision-specific acceptance record.
The coordinator reports the complete local acceptance command passed all 20
steps before the batching performance correction; the final aggregate run and
performance decision remain the coordinator's responsibility.

| Requirement | Evidence reviewed and remaining acceptance work |
| --- | --- |
| A01 | `tests/acceptance/test_service.py` exercises migration/bootstrap and actual MCP/export clients. Integration checkpoint `7db1c3d` reports the complete browser→service→export→analysis journey passing. |
| A02 | Browser owner reports 27 Chromium cases and actual Rust/PostgreSQL smoke; capture, sanitation, idempotence and bounded queue tests were inspected. No claim of Firefox/WebKit validation. |
| A03 | Ingestion's real-PostgreSQL campaign/handoff retry/expiry/signup tests and the corrected browser touch contract were inspected. `test_journey.py` in `7db1c3d` verifies the three-origin browser→OIDC→signup→enrollment→action journey, rollback/outage/retry, six actual exports and selected-touch conversion provenance. |
| A04 | Verified namespaced mapping, multiple worlds, reset/withdrawal/deletion tests were inspected. Final ingestion preserves ended binding provenance for deletion after short event expiry. |
| A05 | Transactional rollback/recovery/dedup/conflict/late delivery tests exist. R12's corrective Rust/TypeScript saturation regressions pass in final producer checkpoint `6afc788`; retained original facts recover independently of delivery queue capacity. |
| A06 | Server policy validation, public-authority denial, profiles, suppressed identifiers and deletion paths were inspected; browser choice/profile tests remain owner evidence. |
| A07 | Query adapter tests cover restricted real PostgreSQL, unsafe SQL/functions/columns, lexical CTE scope, timeout/cancellation and lossless streaming. Source-secret isolation was independently exercised through production authorization. |
| A08 | Independent composed tests prove real remote retrieval, shared definition lifecycle, source-secret isolation, queue progress and resource-generation invalidation. Owner export tests cover precision/checksum/partial/failure/revocation and the corrected R13 cleanup progress. |
| A09 | `test_service.py` exercises actual MCP exports of manufactured source datasets into the external report. The inspected `7db1c3d` journey also uses six actual exported datasets and asserts one acquired, enrolled and observed-playing account plus one verified originating-visit conversion. Fresh mature conversion and D180 rates remain null. |
| A10 | Initial 32 analysis tests were independently run; owner reports 43 after normalization. New scoped-ID and sampled-origin probes were independently executed without repeating the full suite. No live six-month result is inferred. |
| A11 | Browser measurements, streaming source tests and corrected producer saturation behavior were inspected. Integrated ingestion/query/export memory, backpressure and database-impact measurements remain active work. |
| A12 | Contracts, authentication, privacy, source/export and operational documentation were inspected. `bd17bee` adds isolated runtime privileges, container boot and actual engine backup/restore checks; final aggregate revision/evidence remains integration-owned. |
| A13 | Controlled adapter fixtures and source-scope/redaction/pagination/rate-limit paths were inspected; Sentry project-detail behavior was checked against official API documentation. `test_service.py` now exports instant/range/issues/events/detail, checks shared release context, lossless special values, redaction, no-data/failure distinction and unauthorized-project denial. |
| A14 | Foundation and service tests cover bootstrap races, OIDC code/PKCE, machine credentials, parity, escalation, revocation and recovery. Restricted source authority was independently exercised using actual database authorization and HTTP. |

Draft files still under construction are not classified as defects. No
deployment, production query, package publication, push, merge, or pull request
was performed by this review.
