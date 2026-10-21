#!/usr/bin/env python3
"""Manufacture the acceptance export deterministically; no external inputs."""
import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
from uuid import UUID, uuid5

NAMESPACE = UUID("2a309b7e-b65a-47d0-8e21-1d1fd1270118")
START = datetime(2026, 1, 1, tzinfo=timezone.utc)
ANCHOR = START + timedelta(hours=1)
CUTOFF = ANCHOR + timedelta(days=181)
ISSUER = "https://identity.example"
CAMPAIGN = "glass-garden"


def uid(name):
    return str(uuid5(NAMESPACE, name))


def timestamp(value):
    return value.isoformat().replace("+00:00", "Z")


def coverage(start=None, end=None, status="complete"):
    return {"start": timestamp(start or START - timedelta(days=40)),
            "end": timestamp(end or CUTOFF), "status": status, "sample_fraction": "1",
            "reason": None if status == "complete" else "manufactured missing historical journal"}


def manufacture():
    rows = {name: [] for name in ("acquisition", "identity_map", "memberships", "activity")}

    def visit(label, when=START, campaign=CAMPAIGN, linked=True):
        row = {"event_id": uid("visit-event/" + label), "kind": "visit", "campaign": campaign,
               "occurred_at": timestamp(when), "authoritative": False, "property": "portal",
               "visit_id": uid("visit/" + label), "visitor_namespace": "portal/browser",
               "visitor_id": uid("browser/" + label) if linked else None, "linkage_permitted": linked}
        rows["acquisition"].append(row)
        return row

    def signup(label, when=START, campaign=CAMPAIGN, issuer=ISSUER):
        touch = visit(label, when - timedelta(minutes=10), campaign)
        row = {"event_id": uid("signup/" + label), "kind": "signup", "campaign": campaign,
               "occurred_at": timestamp(when), "authoritative": True, "property": "portal",
               "visit_id": touch["visit_id"], "issuer": issuer,
               "subject": uid("subject/" + label), "touch_id": touch["event_id"] if campaign else None,
               "attribution_rule": "first-eligible-tagged-touch-30d/1"}
        rows["acquisition"].append(row)
        return row

    def mapping(account, label, application="game-cobalt", status="verified"):
        row = {"issuer": account["issuer"], "subject": account["subject"],
               "application": application, "local_account_id": uid("local/" + label) if status == "verified" else None,
               "status": status}
        rows["identity_map"].append(row)
        return row

    def membership(mapping, label, world="delta", enrolled=START, history=None):
        row = {"application": mapping["application"], "local_account_id": mapping["local_account_id"],
               "world": world, "player_id": uid("player/" + label), "enrolled_at": timestamp(enrolled),
               "history": history or coverage()}
        rows["memberships"].append(row)
        return row

    def action(member, label, when=ANCHOR, actor="human", outcome="applied", action="construct", authoritative=True):
        rows["activity"].append({"event_id": uid("action/" + label), "application": member["application"],
                                 "world": member["world"], "player_id": member["player_id"],
                                 "occurred_at": timestamp(when), "recorded_at": timestamp(when + timedelta(minutes=2)),
                                 "authoritative": authoritative, "actor": actor, "outcome": outcome,
                                 "action": action, "logical_ms": "9007199254740993", "rules_version": "synthetic-rules/1"})

    alpha = signup("alpha")
    alpha_map = mapping(alpha, "alpha")
    first = membership(alpha_map, "alpha-delta")
    second = membership(alpha_map, "alpha-epsilon", "epsilon")
    third = membership(mapping(alpha, "alpha-other", "game-ochre"), "alpha-other")
    action(first, "alpha-first")
    for day in (1, 7, 30, 180):
        action(second, f"alpha-return-{day}", ANCHOR + timedelta(days=day))
        action(third, f"alpha-other-{day}", ANCHOR + timedelta(days=day, hours=1))
    # Exact repeated delivery does not duplicate accounts, memberships or activity.
    rows["identity_map"].append(dict(alpha_map))
    rows["memberships"].append(dict(first))
    rows["activity"].append(dict(rows["activity"][0]))
    rows["acquisition"].append(dict(alpha))

    edge = membership(mapping(signup("edge"), "edge"), "edge")
    action(edge, "edge-first")
    for day in (1, 7, 30, 180):
        action(edge, f"edge-before-{day}", ANCHOR + timedelta(days=day) - timedelta(microseconds=1))
        action(edge, f"edge-end-{day}", ANCHOR + timedelta(days=day + 1))
    # Same opaque subject in another issuer must not inherit alpha's returns.
    other = signup("same-subject-other-issuer", issuer="https://second-identity.example")
    other["subject"] = alpha["subject"]
    other_member = membership(mapping(other, "other-issuer"), "other-issuer")
    action(other_member, "other-issuer-first")
    mapping(signup("unmapped"), "unmapped", status="unmapped")
    mapping(signup("deleted"), "deleted", status="deleted")
    expired = membership(mapping(signup("expired"), "expired"), "expired",
                         history=coverage(START + timedelta(days=20)))
    action(expired, "expired-first-observed", START + timedelta(days=20, hours=1))
    action(expired, "expired-return", START + timedelta(days=21, hours=1))
    truncated = membership(mapping(signup("truncated"), "truncated"), "truncated",
                           history=coverage(end=START + timedelta(days=100)))
    action(truncated, "truncated-first")
    scheduled = membership(mapping(signup("scheduled"), "scheduled"), "scheduled")
    for label, overrides in (("schedule", {"actor": "scheduler"}), ("background", {"actor": "background"}),
                             ("reject", {"outcome": "rejected"}), ("login", {"action": "login"}),
                             ("read", {"action": "inspect"}), ("browser", {"authoritative": False}),
                             ("other-player", {"actor": "other_player"})):
        action(scheduled, label, **overrides)
    recent = CUTOFF - timedelta(hours=12)
    young = membership(mapping(signup("young", recent), "young"), "young", enrolled=recent)
    action(young, "young-first", recent + timedelta(hours=1))
    mapping(signup("not-enrolled"), "not-enrolled")
    unknown = membership(mapping(signup("unknown", campaign=None), "unknown"), "unknown")
    action(unknown, "unknown-first")
    returning = signup("established", START - timedelta(days=10), campaign="earlier-campaign")
    mapping(returning, "established")
    rows["acquisition"].append({"event_id": uid("revisit/established"), "kind": "revisit", "campaign": CAMPAIGN,
                                "occurred_at": timestamp(START + timedelta(days=1)), "authoritative": True,
                                "property": "portal", "visit_id": None, "issuer": returning["issuer"],
                                "subject": returning["subject"]})
    visit("unconverted")
    visit("minimal-policy", linked=False)
    return rows


