# Committed activity verification

Branch feat/retained-activity, based on 6b999d825b920ef56db257aa97cbdd35d11948d4.
All fixtures are synthetic. No deployment, policy/retention configuration, source
registration, production data access or backfill was performed.

- `INGEST_TEST_DATABASE_URL=<disposable> CARGO_BUILD_JOBS=2 cargo test --locked -p analytics-ingest --test postgres -- --test-threads=1`: 18 passed. Includes authoritative admission, consent refusal, duplicate/conflict handling, detailed-event expiry, compact activity survival and subject deletion/no resurrection.
- `QUERY_TEST_DATABASE_URL=<disposable> CARGO_BUILD_JOBS=2 cargo test --locked -p analytics-query --test analytics`: 1 passed. Includes property/cutoff/expiry isolation and lossless activity time export.
- `CARGO_BUILD_JOBS=2 cargo clippy -p analytics-ingest -p analytics-query --all-targets -- -D warnings`: passed.
- Formatting and diff whitespace checks passed. Required complete repository CI
  is followed on the PR head; no local full build or browser suite was duplicated.

Ingestion test preparation took 49.12 seconds; the first full ingestion execution
was 28.88 seconds. Query dependency preparation took 1m44s and execution 0.57s.
These are development timings, not production throughput claims.

Review identified missing occurrence/expiry indexes for long activity histories.
Added property/time indexes (including received_at for shortened policies) and
verified narrow reads and cleanup predicates avoid sequential history scans with
20,000 synthetic facts. No application failures occurred in the initial ingestion/query checks. The schema
is additive and tested on real PostgreSQL. The browser acceptance harness includes
the new migration because privacy retention/deletion now covers compact activity.

See [the contract](committed-activity.md) for producer responsibilities, gap
semantics, retention/deletion behavior and rollout/rollback. Passing these checks
cannot establish a producer's live historical completeness or authorize removal
of its source records. Source reconciliation and production activation remain
separate operator responsibilities.
