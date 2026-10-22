#!/usr/bin/env python3
"""Real binary/remote example-client smoke test against a dedicated disposable database."""
import datetime
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.request


def run(*command, env, success=True):
    completed = subprocess.run(command, env=env, capture_output=True, text=True)
    assert (completed.returncode == 0) == success, "Unexpected command result (output withheld to protect credentials)"
    return completed


def main():
    root = Path(__file__).resolve().parents[3]
    binary = root / "target/debug/analytics-server"
    client = root / "examples/admin/mcp_client.py"
    with tempfile.TemporaryDirectory(prefix="analytics-cli-") as temp:
        directory = Path(temp)
        directory.chmod(0o700)
        dsn_file = directory / "database-dsn"
        fd = os.open(dsn_file, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(fd, "w") as output:
            output.write(os.environ["TEST_DATABASE_URL"])
        env = dict(os.environ, ANALYTICS_DATABASE_URL_FILE=str(dsn_file))
        handoff = directory / "initial.json"
        run(str(binary), "bootstrap", "--owner", "Synthetic Owner", "--agent", "Synthetic Agent", "--output", str(handoff), env=env)
        initial = json.loads(handoff.read_text())
        run(str(binary), "bootstrap", "--owner", "Duplicate", "--agent", "Duplicate", "--output", str(directory / "duplicate.json"), env=env, success=False)
        assert not (directory / "duplicate.json").exists()
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        url = f"http://127.0.0.1:{port}"
        env["ANALYTICS_PUBLIC_BASE_URL"] = url
        env.pop("ANALYTICS_OIDC_ISSUER", None)
        server = subprocess.Popen([str(binary), "serve", "--bind", f"127.0.0.1:{port}"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for _ in range(200):
                try:
                    with urllib.request.urlopen(url + "/readyz", timeout=1) as response:
                        assert response.status == 200
                    break
                except OSError:
                    if server.poll() is not None:
                        raise AssertionError("Server exited during readiness")
                    time.sleep(0.025)
            else:
                raise AssertionError("Readiness timeout")
            common = ("python3", str(client), "--url", url)
            identity = run(*common, "--credentials", str(handoff), env=env)
            assert json.loads(identity.stdout)["result"]["structuredContent"]["principal_id"] == initial["agent_id"]
            arguments = directory / "rotate.json"
            arguments.write_text(json.dumps({"operation": "rotate", "resource_kind": "credentials", "resource_id": initial["credential"]["id"], "value": {
                "principal_id": initial["agent_id"], "name": "rotated smoke credential",
                "expires_at": (datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(hours=1)).isoformat(),
                "grants": [{"action": "*", "resource": {"kind": "installation", "id": "default"}}]}}))
            rotated = directory / "rotated.json"
            run(*common, "--credentials", str(handoff), "--tool", "admin_execute", "--arguments", str(arguments), "--secret-output", str(rotated), env=env)
            assert rotated.stat().st_mode & 0o777 == 0o600
            run(*common, "--credentials", str(handoff), env=env, success=False)
            run(*common, "--credentials", str(rotated), env=env)
            recovery = directory / "recovery.json"
            run(str(binary), "recover", "--principal", initial["owner_id"], "--output", str(recovery), "--revoke-all", env=env)
            run(*common, "--credentials", str(rotated), env=env, success=False)
            recovered = run(*common, "--credentials", str(recovery), env=env)
            assert json.loads(recovered.stdout)["result"]["structuredContent"]["principal_id"] == initial["owner_id"]
        finally:
            server.terminate()
            server.wait(timeout=10)
    print("PASS: bootstrap with automatic migrations, repeat rejection, remote MCP initialize/identity, protected rotation, revocation and recovery")


if __name__ == "__main__":
    main()
