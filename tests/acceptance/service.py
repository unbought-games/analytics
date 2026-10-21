"""Launch the actual local Rust binary with protected synthetic configuration."""
import json
import os
from pathlib import Path
import socket
import subprocess
import threading
import time
from urllib.error import URLError
from urllib.request import urlopen
from client import Client
from fixtures.server import FixtureServer
from stack import ROOT, load_state, grant_runtime


class Service:
    def __init__(self, name="service", address=None):
        self.state = load_state(os.environ["ANALYTICS_STACK_STATE"])
        self.directory = Path(self.state["directory"]) / name
        self.directory.mkdir(mode=0o700)
        self.secrets = self.directory / "secrets"
        self.secrets.mkdir(mode=0o700)
        self.exports = self.directory / "exports"
        self.exports.mkdir(mode=0o700)
        self.binary = ROOT / "target/debug/analytics-server"
        if not self.binary.is_file():
            raise RuntimeError("build the real analytics-server binary first")
        with socket.socket() as port:
            port.bind(("127.0.0.1", 0))
            self.address = "127.0.0.1:" + str(port.getsockname()[1])
        self.address = address or self.address
        self.url = "http://" + self.address
        self.oidc = FixtureServer(("127.0.0.1", 0))
        self.oidc_thread = threading.Thread(target=self.oidc.serve_forever, daemon=True)
        self.oidc_thread.start()
        self.env = dict(os.environ, ANALYTICS_PUBLIC_BASE_URL=self.url, ANALYTICS_SECRET_DIRECTORY=str(self.secrets), ANALYTICS_EXPORT_DIRECTORY=str(self.exports), ANALYTICS_OIDC_ISSUER=self.oidc.issuer)
        self.log = self.directory / "service.log"
        self.log_handle = self.log.open("xb")
        self.log.chmod(0o600)
        self.process = None

    def command(self, *args, check=True):
        # Only non-secret CLI parameters. DSN and credentials remain protected files.
        return subprocess.run([str(self.binary), *args], env=self.env, cwd=ROOT, check=check, capture_output=True)

    def migrate_bootstrap(self):
        self.command("migrate")
        grant_runtime(self.state)
        self.env["ANALYTICS_DATABASE_URL_FILE"] = self.env["ANALYTICS_RUNTIME_DATABASE_URL_FILE"]
        handoff = self.directory / "bootstrap.json"
        self.command("bootstrap", "--owner", "synthetic-owner", "--agent", "synthetic-agent", "--output", str(handoff), "--expires-days", "30")
        self.handoff = json.loads(handoff.read_text())
        if handoff.stat().st_mode & 0o077:
            raise AssertionError("bootstrap leaked handoff permissions")
        self.client = Client(self.url, self.handoff["credential"]["token"])

    def start(self):
        self.process = subprocess.Popen([str(self.binary), "serve", "--bind", self.address], cwd=ROOT, env=self.env, stdout=self.log_handle, stderr=self.log_handle)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError("service exited during startup; inspect protected service.log locally")
            try:
                with urlopen(self.url + "/readyz", timeout=1) as response:
                    if response.status == 200:
                        return
            except (URLError, TimeoutError):
                time.sleep(.1)
        raise RuntimeError("service did not become ready within 30 seconds")

    def stop(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
    def close(self):
        self.stop()
        self.log_handle.close()
        self.oidc.shutdown()
        self.oidc_thread.join(timeout=5)
        self.oidc.server_close()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()
