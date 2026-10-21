# Durable ingestion and verified journeys

`analytics_ingest::IngestionExtension` is a unit struct implementing the shared
`Extension` contract. Foundation composes its router and MCP tools after applying
migration `0002_ingestion.sql`. All ordinary DTOs are from `analytics_core`;
module additions are exposed as Rust types and MCP JSON Schemas.

## Transport and authority

`POST /v1/ingest/browser` uses `X-Property-Token` and the exact configured `Origin`.
Issue/revoke these public tokens using `ingest_token_issue`/`ingest_token_revoke`
(or POST `/v1/ingest/token`, `/v1/ingest/token/revoke`) under `properties:manage`.
Only their SHA-256 verifiers are stored; token responses disclose the value once.
These are public abuse-control credentials, never proof of a person or account.
They cannot authenticate to server ingestion or analytical/admin routes.

`GET /v1/ingest/config?property_id=<uuid>` (alias `/v1/ingest/policy`) uses the same
public headers and returns `{property_id,policy,handoff_audiences,schemas,limitations}`.
Only non-authoritative event schemas are exposed. Browser policy selection must
not exceed that ceiling. The service checks current policy version, profile,
collection choice, `Sec-GPC: 1` when configured, schema types, origin and identifiers
again on every collection request. Authoritative HTTP uses `Authorization: Bearer`
and current resource grants; public tokens are never Bearer credentials.

`POST /v1/ingest/server`, MCP `ingest_server`, and Rust `ingest_server` share one
implementation requiring `ingest:server` on every batch property. The event's
producer must match its transport. Browser schemas cannot claim signup,
enrollment, committed human action, scheduled effect or system authority. The
schema's category must match exactly. Browser envelopes cannot assert subjects
or attribution rules. Server clocks are `server`/`game`; browser clocks are
`client`. Decimal-string game logical time is preserved separately.

## Durability, ordering and budgets

A batch has 1–100 events and at most 256 KiB (or a lower configured body budget).
The service accepts up to 6,000 event attempts and capability issues per property/minute.
At most 32 HTTP operations enter the module concurrently; excess requests receive
429 before their bodies are parsed. There is no
unbounded server memory queue. Database statement/lock budgets are 15/5 seconds.
Each event receives `accepted`, `duplicate`, `rejected`, or `conflict`, a stable
reason code and an optional decimal receipt cursor. Semantic rejection of one
event does not discard valid siblings. Database failures produce no successful
ACK; retry the identical batch. Response transmission follows transaction commit.

Deduplication uses `(property_id,event_id)` and a canonical typed-envelope hash.
The stable event ID and payload must survive retries. Concurrent commits allocate
receipt cursors under one advisory transaction lock, so a committed cursor cannot
skip an earlier uncommitted receipt. This serialization is deliberate for the
first release; benchmark it before increasing throughput assumptions. Use receipt
cursors for incremental collection and occurrence time for analysis. Late and
out-of-order events retain their original timestamp and receive new receipt order.

The dedupe/late-arrival horizon is the greater of 365 days, event retention and
identity retention. Events outside that occurrence-time horizon are explicitly
rejected (`outside_dedupe_horizon`); clocks over five minutes in the future are
rejected. Receipt hashes expire after that horizon from receipt and are removed
by bounded retention maintenance. This prevents an ancient replay from recreating
history after its receipt has expired. Producers must retain and resolve rejected
facts rather than silently discarding them.

`collection.dropped_events` is a per-event delta, counted only for newly accepted
events, never on deduplicated retries. `/v1/ingest/status?property_id=...` and
`ingest_status` require `status:read` and expose durable rejection, acceptance,
conflict and reported-loss counters. Client loss reports are unverified. Public
traffic, test environments and bot-like activity must be distinguished by query
filters and coverage; event counts do not prove humans or unique people.

## Campaign and identity flow

A tagged landing event has `campaign` fields and **no `touch_id`**. Its accepted
`event_id` can be used as a reference on subsequent events. Storage assigns a
separate property-scoped deterministic touch UUID, avoiding collisions if two
properties reuse the same event UUID. An event-ID alias resolves only to a touch
accessible to that visit/visitor; ambiguous references are rejected. An earlier
accepted event in the same ordered batch is available to later entries.

Unchanged campaign state is not resent as a new touch on every click. The server
stores original touch facts independently, selecting the first eligible tagged
touch within the inclusive configured pre-signup window (default 30 days), ordered
by occurrence time then touch UUID. Last eligible touch is separate evidence.
Unknown remains null. A signup accepted before a late touch is not silently
reattributed: an authorized `ingest_acquisition_correct` operation records an
explicit audit with prior/new touch and reason. Later signup retries, campaigns
and existing-account login never overwrite acquisition.

1. Public POST `/v1/journey/handoff` accepts `HandoffRequest`. Both source and
   destination must permit cross-property linking, and source must allowlist the
   destination property UUID. The opaque capability expires in ten minutes.
2. The destination application's trusted auth flow POSTs `/v1/journey/redeem`
   with `RedeemHandoffRequest` under `identity:manage` on the audience. Redemption
   transfers eligible touch evidence to destination visit/visitor IDs. It neither
   authenticates a user nor redirects a browser. Same-target retry is idempotent;
   a different target, wrong audience or expired token fails. Cancellation leaves
   source touches intact; retry can issue a fresh capability.
