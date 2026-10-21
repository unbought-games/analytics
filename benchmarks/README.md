# Reproducible measurements

`budgets.json` records proposed engineering limits and exact workload sizes. They are not measured results or product guarantees. A11 remains open until browser, ingestion, query, export, memory and failure measurements are all recorded on an integrated revision.

The HTTP driver reads a synthetic `IngestBatch` JSON template and a mode-0600 credential file. It generates fresh event IDs for insert measurements, counts accepted acknowledgments separately from HTTP success, records all response counts, and exits nonzero on incomplete acceptance. It never writes credentials into results. Run against an isolated stack only:

```sh
python3 benchmarks/http.py --url http://127.0.0.1:8080/v1/ingest/server \
  --batch local/synthetic-batch.json --credential-file local/server-credential \
  --requests 1000 --concurrency 4 --output artifacts/benchmarks/normal.json
```

Use a template containing exactly 20 events for normal and 100 for burst; use 2,000 requests and concurrency 32 for burst. Record hardware, container limits, database settings, service build profile, dataset size, revision and dirty status alongside results. Do not compare debug and release runs. Warm-up results are separate from measured results; do not discard failures or include duplicate fast paths as new-event throughput.

A full performance run must additionally sample service container RSS before and during 100,000-row exports, record first-byte/total download time and checksum, inspect PostgreSQL active-query and statement statistics, test cancellation/timeouts, and exercise bounded producer outage recovery. Client throughput alone cannot establish server memory, database impact or durability. Browser-owned measurements are integrated from `packages/browser`; no parallel browser runtime is introduced here.
