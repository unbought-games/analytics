//! Runnable synthetic transaction + bounded HTTP dispatcher.
//! DATABASE_URL, ANALYTICS_URL and ANALYTICS_CREDENTIAL_FILE are local environment
//! configuration. Raw credentials are read only from a protected file.
use analytics_core::{EventEnvelope, IngestAcknowledgement, IngestBatch};
use sqlx::{PgPool, Row};
use std::{error::Error, time::Duration};

pub async fn capture(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    event: &EventEnvelope,
    player: uuid::Uuid,
) -> Result<(), Box<dyn Error>> {
    // Serialize admission to reserve bounded durable capacity before game success.
    sqlx::query("SELECT pg_advisory_xact_lock(935792)")
        .execute(&mut **tx)
        .await?;
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM analytics_outbox WHERE delivered_at IS NULL")
            .fetch_one(&mut **tx)
            .await?;
    if pending >= 10000 {
        return Err("outbox capacity exhausted; command must not commit".into());
    }
    sqlx::query("INSERT INTO synthetic_commands(command_id,player_id) VALUES($1,$2)")
        .bind(event.event_id)
        .bind(player)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO analytics_outbox(event_id,payload) VALUES($1,$2)")
        .bind(event.event_id)
        .bind(serde_json::to_value(event)?)
        .execute(&mut **tx)
        .await?;
    Ok(()) // caller commits application change and outbox together
}
pub async fn dispatch(
    pool: &PgPool,
    endpoint: &str,
    credential: &str,
) -> Result<usize, Box<dyn Error>> {
    let mut tx = pool.begin().await?;
    let rows=sqlx::query("WITH pending AS (SELECT event_id FROM analytics_outbox WHERE delivered_at IS NULL AND NOT quarantined AND available_at<=now() AND (lease_until IS NULL OR lease_until<now()) ORDER BY available_at,event_id LIMIT 50 FOR UPDATE SKIP LOCKED) UPDATE analytics_outbox o SET lease_until=now()+interval '60 seconds',attempts=attempts+1 FROM pending p WHERE o.event_id=p.event_id RETURNING o.event_id,o.payload,o.attempts")
        .fetch_all(&mut *tx).await?;
    tx.commit().await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let events: Vec<EventEnvelope> = rows
        .iter()
        .map(|r| serde_json::from_value(r.get("payload")))
        .collect::<Result<_, _>>()?;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?;
    let result = http
        .post(format!(
            "{}/v1/ingest/server",
            endpoint.trim_end_matches('/')
        ))
        .bearer_auth(credential)
        .json(&IngestBatch { events })
        .send()
        .await;
    let ack = match result {
        Ok(r) if r.status().is_success() => r.json::<IngestAcknowledgement>().await.ok(),
        _ => None,
    };
    for row in &rows {
        let id: uuid::Uuid = row.get("event_id");
        let matches: Vec<_> = ack
            .as_ref()
            .map(|a| a.results.iter().filter(|a| a.event_id == id).collect())
            .unwrap_or_default();
        if matches.len() == 1 && matches!(matches[0].status.as_str(), "accepted" | "duplicate") {
            sqlx::query("UPDATE analytics_outbox SET delivered_at=now(),lease_until=NULL,last_error_code=NULL WHERE event_id=$1").bind(id).execute(pool).await?;
        } else if matches.len() == 1
            && matches!(matches[0].status.as_str(), "conflict" | "rejected")
            && matches[0].code.as_deref() != Some("rate_limited")
        {
            sqlx::query("UPDATE analytics_outbox SET quarantined=true,lease_until=NULL,last_error_code='rejected_requires_review' WHERE event_id=$1").bind(id).execute(pool).await?;
        } else {
            let attempts: i32 = row.get("attempts");
            let backoff = 2i32.pow(attempts.clamp(1, 8) as u32).min(300);
            sqlx::query("UPDATE analytics_outbox SET lease_until=NULL,available_at=now()+make_interval(secs=>$2),last_error_code='transport_unavailable' WHERE event_id=$1 AND delivered_at IS NULL").bind(id).bind(backoff).execute(pool).await?;
        }
    }
    Ok(rows.len())
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let pool = PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
    let endpoint = std::env::var("ANALYTICS_URL")?;
    let credential_path = std::env::var("ANALYTICS_CREDENTIAL_FILE")?;
    let credential = std::fs::read_to_string(credential_path)?;
    println!(
        "processed {} leased events",
        dispatch(&pool, &endpoint, credential.trim()).await?
    );
    Ok(())
}
