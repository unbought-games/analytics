#!/usr/bin/env python3
"""Small remote MCP client. Reads a credential file; never accepts a token argument."""
import argparse
import json
import os
from pathlib import Path
import stat
import urllib.request


def protected_read(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077:
        raise ValueError("Credentials must be a regular file with mode 0600 or stricter")
    return json.loads(path.read_text())


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", required=True)
    parser.add_argument("--credentials", type=Path, required=True)
    parser.add_argument("--tool", default="identity")
    parser.add_argument("--arguments", type=Path)
    parser.add_argument("--secret-output", type=Path)
    args = parser.parse_args()
    arguments = json.loads(args.arguments.read_text()) if args.arguments else {}
    credentials = protected_read(args.credentials)
    token = credentials.get("credential", credentials)["token"]
    secret_operation = args.tool == "admin_execute" and arguments.get("resource_kind") == "credentials" and arguments.get("operation") in ("issue", "rotate")
    if secret_operation and not args.secret_output:
        parser.error("credential issue/rotate requires --secret-output (new protected file)")
    output = None
    if args.secret_output:
        if args.secret_output.parent.stat().st_mode & 0o077:
            parser.error("secret output parent directory must have mode 0700")
        output = os.open(args.secret_output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        def call(payload):
            request = urllib.request.Request(args.url.rstrip("/") + "/mcp", data=json.dumps(payload).encode(), headers={
                "Authorization": "Bearer " + token, "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream", "MCP-Protocol-Version": "2025-11-25"})
            with urllib.request.urlopen(request, timeout=30) as response:
                return None if response.status == 202 else json.load(response)
        initialized = call({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "analytics-admin-example", "version": "1"}}})
        if initialized.get("error"):
            raise RuntimeError("MCP initialization failed")
        call({"jsonrpc": "2.0", "method": "notifications/initialized"})
        result = call({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": args.tool, "arguments": arguments}})
        if output is not None:
            with os.fdopen(output, "w") as stream:
                output = None
                saved = result
                if secret_operation and "token" in result.get("result", {}).get("structuredContent", {}):
                    saved = {"credential": result["result"]["structuredContent"]}
                json.dump(saved, stream, indent=2)
                stream.flush()
                os.fsync(stream.fileno())
            print("Response saved to protected file")
        else:
            print(json.dumps(result, indent=2))
        if result.get("error") or result.get("result", {}).get("isError"):
            raise SystemExit(1)
    finally:
        if output is not None:
            os.close(output)


if __name__ == "__main__":
    main()
