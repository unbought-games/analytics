#!/usr/bin/env python3
"""Actual native release service/isolated PostgreSQL benchmark, no production inputs."""
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
import uuid

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/"tests/acceptance"))
from service import Service
from stack import compose
from export_cohorts import BUDGET
from test_journey import export_query


def rss(pid):
    result=subprocess.run(["ps","-o","rss=","-p",str(pid)],text=True,capture_output=True)
    return int(result.stdout.strip() or "0")*1024


class Samples:
    def __init__(self,pid):
        self.pid=pid
        self.values=[]
        self.stop=threading.Event()
        self.thread=threading.Thread(target=self.run,daemon=True)
    def run(self):
        while not self.stop.is_set():
            self.values.append(rss(self.pid))
            self.stop.wait(.05)
    def __enter__(self):
        self.thread.start()
        return self
    def __exit__(self,*args):
        self.stop.set()
        self.thread.join()
    def result(self):
        return {"samples":len(self.values),"interval_target_ms":50,"baseline_bytes":self.values[0],"peak_bytes":max(self.values),"final_bytes":self.values[-1]}


def main():
    if "ANALYTICS_STACK_STATE" not in os.environ:
        raise RuntimeError("run through tests/acceptance/stack.py")
    binary=ROOT/"target/release/analytics-server"
    if not binary.is_file():
        raise RuntimeError("cargo build --release --locked -p analytics-server first")
    os.environ["ANALYTICS_TEST_BINARY"]=str(binary)
    os.environ["ANALYTICS_MAX_IN_FLIGHT_REQUESTS"]="16"
    output=ROOT/"artifacts/benchmarks"
    output.mkdir(parents=True,exist_ok=True)
    reports={}
    with Service(name="benchmark",database="benchmark_"+uuid.uuid4().hex[:12]) as service:
        service.migrate_bootstrap()
        service.start()
        service.client.initialize()
        credential=service.directory/"load-token"
        credential.write_text(service.handoff["credential"]["token"])
        credential.chmod(0o600)
        for name,requests,concurrency,batchsize in [("normal",250,4,20),("burst",700,32,100)]:
            property_id=str(uuid.uuid4())
            policy={"profile":"minimal","version":1,"effective_at":datetime.now(timezone.utc).isoformat(),"cross_property":False,"event_retention_days":7,"identity_retention_days":7,"attribution_window_days":30,"allowed_query_keys":[],"respect_global_privacy_control":True}
            prop={"id":property_id,"name":"Synthetic "+name+" benchmark","allowed_origins":["http://127.0.0.1"],"policy":policy,"handoff_audiences":[],"enabled":True}
            service.client.tool("admin_execute",{"operation":"put","resource_kind":"properties","resource_id":property_id,"value":prop})
            schema={"property_id":property_id,"name":"load","version":1,"authoritative":True,"category":"system","properties":{}}
            service.client.tool("admin_execute",{"operation":"put","resource_kind":"schemas","resource_id":property_id+":load:1","value":schema})
            event={"event_id":str(uuid.uuid4()),"property_id":property_id,"name":"load","schema_version":1,"producer":"server","category":"system","occurred_at":datetime.now(timezone.utc).isoformat(),"clock":"server","logical_time":None,"visit_id":None,"visitor_id":None,"subject":None,"context":{"environment":"benchmark","release":"synthetic-v1","page":None,"referrer":None,"control":None,"correlation_id":None},"campaign":None,"touch_id":None,"attribution_rule":None,"collection":{"profile":"minimal","policy_version":1,"consent":True,"sample_rate":1,"dropped_events":0},"properties":{}}
            batch=service.directory/(name+"-batch.json")
            batch.write_text(json.dumps({"events":[event]*batchsize}))
            args=[sys.executable,"benchmarks/http.py","--url",service.url+"/v1/ingest/server","--batch",str(batch),"--credential-file",str(credential),"--requests",str(requests),"--concurrency",str(concurrency),"--output",str(output/(name+".json"))]
            if name=="burst":args.append("--allow-backpressure")
            with Samples(service.process.pid) as samples:
                subprocess.run(args,cwd=ROOT,check=True)
            reports[name]=json.loads((output/(name+".json")).read_text())
            reports[name]["service_rss"]=samples.result()
            reports[name]["durable_counters"]=service.client.tool("ingest_status",{"property_id":property_id})["counters"]
        def sql(statement):
            return compose(service.state,"exec","-T","sources","psql","-X","-A","-t","-U","fixture_migration","-d","synthetic","-v","ON_ERROR_STOP=1",input=statement,text=True,capture_output=True).stdout.strip()
        sql("CREATE TABLE game.benchmark_rows(id bigint PRIMARY KEY,value text); INSERT INTO game.benchmark_rows SELECT n,repeat('synthetic',16) FROM generate_series(1,100000) n; GRANT SELECT ON game.benchmark_rows TO synthetic_reader;")
        def impact():
            return json.loads(sql("SELECT json_build_object('xact_commit',xact_commit,'blks_read',blks_read,'blks_hit',blks_hit,'tup_returned',tup_returned,'tup_fetched',tup_fetched,'temp_bytes',temp_bytes) FROM pg_stat_database WHERE datname='synthetic';"))
        before=impact()
        source_id=str(uuid.uuid4())
        secret=service.secrets/"benchmark-source"
        secret.write_text(os.environ["TEST_SOURCE_DATABASE_URL"])
        secret.chmod(0o600)
        source={"id":source_id,"name":"Synthetic100k rows","kind":"postgres","environment":"benchmark","endpoint":"postgresql://synthetic-source/synthetic","secret_ref":secret.name,"allowed_schemas":["game"],"allowed_tables":["game.benchmark_rows"],"allowed_columns":{"game.benchmark_rows":["id","value"]},"allowed_projects":[],"budget":BUDGET,"semantics":{},"enabled":True}
        service.client.tool("admin_execute",{"operation":"put","resource_kind":"sources","resource_id":source_id,"value":source})
        query={"source_id":source_id,"property_id":None,"kind":"postgres","query":{"sql":"SELECT id,value FROM game.benchmark_rows ORDER BY id"},"parameters":{},"window":None,"cutoff":datetime.now(timezone.utc).isoformat(),"definition_version":"benchmark/1","budget":BUDGET}
        started=time.perf_counter()
        with Samples(service.process.pid) as samples:
            manifest=export_query(service,query,service.directory/"100k-export")
        elapsed=time.perf_counter()-started
        after=impact()
        reports["export"]={"rows":manifest["datasets"][0]["row_count"],"bytes":manifest["datasets"][0]["bytes"],"sha256":manifest["datasets"][0]["sha256"],"create_to_verified_download_seconds":elapsed,"service_rss":samples.result(),"source_db_cumulative_delta":{key:after[key]-before[key] for key in before},"source_snapshot_semantics":manifest["provenance"],"query_budget":BUDGET}
        reports["configuration"]={"recorded_at":datetime.now(timezone.utc).isoformat(),"revision":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),"dirty":bool(subprocess.check_output(["git","status","--porcelain"],cwd=ROOT)),"service_build":"native cargo --release; default optimization; no service CPU cap; global HTTP admission16","database":"postgres17.7 containers;1CPU/512MiB each","rss_method":"ps RSS bytes sampled every50ms; may miss shorter peaks","source_impact":"pg_stat_database counters include fixture polling; cumulative stats may lag","outage":"separate actual browser transactional signup/outbox test; not inferred from throughput"}
    limits=json.loads((ROOT/"benchmarks/budgets.json").read_text())["proposed_limits"]
    reports["budget_evaluation"]={
        "normal_p95":reports["normal"]["accepted_request_latency_ms"]["p95"]<=limits["normal_ingest_p95_ms"],
        "normal_p99":reports["normal"]["accepted_request_latency_ms"]["p99"]<=limits["normal_ingest_p99_ms"],
        "rss":max(reports[name]["service_rss"]["peak_bytes"] for name in ("normal","burst","export"))<=limits["engine_peak_rss_mib"]*1024*1024,
        "stream_incremental_rss":reports["export"]["service_rss"]["peak_bytes"]-reports["export"]["service_rss"]["baseline_bytes"]<=limits["export_additional_peak_rss_mib"]*1024*1024,
        "export_rows":reports["export"]["rows"]==100000,
        "burst_backpressure_observed":reports["burst"]["http_refused_events"]>0 or reports["burst"]["event_outcomes"].get("rejected:rate_limited",0)>0
    }
    reports["passed"]=all(reports["budget_evaluation"].values())
    (output/"service.json").write_text(json.dumps(reports,indent=2)+"\n")
    print("Recorded service benchmark:",output/"service.json")
    if not reports["passed"]:
        raise SystemExit("Measured workload exceeded proposed budgets; inspect recorded evaluation")


if __name__=="__main__":main()
