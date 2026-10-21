#!/usr/bin/env python3
"""Manage only this checkout's isolated, generated Compose resources."""
import argparse
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
LOCAL = ROOT / "local" / "acceptance"


def compose(state, *args, **kwargs):
    env = dict(os.environ, ANALYTICS_STACK_DIR=str(state["directory"]), ANALYTICS_UID=str(os.getuid()), ANALYTICS_GID=str(os.getgid()))
    return subprocess.run(["docker", "compose", "--project-name", state["project"], "--file", str(ROOT / "compose.yaml"), *args], cwd=ROOT, env=env, check=True, **kwargs)


def provision():
    LOCAL.mkdir(parents=True, exist_ok=True, mode=0o700)
    directory = Path(tempfile.mkdtemp(prefix="run-", dir=LOCAL))
    state = {"project": "analytics-test-" + secrets.token_hex(6), "directory": str(directory)}
    for name in ("database_password", "source_password"):
        target = directory / name
        target.write_text(secrets.token_hex(32))
        target.chmod(0o600)
    password = (directory / "database_password").read_text()
    dsn = directory / "engine_database_url"
    dsn.write_text(f"postgres://analytics_migration:{password}@postgres:5432/analytics")
    dsn.chmod(0o600)
    (directory / "exports").mkdir(mode=0o700)
    (directory / "secrets").mkdir(mode=0o700)
    (directory / "state.json").write_text(json.dumps(state))
    (directory / "state.json").chmod(0o600)
    try:
        compose(state, "up", "--detach", "--build", "--wait", "--wait-timeout", "180")
        password = (directory / "source_password").read_text()
        # Hex-only generated credential is sent through stdin, never an argument/log.
        sql = f"ALTER ROLE synthetic_reader PASSWORD '{password}';\n"
        compose(state, "exec", "-T", "sources", "psql", "-U", "fixture_migration", "-d", "synthetic", "-v", "ON_ERROR_STOP=1", input=sql, text=True, stdout=subprocess.DEVNULL)
        from cohort_source import install
        install(state)
        return state
    except BaseException:
        shutdown(state)
        raise


def load_state(path):
    path = Path(path).resolve()
    if LOCAL.resolve() not in path.parents:
        raise ValueError("state must belong to this checkout's local/acceptance directory")
    state = json.loads(path.read_text())
    if not re.fullmatch(r"analytics-test-[0-9a-f]{12}", state["project"]):
        raise ValueError("refusing non-test Compose project")
    if Path(state["directory"]).resolve() != path.parent:
        raise ValueError("state directory mismatch")
    return state


def shutdown(state):
    compose(state, "down", "--volumes", "--remove-orphans", "--timeout", "10")
    for name in ("database_password", "source_password", "engine_database_url", "host_database_url"):
        (Path(state["directory"]) / name).unlink(missing_ok=True)


def endpoint(state, service, port):
    value = compose(state, "port", service, str(port), capture_output=True, text=True).stdout.strip()
    if not re.fullmatch(r"127\.0\.0\.1:\d+", value):
        raise ValueError("expected loopback-only published port")
    return value


def environment(state):
    directory = Path(state["directory"])
    password = (directory / "database_password").read_text().strip()
    source_password = (directory / "source_password").read_text().strip()
    host_dsn = directory / "host_database_url"
    host_dsn.write_text(f"postgres://analytics_migration:{password}@{endpoint(state, 'postgres', 5432)}/analytics")
    host_dsn.chmod(0o600)
    return dict(os.environ,
        ANALYTICS_DATABASE_URL_FILE=str(host_dsn),
        DATABASE_URL=f"postgres://analytics_migration:{password}@{endpoint(state, 'postgres', 5432)}/analytics",
        TEST_DATABASE_URL=f"postgres://analytics_migration:{password}@{endpoint(state, 'postgres', 5432)}/analytics",
        TEST_SOURCE_DATABASE_URL=f"postgres://synthetic_reader:{source_password}@{endpoint(state, 'sources', 5432)}/synthetic",
        TEST_FIXTURE_URL="http://" + endpoint(state, "fixtures", 8081),
        ANALYTICS_STACK_STATE=str(directory / "state.json"),
        ANALYTICS_EVIDENCE_DIR=str(directory / "evidence"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    sub.add_parser("up")
    down = sub.add_parser("down")
    down.add_argument("--state", type=Path, required=True)
    run = sub.add_parser("run")
    run.add_argument("--state", type=Path)
    run.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.action == "down":
        if not args.state:
            parser.error("down requires --state")
        shutdown(load_state(args.state))
        return 0
    if args.action == "up":
        state = provision()
        print("State:", Path(state["directory"]) / "state.json")
        return 0
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("run requires a command after --")
    state = load_state(args.state) if args.state else provision()
    try:
        return subprocess.run(command, cwd=ROOT, env=environment(state)).returncode
    finally:
        if not args.state:
            shutdown(state)


if __name__ == "__main__":
    sys.exit(main())
