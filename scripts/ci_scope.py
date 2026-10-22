#!/usr/bin/env python3
"""Choose a PR's validation tier; uncertain changes require full acceptance."""
import os
from pathlib import PurePosixPath
import subprocess
import sys


def select_suite(paths):
    paths = list(paths)
    if not paths:
        return "acceptance"
    suite = "fixtures"
    for path in paths:
        if PurePosixPath(path).name in {
            "Cargo.toml", "Cargo.lock", "package.json", "package-lock.json",
            ".npmignore", ".npmrc", ".dockerignore", "Dockerfile",
        }:
            return "acceptance"
        # Only prose in known documentation locations can avoid executable checks.
        if path in {"README.md", "AGENTS.md", "CHANGELOG.md"} or (
            path.startswith("docs/") and PurePosixPath(path).suffix == ".md"
        ):
            continue
        if path.startswith("examples/analysis/"):
            if suite == "fixtures":
                suite = "analysis"
            continue
        if path.startswith((
            "crates/analytics-ingest/", "crates/analytics-query/",
            "packages/browser/", "examples/producers/", "examples/static-site/",
        )):
            # Component changes still exercise real PostgreSQL, all Rust tests,
            # producer recovery, cohorts and both browser/service origin modes.
            suite = "integration"
            continue
        # Shared auth/core/server, migrations, contracts, dependencies, packaging,
        # harnesses/workflows and new/unclassified locations get the full journey,
        # fresh installation, restore, container and performance checks.
        return "acceptance"
    return suite


def changed_paths(base, cwd=None):
    # --no-renames includes both old and new paths, so moving code into a docs
    # directory cannot disguise deletion of executable code as a prose edit.
    result = subprocess.run(
        ["git", "diff", "--no-renames", "--name-only", "-z", f"{base}...HEAD"],
        cwd=cwd, capture_output=True, check=True,
    )
    return [path.decode("utf-8") for path in result.stdout.split(b"\0") if path]


def scope_for_event(event, base, cwd=None):
    if event != "pull_request" or not base:
        return "acceptance"
    try:
        return select_suite(changed_paths(base, cwd))
    except (subprocess.CalledProcessError, OSError, UnicodeError):
        print("Cannot establish changed paths; running full acceptance.", file=sys.stderr)
        return "acceptance"


if __name__ == "__main__":
    suite = scope_for_event(os.getenv("CI_EVENT", ""), os.getenv("CI_BASE_SHA", ""))
    print(f"suite={suite}")
    print(f"native={str(suite in ('integration', 'acceptance')).lower()}")
