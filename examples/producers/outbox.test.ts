import assert from "node:assert/strict";
import { test } from "node:test";
import { randomUUID, createHash } from "node:crypto";
import { createServer } from "node:http";
import { readFile, mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Pool } from "pg";
import { applyCommand, project, dispatch, pruneJournal } from "./outbox.ts";

function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") return `{${Object.keys(value).sort().map(key => `${JSON.stringify(key)}:${canonical((value as Record<string, unknown>)[key])}`).join(",")}}`;
  return JSON.stringify(value);
}

// Real PostgreSQL source and receiver; the HTTP fixture commits stable-ID
// receipts before ACK, including duplicate/conflict semantics and dropped ACKs.
test("TypeScript journal survives a full queue, outage, queue loss and uncertain ACK", { timeout: 180000 }, async () => {
  const connectionString = process.env.PRODUCER_TEST_DATABASE_URL;
  assert.ok(connectionString, "Set PRODUCER_TEST_DATABASE_URL to disposable PostgreSQL");
  const admin = new Pool({ connectionString, max: 1 });
  const schema = `producer_test_${randomUUID().replaceAll("-", "")}`;
  await admin.query(`CREATE SCHEMA ${schema}`);
  const pool = new Pool({ connectionString, options: `-c search_path=${schema}`, max: 5 });
  const directory = await mkdtemp(join(tmpdir(), "synthetic-producer-"));
  const credential = "synthetic-test-server";
  const credentialFile = join(directory, "credential");
  await writeFile(credentialFile, credential, { mode: 0o600 });
  let maxBatch = 0, maxWireBytes = 0, maxQueue = 0;
  const startHeap = process.memoryUsage().heapUsed;
  let maxHeap = startHeap;
  const receiver = createServer(async (request, response) => {
    try {
      if (request.headers.authorization !== `Bearer ${credential}`) { response.writeHead(401).end(); return; }
      const chunks: Buffer[] = []; let bytes = 0;
      for await (const chunk of request) { bytes += chunk.length; assert.ok(bytes <= 262144); chunks.push(chunk); }
      const { events } = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      assert.ok(events.length <= 50); maxBatch = Math.max(maxBatch, events.length); maxWireBytes = Math.max(maxWireBytes, bytes);
      const client = await pool.connect();
      const results: { event_id: string; status: string; code: string | null; receipt_cursor: string | null }[] = [];
      try {
        await client.query("BEGIN");
        for (const event of events) {
          const fingerprint = createHash("sha256").update(canonical(event)).digest("hex");
          const insert = await client.query("INSERT INTO receiver_facts(event_id,fingerprint) VALUES($1,$2) ON CONFLICT DO NOTHING RETURNING event_id", [event.event_id, fingerprint]);
          const previous = await client.query("SELECT fingerprint FROM receiver_facts WHERE event_id=$1", [event.event_id]);
          const status = insert.rowCount ? "accepted" : previous.rows[0].fingerprint === fingerprint ? "duplicate" : "conflict";
          results.push({ event_id: event.event_id, status, code: status === "conflict" ? "event_id_conflict" : null, receipt_cursor: null });
          if (status === "duplicate") await client.query("UPDATE receiver_counts SET duplicates=duplicates+1");
        }
        await client.query("COMMIT");
      } catch (error) { await client.query("ROLLBACK"); throw error; }
      finally { client.release(); }
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ results, committed_at: new Date().toISOString() }));
    } catch { response.writeHead(500).end(); }
  });
  try {
    await pool.query(await readFile(new URL("./outbox.sql", import.meta.url), "utf8"));
    await pool.query("CREATE TABLE receiver_facts(event_id uuid PRIMARY KEY,fingerprint text); CREATE TABLE receiver_counts(duplicates integer); INSERT INTO receiver_counts VALUES(0)");
    const property = randomUUID(), player = randomUUID();
    const fact = () => ({
      event_id: randomUUID(), property_id: property, name: "action", schema_version: 1, producer: "server", category: "human_action",
      occurred_at: new Date().toISOString(), clock: "server", logical_time: null, visit_id: null, visitor_id: null, subject: null,
      context: { environment: "test", release: null, page: null, referrer: null, control: null, correlation_id: null },
      campaign: null, touch_id: null, attribution_rule: null,
      collection: { profile: "identified", policy_version: 1, consent: true, sample_rate: 1, dropped_events: 0 }, properties: {},
    } as const);
    const first = fact();
    await applyCommand(pool, first, player);
    for (let i = 1; i < 10000; i++) await applyCommand(pool, fact(), player);
    while (await project(pool)) { /* bounded pages, no in-memory journal */ }
    assert.equal((await pool.query("SELECT count(*) FROM analytics_outbox")).rows[0].count, "10000");
    await dispatch(pool, "http://127.0.0.1:1", credentialFile);
    // Successful application commits while the delivery queue is already full.
    for (let i = 0; i < 25; i++) await applyCommand(pool, fact(), player);
    assert.equal((await pool.query("SELECT count(*) FROM synthetic_commands")).rows[0].count, "10025");
    assert.equal(await project(pool), 0);
    // Model receiver commit followed by an ACK lost before journal marking.
    await pool.query("INSERT INTO receiver_facts VALUES($1,$2)", [first.event_id, createHash("sha256").update(canonical(first)).digest("hex")]);
    await pool.query("DELETE FROM analytics_outbox");
    await new Promise<void>(resolve => receiver.listen(0, "127.0.0.1", resolve));
    const address = receiver.address(); assert.ok(address && typeof address !== "string");
    const endpoint = `http://127.0.0.1:${address.port}`;
    let sent = 0;
    while (true) {
      const batch = await dispatch(pool, endpoint, credentialFile); assert.ok(batch <= 50);
      const queued = Number((await pool.query("SELECT count(*) FROM analytics_outbox")).rows[0].count);
      maxQueue = Math.max(maxQueue, queued); assert.ok(queued <= 10000);
      maxHeap = Math.max(maxHeap, process.memoryUsage().heapUsed);
      if (!batch) break; sent += batch;
    }
    assert.equal(sent, 10025);
    assert.equal((await pool.query("SELECT count(*) FROM receiver_facts")).rows[0].count, "10025");
    assert.equal((await pool.query("SELECT count(*) FROM synthetic_commands WHERE analytics_delivered_at IS NOT NULL")).rows[0].count, "10025");
    assert.equal((await pool.query("SELECT count(*) FROM analytics_outbox")).rows[0].count, "0");
    assert.equal((await pool.query("SELECT duplicates FROM receiver_counts")).rows[0].duplicates, 1);
    assert.ok(maxHeap - startHeap < 128 * 1024 * 1024);
    const lost = fact(); await applyCommand(pool, lost, player);
    await pool.query("UPDATE synthetic_commands SET applied_at=now()-interval '2 days' WHERE command_id=$1", [lost.event_id]);
    await project(pool);
    assert.deepEqual(await pruneJournal(pool, new Date(Date.now() - 86400000)), { removed: "1", unavailable_events: "1" });
    assert.equal((await pool.query("SELECT unconfirmed_pruned_events FROM producer_delivery_coverage")).rows[0].unconfirmed_pruned_events, "1");
    assert.equal((await pool.query("SELECT count(*) FROM analytics_outbox")).rows[0].count, "0");
    console.log(JSON.stringify({ committed: 10025, recovered: sent, maxBatch, maxWireBytes, maxQueueAfterRebuild: maxQueue, measuredHeapGrowthBytes: maxHeap - startHeap, retentionGap: 1 }));
  } finally {
    await new Promise<void>(resolve => receiver.close(() => resolve()));
    await pool.end(); await admin.query(`DROP SCHEMA ${schema} CASCADE`); await admin.end(); await rm(directory, { recursive: true });
  }
});
