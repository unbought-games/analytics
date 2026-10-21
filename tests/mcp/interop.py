#!/usr/bin/env python3
"""Official MCP SDK interop against a disposable, already-running Rust service."""
from __future__ import annotations

import argparse
import asyncio
from contextlib import asynccontextmanager
from datetime import datetime, timedelta, timezone
import hashlib
import importlib.metadata
import json
import logging
from pathlib import Path
import re
import stat
import sys
from urllib.parse import urljoin, urlsplit
from uuid import uuid4

import httpx2
from mcp import Client
from mcp.client.streamable_http import streamable_http_client

PROTOCOL = "2025-11-25"


def check(condition, message):
    if not condition:
        raise AssertionError(message)


def read_credential(path: Path) -> str:
    info = path.lstat()
    check(stat.S_ISREG(info.st_mode) and not info.st_mode & 0o077,
          "Credential input must be a regular protected file (0600 or stricter)")
    value = json.loads(path.read_text())
    return value.get("credential", value)["token"]


def same_origin(base: str, location: str) -> str:
    result = urljoin(base + "/", location)
    left, right = urlsplit(base), urlsplit(result)
    check((left.scheme, left.netloc) == (right.scheme, right.netloc),
          "Artifact/metadata URL must stay on the configured service origin")
    check(not right.username and not right.password, "URLs cannot carry credentials")
    return result


@asynccontextmanager
async def sdk(base, token, observed):
    async def on_request(request):
        if request.method == "POST" and request.url.path == "/mcp":
            payload = json.loads(request.content)
            observed["methods"].add(payload.get("method"))

    async def on_response(response):
        observed["statuses"].append(response.status_code)

    headers = {"Authorization": "Bearer " + token} if token else {}
    async with httpx2.AsyncClient(headers=headers, timeout=15, follow_redirects=False,
                                 event_hooks={"request": [on_request], "response": [on_response]}) as http:
        transport = streamable_http_client(base + "/mcp", http_client=http)
        # The server intentionally pins the released 2025 protocol. This uses the
        # SDK's documented initialize/initialized path, not a handwritten client.
        async with Client(transport, mode="legacy", read_timeout_seconds=15, cache=None) as client:
            check(client.protocol_version == PROTOCOL, "Unexpected negotiated protocol version")
            yield client


async def tool(client, name, arguments=None):
    result = await client.call_tool(name, arguments or {})
    check(not result.is_error, "Official SDK tool call failed: " + name)
    check(isinstance(result.structured_content, dict), "Tool structured output must be an object")
    return result.structured_content


async def admin(client, operation, kind, key=None, value=None):
    args = {"operation": operation, "resource_kind": kind}
    if key is not None:
        args["resource_id"] = key
    if value is not None:
        args["value"] = value
    return await tool(client, "admin_execute", args)


async def denied_sdk(base, token):
    observed = {"methods": set(), "statuses": []}
    rejected = False
    try:
        async with sdk(base, token, observed) as client:
            await tool(client, "identity")
    except Exception:
        # Do not dump SDK exception groups or request context containing credentials.
        rejected = True
    check(rejected and 401 in observed["statuses"], "SDK must observe an HTTP 401 for rejected credential")


async def artifact(base, manifest_url, token):
    manifest_url = same_origin(base, manifest_url)
    async with httpx2.AsyncClient(headers={"Authorization": "Bearer " + token},
                                 timeout=30, follow_redirects=False) as http:
        response = await http.get(manifest_url)
        check(response.status_code == 200, "Authenticated manifest retrieval failed")
        manifest = response.json()
        check(manifest["status"] in ("complete", "partial"), "Fixture export is not retrievable")
        datasets = manifest["datasets"]
        check(bool(datasets), "Fixture manifest must include at least one dataset")
        downloaded = 0
        for dataset in datasets:
            url = same_origin(base, dataset["download_url"])
            digest, size = hashlib.sha256(), 0
            async with http.stream("GET", url) as stream:
                check(stream.status_code == 200, "Authenticated file retrieval failed")
                async for chunk in stream.aiter_bytes():
                    size += len(chunk)
                    check(size <= 16 * 1024 * 1024, "Use a small synthetic artifact fixture (16 MiB maximum)")
                    digest.update(chunk)
            check(size == dataset["bytes"], "Dataset byte count mismatch")
            check(digest.hexdigest() == dataset["sha256"], "Dataset checksum mismatch")
            downloaded += size
        return {"files": len(datasets), "bytes": downloaded, "manifest_url": manifest_url,
                "file_url": same_origin(base, datasets[0]["download_url"])}


