"""Three synthetic web properties and a transactional application/outbox producer."""
import base64
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlsplit
from urllib.request import Request, urlopen
from cohort_source import literal
from fixtures.server import b64
from stack import ROOT, compose, endpoint


def now():
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


class Journey:
    def __init__(self, service):
        self.service = service
        self.sites = {}
        self.pending = {}
        self.sessions = {}
        self.account = str(uuid.uuid4())
        self.player = str(uuid.uuid4())
        self.world = "synthetic-world"
        self.subject = None
        self.issuer_namespace = "synthetic-oidc"  # Explicit binding to service.oidc.issuer; never email/UUID equality.
        self.events = {}
        self.sql((ROOT / "examples/producers/outbox.sql").read_text())
        self.sql('''CREATE SCHEMA live;
CREATE TABLE live.accounts (application text,local_account_id text PRIMARY KEY,issuer text,subject text,created_at text);
CREATE TABLE live.memberships (application text,local_account_id text,world text,player_id text,enrolled_at text,history jsonb);
CREATE TABLE live.activity (event_id text,application text,world text,player_id text,occurred_at text,recorded_at text,authoritative boolean,actor text,outcome text,action text,logical_ms text);
GRANT USAGE ON SCHEMA live TO synthetic_reader;
GRANT SELECT ON ALL TABLES IN SCHEMA live TO synthetic_reader;''')
        for name in ("company", "game", "identity"):
            server = ThreadingHTTPServer(("127.0.0.1", 0), Site)
            server.journey = self
            server.site_name = name
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            self.sites[name] = {"server": server, "thread": thread, "origin": f"http://127.0.0.1:{server.server_port}", "property": str(uuid.uuid4())}
            thread.start()
        schemas = json.loads((ROOT / "examples/static-site/schemas.json").read_text())
        for name, site in self.sites.items():
            other = [v["property"] for k, v in self.sites.items() if k != name]
            prop = {"id": site["property"], "name": "Synthetic " + name, "allowed_origins": [site["origin"]], "handoff_audiences": other, "enabled": True, "policy": {"profile": "identified", "version": 1, "effective_at": now(), "cross_property": True, "event_retention_days": 365, "identity_retention_days": 365, "attribution_window_days": 30, "allowed_query_keys": [], "respect_global_privacy_control": True}}
            self.admin("properties", prop["id"], prop)
            for schema in schemas:
                schema = dict(schema, property_id=prop["id"])
                self.admin("schemas", f'{prop["id"]}:{schema["name"]}:1', schema)
            for event_name, category in [("signup", "signup"), ("enrollment", "enrollment"), ("action", "human_action"), ("scheduled", "scheduled"), ("login", "custom")]:
                schema = {"property_id": prop["id"], "name": event_name, "version": 1, "authoritative": True, "category": category, "properties": {key: {"kind": "string", "required": False, "max_length": 128, "unit": None} for key in ("target_namespace", "target_id", "world", "action", "outcome")}}
                self.admin("schemas", f'{prop["id"]}:{event_name}:1', schema)
            issued = service.client.tool("ingest_token_issue", {"property_id": prop["id"], "expires_at": (datetime.now(timezone.utc) + timedelta(hours=1)).isoformat()})
            site["token"] = issued["token"]
        credential = service.directory / "producer-credential"
        credential.write_text(service.handoff["credential"]["token"])
        credential.chmod(0o600)
        password = (Path(service.state["directory"]) / "database_password").read_text()
        self.producer_env = dict(os.environ, DATABASE_URL=f"postgres://fixture_migration:{password}@{endpoint(service.state,'sources',5432)}/synthetic", ANALYTICS_URL=service.url, ANALYTICS_CREDENTIAL_FILE=str(credential))

    def admin(self, kind, key, value):
        return self.service.client.tool("admin_execute", {"operation": "put", "resource_kind": kind, "resource_id": key, "value": value})

    def sql(self, sql):
        return compose(self.service.state, "exec", "-T", "sources", "psql", "-X", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-U", "fixture_migration", "-d", "synthetic", input=sql, text=True, capture_output=True).stdout.strip()

    def event(self, site, name, category, journey, properties):
        return {"event_id": str(uuid.uuid4()), "property_id": self.sites[site]["property"], "name": name, "schema_version": 1, "producer": "server", "category": category, "occurred_at": now(), "clock": "server", "logical_time": None, "visit_id": journey.get("visit_id"), "visitor_id": journey.get("visitor_id"), "subject": {"namespace": self.issuer_namespace, "id": self.subject}, "context": {"environment": "synthetic", "release": "synthetic-v1", "page": None, "referrer": None, "control": None, "correlation_id": None}, "campaign": None, "touch_id": None, "attribution_rule": None, "collection": {"profile": "identified", "policy_version": 1, "consent": True, "sample_rate": 1, "dropped_events": 0}, "properties": properties}

    def capture(self, kind, journey, rollback=False):
        site = "identity" if kind == "signup" else "game"
        category = {"signup": "signup", "enrollment": "enrollment", "action": "human_action", "scheduled": "scheduled"}[kind]
        props = {"target_namespace": "synthetic-game", "target_id": self.account} if kind == "signup" else {"target_namespace": "synthetic-player", "target_id": self.player, "world": self.world}
        if kind in ("action", "scheduled"):
            props = {"action": "construct", "outcome": "applied"}
        event = self.event(site, kind, category, journey, props)
        when = literal(event["occurred_at"])
        if kind == "signup":
            fact = f"INSERT INTO live.accounts VALUES ('synthetic-game',{literal(self.account)},{literal(self.service.oidc.issuer)},{literal(self.subject)},{when});"
        elif kind == "enrollment":
            history = {"start": event["occurred_at"], "end": event["occurred_at"], "status": "partial", "sample_fraction": "1", "reason": "Live synthetic journey has no historical retention window"}
            fact = f"INSERT INTO live.memberships VALUES ('synthetic-game',{literal(self.account)},{literal(self.world)},{literal(self.player)},{when},{literal(history)});"
        else:
            actor = "human" if kind == "action" else "scheduled"
            fact = f"INSERT INTO live.activity VALUES ({literal(event['event_id'])},'synthetic-game',{literal(self.world)},{literal(self.player)},{when},{when},true,{literal(actor)},'applied','construct','9007199254740993');"
        self.sql("BEGIN; " + fact + f" INSERT INTO synthetic_commands(command_id,player_id,event_payload) VALUES ({literal(event['event_id'])},{literal(self.player)},{literal(event)}); " + ("ROLLBACK;" if rollback else "COMMIT;"))
        if not rollback:
            self.events[kind] = event
        return {"committed": not rollback, "event_id": event["event_id"], "local_account_id": self.account if kind == "signup" else None}

    def dispatch(self):
        result = subprocess.run([str(ROOT / "target/debug/examples/outbox")], env=self.producer_env, cwd=ROOT, capture_output=True)
        if result.returncode:
            raise RuntimeError("synthetic outbox dispatcher failed")
        return self.sql("SELECT count(*) FROM synthetic_commands WHERE analytics_delivered_at IS NULL;")

    def link(self, kind):
        event = self.events[kind]
        return self.service.client.tool("ingest_identity_link", {"property_id": event["property_id"], "subject": event["subject"], "target": {"namespace": event["properties"]["target_namespace"], "id": event["properties"]["target_id"]}, "relationship": "account" if kind == "signup" else "player", "world": event["properties"].get("world"), "evidence_event_id": event["event_id"]})

    def close(self):
        for site in self.sites.values():
            site["server"].shutdown()
            site["thread"].join(timeout=5)
            site["server"].server_close()


class Site(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, value, status=200, headers=None):
        body = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        journey = self.server.journey
        site = journey.sites[self.server.site_name]
        path = urlsplit(self.path).path
        if path == "/auth":
            state, nonce, verifier = secrets.token_urlsafe(24), secrets.token_urlsafe(24), secrets.token_urlsafe(48)
            journey.pending[state] = (nonce, verifier)
            query = {"client_id": "synthetic-client", "response_type": "code", "scope": "openid", "redirect_uri": site["origin"] + "/callback", "state": state, "nonce": nonce, "resource": journey.service.url + "/mcp", "code_challenge_method": "S256", "code_challenge": b64(hashlib.sha256(verifier.encode()).digest())}
            return self.reply({}, 302, {"Location": journey.service.oidc.issuer + "/authorize?" + urlencode(query)})
        if path == "/callback":
            query = {k:v[-1] for k,v in parse_qs(urlsplit(self.path).query).items()}
            nonce, verifier = journey.pending.pop(query["state"])
            request = Request(journey.service.oidc.issuer + "/token", data=urlencode({"client_id":"synthetic-client", "grant_type":"authorization_code", "code":query["code"], "code_verifier":verifier, "redirect_uri":site["origin"] + "/callback"}).encode(), headers={"Content-Type":"application/x-www-form-urlencoded"})
            with urlopen(request, timeout=5) as response:
                tokens = json.load(response)
            header, payload, signature = tokens["id_token"].split(".")
            claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload)%4)))
            public = journey.service.directory / "oidc-public.pem"
            public.write_bytes(subprocess.check_output(["openssl","pkey","-in",str(journey.service.oidc.key),"-pubout"], stderr=subprocess.DEVNULL))
            sig = journey.service.directory / "oidc-signature"
            sig.write_bytes(base64.urlsafe_b64decode(signature + "=" * (-len(signature)%4)))
            checked = subprocess.run(["openssl","dgst","-sha256","-verify",str(public),"-signature",str(sig)], input=(header+"."+payload).encode(), capture_output=True)
            if checked.returncode or claims["iss"] != journey.service.oidc.issuer or claims["aud"] != "synthetic-client" or claims["nonce"] != nonce or claims["exp"] <= time.time():
                return self.reply({"error":"invalid_identity"},401)
            session = secrets.token_urlsafe(32)
            journey.sessions[session] = claims["sub"]
            journey.subject = claims["sub"]
            return self.reply({},302,{"Location":site["origin"]+"/signup", "Set-Cookie":"fixture_session="+session+"; HttpOnly; SameSite=Lax; Path=/"})
        if path == "/sdk.js":
            body = (ROOT / "packages/browser/dist/analytics.global.js").read_bytes()
            content_type = "text/javascript"
        else:
            options = {"endpoint":journey.service.url,"propertyId":site["property"],"publicToken":site["token"],"policyVersion":1,"profile":"identified","choice":{"enabled":True,"persistentAttribution":True,"crossProperty":True},"routes":["/open","/learn","/play","/signup"],"handoffOrigins":{value["origin"]:value["property"] for key,value in journey.sites.items() if key != self.server.site_name},"environment":"synthetic","release":"synthetic-v1","queue":{"flushIntervalMs":60000}}
            body = ('''<!doctype html><title>Synthetic journey</title><button id="explore" data-analytics="explore">Explore</button><a id="authenticate" href="/auth" data-analytics-ignore>Authenticate synthetic visitor</a><script src="/sdk.js"></script><script>const options=''' + json.dumps(options) + '''; options.redeemBridge=async(request,signal)=>{const r=await fetch('/bridge/redeem',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(request),signal});if(!r.ok)throw Error('redeem denied');};window.client=Analytics.init(options);window.ready=client.ready;</script>''').encode()
            content_type = "text/html"
        self.send_response(200)
        self.send_header("Content-Type",content_type)
        self.send_header("Content-Length",str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        journey = self.server.journey
        size = int(self.headers.get("Content-Length", "0"))
        if size > 8192:
            return self.reply({"error":"too_large"},413)
        body = json.loads(self.rfile.read(size))
        try:
            if self.path == "/bridge/redeem":
                if body["audience"] != journey.sites[self.server.site_name]["property"]:
                    return self.reply({"error":"wrong_audience"},403)
                return self.reply(journey.service.client.tool("ingest_redeem",body))
            session = dict(part.strip().split("=",1) for part in self.headers.get("Cookie","").split(";") if "=" in part).get("fixture_session")
            if session not in journey.sessions:
                return self.reply({"error":"authenticate_first"},401)
            if self.path in ("/signup","/enrollment","/action","/scheduled"):
                return self.reply(journey.capture(self.path[1:],body["journey"],body.get("rollback",False)))
            return self.reply({"error":"not_found"},404)
        except Exception:
            return self.reply({"error":"synthetic_operation_failed"},500)
