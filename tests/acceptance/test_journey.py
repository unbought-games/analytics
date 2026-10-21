"""Real Chromium → OIDC → transactional producer → Rust MCP files → local analysis."""
import json
import os
from pathlib import Path
import subprocess
import time
import unittest
import uuid
from urllib.request import Request, urlopen
from client import Client
from export_cohorts import BUDGET
from journey import Journey, now
from service import Service
from stack import ROOT


def export_query(service, query, directory):
    status = service.client.tool("export_create", {"query":query,"format":"jsonl","retention_hours":24})
    deadline = time.monotonic()+45
    while status["status"] in ("queued","running") and time.monotonic()<deadline:
        time.sleep(.05)
        status = service.client.tool("export_status",{"id":status["id"]})
    if status["status"] != "complete":
        raise AssertionError("live export did not complete: "+status["status"]+" "+str(status.get("error")))
    reader = Client(service.url,service.handoff["credential"]["token"])
    reader.initialize()
    manifest = reader.tool("export_manifest",{"id":status["id"]})
    directory.mkdir(parents=True,mode=0o700)
    (directory/"manifest.json").write_text(json.dumps(manifest,indent=2)+"\n")
    for dataset in manifest["datasets"]:
        reader.download(dataset,directory)
    return manifest


class JourneyTests(unittest.TestCase):
    def test_browser_signup_enrollment_outage_recovery_and_actual_report(self):
        with Service(name="journey",database="journey_"+uuid.uuid4().hex[:12]) as service:
            service.migrate_bootstrap()
            service.start()
            service.client.initialize()
            journey = Journey(service)
            self.addCleanup(journey.close)
            config = service.directory/"browser-sites.json"
            config.write_text(json.dumps({name:site["origin"] for name,site in journey.sites.items()}))
            error_log = (service.directory/"browser-error.log").open("w")
            process = subprocess.Popen(["node","tests/acceptance/browser_journey.mjs",str(config)],cwd=ROOT,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=error_log,text=True)
            phases=[]
            first_batch=None
            try:
                for line in process.stdout:
                    result=json.loads(line)
                    phases.append(result["phase"])
                    if result["phase"]=="ready_signup":
                        first_batch=result["firstBatch"]
                        self.assertEqual(journey.sql("SELECT count(*) FROM live.accounts;"),"0")
                        self.assertEqual(journey.sql("SELECT count(*) FROM analytics_outbox;"),"0")
                        # Identical UUID in another property is a distinct public event.
                        collision=json.loads(json.dumps(first_batch["events"][0]))
                        collision.update(property_id=journey.sites["game"]["property"],visit_id=str(uuid.uuid4()),visitor_id=str(uuid.uuid4()))
                        collision["context"]["page"]=journey.sites["game"]["origin"]+"/play"
                        collision["campaign"]["utm_campaign"]="collision-only"
                        request=Request(service.url+"/v1/ingest/browser",data=json.dumps({"events":[collision]}).encode(),headers={"Content-Type":"application/json","X-Property-Token":journey.sites["game"]["token"],"Origin":journey.sites["game"]["origin"]})
                        with urlopen(request,timeout=5) as response:
                            self.assertEqual(json.load(response)["results"][0]["status"],"accepted")
                        service.stop()
                    elif result["phase"]=="signup_committed":
                        self.assertEqual(journey.sql("SELECT count(*) FROM live.accounts;"),"1")
                        self.assertEqual(journey.dispatch(),"1")
                        service.start()
                        time.sleep(2.1)
                        pending=journey.dispatch()
                        if pending != "0":
                            diagnostics=journey.sql("SELECT last_error_code,quarantined,attempts FROM analytics_outbox;")
                            acknowledgment=service.client.tool("ingest_server",{"events":[journey.events["signup"]]})
                            self.fail("Outbox recovery: "+diagnostics+" acknowledgment="+json.dumps(acknowledgment))
                        journey.link("signup")
                    elif result["phase"]=="enrollment_committed":
                        self.assertEqual(journey.dispatch(),"0")
                        journey.link("enrollment")
                    elif result["phase"]=="action_committed":
                        self.assertEqual(journey.dispatch(),"0")
                    elif result["phase"]=="scheduled_committed":
                        self.assertEqual(journey.dispatch(),"0")
                        replay=service.client.tool("ingest_server",{"events":list(journey.events.values())})
                        self.assertTrue(all(row["status"]=="duplicate" for row in replay["results"]))
                    elif result["phase"]=="complete":
                        self.assertEqual(journey.dispatch(),"0")
                        break
                    process.stdin.write("continue\n")
                    process.stdin.flush()
                self.assertEqual(process.wait(timeout=20),0,"browser journey failed; inspect protected local browser-error.log")
                self.assertEqual(phases,["ready_signup","signup_committed","enrollment_committed","action_committed","scheduled_committed","complete"])
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=5)
                process.stdin.close()
                process.stdout.close()
                error_log.close()
            self.assertNotEqual(journey.subject,journey.account)
            self.assertNotEqual(journey.account,journey.player)
            self.assertEqual(journey.sql("SELECT count(*) FROM analytics_outbox WHERE delivered_at IS NOT NULL;"),"4")
            source_id=str(uuid.uuid4())
            secret=service.secrets/"live-source"
            secret.write_text(os.environ["TEST_SOURCE_DATABASE_URL"])
            secret.chmod(0o600)
            membership_columns=["application","local_account_id","world","player_id","enrolled_at","history"]
            activity_columns=["event_id","application","world","player_id","occurred_at","recorded_at","authoritative","actor","outcome","action","logical_ms"]
            journey.admin("sources",source_id,{"id":source_id,"name":"Live manufactured application","kind":"postgres","environment":"synthetic","endpoint":"postgresql://synthetic-source/synthetic","secret_ref":secret.name,"allowed_schemas":["live"],"allowed_tables":["live.memberships","live.activity"],"allowed_columns":{"live.memberships":membership_columns,"live.activity":activity_columns},"allowed_projects":[],"budget":BUDGET,"semantics":{},"enabled":True})
            cutoff=now()
            root=service.directory/"actual-exports"
            bundle={"format_version":"cohort-bundle/1","inputs":[]}
            cases=[("acquisition","identity","acquisition"),("identity_map","identity","identity_map"),("acquisition_visits","company","visits"),("acquisition_touches","company","touches"),("memberships",None,None),("activity",None,None)]
            manifests={}
            for role,site,dataset in cases:
                query={"source_id":None if site else source_id,"property_id":journey.sites[site]["property"] if site else None,"kind":"analytics" if site else "postgres","query":{"dataset":dataset} if site else {"sql":"SELECT "+", ".join(membership_columns if role=="memberships" else activity_columns)+" FROM live."+role+" ORDER BY "+("player_id" if role=="memberships" else "occurred_at,event_id")},"parameters":{},"window":None,"cutoff":cutoff,"definition_version":"cohort-input/1","budget":BUDGET}
                manifest=export_query(service,query,root/role)
                manifests[role]=manifest
                bundle["inputs"].append({"name":role,"manifest_file":role+"/manifest.json","dataset_name":manifest["datasets"][0]["name"]})
            bundle_path=root/"bundle.json"
            bundle_path.write_text(json.dumps(bundle,indent=2)+"\n")
            subprocess.run([str(ROOT/"examples/analysis/analyze"),"--manifest",str(bundle_path),"--cutoff",cutoff,"--output",str(service.directory/"actual-report")],cwd=ROOT,check=True,capture_output=True)
            report=json.loads((service.directory/"actual-report/campaign-report.json").read_text())
            campaign=next(row for row in report["campaigns"] if row["campaign"]=="live-synthetic")
            self.assertEqual(campaign["acquired_accounts"],1)
            self.assertEqual(campaign["observed_converted_visits"],1)
            self.assertEqual(campaign["enrolled_accounts"],1)
            self.assertEqual(campaign["first_observed_play_accounts"],1)
            self.assertIsNone(campaign["signup_conversion"]["rate"])
            self.assertIsNone(campaign["retention"]["D180"]["rate"])
            self.assertEqual(report["provenance"]["normalization"]["matched_signups"],1)
            (service.directory/"journey-evidence.json").write_text(json.dumps({"phases":phases,"actual_cutoff":cutoff,"same_batch_events":len(first_batch["events"]),"signup_account_differs_from_subject":True,"outage_recovered":True,"same_uuid_distinct_property_accepted":True,"issuer_namespace_binding":{"synthetic-oidc":service.oidc.issuer},"rollback_excluded":True,"actual_export_ids":{key:value["export_id"] for key,value in manifests.items()},"observed_converted_visits":1,"live_retention_available":False},indent=2)+"\n")


if __name__=="__main__":
    unittest.main()
