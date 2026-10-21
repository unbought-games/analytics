#!/usr/bin/env python3
"""Inspect npm's actual dry-run tarball inventory without executing package scripts."""
import json
from pathlib import Path
import subprocess

ROOT=Path(__file__).resolve().parents[2]
result=subprocess.run(["npm","pack","--dry-run","--json","--ignore-scripts"],cwd=ROOT/"packages/browser",check=True,capture_output=True,text=True)
files=json.loads(result.stdout)[0]["files"]
for entry in files:
    name=entry["path"]
    if name not in {"package.json","README.md","LICENSE"} and not name.startswith("dist/"):
        raise SystemExit("Unexpected package entry: "+name)
    if any(part in {"..","secrets","local",".env"} for part in Path(name).parts):
        raise SystemExit("Private path in package")
required={"dist/index.js","dist/index.cjs","dist/analytics.global.js","dist/index.d.ts"}
if not required.issubset({entry["path"] for entry in files}):
    raise SystemExit("Built browser distribution is incomplete")
print("Verified npm package inventory:",len(files),"allowlisted files")
