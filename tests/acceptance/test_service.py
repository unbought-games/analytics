"""Real Rust process, CLI, MCP and external PostgreSQL; no mocked service state."""
from datetime import datetime, timedelta, timezone
import shutil
import subprocess
import hashlib
import json
from pathlib import Path
import secrets
import time
import unittest
from urllib.error import HTTPError
from urllib.parse import parse_qs, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener, urlopen
from client import Client
from export_cohorts import analyze, configure, download_bundle
from fixtures.server import b64
from service import Service
from stack import compose


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


class ServiceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.service = Service()
        cls.addClassCleanup(cls.service.close)
        cls.service.migrate_bootstrap()
        cls.service.start()
        cls.service.client.initialize()

    def test_01_atomic_repeat_bootstrap_and_protocol_identity(self):
        service = self.service
        identity = service.client.tool("identity")
        self.assertEqual(identity["principal_id"], service.handoff["agent_id"])
        tools = service.client.rpc("tools/list")["tools"]
        self.assertIn("export_create", [tool["name"] for tool in tools])
        self.assertIn("admin_execute", [tool["name"] for tool in tools])
        output = service.directory / "repeat-bootstrap.json"
        result = service.command("bootstrap", "--owner", "another-owner", "--agent", "another-agent", "--output", str(output), check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(output.exists())
        token = service.handoff["credential"]["token"].encode()
        self.assertNotIn(token, result.stdout + result.stderr)
        self.assertNotIn(token, service.log.read_bytes())
        self.assertEqual(service.client.tool("identity")["principal_id"], identity["principal_id"])
        with self.assertRaises(HTTPError) as raised:
            Client(service.url, "invalid-synthetic-token").initialize()
        self.assertEqual(raised.exception.code, 401)
        self.assertIn("resource_metadata", raised.exception.headers["WWW-Authenticate"])

    def test_02_real_oidc_code_flow_human_agent_parity_and_token_types(self):
        service = self.service
        owner = service.handoff["owner_id"]
        service.client.tool("admin_execute", {"operation": "put", "resource_kind": "oidc_bindings", "resource_id": owner, "value": {"principal_id": owner, "issuer": service.oidc.issuer, "subject": "synthetic-human-subject"}})
        with urlopen(service.url + "/.well-known/oauth-protected-resource/mcp", timeout=5) as response:
            metadata = json.load(response)
        self.assertEqual(metadata["resource"], service.url + "/mcp")
        self.assertEqual(metadata["authorization_servers"], [service.oidc.issuer])
        with urlopen(service.oidc.issuer + "/.well-known/openid-configuration", timeout=5) as response:
            provider = json.load(response)
        verifier = secrets.token_urlsafe(48)
        query = {"client_id": "synthetic-client", "response_type": "code", "scope": "openid", "redirect_uri": "http://127.0.0.1:12345/callback", "state": secrets.token_urlsafe(16), "nonce": secrets.token_urlsafe(16), "resource": metadata["resource"], "code_challenge_method": "S256", "code_challenge": b64(hashlib.sha256(verifier.encode()).digest())}
        with self.assertRaises(HTTPError) as raised:
            build_opener(NoRedirect()).open(provider["authorization_endpoint"] + "?" + urlencode(query), timeout=5)
        self.assertEqual(raised.exception.code, 302)
        callback = parse_qs(urlsplit(raised.exception.headers["Location"]).query)
        self.assertEqual(callback["state"], [query["state"]])
        exchange = {"grant_type": "authorization_code", "code": callback["code"][0], "code_verifier": verifier, "redirect_uri": query["redirect_uri"], "client_id": query["client_id"]}
        request = Request(provider["token_endpoint"], data=urlencode(exchange).encode(), headers={"Content-Type": "application/x-www-form-urlencoded"})
        with urlopen(request, timeout=5) as response:
            tokens = json.load(response)
        human = Client(service.url, tokens["access_token"])
        human.initialize()
        self.assertEqual(human.tool("identity")["principal_id"], owner)
        self.assertEqual(human.tool("identity")["grants"], service.client.tool("identity")["grants"])
        self.assertEqual(human.tool("status"), service.client.tool("status"))
        for token in [tokens["id_token"], service.oidc.jwt({"iss": service.oidc.issuer, "aud": "wrong-resource", "sub": "synthetic-human-subject", "iat": int(time.time()), "exp": int(time.time()) + 300}, "at+jwt"), service.oidc.jwt({"iss": service.oidc.issuer, "aud": service.url + "/mcp", "sub": "synthetic-human-subject", "iat": int(time.time()) - 600, "exp": int(time.time()) - 300}, "at+jwt")]:
            with self.assertRaises(HTTPError) as denied:
                Client(service.url, token).tool("identity")
            self.assertEqual(denied.exception.code, 401)
        with self.assertRaises(HTTPError) as replay:
            urlopen(request, timeout=5)
        self.assertEqual(replay.exception.code, 400)

    def test_03_actual_cohort_exports_download_reconnect_and_external_report(self):
        service = self.service
        source = configure(service, known_coverage=True)
        bundle = download_bundle(service, source, service.directory / "known-cohorts")
        report = analyze(bundle, service.directory / "known-report")
        campaign = next(row for row in report["campaigns"] if row["campaign"] == "glass-garden")
        self.assertEqual(campaign["acquired_accounts"], 10)
        self.assertEqual(campaign["activation"]["eligible_mature_denominator"], 6)
        self.assertEqual(campaign["activation"]["numerator"], 4)
        self.assertEqual(campaign["signup_conversion"]["numerator"], 9)
        self.assertEqual(campaign["signup_conversion"]["eligible_mature_denominator"], 10)
        for day in ("D1", "D7", "D30"):
            self.assertEqual(campaign["retention"][day]["numerator"], 1)
            self.assertEqual(campaign["retention"][day]["eligible_mature_denominator"], 4)
        self.assertEqual(campaign["retention"]["D180"]["eligible_mature_denominator"], 3)
        self.assertEqual(campaign["retention"]["D180"]["numerator"], 1)
        unknown_source = configure(service, known_coverage=False)
        unknown = download_bundle(service, unknown_source, service.directory / "unknown-cohorts")
        unavailable = analyze(unknown, service.directory / "unknown-report")
        campaign = next(row for row in unavailable["campaigns"] if row["campaign"] == "glass-garden")
        self.assertIsNone(campaign["activation"]["rate"])
        self.assertIsNone(campaign["retention"]["D180"]["rate"])
        self.assertIsNone(campaign["signup_conversion"]["rate"])

    def test_04_runtime_role_cannot_administer_database(self):
        def sql(statement):
            return compose(self.service.state, "exec", "-T", "postgres", "psql", "-X", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-U", "analytics_runtime", "-d", "analytics", input=statement, text=True, capture_output=True).stdout.strip()
        self.assertEqual(sql("SELECT rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls FROM pg_roles WHERE rolname=current_user;"), "f")
        self.assertEqual(sql("SELECT current_user;"), "analytics_runtime")
        for statement in ["CREATE SCHEMA forbidden;", "CREATE ROLE forbidden;", "CREATE DATABASE forbidden;", "ALTER TABLE principals ADD COLUMN forbidden text;"]:
            with self.assertRaises(subprocess.CalledProcessError):
                sql(statement)
        self.assertTrue(self.service.client.tool("status")["ready"])

    def test_99_restore_engine_database_exports_revocations_and_new_queries(self):
        service = self.service
        source = configure(service, known_coverage=True)
        bundle = download_bundle(service, source, service.directory / "before-restore")
        saved_manifest = json.loads((bundle.parent / "acquisition/manifest.json").read_text())
        credential = service.client.tool("admin_execute", {"operation": "issue", "resource_kind": "credentials", "resource_id": None, "value": {"principal_id": service.handoff["agent_id"], "name": "revoked-before-restore", "expires_at": (datetime.now(timezone.utc) + timedelta(hours=1)).isoformat(), "grants": [{"action": "*", "resource": {"kind": "installation", "id": "default"}}]}})
        service.client.tool("admin_execute", {"operation": "revoke", "resource_kind": "credentials", "resource_id": credential["id"], "value": None})
        service.stop()  # Quiesce application writes before database + filesystem backup.
        dump = compose(service.state, "exec", "-T", "postgres", "pg_dump", "-U", "analytics_migration", "-d", "analytics", "-Fc", capture_output=True).stdout
        def sql(statement, database="postgres"):
            return compose(service.state, "exec", "-T", "postgres", "psql", "-X", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-U", "analytics_migration", "-d", database, input=statement, text=True, capture_output=True).stdout.strip()
        sql("CREATE DATABASE engine_restore;")
        try:
            compose(service.state, "exec", "-T", "postgres", "pg_restore", "-U", "analytics_migration", "-d", "engine_restore", "--exit-on-error", input=dump, capture_output=True)
            for table in ["principals", "grants", "credentials", "oidc_bindings", "admin_objects", "audit_records", "ingest_events", "ingest_acquisitions", "ingest_identity_links", "export_jobs", "_sqlx_migrations"]:
                self.assertEqual(sql("SELECT count(*) FROM " + table + ";", "analytics"), sql("SELECT count(*) FROM " + table + ";", "engine_restore"))
            with Service(name="restored", address=service.address) as restored:
                shutil.copytree(service.exports, restored.exports, dirs_exist_ok=True)
                shutil.copytree(service.secrets, restored.secrets, dirs_exist_ok=True)
                runtime = Path(service.env["ANALYTICS_DATABASE_URL_FILE"]).read_text()
                dsn = restored.directory / "database-url"
                dsn.write_text(runtime.rsplit("/", 1)[0] + "/engine_restore")
                dsn.chmod(0o600)
                restored.env["ANALYTICS_DATABASE_URL_FILE"] = str(dsn)
                restored.env["ANALYTICS_OIDC_ISSUER"] = service.oidc.issuer
                restored.handoff = service.handoff
                restored.client = Client(restored.url, service.handoff["credential"]["token"])
                restored.start()
                restored.client.initialize()
                self.assertEqual(restored.client.tool("identity")["principal_id"], service.handoff["agent_id"])
                with self.assertRaises(HTTPError) as denied:
                    Client(restored.url, credential["token"]).tool("identity")
                self.assertEqual(denied.exception.code, 401)
                manifest = restored.client.tool("export_manifest", {"id": saved_manifest["export_id"]})
                self.assertEqual(manifest, saved_manifest)
                for dataset in manifest["datasets"]:
                    restored.client.download(dataset, restored.directory / "restored-download")
                new_bundle = download_bundle(restored, source, restored.directory / "fresh-export")
                report = analyze(new_bundle, restored.directory / "fresh-report")
                campaign = next(row for row in report["campaigns"] if row["campaign"] == "glass-garden")
                self.assertEqual(campaign["retention"]["D180"]["numerator"], 1)
                output = restored.directory / "forbidden-rebootstrap.json"
                result = restored.command("bootstrap", "--owner", "duplicate", "--agent", "duplicate", "--output", str(output), check=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(output.exists())
        finally:
            sql("DROP DATABASE engine_restore WITH (FORCE);")


if __name__ == "__main__":
    unittest.main()
