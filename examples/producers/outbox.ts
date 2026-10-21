/** Producer-owned journal + reconstructible bounded delivery projection.
 * Application commits never inspect queue capacity or call analytics.
 */
import { Pool } from "pg";
import { readFile } from "node:fs/promises";
import type { IngestBatch, IngestAcknowledgement } from "../../packages/browser/src/wire.js";
const statements = new Map<string, Promise<string>>();
const sql = (name: string): Promise<string> => {
  let statement = statements.get(name);
  if (!statement) { statement = readFile(new URL(name, import.meta.url), "utf8"); statements.set(name, statement); }
  return statement;
};

/** The synthetic command's complete original fact is stored in its own journal. */
export async function applyCommand(pool: Pool, event: IngestBatch["events"][number], playerId: string): Promise<void> {
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    await client.query("INSERT INTO synthetic_commands(command_id,player_id,event_payload) VALUES($1,$2,$3)", [event.event_id, playerId, JSON.stringify(event)]);
    await client.query("COMMIT");
  } catch (error) { await client.query("ROLLBACK"); throw error; }
  finally { client.release(); }
}

/** At most 100 facts per projection call; total queue never exceeds 10,000. */
export async function project(pool: Pool): Promise<number> {
  const statement = await sql("project.sql");
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    await client.query("SELECT pg_advisory_xact_lock(935792)");
    const result = await client.query(statement);
    await client.query("COMMIT");
    return result.rowCount ?? 0;
  } catch (error) { await client.query("ROLLBACK"); throw error; }
  finally { client.release(); }
}

/** Explicit application history maintenance, never automatic queue eviction. */
export async function pruneJournal(pool: Pool, before: Date): Promise<{ removed: string; unavailable_events: string }> {
  const result = await pool.query(await sql("retention.sql"), [before]);
  return result.rows[0];
}

export async function dispatch(pool: Pool, endpoint: string, credentialFile: string): Promise<number> {
  await project(pool);
  const statement = await sql("claim.sql");
  const client = await pool.connect();
  let rows: { event_id: string; payload: IngestBatch["events"][number]; attempts: number }[];
  try {
    await client.query("BEGIN");
    rows = (await client.query(statement)).rows;
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
      if (matches.length !== 1) continue; // Lease recovery retries ambiguous ACKs.
      const result = matches[0]!;
      if (result.status === "accepted" || result.status === "duplicate") {
        await pool.query(await sql("acknowledge.sql"), [row.event_id]);
      } else if (result.code === "rate_limited") {
        await pool.query("UPDATE analytics_outbox SET lease_until=NULL,available_at=now()+interval '60 seconds',last_error_code='rate_limited' WHERE event_id=$1", [row.event_id]);
      } else if (result.status === "rejected" || result.status === "conflict") {
        await pool.query(await sql("quarantine.sql"), [row.event_id]);
      }
    }
  } catch {
    for (const row of rows) await pool.query(
      "UPDATE analytics_outbox SET lease_until=NULL,available_at=now()+make_interval(secs=>$2),last_error_code='transport_unavailable' WHERE event_id=$1",
      [row.event_id, Math.min(300, 2 ** Math.min(row.attempts, 9))]);
  }
  return rows.length;
}
