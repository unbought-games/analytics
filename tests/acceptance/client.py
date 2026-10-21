"""Independent HTTP/MCP client and authenticated streaming artifact verifier."""
import hashlib
import json
import os
from pathlib import Path
import stat
from urllib.parse import urljoin, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

PROTOCOL = "2025-11-25"
MAX_RESPONSE = 8 * 1024 * 1024


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


def protected_secret(path):
    path = Path(path)
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077:
        raise ValueError("credential must be a regular file accessible only to its owner")
    return path.read_text().strip()


class Client:
    def __init__(self, base_url, token):
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.opener = build_opener(NoRedirect())
        self.sequence = 0
        self.session = None

    def url(self, path):
        url = urljoin(self.base_url + "/", path)
        origin = urlsplit(self.base_url)
        target = urlsplit(url)
        if (origin.scheme, origin.netloc) != (target.scheme, target.netloc) or target.username or target.password:
            raise ValueError("refusing cross-origin credential disclosure")
        return url

    def request(self, method, path, value=None, headers=None):
        request_headers = {"Authorization": "Bearer " + self.token, "Accept": "application/json, text/event-stream"}
        request_headers.update(headers or {})
        if value is not None:
            request_headers["Content-Type"] = "application/json"
        request = Request(self.url(path), data=json.dumps(value).encode() if value is not None else None, headers=request_headers, method=method)
        with self.opener.open(request, timeout=30) as response:
            raw = response.read(MAX_RESPONSE + 1)
            if len(raw) > MAX_RESPONSE:
                raise ValueError("response exceeded independent client limit")
            if response.headers.get("Mcp-Session-Id"):
                self.session = response.headers["Mcp-Session-Id"]
            if not raw:
                return None
            if response.headers.get_content_type() == "text/event-stream":
                events = []
                for block in raw.decode().replace("\r\n", "\n").split("\n\n"):
                    data = "\n".join(line[5:].lstrip() for line in block.splitlines() if line.startswith("data:"))
                    if data:
                        events.append(json.loads(data))
                matching = [item for item in events if item.get("id") == value.get("id")]
                if len(matching) != 1:
                    raise ValueError("missing or duplicate JSON-RPC response")
                return matching[0]
            return json.loads(raw)

    def rpc(self, method, params=None, notification=False):
        self.sequence += 1
        message = {"jsonrpc": "2.0", "method": method, "params": params or {}}
        if not notification:
            message["id"] = self.sequence
        headers = {"MCP-Protocol-Version": PROTOCOL}
        if self.session:
            headers["Mcp-Session-Id"] = self.session
        result = self.request("POST", "/mcp", message, headers)
        if notification:
            return None
        if not isinstance(result, dict) or result.get("jsonrpc") != "2.0" or result.get("id") != self.sequence:
            raise ValueError("invalid JSON-RPC response")
        if "error" in result:
            # Server error data may contain sensitive inputs; expose code only.
            raise RuntimeError("MCP error code " + str(result["error"].get("code")))
        return result["result"]

    def initialize(self):
        result = self.rpc("initialize", {"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "synthetic-acceptance", "version": "0.1.0"}})
        if result["protocolVersion"] != PROTOCOL:
            raise ValueError("unexpected negotiated MCP version")
        self.rpc("notifications/initialized", notification=True)
        return result

    def tool(self, name, arguments=None):
        result = self.rpc("tools/call", {"name": name, "arguments": arguments or {}})
        if result.get("isError"):
            raise RuntimeError("MCP tool failed: " + name)
        if "structuredContent" in result:
            return result["structuredContent"]
        text = [item["text"] for item in result.get("content", []) if item.get("type") == "text"]
        if len(text) != 1:
            raise ValueError("expected structured tool output")
        return json.loads(text[0])

    def download(self, dataset, directory):
        directory = Path(directory)
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        name = dataset["file_name"]
        if Path(name).name != name or name in ("", ".", ".."):
            raise ValueError("invalid dataset filename")
        target = directory / name
        digest = hashlib.sha256()
        size = rows = 0
        request = Request(self.url(dataset["download_url"]), headers={"Authorization": "Bearer " + self.token})
        try:
            with self.opener.open(request, timeout=30) as response, target.open("xb") as output:
                os.chmod(target, 0o600)
                # Fixed-size reads bound client memory regardless of dataset size.
                while True:
                    chunk = response.read(65536)
                    if not chunk:
                        break
                    size += len(chunk)
                    rows += chunk.count(b"\n")
                    if size > dataset["bytes"]:
                        raise ValueError("artifact exceeded declared size")
                    digest.update(chunk)
                    output.write(chunk)
            if size != dataset["bytes"] or digest.hexdigest() != dataset["sha256"] or rows != dataset["row_count"]:
                raise ValueError("artifact checksum/size/row count mismatch")
        except BaseException:
            # Never remove a file that pre-existed this download.
            if 'output' in locals():
                target.unlink(missing_ok=True)
            raise
        return target
