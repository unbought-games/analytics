/** Synthetic producer. Run with Node and the pg package installed locally.
 * Never call external analytics while the application transaction is open.
 */
import { Pool } from "pg";
import { readFile } from "node:fs/promises";
import type { IngestBatch, IngestAcknowledgement } from "../../packages/browser/src/wire.js";

export async function dispatch(pool: Pool, endpoint: string, credentialFile: string): Promise<number> {
  const client = await pool.connect();
  let rows: { event_id: string; payload: IngestBatch["events"][number]; attempts: number }[];
  try {
    await client.query("BEGIN");
    rows = (await client.query(`WITH pending AS (
      SELECT event_id FROM analytics_outbox WHERE delivered_at IS NULL
      AND NOT quarantined AND available_at<=now() AND (lease_until IS NULL OR lease_until<now())
      ORDER BY available_at,event_id LIMIT 50 FOR UPDATE SKIP LOCKED
    ) UPDATE analytics_outbox o SET lease_until=now()+interval '60 seconds',attempts=attempts+1
      FROM pending p WHERE o.event_id=p.event_id RETURNING o.event_id,o.payload,o.attempts`)).rows;
    await client.query("COMMIT");
  } catch (error) { await client.query("ROLLBACK"); throw error; }
  finally { client.release(); }
  if (!rows.length) return 0;
  try {
    const credential = (await readFile(credentialFile, "utf8")).trim();
    const response = await fetch(new URL("/v1/ingest/server", endpoint), {
      method: "POST", headers: { "content-type": "application/json", authorization: `Bearer ${credential}` },
      body: JSON.stringify({ events: rows.map(r => r.payload) }), signal: AbortSignal.timeout(10000),
      redirect: "error",
    });
    if (!response.ok) throw new Error("transport_unavailable");
    const ack = await response.json() as IngestAcknowledgement;
    for (const row of rows) {
      const matches = ack.results.filter(a => a.event_id === row.event_id);
      if (matches.length !== 1) continue; // Lease recovery retries an ambiguous ACK.
      const result = matches[0]!;
      if (result.status === "accepted" || result.status === "duplicate") {
        await pool.query("UPDATE analytics_outbox SET delivered_at=now(),lease_until=NULL,last_error_code=NULL WHERE event_id=$1", [row.event_id]);
      } else if (result.code === "rate_limited") {
        await pool.query("UPDATE analytics_outbox SET lease_until=NULL,available_at=now()+interval '60 seconds',last_error_code='rate_limited' WHERE event_id=$1", [row.event_id]);
      } else {
        await pool.query("UPDATE analytics_outbox SET quarantined=true,lease_until=NULL,last_error_code='rejected_requires_review' WHERE event_id=$1", [row.event_id]);
      }
    }
  } catch {
    for (const row of rows) await pool.query(
      "UPDATE analytics_outbox SET lease_until=NULL,available_at=now()+make_interval(secs=>$2),last_error_code='transport_unavailable' WHERE event_id=$1 AND delivered_at IS NULL",
      [row.event_id, Math.min(300, 2 ** Math.min(row.attempts, 9))]);
  }
  return rows.length;
}
