"""Configure a manufactured source and retrieve actual server-generated cohort files."""
import json
import os
from pathlib import Path
import subprocess
import time
import uuid
from client import Client
from cohort_source import ROLES, columns, projection
from stack import ROOT

CUTOFF = "2026-07-01T01:00:00Z"
BUDGET = {"timeout_ms": 30000, "max_rows": 100000, "max_bytes": 67108864, "max_concurrency": 2}
COVERAGE = {"complete": True, "history_start": "2025-11-22T00:00:00Z", "history_end": CUTOFF, "sample_rate": 1, "policy_versions": [], "limitations": []}


def configure(service, known_coverage=True):
    source_id = str(uuid.uuid4())
    secret = service.secrets / ("cohort-postgres-" + source_id)
    secret.write_text(os.environ["TEST_SOURCE_DATABASE_URL"])
    secret.chmod(0o600)
    source = {"id": source_id, "name": "Manufactured cohort history", "kind": "postgres", "environment": "fixture", "endpoint": "postgresql://synthetic-source/synthetic", "secret_ref": secret.name, "allowed_schemas": ["cohort"], "allowed_tables": ["cohort.synthetic_" + role for role in ROLES], "allowed_columns": {"cohort.synthetic_" + role: ["fixture_ordinal"] + [column["name"] for column in columns(role)] for role in ROLES}, "allowed_projects": [], "budget": BUDGET, "semantics": {"coverage": COVERAGE} if known_coverage else {}, "enabled": True}
    service.client.tool("admin_execute", {"operation": "put", "resource_kind": "sources", "resource_id": source_id, "value": source})
    return source_id


def download_bundle(service, source_id, output):
    output = Path(output)
    output.mkdir(parents=True, mode=0o700)
    bundle = {"format_version": "cohort-bundle/1", "inputs": []}
    for role in ROLES:
        query = {"source_id": source_id, "property_id": None, "kind": "postgres", "query": {"sql": projection(role), "dataset": role}, "parameters": {}, "window": None, "cutoff": CUTOFF, "definition_version": "cohort-input/1", "budget": BUDGET}
        status = service.client.tool("export_create", {"query": query, "format": "jsonl", "retention_hours": 24})
        deadline = time.monotonic() + 45
        while status["status"] in ("queued", "running") and time.monotonic() < deadline:
            time.sleep(.05)
            status = service.client.tool("export_status", {"id": status["id"]})
        if status["status"] != "complete":
            raise AssertionError("manufactured cohort export did not complete: " + status["status"])
        # A fresh independent client retrieves each manifest/file after reconnect.
        reader = Client(service.url, service.handoff["credential"]["token"])
        reader.initialize()
        manifest = reader.tool("export_manifest", {"id": status["id"]})
        directory = output / role
        directory.mkdir(mode=0o700)
        (directory / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        for dataset in manifest["datasets"]:
            reader.download(dataset, directory)
        if len(manifest["datasets"]) != 1:
            raise AssertionError("expected one cohort projection per export")
        bundle["inputs"].append({"name": role, "manifest_file": role + "/manifest.json", "dataset_name": manifest["datasets"][0]["name"]})
    path = output / "manifest.json"
    path.write_text(json.dumps(bundle, indent=2) + "\n")
    return path


def analyze(bundle, output):
    subprocess.run([str(ROOT / "examples/analysis/analyze"), "--manifest", str(bundle), "--cutoff", CUTOFF, "--output", str(output)], check=True, cwd=ROOT, capture_output=True)
    return json.loads((Path(output) / "campaign-report.json").read_text())
