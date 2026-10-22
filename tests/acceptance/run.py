#!/usr/bin/env python3
"""Record commands and outcomes; missing integrated prerequisites are failures."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=["fixtures", "integration", "acceptance"], default="fixtures")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts" / "acceptance" / "commands.json")
    args = parser.parse_args()
    python = sys.executable
    commands = [[python, "-m", "unittest", "discover", "-s", "tests/acceptance", "-p", "test_fixtures.py", "-v"], [python, "-m", "unittest", "discover", "-s", "tests/acceptance", "-p", "test_client.py", "-v"], [python, "tests/acceptance/secret_check.py"]]
    missing = []
    if args.suite in ("integration", "acceptance"):
        if not os.getenv("ANALYTICS_STACK_STATE"):
            missing.append("isolated stack environment; invoke through stack.py run")
        else:
            commands.append([python, "-m", "unittest", "discover", "-s", "tests/acceptance", "-p", "test_stack.py", "-v"])
            commands.append([python, "-m", "unittest", "discover", "-s", "tests/acceptance", "-p", "test_build_context.py", "-v"])
            commands.append(["cargo", "test", "--locked", "--workspace"])
            commands.append(["cargo", "test", "--locked", "-p", "analytics-server", "--test", "independent_review", "--", "--ignored", "--nocapture"])
            commands.append(["cargo", "build", "--locked", "-p", "analytics-server"])
            commands.append(["cargo", "build", "--locked", "-p", "analytics-ingest", "--example", "outbox"])
            commands.append(["npm", "--prefix", "examples/producers", "run", "typecheck"])
            commands.append(["npm", "--prefix", "examples/producers", "test"])
            commands.append([python, "-m", "unittest", "discover", "-s", "examples/analysis", "-p", "test_*.py", "-v"])
            if (ROOT / "package.json").is_file():
                commands.append(["npm", "run", "test:browser"])
                commands.append(["npm", "run", "test:browser:service"])
            else:
                missing.append("browser workspace package.json")
    if args.suite == "acceptance":
        # Never allow discovery of zero tests to turn missing acceptance coverage green.
        # Do not allow unittest's 'Ran 0 tests OK' to turn missing coverage green.
        required = ["crates/analytics-server/src/main.rs", "packages/browser/package.json", "examples/analysis/analyze", "tests/acceptance/test_journey.py"]
        missing.extend(path for path in required if not (ROOT / path).is_file())
        if not missing:
            commands.append([python, "-m", "unittest", "discover", "-s", "tests/acceptance", "-p", "test_startup.py", "-v"])
            commands.append([python, "-m", "unittest", "discover", "-s", "tests/acceptance", "-p", "test_service.py", "-v"])
            commands.append([python, "-m", "unittest", "discover", "-s", "tests/acceptance", "-p", "test_journey.py", "-v"])
            commands.append([python, "tests/acceptance/stack.py", "run", "--", python, "crates/analytics-server/tests/cli_smoke.py"])
            commands.append([python, "tests/acceptance/stack.py", "run", "--", python, "-m", "unittest", "discover", "-s", "tests/acceptance", "-p", "test_container.py", "-v"])
            commands.append([python, "scripts/generate-contract.py", "--check"])
            commands.append([python, "tests/acceptance/package_check.py"])
            commands.append(["cargo", "build", "--release", "--locked", "-p", "analytics-server"])
            commands.append([python, "tests/acceptance/stack.py", "run", "--", python, "benchmarks/service.py"])
    results = []
    for command in commands:
        start = time.monotonic()
        print("Running:", " ".join(command), flush=True)
        try:
            result = subprocess.run(command, cwd=ROOT)
            code = result.returncode
        except FileNotFoundError:
            code = 127
        results.append({"command": command, "exit_code": code, "elapsed_seconds": time.monotonic() - start})
    passed = not missing and all(row["exit_code"] == 0 for row in results)
    evidence = {"recorded_at": datetime.now(timezone.utc).isoformat(), "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(), "dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT)), "source_tree_hashes": {part: subprocess.check_output(["git", "rev-parse", "HEAD:" + part], cwd=ROOT, text=True).strip() for part in ["crates", "migrations", "Cargo.lock", "packages/browser", "examples/producers", "tests/acceptance", "contracts"]}, "suite": args.suite, "passed": passed, "missing_prerequisites": missing, "commands": results, "acceptance_complete": passed and args.suite == "acceptance"}
    # Completion is for controlled synthetic acceptance only, never deployment/live collection.
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(evidence, indent=2) + "\n")
    if missing:
        print("Missing integrated prerequisites: " + ", ".join(missing))
    print("Evidence:", args.output)
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