3. A transactionally captured trusted signup includes the issuer namespace/subject
   plus verified journey IDs. Identified policy permits future binding and immutable
   acquisition. Login is a distinct authoritative custom schema, never signup.
4. For proof-based browser identification, trusted POST `/v1/identity/proof`
   takes `{property_id,visit_id?,visitor_id?,evidence_event_id}`. Stored evidence
   must be a server event with that same journey and a subject. It returns
   `{proof,expires_at}` with a five-minute lifetime. Public POST
   `/v1/identity/identify` takes `{property_id,visit_id?,visitor_id?,proof}` with
   token/origin headers. The proof binds exact property and journey IDs; browser
   supplied subject assertions are never accepted. Subsequent browser rows may
   carry the server-resolved subject while remaining browser observations.
5. Trusted POST `/v1/identity/link` / `ingest_identity_link` uses the core
   `IdentityLinkRequest`. Evidence must be a stored server signup/enrollment with
   the same subject and allowlisted `target_namespace`, `target_id`, optional
   `world` properties. Account and player namespaces remain distinct; multiple
   world memberships are supported. Conflicting owners cannot overwrite a link.

## Lifecycle and integration

Public reset `/v1/identity/reset` and withdrawal `/v1/privacy/withdraw` accept
`BrowserChoiceRequest {property_id,visit_id?,visitor_id?}`. This supports journey
withdrawal without durable visitor IDs. Old identifiers are suppressed; minimal ended-binding provenance remains until
identity retention/deletion so later deletion can still find original touches. Ended
bindings never enrich future events. Reset
requires fresh identifiers before collection resumes. Trusted subject withdrawal
is `/v1/privacy/withdraw-subject` / `ingest_withdraw`. Scoped deletion is
`/v1/privacy/delete` / `ingest_delete` and requires `privacy:delete`. It traverses
verified namespaced mappings in the authorized property, deletes linked history,
invalidates proofs/handoffs and expires that property's existing export jobs
(`status='expired',manifest=NULL`). Query workers/downloads must recheck job status.
Cross-property deletion is repeated for each authorized property; external game
systems and already downloaded files have their own deletion responsibility.

`ingest_retention` / POST `/v1/ingest/retention` removes up to 10,000 expired rows
per table and invalidates exports when facts disappear. Schedule repeated calls
through the operator's maintenance process. Suppression hashes intentionally remain
to prevent deleted identities returning through delayed retries; they are not
analytical identity mappings. Minimal acquisition/provenance retention may exceed
short browser-event retention, subject to deletion and documented coverage.

Synthetic producers in `examples/producers` demonstrate an application transaction
and bounded durable outbox, leasing, timeout/backoff, retry after uncertain ACK,
and quarantine of permanent rejection. PostgreSQL capacity reservation occurs
before application success. No generic post-commit callback can replace that
transactional capture. Quarantined/unacknowledged authoritative facts are retained
for repair; only acknowledged rows may be compacted under an operator policy.

## Verification checkpoint

Executed against disposable Docker `postgres:17`, loopback published port:

```sh
INGEST_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:53668/postgres cargo test -p analytics-ingest --tests
```

Result: two validation/admission unit tests and eight real PostgreSQL integration tests pass.
They cover ACK/dedupe/conflict, 180-day late arrival, cross-property event-ID reuse,
policy/authority/origin/prohibited fields, campaign first/last transfer and expiry,
login versus signup, logical scheduled events, differing namespaces/multiple worlds,
proof/reset/withdrawal/deletion/export invalidation, transactional rollback, outage
recovery and crash-after-ACK replay. A test-only authorization implementation isolates
module behavior; foundation and assembled acceptance tests verify real credentials.
Followup coverage includes concurrent identical delivery, partial per-event ACKs,
current-policy changes on retry, dedupe expiry/horizon, quota and loss counters,
exact attribution window endpoints, audited correction, revoked public tokens,
scheduled-event proof denial, and reset → short-event expiry → subject deletion
removing original touches, cross-property references and dedupe receipts. Necessary
hashed event suppression tombstones prevent replay after deletion. Correction audit
identifiers are redacted while principal/action history remains.

Additional final checks:

```sh
INGEST_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:53668/postgres cargo test -p analytics-ingest --all-targets
cargo clippy -p analytics-ingest --all-targets -- -D warnings
cargo fmt -p analytics-ingest -- --check
cargo run -q -p analytics-ingest --example openapi > /tmp/analytics-ingest-openapi.json
```

All targets passed; Clippy and formatting checks passed. The additive OpenAPI
fragment has 18 paths and resolvable component references. Integration merges
its `paths` and `components` into the foundation contract; do not replace the
foundation document. Existing disposable fixtures must be recreated when adopting
the finalized initial migration.

The browser implementation chat independently ran its Chromium SDK against the
checkpoint's real loopback/PostgreSQL harness: landing, button and SPA navigation
committed in one batch, retry deduplicated, a forged subject was rejected and reset
created a fresh boundary. Its `npm run test:browser:service` owns that reproduction.
The producer Rust example is covered by actual HTTP recovery tests. The TypeScript
producer uses the same generated wire DTOs and transactional SQL; its standalone
runtime validation is tracked with browser/integration work. Overall A01–A14,
full foundation credential tests, assembled export deletion, performance budgets
and deployment/publication remain the orchestrating acceptance scope; this module
does not claim those completed from isolated tests.
