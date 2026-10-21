#!/usr/bin/env python3
"""Compose the authoritative foundation and module OpenAPI generators."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def generate(package):
    result = subprocess.run(["cargo","run","--locked","--quiet","-p",package,"--example","openapi"],cwd=ROOT,check=True,stdout=subprocess.PIPE,text=True)
    return json.loads(result.stdout)


def verify_refs(value, document):
    if isinstance(value, dict):
        if "$ref" in value:
            ref=value["$ref"]
            if not ref.startswith("#/"):
                raise ValueError("Only self-contained local schema references are allowed")
            target=document
            for part in ref[2:].split("/"):
                target=target[part.replace("~1","/").replace("~0","~")]
        for child in value.values():
            verify_refs(child,document)
    elif isinstance(value,list):
        for child in value:
            verify_refs(child,document)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check",action="store_true")
    args=parser.parse_args()
    document=generate("analytics-core")
    # Early foundation placeholder is superseded by durable export creation.
    document["paths"].pop("/v1/query",None)
    for package in ("analytics-ingest","analytics-query"):
        module=generate(package)
        for path,operations in module["paths"].items():
            # Module-owned routes refine the early checkpoint's placeholder DTOs.
            document["paths"].setdefault(path,{}).update(operations)
        for group,entries in module.get("components",{}).items():
            target=document["components"].setdefault(group,{})
            for name,schema in entries.items():
                if name in target and target[name] != schema:
                    raise ValueError("Conflicting shared schema: "+group+"/"+name)
                target[name]=schema
    verify_refs(document,document)
    encoded=json.dumps(document,indent=2,sort_keys=True)+"\n"
    output=ROOT/"contracts/openapi.json"
    if args.check:
        if output.read_text()!=encoded:
            print("Combined OpenAPI is stale: run python3 scripts/generate-contract.py")
            return 1
    else:
        output.write_text(encoded)
    print("Combined OpenAPI:",len(document["paths"]),"paths; all local references resolved")
    return 0


if __name__=="__main__":
    sys.exit(main())
