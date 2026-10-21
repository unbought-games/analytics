import base64
import hashlib
import json
from pathlib import Path
import secrets
import subprocess
import tempfile
import threading
import unittest
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener, urlopen
from fixtures.server import FixtureServer, b64


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


class FixtureTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = FixtureServer(("127.0.0.1", 0))
        cls.thread = threading.Thread(target=cls.server.serve_forever, daemon=True)
        cls.thread.start()
        cls.url = cls.server.issuer

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.thread.join()
        cls.server.server_close()

    def request(self, path, data=None, auth=False):
        headers = {"Authorization": "Bearer synthetic-fixture"} if auth else {}
        if data is not None:
            data = urlencode(data).encode()
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        with urlopen(Request(self.url + path, data=data, headers=headers), timeout=5) as response:
            return response.status, json.load(response), response.headers

    def grant(self):
        verifier = secrets.token_urlsafe(48)
        query = {"client_id": "synthetic-client", "response_type": "code", "scope": "openid", "redirect_uri": "http://127.0.0.1:12345/callback", "state": "synthetic-state", "nonce": "synthetic-nonce", "code_challenge_method": "S256", "code_challenge": b64(hashlib.sha256(verifier.encode()).digest())}
        with self.assertRaises(HTTPError) as raised:
            build_opener(NoRedirect()).open(self.url + "/authorize?" + urlencode(query), timeout=5)
        self.assertEqual(raised.exception.code, 302)
        result = parse_qs(urlsplit(raised.exception.headers["Location"]).query)
        self.assertEqual(result["state"], [query["state"]])
        return {"grant_type": "authorization_code", "client_id": query["client_id"], "redirect_uri": query["redirect_uri"], "code": result["code"][0], "code_verifier": verifier}

    def test_oidc_code_pkce_signature_and_replay(self):
        _, discovery, _ = self.request("/.well-known/openid-configuration")
        self.assertEqual(discovery["issuer"], self.url)
        grant = self.grant()
        _, tokens, _ = self.request("/token", grant)
        access_header = tokens["access_token"].split(".")[0]
        self.assertEqual(json.loads(base64.urlsafe_b64decode(access_header + "=" * (-len(access_header) % 4)))["typ"], "at+jwt")
        header, payload, signature = tokens["id_token"].split(".")
        claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
        self.assertEqual(claims["nonce"], "synthetic-nonce")
        self.assertEqual(claims["aud"], "synthetic-client")
        with tempfile.TemporaryDirectory() as directory:
            public = Path(directory) / "public.pem"
            sig = Path(directory) / "signature"
            public.write_bytes(subprocess.check_output(["openssl", "pkey", "-in", str(self.server.key), "-pubout"], stderr=subprocess.DEVNULL))
            sig.write_bytes(base64.urlsafe_b64decode(signature + "=" * (-len(signature) % 4)))
            result = subprocess.run(["openssl", "dgst", "-sha256", "-verify", str(public), "-signature", str(sig)], input=(header + "." + payload).encode(), capture_output=True)
            self.assertEqual(result.returncode, 0)
        with self.assertRaises(HTTPError) as raised:
            self.request("/token", grant)
        self.assertEqual(raised.exception.code, 400)
        _, jwks, _ = self.request("/jwks")
        self.assertEqual(jwks["keys"][0]["kid"], "fixture-rsa")
        self.assertNotIn("d", jwks["keys"][0])

    def test_oidc_rejects_bad_verifier(self):
        grant = self.grant()
        grant["code_verifier"] = "wrong"
        with self.assertRaises(HTTPError) as raised:
            self.request("/token", grant)
        self.assertEqual(raised.exception.code, 400)

    def test_prometheus_gaps_special_values_and_failure(self):
        _, body, _ = self.request("/api/v1/query_range?query=synthetic_requests_total")
        self.assertEqual(body["data"]["result"][0]["values"][1][1], "NaN")
        _, body, _ = self.request("/api/v1/query?query=fixture_empty")
        self.assertEqual(body["data"]["result"], [])
        for query, status in [("fixture_rate_limit", 429), ("fixture_unavailable", 503)]:
            with self.assertRaises(HTTPError) as raised:
                self.request("/api/v1/query?query=" + query)
            self.assertEqual(raised.exception.code, status)

    def test_sentry_pagination_scope_and_sensitive_fixture(self):
        path = "/api/0/organizations/synthetic/issues/"
        _, rows, headers = self.request(path, auth=True)
        self.assertEqual(rows[0]["count"], "9007199254740993")
        self.assertIn('results="true"', headers["Link"])
        _, rows, headers = self.request(path + "?cursor=1:0:0", auth=True)
        self.assertEqual(rows[0]["id"], "102")
        self.assertIn('results="false"', headers["Link"])
        _, event, _ = self.request("/api/0/issues/101/events/event-101/", auth=True)
        self.assertIn("user", event)  # Adapter must redact; fixture intentionally includes it.
        with self.assertRaises(HTTPError) as raised:
            self.request("/api/0/organizations/other/issues/", auth=True)
        self.assertEqual(raised.exception.code, 403)
        with self.assertRaises(HTTPError) as raised:
            self.request(path, data={}, auth=True)
        self.assertEqual(raised.exception.code, 405)


if __name__ == "__main__":
    unittest.main()
