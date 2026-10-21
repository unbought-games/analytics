use analytics_core::*;
use chrono::{DateTime, Duration, Utc};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

pub(crate) const LOCK: i64 = 710234981;
pub(crate) fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
pub(crate) fn subject_hash(subject: &NamespacedId) -> String {
    hash(&serde_json::to_string(&(subject.namespace.as_str(), subject.id.as_str())).unwrap())
}
pub(crate) fn profile_name(p: &CollectionProfile) -> &'static str {
    match p {
        CollectionProfile::Minimal => "minimal",
        CollectionProfile::Journey => "journey",
        CollectionProfile::Identified => "identified",
    }
}
pub(crate) async fn lock(db: &mut PgConnection) -> ApiResult<()> {
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *db)
        .await?;
    sqlx::query("SET LOCAL statement_timeout = '15s'")
        .execute(&mut *db)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(LOCK)
        .execute(db)
        .await?;
    Ok(())
}
pub(crate) async fn object<T: DeserializeOwned>(
    db: &mut PgConnection,
    kind: &str,
    id: &str,
) -> ApiResult<T> {
    let value: Option<Value> =
        sqlx::query_scalar("SELECT value FROM admin_objects WHERE kind=$1 AND id=$2 FOR SHARE")
            .bind(kind)
            .bind(id)
            .fetch_optional(db)
            .await?;
    serde_json::from_value(value.ok_or_else(|| ApiError::bad_request("Unknown configuration"))?)
        .map_err(|_| ApiError::internal())
}
pub(crate) async fn property(db: &mut PgConnection, id: Uuid) -> ApiResult<PropertyDefinition> {
    let p: PropertyDefinition = object(db, "properties", &id.to_string()).await?;
    if !p.enabled
        || p.id != id
        || p.policy.effective_at > Utc::now()
        || !(1..=3650).contains(&p.policy.event_retention_days)
        || !(1..=3650).contains(&p.policy.identity_retention_days)
        || !(1..=365).contains(&p.policy.attribution_window_days)
    {
        return Err(ApiError::bad_request(
            "Property disabled or policy invalid/not effective",
        ));
    }
    Ok(p)
}
pub(crate) async fn suppressed(
    db: &mut PgConnection,
    property: Uuid,
    visit: Option<&str>,
    visitor: Option<&str>,
    subject: Option<&NamespacedId>,
) -> ApiResult<bool> {
    let mut identifiers = Vec::new();
    if let Some(v) = visit {
        identifiers.push(("visit", hash(v)));
    }
    if let Some(v) = visitor {
        identifiers.push(("visitor", hash(v)));
    }
    if let Some(v) = subject {
        identifiers.push(("subject", subject_hash(v)));
    }
    for (kind, h) in identifiers {
        let found: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ingest_suppressions WHERE property_id=$1 AND kind=$2 AND identifier_hash=$3)")
            .bind(property).bind(kind).bind(h).fetch_one(&mut *db).await?;
        if found {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(crate) async fn suppress(
    db: &mut PgConnection,
    property: Uuid,
    kind: &str,
    h: String,
    reason: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO ingest_suppressions(property_id,kind,identifier_hash,reason) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
        .bind(property).bind(kind).bind(h).bind(reason).execute(db).await?;
    Ok(())
}
pub(crate) async fn count(
    db: &mut PgConnection,
    property: Uuid,
    code: &str,
    amount: i64,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO ingest_counters(property_id,code,total) VALUES($1,$2,$3) ON CONFLICT(property_id,code) DO UPDATE SET total=ingest_counters.total+EXCLUDED.total")
        .bind(property).bind(code).bind(amount).execute(db).await?;
    Ok(())
}
pub(crate) async fn audit(
    db: &mut PgConnection,
    actor: &AuthContext,
    property: Uuid,
    action: &str,
    details: Value,
) -> ApiResult<()> {
    sqlx::query(
        "INSERT INTO ingest_audit(principal_id,property_id,action,details) VALUES($1,$2,$3,$4)",
    )
    .bind(actor.principal_id)
    .bind(property)
    .bind(action)
    .bind(details)
    .execute(db)
    .await?;
    Ok(())
}
pub(crate) fn touch_uuid(property: Uuid, event: Uuid) -> Uuid {
    let digest = Sha256::digest(format!("{property}:{event}"));
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}
pub(crate) async fn resolve_touch(
    db: &mut PgConnection,
    id: Uuid,
    property: Uuid,
    visit: Option<&str>,
    visitor: Option<&str>,
    now: DateTime<Utc>,
) -> ApiResult<Option<Uuid>> {
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT t.touch_id FROM ingest_touches t WHERE (t.touch_id=$1 OR t.event_id=$1) AND t.expires_at>$5 AND ((t.property_id=$2 AND (($3::text IS NOT NULL AND t.visit_id=$3) OR ($4::text IS NOT NULL AND t.visitor_id=$4))) OR EXISTS(SELECT 1 FROM ingest_touch_access a WHERE a.touch_id=t.touch_id AND a.property_id=$2 AND a.expires_at>$5 AND (($3::text IS NOT NULL AND a.visit_id=$3) OR ($4::text IS NOT NULL AND a.visitor_id=$4)))) LIMIT 2")
        .bind(id).bind(property).bind(visit).bind(visitor).bind(now).fetch_all(db).await?;
    Ok(if ids.len() == 1 { Some(ids[0]) } else { None })
}
pub(crate) async fn attribute(
    db: &mut PgConnection,
    event: &EventEnvelope,
    p: &PropertyDefinition,
    now: DateTime<Utc>,
) -> ApiResult<()> {
    let Some(subject) = &event.subject else {
        return Ok(());
    };
    if event.category != EventCategory::Signup {
        return Ok(());
    }
    let query = "SELECT t.touch_id,t.campaign,t.occurred_at FROM ingest_touches t WHERE t.expires_at>$4 AND t.occurred_at BETWEEN $5 AND $6 AND ((t.property_id=$1 AND (($2::text IS NOT NULL AND t.visit_id=$2) OR ($3::text IS NOT NULL AND t.visitor_id=$3))) OR EXISTS(SELECT 1 FROM ingest_touch_access a WHERE a.touch_id=t.touch_id AND a.property_id=$1 AND a.expires_at>$4 AND (($2::text IS NOT NULL AND a.visit_id=$2) OR ($3::text IS NOT NULL AND a.visitor_id=$3))))";
    let sql = format!(
        "({query} ORDER BY t.occurred_at,t.touch_id LIMIT 1) UNION ALL ({query} ORDER BY t.occurred_at DESC,t.touch_id DESC LIMIT 1) ORDER BY occurred_at,touch_id"
    );
    let touches = sqlx::query(&sql)
        .bind(p.id)
        .bind(&event.visit_id)
        .bind(&event.visitor_id)
        .bind(now)
        .bind(event.occurred_at - Duration::days(p.policy.attribution_window_days as i64))
        .bind(event.occurred_at)
        .fetch_all(&mut *db)
        .await?;
    let first = touches.first().map(|r| r.get::<Uuid, _>("touch_id"));
    let last = touches.last().map(|r| r.get::<Uuid, _>("touch_id"));
    let campaign = touches.first().map(|r| r.get::<Value, _>("campaign"));
    sqlx::query("INSERT INTO ingest_acquisitions(property_id,subject_namespace,subject_id,signup_event_id,signed_up_at,first_touch_id,last_touch_id,campaign,rule_version,policy_version,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,'first-tagged-touch-v1',$9,$10) ON CONFLICT DO NOTHING")
        .bind(p.id).bind(&subject.namespace).bind(&subject.id).bind(event.event_id).bind(event.occurred_at).bind(first).bind(last).bind(campaign).bind(p.policy.version)
        .bind(now+Duration::days(p.policy.identity_retention_days as i64)).execute(&mut *db).await?;
    if event.visit_id.is_some() || event.visitor_id.is_some() {
        let old=sqlx::query("SELECT subject_namespace,subject_id FROM ingest_bindings WHERE property_id=$1 AND visit_id=$2 AND visitor_id=$3")
            .bind(p.id).bind(event.visit_id.as_deref().unwrap_or("")).bind(event.visitor_id.as_deref().unwrap_or(""))
            .fetch_optional(&mut *db).await?;
        if let Some(old) = old {
            if old.get::<String, _>("subject_namespace") != subject.namespace
                || old.get::<String, _>("subject_id") != subject.id
            {
                return Err(ApiError::bad_request(
                    "Reset required before account switching",
                ));
            }
        }
        sqlx::query("INSERT INTO ingest_bindings(property_id,visit_id,visitor_id,subject_namespace,subject_id,evidence_event_id,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING")
            .bind(p.id).bind(event.visit_id.as_deref().unwrap_or("")).bind(event.visitor_id.as_deref().unwrap_or(""))
            .bind(&subject.namespace).bind(&subject.id).bind(event.event_id).bind(now+Duration::days(p.policy.identity_retention_days as i64)).execute(db).await?;
    }
    Ok(())
}
pub(crate) fn result(
    id: Uuid,
    status: &str,
    code: Option<&str>,
    cursor: Option<i64>,
) -> EventAcknowledgement {
    EventAcknowledgement {
        event_id: id,
        status: status.into(),
        code: code.map(str::to_owned),
        receipt_cursor: cursor.map(|v| v.to_string()),
    }
}
pub(crate) fn json<T: serde::Serialize>(value: T) -> ApiResult<Value> {
    serde_json::to_value(value).map_err(|_| ApiError::internal())
}
pub(crate) fn parse<T: DeserializeOwned>(value: Value) -> ApiResult<T> {
    serde_json::from_value(value).map_err(|_| ApiError::bad_request("Invalid operation arguments"))
}
pub(crate) fn done() -> Value {
    json!({"ok":true})
}
