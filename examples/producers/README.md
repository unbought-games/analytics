# Synthetic durable journal producer

Application success depends on its own durable command journal, never analytics
availability or delivery queue capacity. These examples use a synthetic command
fact whose serialized fields match the analytics wire envelope; a real producer
maps its native committed journal into the envelope without inventing history.

Apply `outbox.sql` only to a disposable synthetic producer database. Rust
`capture(&mut transaction, &event, player)` records the business fact in the
caller's application transaction. TypeScript `applyCommand(pool, event, player)`
performs the same atomic synthetic command transaction. Rollback leaves no fact.
Neither function reads the queue, takes a projection lock, or makes a network call.

`project` scans undelivered retained facts without a forward-only cursor. It fills
at most 100 queue slots per call, under a dispatcher-only lock, stopping at 10,000
rows. The source journal remains the recoverable authority. An earlier transaction
that commits after later ones is still discovered. New application transactions
continue even while the queue is full. This bounds analytics work; the application's
original journal follows its own business storage and retention policy.

`dispatch` projects a page and leases at most 50 records, with an aggregate 240,000
byte payload budget, then commits before HTTP. The queue stores immutable payloads
and stable event IDs. Successful/duplicate ACKs atomically mark source facts
acknowledged and remove queue rows. Failed requests back off up to five minutes;
crashed leases expire after a minute. Conflicts and permanent rejections quarantine
the source fact for operator review, preserving it while allowing other work to
proceed. The queue can be deleted and reconstructed from unacknowledged source
facts, including after an uncertain ACK. Do not modify a fact's event ID on retry.

The shared `project.sql`, `claim.sql`, `acknowledge.sql`, `quarantine.sql` and
`retention.sql` are used by both implementations to prevent SQL behavior drift.
Run repeated dispatcher invocations under a bounded worker scheduler. Rust's CLI
keeps the same environment interface:

```sh
# Local environment points only at the configured producer and receiver.
# ANALYTICS_CREDENTIAL_FILE is an operator-protected credential file, not an argument.
cargo run -p analytics-ingest --example outbox
```

Required environment: `DATABASE_URL`, `ANALYTICS_URL`, `ANALYTICS_CREDENTIAL_FILE`.
No production endpoints or credentials are provided by this repository.

## Retention and coverage

A native journal is not an infinite analytics buffer. Select its business retention
policy independently (365 days is an illustrative configuration) and use
`prune_journal(pool, before)` / `pruneJournal(pool, before)` for bounded maintenance.
They delete at most 1,000 original records per call, atomically recording a coverage
gap before removing anything that has no confirmed delivery ACK. Queue rows cascade
away with their original source fact. Retention is never invoked automatically to
make room in the delivery queue.

`producer_delivery_coverage` reports `pending_journal`, `projected_queue`,
`quarantined`, `unconfirmed_pruned_events`, `retained_history_start` and
`last_coverage_gap`. `producer_coverage_gaps` records exact pruned counts and original
time/sequence bounds with reason `source_retention_without_ack`. After such a gap,
replay from the source is impossible; receiver presence may be uncertain after a
lost ACK. Analyses must report that uncertainty instead of claiming complete
history or zero activity. A native application implementing its own pruning must
provide the same atomic coverage accounting. Generic analytics cannot recover
business facts the producer no longer retains.

## Reproducible tests

Use a real disposable PostgreSQL instance. The Rust suite uses actual ingestion
HTTP, with isolated synthetic authorization; the Node suite uses a PostgreSQL-backed
HTTP receiver fixture that commits event IDs/hashes before ACK. No production
queries occur. Node 24 strips the TypeScript syntax; the runtime type-only wire
import uses the browser package's generated contract.

```sh
INGEST_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:56902/postgres cargo test -p analytics-ingest --all-targets
npm --prefix examples/producers ci --ignore-scripts
npm --prefix examples/producers run typecheck
PRODUCER_TEST_DATABASE_URL=postgres://postgres@127.0.0.1:56902/postgres npm --prefix examples/producers test
```

Each producer regression commits 10,025 facts, including 25 after the queue reaches
10,000 during an outage, rebuilds a lost queue, delivers all facts exactly once at
the receiver, and retries an uncertain ACK. It proves 50-row batch and queue bounds
and reports one pruned unacknowledged fact explicitly. The TypeScript test also
measures heap growth and asserts it remains below 128 MiB for this fixed workload;
this is a fixture budget, not a universal deployment guarantee.


Recorded corrective run: the TypeScript regression passed in 38.49 seconds while
sharing the disposable database with Rust tests. It recovered all 10,025 facts,
observed at most 50 events / 30,262 wire bytes per request, and measured 32,058,144
bytes of heap growth. Initial queue saturation was exactly 10,000 rows; rebuilding
while dispatching peaked at 5,000. These are controlled synthetic measurements.
Rust's final run passed two unit tests and eleven PostgreSQL suites in 104.09
seconds, including the actual-ingestion 10,025-fact recovery and isolated quotas.
Clippy and formatting checks passed. Type checking used the generated wire contract
from browser commit `ab563646e449137850c815d37b0dfb00f52a8af1`; in this isolated
worktree it ran in an ignored mirror using:

```sh
examples/producers/node_modules/.bin/tsc -p local/producer-typecheck/examples/producers/tsconfig.json
```

The normal `typecheck` command above resolves the same wire import directly in the
assembled repository. No package publication, production data or deployment was
part of these checks.
