"""Local-file input boundary; transport DTO adaptation is isolated here.

Rows follow cohort-input/1, a query result schema, not a universal game schema.
"""
import hashlib
import json
from pathlib import Path
from metrics import sample_fraction, stamp, utc
from normalize import normalize_acquisition

DATASETS = ("acquisition", "identity_map", "memberships", "activity")
ROW_FIELDS = {
    "acquisition_touches": {"touch_id": str, "property": str, "visit_id": (str, type(None)),
                            "campaign": (str, type(None)), "occurred_at": str, "linkage_permitted": bool},
    "acquisition": {"event_id": str, "kind": str, "campaign": (str, type(None)),
                    "occurred_at": str, "authoritative": bool, "property": str,
                    "visit_id": (str, type(None))},
    "identity_map": {"issuer": str, "subject": str, "application": str,
                     "local_account_id": (str, type(None)), "status": str},
    "memberships": {"application": str, "local_account_id": str, "world": str,
                    "player_id": str, "enrolled_at": str, "history": dict},
    "activity": {"event_id": str, "application": str, "world": str, "player_id": str,
                 "occurred_at": str, "recorded_at": str, "authoritative": bool,
                 "actor": str, "outcome": str, "action": str, "logical_ms": str},
}


def digest(content):
    return hashlib.sha256(content).hexdigest()


def parse_json(content):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("duplicate JSON object key")
            result[key] = value
        return result
    def nonfinite(_):
        raise ValueError("non-finite JSON value")
    return json.loads(content, object_pairs_hook=pairs, parse_constant=nonfinite)


def validate_coverage(value):
    if not isinstance(value, dict):
        raise ValueError("coverage must be an object")
    if value["status"] not in ("complete", "partial", "unavailable"):
        raise ValueError("invalid coverage status")
    if utc(value["start"]) > utc(value["end"]):
        raise ValueError("reversed coverage interval")
    sample_fraction(value["sample_fraction"])


def validate_row(name, row):
    if not isinstance(row, dict):
        raise ValueError("dataset row must be an object")
    for field, expected in ROW_FIELDS[name].items():
        if field not in row or not isinstance(row[field], expected):
            raise ValueError(f"invalid {name}.{field}")
    for key in ("occurred_at", "recorded_at", "enrolled_at"):
        if key in row:
            utc(row[key])
    if name == "acquisition":
        if row["kind"] not in ("visit", "signup", "revisit"):
            raise ValueError("unsupported acquisition kind")
        fields = {"visitor_namespace": str, "visitor_id": (str, type(None)), "linkage_permitted": bool} if row["kind"] == "visit" else {"issuer": str, "subject": str}
        for field, expected in fields.items():
            if not isinstance(row.get(field), expected):
                raise ValueError(f"invalid acquisition.{field}")
        if row["kind"] == "visit" and not isinstance(row["visit_id"], str):
            raise ValueError("visit needs an explicitly scoped visit ID")
        if row["kind"] == "signup" and row["authoritative"]:
            if not isinstance(row.get("attribution_rule"), str):
                raise ValueError("signup needs attribution rule provenance")
            if row["campaign"] is not None and not isinstance(row.get("touch_id"), str):
                raise ValueError("attributed signup needs selected touch provenance")
    if name == "acquisition" and any(k in row for k in ("attributed_visit_property", "attributed_visit_id", "attributed_visit_verified")):
        if type(row.get("attributed_visit_verified")) is not bool:
            raise ValueError("attributed visit requires explicit verification status")
        for key in ("attributed_visit_property", "attributed_visit_id"):
            if key not in row or not isinstance(row[key], (str, type(None))):
                raise ValueError("invalid attributed visit scope")
        if row["attributed_visit_verified"]:
            if (row["kind"] != "signup" or row["authoritative"] is not True
                    or not isinstance(row.get("touch_id"), str)
                    or not isinstance(row.get("attribution_rule"), str)
                    or not all(isinstance(row[k], str) and row[k] for k in ("attributed_visit_property", "attributed_visit_id"))):
                raise ValueError("verified attributed visit requires authoritative selected-touch evidence")
    if name == "identity_map":
        if row["status"] not in ("verified", "unmapped", "deleted"):
            raise ValueError("invalid mapping status")
        if row["status"] == "verified" and not isinstance(row["local_account_id"], str):
            raise ValueError("verified mapping needs local account")
    if name == "memberships":
        validate_coverage(row["history"])
    if name == "activity" and not row["logical_ms"].isdigit():
        raise ValueError("logical milliseconds must be a lossless nonnegative integer string")


def local_file(parent, value):
    relative = Path(value)
    candidate = (parent / relative).resolve()
    if relative.is_absolute() or not candidate.is_relative_to(parent):
        raise ValueError("input file must be inside downloaded bundle directory")
    if candidate.stat().st_size > 64 * 1024 * 1024:
        raise ValueError("local example limits each input file to 64 MiB; aggregate at the source")
    return candidate