def write_export(output, rows=None):
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    rows = manufacture() if rows is None else rows
    bundle = {"format_version": "cohort-bundle/1", "inputs": []}
    for name, records in rows.items():
        records = sorted(records, key=lambda r: json.dumps(r, sort_keys=True))
        body = "".join(json.dumps(r, sort_keys=True, separators=(",", ":")) + "\n" for r in records).encode()
        (output / f"{name}.jsonl").write_bytes(body)
        columns = []
        for field in sorted({key for row in records for key in row}):
            values = [r[field] for r in records if field in r and r[field] is not None]
            kind = "object" if values and isinstance(values[0], dict) else "boolean" if values and isinstance(values[0], bool) else "string"
            namespace = {"issuer": "identity/issuer", "subject": "identity/issuer-subject",
                         "local_account_id": "application/local-account", "player_id": "application/world/player",
                         "visit_id": "property/visit", "visitor_id": "visitor-namespace/browser"}.get(field)
            columns.append({"name": field, "data_type": "uint64-string" if field == "logical_ms" else kind,
                            "nullable": any(r.get(field) is None for r in records), "namespace": namespace,
                            "unit": "milliseconds" if field == "logical_ms" else None,
                            "description": "UTC wall clock" if field.endswith("_at") else None})
        export_id = uid("export/" + name)
        manifest = {"export_id": export_id, "format_version": "1", "status": "complete",
                    "datasets": [{"name": name, "file_name": f"{name}.jsonl", "media_type": "application/x-ndjson",
                                  "bytes": len(body), "sha256": hashlib.sha256(body).hexdigest(),
                                  "row_count": len(records), "columns": columns,
                                  "download_url": f"https://analytics.example/v1/exports/{export_id}/files/{name}.jsonl",
                                  "ordering": ["canonical JSON row (lexical)"]}],
                    "query": {"source_id": uid("source/" + name), "property_id": None, "kind": "postgres",
                              "query": {"sql": f"SELECT * FROM synthetic_{name}"}, "parameters": {},
                              "window": {"start": coverage()["start"], "end": timestamp(CUTOFF)},
                              "cutoff": timestamp(CUTOFF), "definition_version": "cohort-input/1",
                              "budget": {"timeout_ms": 1000, "max_rows": 10000, "max_bytes": 1048576, "max_concurrency": 1}},
                    "created_at": timestamp(CUTOFF + timedelta(hours=1)),
                    "expires_at": "2027-07-01T01:00:00Z", "source_read_at": [timestamp(CUTOFF + timedelta(minutes=30))],
                    "watermark": "synthetic-receipt/1000", "continuation": None,
                    "coverage": {"complete": True, "history_start": coverage()["start"], "history_end": timestamp(CUTOFF),
                                 "limitations": [], "sample_rate": 1, "policy_versions": [1]},
                    "provenance": {"source": "synthetic-" + name, "environment": "fixture", "property": "portal",
                                   "producer": "deterministic-cohort-generator/1", "snapshot_semantics": "independent source reads",
                                   "timestamp_convention": "UTC wall clocks; logical milliseconds separate",
                                   "null_semantics": "unknown or not applicable, never zero"}}
        manifest_name = f"{name}.manifest.json"
        (output / manifest_name).write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        bundle["inputs"].append({"name": name, "manifest_file": manifest_name, "dataset_name": name})
    (output / "manifest.json").write_text(json.dumps(bundle, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return bundle


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(__file__).parent / "manufactured")
    write_export(parser.parse_args().output)
