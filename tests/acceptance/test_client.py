import hashlib
import json
from pathlib import Path
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from client import Client, protected_secret

DATA = b'{"id":"9007199254740993","value":null}\n'


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.headers.get("Authorization") != "Bearer synthetic":
            self.send_error(401)
            return
        if self.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "http://127.0.0.1:1/stolen")
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Length", str(len(DATA)))
        self.end_headers()
        self.wfile.write(DATA)


class ClientTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.client = Client(f"http://127.0.0.1:{cls.server.server_port}", "synthetic")

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.thread.join()
        cls.server.server_close()

    def dataset(self, **updates):
        return dict({"file_name": "data.jsonl", "download_url": "/data", "bytes": len(DATA), "sha256": hashlib.sha256(DATA).hexdigest(), "row_count": 1}, **updates)

    def test_streamed_authenticated_download_verifies_and_preserves(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.client.download(self.dataset(), directory)
            self.assertEqual(json.loads(path.read_bytes())["id"], "9007199254740993")
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            with self.assertRaises(FileExistsError):
                self.client.download(self.dataset(), directory)
            self.assertEqual(path.read_bytes(), DATA)

    def test_bad_checksum_cleans_partial_file(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                self.client.download(self.dataset(sha256="0" * 64), directory)
            self.assertFalse((Path(directory) / "data.jsonl").exists())

    def test_refuses_credential_redirect_and_path_escape(self):
        with tempfile.TemporaryDirectory() as directory:
            for update in [{"download_url": "https://example.invalid/file"}, {"file_name": "../escape"}]:
                with self.assertRaises(ValueError):
                    self.client.download(self.dataset(**update), directory)
            with self.assertRaises(HTTPError) as raised:
                self.client.download(self.dataset(download_url="/redirect"), directory)
            self.assertEqual(raised.exception.code, 302)

    def test_requires_protected_regular_credential_file(self):
        with tempfile.TemporaryDirectory() as directory:
            secret = Path(directory) / "credential"
            secret.write_text("synthetic")
            secret.chmod(0o644)
            with self.assertRaises(ValueError):
                protected_secret(secret)
            secret.chmod(0o600)
            self.assertEqual(protected_secret(secret), "synthetic")
            link = Path(directory) / "link"
            link.symlink_to(secret)
            with self.assertRaises(ValueError):
                protected_secret(link)


if __name__ == "__main__":
    unittest.main()
