#!/usr/bin/env python3
"""Small deterministic tracked-content check; complements code review, not a vault."""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PATTERNS = {
    "private key": re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----"),
    "GitHub credential": re.compile(rb"(?:gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{60,})"),
    "AWS access key": re.compile(rb"AKIA[A-Z0-9]{16}"),
    "database credential": re.compile(rb"postgres(?:ql)?://[^\s:/]+:[^\s@${<>]+@"),
    "bearer JWT": re.compile(rb"Bearer eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
}


def main():
    paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).split(b"\0")
    failures = []
    for raw in paths:
        if not raw:
            continue
        path = Path(raw.decode())
        if not (ROOT / path).is_file():
            continue
        data = (ROOT / path).read_bytes()
        for label, pattern in PATTERNS.items():
            if pattern.search(data):
                failures.append(f"{path}: possible {label}")
        if any(part in {"secrets", ".env", "local", "exports"} for part in path.parts) or path.suffix in {".pem", ".key"}:
            failures.append(f"{path}: prohibited secret/local artifact path")
    for failure in failures:
        print(failure)  # Never print matching secret content.
    print(f"Tracked secret check: {len(failures)} finding(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