async def run(args):
    base = args.url.rstrip("/")
    parsed = urlsplit(base)
    check(parsed.scheme in ("http", "https") and not parsed.username and not parsed.password
          and not parsed.query and not parsed.fragment and parsed.path in ("", "/"),
          "--url must be a service origin without credentials, query, or path")
    # This test creates configuration. It is deliberately restricted to local fixtures.
    check(parsed.hostname in ("localhost", "127.0.0.1", "::1"), "Run only against a loopback disposable test service")
    bearer = read_credential(args.credentials)
    observed = {"methods": set(), "statuses": []}
    async with httpx2.AsyncClient(timeout=15, follow_redirects=False) as http:
        challenge = await http.post(base + "/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "ping"})
        check(challenge.status_code == 401, "Unauthenticated MCP request must return 401")
        match = re.search(r'resource_metadata="([^"]+)"', challenge.headers.get("www-authenticate", ""))
        check(match is not None, "Missing OAuth protected-resource challenge")
        metadata_response = await http.get(same_origin(base, match.group(1)))
        check(metadata_response.status_code == 200, "Protected-resource discovery failed")
        metadata = metadata_response.json()
        check(metadata["resource"] == base + "/mcp", "Resource audience mismatch")
        check("header" in metadata["bearer_methods_supported"], "Bearer header capability missing")
        check(isinstance(metadata["authorization_servers"], list), "Authorization-server discovery list missing")
        if args.expect_issuer:
            check(args.expect_issuer in metadata["authorization_servers"], "Configured issuer missing from discovery")
    await denied_sdk(base, None)
    property_id = str(uuid4())
    temporary_credential = None
    created_property = False
    artifact_result = None
    async with sdk(base, bearer, observed) as owner:
        listing = await owner.list_tools()
        names = {entry.name for entry in listing.tools}
        check({"identity", "admin_execute", "status"}.issubset(names), "Required administration tools missing")
        identity = await tool(owner, "identity")
        principal_id = identity["principal_id"]
        now = datetime.now(timezone.utc).isoformat()
        property_value = {"id": property_id, "name": "Synthetic official SDK fixture",
                          "allowed_origins": ["https://synthetic.example"], "handoff_audiences": [], "enabled": True,
                          "policy": {"profile": "minimal", "version": 1, "effective_at": now,
                                     "cross_property": False, "event_retention_days": 30, "identity_retention_days": 190,
                                     "attribution_window_days": 30, "allowed_query_keys": [],
                                     "respect_global_privacy_control": True}}
        try:
            created = await admin(owner, "put", "properties", property_id, property_value)
            created_property = True
            check(created["id"] == property_id, "SDK property creation returned the wrong resource")
            check((await admin(owner, "get", "properties", property_id))["policy"]["profile"] == "minimal",
                  "SDK property read did not preserve the selected policy")
            # A new credential of the same principal preserves export ownership but
            # has only read permissions. The bootstrap credential remains untouched.
            grants = [{"action": action, "resource": {"kind": "installation", "id": "default"}}
                      for action in ("properties:read", "query:read", "sources:read", "exports:read")]
            temporary_credential = await admin(owner, "issue", "credentials", value={
                "principal_id": principal_id, "name": "synthetic official SDK read-only credential",
                "expires_at": (datetime.now(timezone.utc) + timedelta(minutes=15)).isoformat(), "grants": grants})
            limited = temporary_credential["token"]
            async with sdk(base, limited, observed) as restricted:
                check((await tool(restricted, "identity"))["principal_id"] == principal_id, "Scoped identity mismatch")
                await admin(restricted, "get", "properties", property_id)
                denial = await restricted.call_tool("admin_execute", {
                    "operation": "delete", "resource_kind": "properties", "resource_id": property_id})
                check(denial.is_error and denial.structured_content["error"]["code"] == "forbidden",
                      "Read-only SDK credential gained mutation authority")
                if args.manifest_url:
                    artifact_result = await artifact(base, args.manifest_url, limited)
                await admin(owner, "revoke", "credentials", temporary_credential["id"])
                # Verify the existing official SDK connection cannot retain authority.
                statuses_before = len(observed["statuses"])
                revoked = False
                try:
                    await restricted.call_tool("identity", {})
                except Exception:
                    revoked = True
                check(revoked and 401 in observed["statuses"][statuses_before:], "Held SDK connection survived revocation")
            await denied_sdk(base, limited)
            if artifact_result:
                async with httpx2.AsyncClient(headers={"Authorization": "Bearer " + limited}, timeout=15) as http:
                    for url in (artifact_result["manifest_url"], artifact_result["file_url"]):
                        response = await http.get(url)
                        check(response.status_code in (401, 403), "Revoked credential retained artifact access")
        finally:
            if temporary_credential:
                await admin(owner, "revoke", "credentials", temporary_credential["id"])
            if created_property:
                await admin(owner, "delete", "properties", property_id)
    check({"initialize", "notifications/initialized", "tools/list", "tools/call"}.issubset(observed["methods"]),
          "Official SDK did not execute the required handshake and tool methods")
    return {"status": "passed", "sdk": "mcp", "sdk_version": importlib.metadata.version("mcp"),
            "protocol_version": PROTOCOL, "initialize": True, "tools_list": True, "identity": True,
            "admin_mutation": True, "scope_denial": True, "held_connection_revocation": True,
            "reconnect_revocation": True, "resource_metadata": True,
            "configured_issuer": bool(args.expect_issuer),
            "remote_artifact": ({"files": artifact_result["files"], "bytes": artifact_result["bytes"],
                                 "checksums": "verified", "revocation": "denied"} if artifact_result else "not_requested")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--credentials", type=Path, required=True)
    parser.add_argument("--manifest-url", help="Existing small synthetic export manifest to verify and stream")
    parser.add_argument("--expect-issuer", help="Configured synthetic issuer expected in resource metadata")
    args = parser.parse_args()
    logging.disable(logging.CRITICAL)
    try:
        print(json.dumps(asyncio.run(run(args)), sort_keys=True))
    except Exception as error:
        # Exception payloads can contain HTTP request contexts; keep credentials private.
        print("Official MCP interoperability failed: " + type(error).__name__, file=sys.stderr)
        if isinstance(error, AssertionError):
            print(str(error), file=sys.stderr)
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
