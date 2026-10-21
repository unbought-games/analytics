//! Transaction-local batch work. No cache survives a request or its policy locks.
use crate::store::*;
use analytics_core::*;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

pub(crate) type Receipts = HashMap<(Uuid, Uuid), (String, Option<i64>)>;
pub(crate) type Counters = BTreeMap<(Uuid, String), i64>;
pub(crate) fn increment(counters: &mut Counters, property: Uuid, code: &str, amount: i64) {
    *counters.entry((property, code.to_owned())).or_default() += amount;
}
pub(crate) async fn preflight(
    db: &mut PgConnection,
    events: &[EventEnvelope],
    now: DateTime<Utc>,
) -> ApiResult<(Vec<(bool, bool)>, Receipts)> {
    let input:Vec<Value>=events.iter().enumerate().map(|(ordinal,e)|json!({"ordinal":ordinal,"property_id":e.property_id,"event_id":e.event_id,"event_hash":hash(&e.event_id.to_string()),"visit_hash":e.visit_id.as_deref().map(hash),"visitor_hash":e.visitor_id.as_deref().map(hash),"subject_hash":e.subject.as_ref().map(subject_hash)})).collect();
    let input = json!(input);
    // Expired IDs are cleaned once, under the same global receipt/privacy lock.
    sqlx::query("WITH keys AS MATERIALIZED (SELECT DISTINCT property_id,event_id FROM jsonb_to_recordset($1) AS v(property_id uuid,event_id uuid)), expired_events AS (DELETE FROM ingest_events e USING keys k WHERE e.property_id=k.property_id AND e.event_id=k.event_id AND e.expires_at<=$2 RETURNING e.event_id) DELETE FROM ingest_receipts r USING keys k WHERE r.property_id=k.property_id AND r.event_id=k.event_id AND r.expires_at<=$2")
        .bind(&input).bind(now).execute(&mut *db).await?;
    let rows=sqlx::query("SELECT v.ordinal,v.property_id,v.event_id,r.fingerprint,r.receipt_cursor,EXISTS(SELECT 1 FROM ingest_suppressions s WHERE s.property_id=v.property_id AND s.kind='event' AND s.identifier_hash=v.event_hash) AS deleted,EXISTS(SELECT 1 FROM ingest_suppressions s WHERE s.property_id=v.property_id AND ((s.kind='visit' AND s.identifier_hash=v.visit_hash) OR (s.kind='visitor' AND s.identifier_hash=v.visitor_hash) OR (s.kind='subject' AND s.identifier_hash=v.subject_hash))) AS suppressed FROM jsonb_to_recordset($1) AS v(ordinal integer,property_id uuid,event_id uuid,event_hash text,visit_hash text,visitor_hash text,subject_hash text) LEFT JOIN ingest_receipts r ON r.property_id=v.property_id AND r.event_id=v.event_id AND r.expires_at>$2 ORDER BY v.ordinal")
        .bind(input).bind(now).fetch_all(db).await?;
    let mut flags = Vec::with_capacity(events.len());
    let mut receipts = HashMap::new();
    for row in rows {
        flags.push((row.get("deleted"), row.get("suppressed")));
        if let Some(fingerprint) = row.get::<Option<String>, _>("fingerprint") {
            receipts.insert(
                (row.get("property_id"), row.get("event_id")),
                (fingerprint, row.get("receipt_cursor")),
            );
        }
    }
    Ok((flags, receipts))
}
pub(crate) async fn reserve_quota(
    db: &mut PgConnection,
    property: Uuid,
    browser: bool,
    attempts: i32,
) -> ApiResult<i32> {
    let limit = if browser { 6000i32 } else { 60000i32 };
    let used:i32=sqlx::query_scalar("INSERT INTO ingest_quotas(property_id,bucket,window_start,events) VALUES($1,$2,date_trunc('minute',clock_timestamp()),$3) ON CONFLICT(property_id,bucket) DO UPDATE SET events=CASE WHEN ingest_quotas.window_start=date_trunc('minute',clock_timestamp()) THEN LEAST(ingest_quotas.events::bigint+$3,2147483647)::integer ELSE $3 END,window_start=date_trunc('minute',clock_timestamp()) RETURNING events")
        .bind(property).bind(if browser {"public"}else{"server"}).bind(attempts).fetch_one(db).await?;
    Ok((limit - (used - attempts)).clamp(0, attempts))
}
pub(crate) fn pending(
    e: &EventEnvelope,
    p: &PropertyDefinition,
    fingerprint: &str,
    now: DateTime<Utc>,
    ordinal: usize,
) -> Value {
    json!({"ordinal":ordinal,"property_id":p.id,"event_id":e.event_id,"payload":e,"fingerprint":fingerprint,"occurred_at":e.occurred_at,"producer":e.producer,"category":e.category,"policy_version":p.policy.version,"profile":profile_name(&e.collection.profile),"visit_id":e.visit_id,"visitor_id":e.visitor_id,"subject_namespace":e.subject.as_ref().map(|s|&s.namespace),"subject_id":e.subject.as_ref().map(|s|&s.id),"expires_at":now+Duration::days(p.policy.event_retention_days as i64),"receipt_expires_at":now+Duration::days(p.policy.identity_retention_days.max(p.policy.event_retention_days).max(365) as i64),"subject_hash":e.subject.as_ref().map(subject_hash),"visit_hash":e.visit_id.as_deref().map(hash),"visitor_hash":e.visitor_id.as_deref().map(hash)})
}
pub(crate) async fn flush(
    db: &mut PgConnection,
    pending: &mut Vec<Value>,
    receipts: &mut Receipts,
) -> ApiResult<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let rows=sqlx::query("WITH data AS MATERIALIZED (SELECT * FROM jsonb_to_recordset($1) AS v(ordinal integer,property_id uuid,event_id uuid,payload jsonb,fingerprint text,occurred_at timestamptz,producer text,category text,policy_version integer,profile text,visit_id text,visitor_id text,subject_namespace text,subject_id text,expires_at timestamptz,receipt_expires_at timestamptz,subject_hash text,visit_hash text,visitor_hash text)), inserted AS (INSERT INTO ingest_events(property_id,event_id,payload,fingerprint,occurred_at,producer,category,policy_version,profile,visit_id,visitor_id,subject_namespace,subject_id,expires_at) SELECT property_id,event_id,payload,fingerprint,occurred_at,producer,category,policy_version,profile,visit_id,visitor_id,subject_namespace,subject_id,expires_at FROM data ORDER BY ordinal RETURNING property_id,event_id,receipt_cursor) INSERT INTO ingest_receipts(property_id,event_id,fingerprint,receipt_cursor,expires_at,subject_hash,visit_hash,visitor_hash) SELECT d.property_id,d.event_id,d.fingerprint,i.receipt_cursor,d.receipt_expires_at,d.subject_hash,d.visit_hash,d.visitor_hash FROM data d JOIN inserted i USING(property_id,event_id) RETURNING property_id,event_id,receipt_cursor")
        .bind(json!(pending)).fetch_all(db).await?;
    for row in rows {
        let key = (row.get("property_id"), row.get("event_id"));
        receipts.get_mut(&key).ok_or_else(ApiError::internal)?.1 = Some(row.get("receipt_cursor"));
    }
    pending.clear();
    Ok(())
}
pub(crate) async fn flush_counters(db: &mut PgConnection, counters: Counters) -> ApiResult<()> {
    if counters.is_empty() {
        return Ok(());
    }
    let input: Vec<Value> = counters
        .into_iter()
        .map(|((property, code), total)| json!({"property_id":property,"code":code,"total":total}))
        .collect();
    sqlx::query("INSERT INTO ingest_counters(property_id,code,total) SELECT property_id,code,total FROM jsonb_to_recordset($1) AS v(property_id uuid,code text,total bigint) ON CONFLICT(property_id,code) DO UPDATE SET total=ingest_counters.total+EXCLUDED.total")
        .bind(json!(input)).execute(db).await?;
    Ok(())
}
