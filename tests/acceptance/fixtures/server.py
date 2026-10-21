"""Synthetic HTTP fixtures. Never expose this deliberately automatic IdP publicly."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlsplit


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


class FixtureServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, issuer=None):
        super().__init__(address, Handler)
        self.issuer = issuer or f"http://127.0.0.1:{self.server_port}"
        self.lock = threading.Lock()
        self.codes = {}
        self.temp = tempfile.TemporaryDirectory(prefix="analytics-oidc-")
        self.key = Path(self.temp.name) / "signing.key"
        subprocess.run(["openssl", "genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048", "-out", str(self.key)], check=True, capture_output=True)
        self.key.chmod(0o600)
        modulus = subprocess.check_output(["openssl", "rsa", "-in", str(self.key), "-noout", "-modulus"], stderr=subprocess.DEVNULL).decode().strip().split("=", 1)[1]
        self.jwk = {"kty": "RSA", "kid": "fixture-rsa", "use": "sig", "alg": "RS256", "n": b64(bytes.fromhex(modulus)), "e": "AQAB"}
        self.requests = []

    def jwt(self, claims, token_type="JWT"):
        data = (b64(json.dumps({"alg": "RS256", "kid": "fixture-rsa", "typ": token_type}).encode()) + "." + b64(json.dumps(claims).encode())).encode()
        signed = subprocess.run(["openssl", "dgst", "-sha256", "-sign", str(self.key)], input=data, check=True, capture_output=True).stdout
        return data.decode() + "." + b64(signed)

    def server_close(self):
        super().server_close()
        self.temp.cleanup()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass  # Authorization codes, bearer tokens and query strings are never logged.

    def reply(self, status, value, headers=None):
        body = json.dumps(value, allow_nan=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for key, val in (headers or {}).items():
            self.send_header(key, val)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urlsplit(self.path)
        q = {key: val[-1] for key, val in parse_qs(url.query, keep_blank_values=True).items()}
        path = url.path
        issuer = self.server.issuer
        with self.server.lock:
            # Bounded counters, no request bodies, credentials or URL query values.
            if len(self.server.requests) < 1000:
                self.server.requests.append(path)
        if path == "/healthz":
            return self.reply(200, {"status": "ok", "synthetic": True})
        if path == "/.well-known/openid-configuration":
            return self.reply(200, {"issuer": issuer, "authorization_endpoint": issuer + "/authorize", "token_endpoint": issuer + "/token", "jwks_uri": issuer + "/jwks", "response_types_supported": ["code"], "subject_types_supported": ["public"], "id_token_signing_alg_values_supported": ["RS256"], "token_endpoint_auth_methods_supported": ["none"], "code_challenge_methods_supported": ["S256"], "scopes_supported": ["openid"]})
        if path == "/jwks":
            return self.reply(200, {"keys": [self.server.jwk]})
        if path == "/authorize":
            redirect = urlsplit(q.get("redirect_uri", ""))
            if (q.get("client_id") != "synthetic-client" or q.get("response_type") != "code" or q.get("code_challenge_method") != "S256" or not re.fullmatch(r"[A-Za-z0-9_-]{43}", q.get("code_challenge", "")) or redirect.scheme != "http" or redirect.hostname != "127.0.0.1" or redirect.path != "/callback" or redirect.query or redirect.fragment or "openid" not in q.get("scope", "").split()):
                return self.reply(400, {"error": "invalid_request"})
            with self.server.lock:
                self.server.codes = {code: value for code, value in self.server.codes.items() if value["expires"] > time.time()}
                if len(self.server.codes) >= 100:
                    return self.reply(429, {"error": "slow_down"})
                code = secrets.token_urlsafe(24)
                self.server.codes[code] = dict(q, expires=time.time() + 60)
            return self.reply(302, {}, {"Location": q["redirect_uri"] + "?" + urlencode({"code": code, "state": q.get("state", "")})})
        query = q.get("query", "")
        if query == "fixture_unavailable":
            return self.reply(503, {"error": "synthetic unavailable"})
        if query == "fixture_rate_limit":
            return self.reply(429, {"error": "synthetic rate limit"}, {"Retry-After": "1"})
        if path in ("/api/v1/query", "/api/v1/query_range"):
            if query == "fixture_empty":
                result = []
            else:
                result = [{"metric": {"__name__": "synthetic_requests_total", "environment": "test", "release": "synthetic-v1"}}]
                if path.endswith("query_range"):
                    result[0]["values"] = [[1792540800, "7"], [1792540860, "NaN"], [1792540980, "+Inf"]]
                else:
                    result[0]["value"] = [1792540800, "7"]
            return self.reply(200, {"status": "success", "data": {"resultType": "matrix" if path.endswith("query_range") else "vector", "result": result}, "warnings": ["synthetic fixture; missing samples are not zero"]})
        if path == "/api/v1/labels":
            return self.reply(200, {"status": "success", "data": ["__name__", "environment", "release"]})
        if path == "/api/v1/label/__name__/values":
            return self.reply(200, {"status": "success", "data": ["synthetic_requests_total"]})
        if path == "/api/v1/series":
            return self.reply(200, {"status": "success", "data": [{"__name__": "synthetic_requests_total", "environment": "test"}]})
        if path.startswith("/api/0/"):
            if not self.headers.get("Authorization", "").startswith("Bearer "):
                return self.reply(401, {"detail": "fixture bearer required"})
            allowed = ("/api/0/organizations/synthetic/issues/", "/api/0/projects/synthetic/game/issues/", "/api/0/projects/synthetic/game/events/", "/api/0/issues/101/events/", "/api/0/issues/101/events/event-101/", "/api/0/projects/synthetic/game/events/event-101/")
            if path not in allowed or q.get("project", "101") != "101":
                return self.reply(403, {"detail": "fixture scope denied"})
            if path.endswith("/issues/"):
                page = q.get("cursor", "0:0:0")
                if page not in ("0:0:0", "1:0:0"):
                    return self.reply(400, {"detail": "invalid cursor"})
                more = page == "0:0:0"
                rows = [{"id": "101" if more else "102", "title": "Synthetic failure", "count": "9007199254740993", "firstSeen": "2026-10-21T00:00:00Z", "lastSeen": "2026-10-21T00:01:00Z", "project": {"id": "101", "slug": "game"}, "release": "synthetic-v1"}]
                link = f'<{issuer}{path}?cursor=1:0:0>; rel="next"; results="{str(more).lower()}"; cursor="1:0:0"'
                return self.reply(200, rows, {"Link": link})
            event = {"eventID": "event-101", "id": "event-101", "dateCreated": "2026-10-21T00:01:00Z", "environment": "test", "release": {"version": "synthetic-v1"}, "user": {"email": "private@example.invalid", "ip_address": "192.0.2.1"}, "request": {"headers": [["Authorization", "synthetic-sensitive-marker"]]}, "contexts": {"trace": {"trace_id": "synthetic-trace"}}}
            return self.reply(200, event if path.endswith("event-101/") else [event])
        return self.reply(404, {"error": "not_found"})

    def do_POST(self):
        if urlsplit(self.path).path != "/token":
            return self.reply(405, {"error": "read_only_fixture"})
        size = int(self.headers.get("Content-Length", "0"))
        if size > 8192:
            return self.reply(413, {"error": "invalid_request"})
        q = {k: v[-1] for k, v in parse_qs(self.rfile.read(size).decode()).items()}
        with self.server.lock:
            grant = self.server.codes.pop(q.get("code"), None)
        if (not grant or grant["expires"] < time.time() or q.get("grant_type") != "authorization_code" or q.get("client_id") != grant["client_id"] or q.get("redirect_uri") != grant["redirect_uri"] or b64(hashlib.sha256(q.get("code_verifier", "").encode()).digest()) != grant["code_challenge"]):
            return self.reply(400, {"error": "invalid_grant"})
        now = int(time.time())
        claims = {"iss": self.server.issuer, "aud": "synthetic-client", "sub": "synthetic-human-subject", "iat": now, "exp": now + 300, "nonce": grant.get("nonce", "")}
        access_claims = dict(claims, aud=grant.get("resource", "synthetic-client"))
        access_claims.pop("nonce", None)
        return self.reply(200, {"access_token": self.server.jwt(access_claims, "at+jwt"), "token_type": "Bearer", "expires_in": 300, "id_token": self.server.jwt(claims)})

    do_DELETE = do_POST
    do_PUT = do_POST
    do_PATCH = do_POST


if __name__ == "__main__":
    server = FixtureServer((os.getenv("FIXTURE_HOST", "127.0.0.1"), int(os.getenv("FIXTURE_PORT", "8081"))), os.getenv("OIDC_ISSUER"))
    try:
        server.serve_forever()
    finally:
        server.server_close()
