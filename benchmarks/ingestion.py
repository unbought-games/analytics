#!/usr/bin/env python3
"""Bounded client load measurement; all rows and errors remain synthetic."""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import time
from urllib.error import HTTPError, URLError
from urllib.request import Request
from urllib.parse import urlsplit
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests" / "acceptance"))
from client import Client, protected_secret


def percentile(values, p):
    return sorted(values)[max(0, math.ceil(len(values) * p) - 1)] if values else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--batch", type=Path, required=True, help="synthetic IngestBatch template")
    parser.add_argument("--credential-file", type=Path, required=True)
    parser.add_argument("--requests", type=int, default=1000)
    parser.add_argument("--concurrency", type=int, default=8)
    parser.add_argument("--timeout", type=float, default=10)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--allow-backpressure", action="store_true")
    args = parser.parse_args()
    if not 1 <= args.requests <= 100000 or not 1 <= args.concurrency <= 128:
        parser.error("bounded requests/concurrency required")
    template = json.loads(args.batch.read_text())
    if not 1 <= len(template["events"]) <= 1000:
        parser.error("batch must contain 1..1000 events")
    target = urlsplit(args.url)
    if target.hostname not in ("127.0.0.1", "localhost", "::1") or target.username or target.password or target.path != "/v1/ingest/server":
        parser.error("this acceptance driver only targets loopback synthetic server ingestion")
    token = protected_secret(args.credential_file)
    client = Client(args.url, token)
    def send(_):
        # Independent unique events exercise insert cost, not deduplication fast paths.
        events = [dict(event, event_id=str(uuid.uuid4())) for event in template["events"]]
        body = json.dumps({"events": events}).encode()
        start = time.perf_counter()
        accepted = 0
        outcomes = Counter()
        status = "transport_error"
        try:
            with client.opener.open(Request(args.url, data=body, headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"}), timeout=args.timeout) as response:
                status = str(response.status)
                result = json.loads(response.read(8 * 1024 * 1024))
                accepted = sum(row["status"] == "accepted" for row in result["results"])
                outcomes.update(row["status"] + (":" + row["code"] if row.get("code") else "") for row in result["results"])
        except HTTPError as error:
            status = str(error.code)
        except URLError as error:
            status = "transport_error:" + type(error.reason).__name__
        except TimeoutError:
            status = "transport_error:TimeoutError"
        except (ValueError, KeyError) as error:
            status = "invalid_ack:" + type(error).__name__
        return (time.perf_counter() - start) * 1000, status, accepted, len(body), outcomes
    start = time.perf_counter()
    # executor.map has at most 100k bounded inputs, at most concurrency active HTTP calls.
    with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        observations = list(pool.map(send, range(args.requests)))
    elapsed = time.perf_counter() - start
    latencies = [row[0] for row in observations]
    accepted = sum(row[2] for row in observations)
    accepted_latencies = [row[0] for row in observations if row[2] > 0]
    refused = sum(len(template["events"]) for row in observations if row[1] in ("429", "503"))
    outcomes = Counter()
    for row in observations:
        outcomes.update(row[4])
    report = {"schema_version": 1, "recorded_at": datetime.now(timezone.utc).isoformat(), "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(), "working_tree_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT)), "platform": platform.platform(), "python": platform.python_version(), "cpu_count": os.cpu_count(), "workload": {"requests": args.requests, "concurrency": args.concurrency, "events_per_request": len(template["events"]), "timeout_seconds": args.timeout, "request_bytes": sum(row[3] for row in observations)}, "elapsed_seconds": elapsed, "responses": dict(Counter(row[1] for row in observations)), "accepted_events": accepted, "event_outcomes": dict(outcomes), "http_refused_events": refused, "accepted_request_latency_ms": {"p50": percentile(accepted_latencies,.5), "p95": percentile(accepted_latencies,.95), "p99": percentile(accepted_latencies,.99)}, "accepted_events_per_second": accepted / elapsed, "request_latency_ms": {"p50": percentile(latencies, .50), "p95": percentile(latencies, .95), "p99": percentile(latencies, .99), "max": max(latencies), "mean": statistics.mean(latencies)}, "limitations": ["Client elapsed latency includes JSON handling and connection establishment.", "No server RSS, database impact, or export memory is inferred from HTTP latency.", "A successful HTTP status is not counted as accepted unless per-event acknowledgment says accepted."]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Measured {accepted} accepted events; evidence written to {args.output}")
    accounted = sum(outcomes.values()) + refused == args.requests * len(template["events"])
    permitted = set(outcomes).issubset({"accepted", "rejected:rate_limited"})
    return 0 if accepted == args.requests * len(template["events"]) or (args.allow_backpressure and accounted and permitted) else 1


if __name__ == "__main__":
    sys.exit(main())
