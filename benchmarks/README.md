# Reproducible measurements

`budgets.json` records proposed engineering limits and exact workload sizes. They are engineering acceptance targets, not product guarantees. Actual service measurements and failed runs are retained in `results/`; A11 remains open until the final required workload passes or an explicitly reviewed limitation is recorded.

The HTTP driver reads a synthetic `IngestBatch` JSON template and a mode-0600 credential file. It generates fresh event IDs for insert measurements, counts accepted acknowledgments separately from HTTP success, records all response counts, and exits nonzero on incomplete acceptance. It never writes credentials into results. Run against an isolated stack only:

```sh
python3 benchmarks/ingestion.py --url http://127.0.0.1:8080/v1/ingest/server \
  --batch local/synthetic-batch.json --credential-file local/server-credential \
  --requests 1000 --concurrency 4 --output artifacts/benchmarks/normal.json
```

The automated `make benchmark` workload uses 250 normal requests × 20 events at concurrency 4, then 700 burst requests × 100 events at concurrency 32 with global admission capped at 16. It explicitly counts every accepted event, per-event rejection and HTTP-level refusal. Refused work is not included in accepted throughput. Record hardware, container limits, database settings, service build profile, dataset size, revision and dirty status alongside results. Do not compare debug and release runs. Warm-up results are separate from measured results; do not discard failures or include duplicate fast paths as new-event throughput.

A full performance run must additionally sample service container RSS before and during 100,000-row exports, record first-byte/total download time and checksum, inspect PostgreSQL active-query and statement statistics, test cancellation/timeouts, and exercise bounded producer outage recovery. Client throughput alone cannot establish server memory, database impact or durability. Browser-owned measurements are integrated from `packages/browser`; no parallel browser runtime is introduced here.


## Recorded baseline (failed latency targets)

`2026-10-21-before-ingestion-optimization.json` records a clean `6650472` native release run against isolated PostgreSQL 17.7 (one CPU/512 MiB per database container). All 5,000 normal events were accepted in 27.119 seconds (184.37 events/s), with accepted-request p50/p95/p99 of 331.55/807.49/1,261.52 ms. This **fails** the proposed 250 ms p95 and 1,000 ms p99 limits; limits were not raised to turn the result green.

The 70,000-event burst accepted 1,900 and explicitly refused 68,100 with HTTP 503 under admission 16; successful requests had p95 6,426.67 ms. The service did not acknowledge refused facts. Separate durable producer tests verify authoritative recovery. Service sampled RSS peaked at 16.55 MB normal and 20.09 MB burst. A 100,000-row/16,988,895-byte export completed and checksum-verified in 2.841 seconds; sampled service RSS rose from 20.22 to 22.15 MB. Database counters are recorded with their cumulative-statistics caveat. These observations do not establish a portable peak memory guarantee.

This failed baseline is retained for comparison with the targeted ingestion improvement and final run. No production traffic or source data was used.

## Optimized release result

`2026-10-21-optimized-initial-lock.json` records a clean `8d727d2` run after transaction-local batching and shared admission/deadlines, with the **same workload and limits**. Content hashes identify the exact crate, migration, dependency and harness trees independently of cherry-picked commit history. All proposed gates passed:

| Measurement | Failed baseline | Optimized |
| --- | ---: | ---: |
| Normal accepted events | 5,000 | 5,000 |
| Normal accepted events/s | 184.37 | 3,067.53 |
| Normal accepted-request p50 / p95 / p99 | 331.55 / 807.49 / 1,261.52 ms | 24.28 / 35.66 / 54.91 ms |
| Burst accepted / explicitly refused events | 1,900 / 68,100 | 12,400 / 57,600 |
| Burst accepted-request p95 | 6,426.67 ms | 428.11 ms |
| Normal / burst sampled process RSS peak | 16.55 / 20.09 MB | 17.04 / 25.28 MB |
| 100,000-row verified export | 2.841 s | 3.119 s |
| Export sampled RSS baseline → peak | 20.22 → 22.15 MB | 21.17 → 23.30 MB |

The optimized export is 16,988,895 bytes with a verified SHA-256. A separate authenticated reconnect/download received its first 64 KiB in 21.02 ms and completed in 2.359 s. Source counters recorded 100,918 tuples returned, 100,913 fetched, 81 blocks read, 3,957 hits and zero temporary bytes; those cumulative statistics include fixture polling and may lag.

The burst intentionally exceeds admission capacity. HTTP 503 responses are explicit backpressure, not acknowledged delivery; authoritative producers must retain/retry their journal facts. Successful-request percentiles are reported separately from fast refusals. Sampled RSS is a whole-process observation with 50 ms target sampling, not proof of a portable absolute peak. Browser bundle/interaction/heap evidence is recorded separately in `packages/browser/benchmarks/`.

The initial optimized run used the task-resolved lockfile. A final unchanged workload is rerun with the public integration branch's authoritative lockfile; both dependency graph hashes are recorded rather than treating them as interchangeable.