def load_manifest(path, cutoff):
    """Load a local bundle of exact analytics-core ExportManifest v1 objects.

    Never downloads a URL. Retrieval/authentication remains the client's job.
    Missing/unknown completeness is never promoted to complete.
    """
    path = Path(path).resolve()
    raw = local_file(path.parent, path.name).read_bytes()
    bundle = parse_json(raw)
    if not isinstance(bundle, dict):
        raise ValueError("bundle must be an object")
    if bundle["format_version"] != "cohort-bundle/1":
        raise ValueError("unsupported analysis bundle version")
    data, metadata, sources = {}, {}, []
    origin_visits, origin_touches, origin_metadata = [], [], []
    for binding in bundle["inputs"]:
        role = binding["name"]
        name = "acquisition" if role == "acquisition_visits" else role
        extra = role in ("acquisition_visits", "acquisition_touches")
        if (name not in DATASETS and name != "acquisition_touches") or (not extra and name in data):
            raise ValueError("unknown or duplicate dataset binding")
        manifest_path = local_file(path.parent, binding["manifest_file"])
        manifest_raw = manifest_path.read_bytes()
        manifest = parse_json(manifest_raw)
        if not isinstance(manifest, dict):
            raise ValueError("server manifest must be an object")
        if manifest["format_version"] != "1":
            raise ValueError("unsupported server export version")
        if manifest["query"]["definition_version"] != "cohort-input/1":
            raise ValueError("unsupported query result definition")
        if manifest["status"] not in ("complete", "partial"):
            raise ValueError("export is not readable")
        if utc(cutoff) > utc(manifest["query"]["cutoff"]):
            raise ValueError("analysis cutoff exceeds export cutoff")
        utc(manifest["created_at"])
        utc(manifest["expires_at"])
        if not manifest["source_read_at"]:
            raise ValueError("missing source read boundary")
        for read_at in manifest["source_read_at"]:
            utc(read_at)
        matching = [d for d in manifest["datasets"] if d["name"] == binding["dataset_name"]]
        if len(matching) != 1:
            raise ValueError("dataset binding must resolve exactly once")
        entry = matching[0]
        if entry["media_type"] != "application/x-ndjson":
            raise ValueError("unsupported dataset media type")
        if not entry["columns"] or not manifest["provenance"] or not manifest["query"]:
            raise ValueError("missing schema or provenance")
        columns = entry["columns"]
        if not isinstance(columns, list) or any(not isinstance(c, dict) for c in columns):
            raise ValueError("columns must be a list of definitions")
        names = [c["name"] for c in columns]
        if len(set(names)) != len(names) or not set(ROW_FIELDS[name]).issubset(names):
            raise ValueError("schema lacks required columns or contains duplicate columns")
        for column in columns:
            if not isinstance(column["data_type"], str) or type(column["nullable"]) is not bool:
                raise ValueError("invalid column type metadata")
        if any(type(entry[k]) is not int or entry[k] < 0 for k in ("bytes", "row_count")):
            raise ValueError("invalid file count metadata")
        # The bundle explicitly binds a local download. Server URLs are never followed.
        body = local_file(manifest_path.parent, entry["file_name"]).read_bytes()
        if len(body) != entry["bytes"] or digest(body) != entry["sha256"]:
            raise ValueError("dataset integrity check failed")
        rows = []
        for line in body.decode("utf-8").splitlines():
            row = parse_json(line)
            validate_row(name, row)
            rows.append(row)
        if len(rows) != entry["row_count"]:
            raise ValueError("dataset row count mismatch")
        coverage = manifest["coverage"]
        if type(coverage["complete"]) is not bool:
            raise ValueError("coverage.complete must be boolean")
        complete = (coverage["complete"] and coverage["history_start"] is not None
                    and coverage["history_end"] is not None and coverage["sample_rate"] == 1)
        normalized = {"status": "complete" if complete else "partial",
                      "start": coverage["history_start"], "end": coverage["history_end"],
                      "sample_fraction": coverage["sample_rate"], "limitations": coverage["limitations"]}
        if complete:
            validate_coverage(normalized)
        meta = {"name": name, "coverage": normalized,
                "status": "complete" if manifest["status"] == "complete" and manifest["continuation"] is None else "partial"}
        if extra:
            (origin_visits if role == "acquisition_visits" else origin_touches).extend(rows)
            origin_metadata.append(meta)
        else:
            data[name], metadata[name] = rows, meta
        sources.append({"name": role, "manifest_file": binding["manifest_file"],
                        "manifest_sha256": digest(manifest_raw), "manifest": manifest})
    if set(data) != set(DATASETS):
        raise ValueError("all four named datasets are required, even when unavailable")
    normalization = None
    if origin_metadata:
        data["acquisition"], normalization = normalize_acquisition(data["acquisition"], origin_visits, origin_touches)
        inputs = [metadata["acquisition"], *origin_metadata]
        complete = all(m["status"] == "complete" and m["coverage"]["status"] == "complete" for m in inputs)
        if complete:
            start = max(utc(m["coverage"]["start"]) for m in inputs)
            end = min(utc(m["coverage"]["end"]) for m in inputs)
            complete = start <= end
            if complete:
                metadata["acquisition"]["coverage"].update(start=stamp(start), end=stamp(end))
        if not complete:
            metadata["acquisition"]["status"] = "partial"
    provenance = {"manifest_file": path.name, "manifest_sha256": digest(raw),
                  "format_version": bundle["format_version"], "inputs": sources}
    if normalization is not None:
        provenance["normalization"] = normalization
    return data, metadata, provenance
