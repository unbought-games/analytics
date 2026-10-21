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
