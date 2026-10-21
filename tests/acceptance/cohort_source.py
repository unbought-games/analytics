"""Load manufactured analytical rows into a real external PostgreSQL source."""
import json
from pathlib import Path
import re
from stack import ROOT, compose

ROLES = ("acquisition", "identity_map", "memberships", "activity")
FIXTURES = ROOT / "tests/fixtures/cohorts/manufactured"


def identifier(value):
    if not re.fullmatch(r"[a-z_][a-z0-9_]*", value):
        raise ValueError("invalid manufactured column name")
    return '"' + value + '"'


def literal(value):
    if value is None:
        return "NULL"
    if isinstance(value, bool):
        return "TRUE" if value else "FALSE"
    if isinstance(value, (dict, list)):
        return "'" + json.dumps(value).replace("'", "''") + "'::jsonb"
    if not isinstance(value, str):
        raise ValueError("manufactured scalar must retain its exact string type")
    return "'" + value.replace("'", "''") + "'"


def columns(role):
    manifest = json.loads((FIXTURES / (role + ".manifest.json")).read_text())
    return manifest["datasets"][0]["columns"]


def projection(role):
    if role not in ROLES:
        raise ValueError("invalid cohort role")
    names = [column["name"] for column in columns(role)]
    return "SELECT " + ", ".join(identifier(name) for name in names) + " FROM cohort.synthetic_" + role + " ORDER BY fixture_ordinal"


def install(state):
    statements = ["BEGIN;", "CREATE SCHEMA cohort;", "GRANT USAGE ON SCHEMA cohort TO synthetic_reader;"]
    type_map = {"string": "text", "uint64-string": "text", "boolean": "boolean", "object": "jsonb"}
    for role in ROLES:
        schema = columns(role)
        table = "cohort.synthetic_" + role
        definitions = [identifier(column["name"]) + " " + type_map[column["data_type"]] for column in schema]
        statements.append("CREATE TABLE " + table + " (fixture_ordinal bigint PRIMARY KEY, " + ", ".join(definitions) + ");")
        rows = [json.loads(line) for line in (FIXTURES / (role + ".jsonl")).read_text().splitlines()]
        for index, row in enumerate(rows):
            values = [str(index)] + [literal(row.get(column["name"])) for column in schema]
            statements.append("INSERT INTO " + table + " VALUES (" + ", ".join(values) + ");")
        statements.append("GRANT SELECT ON " + table + " TO synthetic_reader;")
    statements.append("COMMIT;")
    compose(state, "exec", "-T", "sources", "psql", "-X", "-U", "fixture_migration", "-d", "synthetic", "-v", "ON_ERROR_STOP=1", input="\n".join(statements), text=True, capture_output=True)
