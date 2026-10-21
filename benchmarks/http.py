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
from urllib.request import Request, urlopen
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests" / "acceptance"))
from client import protected_secret


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
    args = parser.parse_args()
    if not 1 <= args.requests <= 100000 or not 1 <= args.concurrency <= 128:
        parser.error("bounded requests/concurrency required")
    template = json.loads(args.batch.read_text())
    if not 1 <= len(template["events"]) <= 1000:
        parser.error("batch must contain 1..1000 events")
    token = protected_secret(args.credential_file)
    def send(_):
        # Independent unique events exercise insert cost, not deduplication fast paths.
        events = [dict(event, event_id=str(uuid.uuid4())) for event in template["events"]]
        body = json.dumps({"events": events}).encode()
        start = time.perf_counter()
        accepted = 0
        status = "transport_error"
        try:
            with urlopen(Request(args.url, data=body, headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"}), timeout=args.timeout) as response:
                status = str(response.status)
                result = json.loads(response.read(8 * 1024 * 1024))
                accepted = sum(row["status"] == "accepted" for row in result["results"])
        except HTTPError as error:
            status = str(error.code)
        except (URLError, TimeoutError, ValueError, KeyError):
            pass
        return (time.perf_counter() - start) * 1000, status, accepted, len(body)
    start = time.perf_counter()
    # executor.map has at most 100k bounded inputs, at most concurrency active HTTP calls.
    with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        observations = list(pool.map(send, range(args.requests)))
    elapsed = time.perf_counter() - start
    latencies = [row[0] for row in observations]
    accepted = sum(row[2] for row in observations)
    report = {"schema_version": 1, "recorded_at": datetime.now(timezone.utc).isoformat(), "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(), "working_tree_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT)), "platform": platform.platform(), "python": platform.python_version(), "cpu_count": os.cpu_count(), "workload": {"requests": args.requests, "concurrency": args.concurrency, "events_per_request": len(template["events"]), "timeout_seconds": args.timeout, "request_bytes": sum(row[3] for row in observations)}, "elapsed_seconds": elapsed, "responses": dict(Counter(row[1] for row in observations)), "accepted_events": accepted, "accepted_events_per_second": accepted / elapsed, "request_latency_ms": {"p50": percentile(latencies, .50), "p95": percentile(latencies, .95), "p99": percentile(latencies, .99), "max": max(latencies), "mean": statistics.mean(latencies)}, "limitations": ["Client elapsed latency includes JSON handling and connection establishment.", "No server RSS, database impact, or export memory is inferred from HTTP latency.", "A successful HTTP status is not counted as accepted unless per-event acknowledgment says accepted."]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Measured {accepted} accepted events; evidence written to {args.output}")
    return 0 if accepted == args.requests * len(template["events"]) else 1


if __name__ == "__main__":
    sys.exit(main())
