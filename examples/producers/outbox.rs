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
    // The event envelope is the synthetic command journal's original fact.
    // This path has no analytics queue read, queue lock, or external operation.
    sqlx::query(
        "INSERT INTO synthetic_commands(command_id,player_id,event_payload) VALUES($1,$2,$3)",
    )
    .bind(event.event_id)
    .bind(player)
    .bind(serde_json::to_value(event)?)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
/// Materialize at most 100 journal facts, keeping the entire delivery queue <=10k.
pub async fn project(pool: &PgPool) -> Result<u64, Box<dyn Error>> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(935792)")
        .execute(&mut *tx)
        .await?;
    let inserted = sqlx::query(include_str!("project.sql"))
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(inserted)
}
/// Call only as explicit application history maintenance, never to make queue space.
pub async fn prune_journal(
    pool: &PgPool,
    before: chrono::DateTime<chrono::Utc>,
) -> Result<(i64, i64), Box<dyn Error>> {
    let row = sqlx::query(include_str!("retention.sql"))
        .bind(before)
        .fetch_one(pool)
        .await?;
    Ok((row.get("removed"), row.get("unavailable_events")))
}

pub async fn dispatch(
    pool: &PgPool,
    endpoint: &str,
    credential: &str,
) -> Result<usize, Box<dyn Error>> {
    project(pool).await?;
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(include_str!("claim.sql"))
        .fetch_all(&mut *tx)
        .await?;
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
            sqlx::query(include_str!("acknowledge.sql"))
                .bind(id)
                .execute(pool)
                .await?;
        } else if matches.len() == 1
            && matches!(matches[0].status.as_str(), "conflict" | "rejected")
            && matches[0].code.as_deref() != Some("rate_limited")
        {
            sqlx::query(include_str!("quarantine.sql"))
                .bind(id)
                .execute(pool)
                .await?;
        } else {
            let attempts: i32 = row.get("attempts");
            let backoff = 2i32.pow(attempts.clamp(1, 8) as u32).min(300);
            sqlx::query("UPDATE analytics_outbox SET lease_until=NULL,available_at=now()+make_interval(secs=>$2),last_error_code='transport_unavailable' WHERE event_id=$1").bind(id).bind(backoff).execute(pool).await?;
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
