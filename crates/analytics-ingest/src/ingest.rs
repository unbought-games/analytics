use crate::{batch, store::*, validation::*};
use analytics_core::*;
use chrono::{Duration, Utc};
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

fn rank(p: &CollectionProfile) -> u8 {
    match p {
        CollectionProfile::Minimal => 0,
        CollectionProfile::Journey => 1,
        CollectionProfile::Identified => 2,
    }
}
fn validate(
    e: &EventEnvelope,
    p: &PropertyDefinition,
    s: &EventSchemaDefinition,
    browser: bool,
) -> Result<(), &'static str> {
    if e.property_id != p.id
        || !safe_key(&e.name)
        || e.schema_version < 1
        || s.name != e.name
        || s.property_id != e.property_id
        || s.version != e.schema_version
        || s.category != e.category
    {
        return Err("schema_mismatch");
    }
    if e.collection.policy_version != p.policy.version
        || rank(&e.collection.profile) > rank(&p.policy.profile)
    {
        return Err("policy_mismatch");
    }
    if !e.collection.consent {
        return Err("collection_disabled");
    }
    if !(e.collection.sample_rate > 0. && e.collection.sample_rate <= 1.)
        || e.collection.dropped_events > 1_000_000
    {
        return Err("invalid_coverage");
    }
    if e.occurred_at
        < Utc::now()
            - Duration::days(
                p.policy
                    .identity_retention_days
                    .max(p.policy.event_retention_days)
                    .max(365) as i64,
            )
    {
        return Err("outside_dedupe_horizon");
    }
    if e.occurred_at > Utc::now() + Duration::minutes(5) {
        return Err("future_timestamp");
    }
    if !bounded_text(&e.context.environment, 80)
        || [
            &e.context.release,
            &e.context.control,
            &e.context.correlation_id,
        ]
        .iter()
        .any(|v| v.as_ref().is_some_and(|v| !bounded_text(v, 256)))
    {
        return Err("invalid_context");
    }
    if [&e.context.page, &e.context.referrer]
        .iter()
        .any(|v| v.as_ref().is_some_and(|v| !safe_url(v)))
    {
        return Err("unsafe_url");
    }
    if [&e.visit_id, &e.visitor_id]
        .iter()
        .any(|v| v.as_ref().is_some_and(|v| !bounded_text(v, 128)))
    {
        return Err("invalid_identity");
    }
    if let Some(s) = &e.subject
        && (!safe_key(&s.namespace) || !bounded_text(&s.id, 256))
    {
        return Err("invalid_identity");
    }
    match e.collection.profile {
        CollectionProfile::Minimal
            if e.visit_id.is_some()
                || e.visitor_id.is_some()
                || e.subject.is_some()
                || e.touch_id.is_some() =>
        {
            return Err("profile_identity_forbidden");
        }
        CollectionProfile::Journey if e.visitor_id.is_some() || e.subject.is_some() => {
            return Err("profile_identity_forbidden");
        }
        _ => {}
    }
    if e.attribution_rule.is_some() {
        return Err("attribution_is_server_owned");
    }
    if browser {
        if e.producer != Producer::Browser
            || e.clock != "client"
            || e.subject.is_some()
            || e.logical_time.is_some()
            || s.authoritative
            || matches!(
                e.category,
                EventCategory::Signup
                    | EventCategory::Enrollment
                    | EventCategory::HumanAction
                    | EventCategory::Scheduled
                    | EventCategory::System
            )
        {
            return Err("browser_authority_forbidden");
        }
    } else if e.producer != Producer::Server || !matches!(e.clock.as_str(), "server" | "game") {
        return Err("producer_mismatch");
    }
    if matches!(
        e.category,
        EventCategory::Signup
            | EventCategory::Enrollment
            | EventCategory::HumanAction
            | EventCategory::Scheduled
    ) && !s.authoritative
    {
        return Err("authoritative_schema_required");
    }
    if matches!(
        e.category,
        EventCategory::Signup | EventCategory::Enrollment
    ) && e.subject.is_none()
    {
        return Err("verified_subject_required");
    }
    if let Some(v) = &e.logical_time
        && (e.clock != "game"
            || v.len() > 30
            || v.is_empty()
            || !v.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("invalid_logical_time");
    }
    if let Some(c) = &e.campaign
        && [
            &c.utm_source,
            &c.utm_medium,
            &c.utm_campaign,
            &c.utm_term,
            &c.utm_content,
            &c.experiment_id,
        ]
        .iter()
        .any(|v| v.as_ref().is_some_and(|v| !bounded_text(v, 256)))
    {
        return Err("invalid_campaign");
    }
    if e.properties.len() > 32 {
        return Err("too_many_properties");
    }
    for (key, v) in &e.properties {
        let Some(t) = s.properties.get(key) else {
            return Err("property_not_allowed");
        };
        if !safe_key(key)
            || prohibited_key(key)
            || !scalar(v, &t.kind)
            || v.as_str()
                .is_some_and(|v| v.len() > t.max_length.unwrap_or(512).min(512))
        {
            return Err("invalid_property");
        }
    }
    if s.properties
        .iter()
        .any(|(k, t)| t.required && !e.properties.contains_key(k))
    {
        return Err("missing_property");
    }
    Ok(())
}

pub async fn ingest_server(
    state: &AppState,
    actor: &AuthContext,
    batch: IngestBatch,
) -> ApiResult<IngestAcknowledgement> {
    for id in batch
        .events
        .iter()
        .map(|e| e.property_id)
        .collect::<std::collections::BTreeSet<_>>()
    {
        state
            .auth
            .authorize(actor, "ingest:server", &Resource::property(id))
            .await?;
    }
    ingest(state, batch, false, false).await
}
pub(crate) async fn ingest(
    state: &AppState,
    batch: IngestBatch,
    browser: bool,
    gpc: bool,
) -> ApiResult<IngestAcknowledgement> {
    if batch.events.is_empty() || batch.events.len() > 100 {
        return Err(ApiError::bad_request("Batch requires 1 to 100 events"));
    }
    if serde_json::to_vec(&batch)
        .map_err(|_| ApiError::bad_request("Invalid batch"))?
        .len()
        > state.config.max_body_bytes.min(262144)
    {
        return Err(ApiError::bad_request("Batch exceeds byte budget"));
    }
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    let now = Utc::now();
    let (flags, mut receipts) = batch::preflight(&mut tx, &batch.events, now).await?;
    let mut attempts = HashMap::<Uuid, i32>::new();
    for e in &batch.events {
        *attempts.entry(e.property_id).or_default() += 1;
    }
    let mut properties = HashMap::<Uuid, Option<PropertyDefinition>>::new();
    let mut schemas = HashMap::<String, Option<EventSchemaDefinition>>::new();
    let mut quota_remaining = HashMap::<Uuid, i32>::new();
    let mut counters = batch::Counters::new();
    let mut pending = Vec::with_capacity(batch.events.len());
    let mut results = Vec::with_capacity(batch.events.len());
    let mut result_keys = Vec::with_capacity(batch.events.len());
    for (ordinal, mut e) in batch.events.into_iter().enumerate() {
        result_keys.push((e.property_id, e.event_id));
        if let std::collections::hash_map::Entry::Vacant(cached) = properties.entry(e.property_id) {
            let p = match property(&mut tx, e.property_id).await {
                Ok(p) => Some(p),
                Err(error) if error.status.is_client_error() => None,
                Err(error) => return Err(error),
            };
            if p.is_some() {
                quota_remaining.insert(
                    e.property_id,
                    batch::reserve_quota(&mut tx, e.property_id, browser, attempts[&e.property_id])
                        .await?,
                );
            }
            cached.insert(p);
        }
        let Some(p) = properties[&e.property_id].clone() else {
            results.push(result(
                e.event_id,
                "rejected",
                Some("property_unavailable"),
                None,
            ));
            continue;
        };
        let remaining = quota_remaining.get_mut(&p.id).unwrap();
        if *remaining == 0 {
            batch::increment(&mut counters, p.id, "rate_limited", 1);
            results.push(result(e.event_id, "rejected", Some("rate_limited"), None));
            continue;
        }
        *remaining -= 1;
        let (deleted, is_suppressed) = flags[ordinal];
        if deleted {
            batch::increment(&mut counters, p.id, "deleted_event", 1);
            results.push(result(e.event_id, "rejected", Some("deleted_event"), None));
            continue;
        }
        if is_suppressed {
            batch::increment(&mut counters, p.id, "suppressed", 1);
            results.push(result(e.event_id, "rejected", Some("suppressed"), None));
            continue;
        }
        let payload = serde_json::to_value(&e).map_err(|_| ApiError::internal())?;
        let fingerprint = hash(&payload.to_string());
        if let Some((previous, cursor)) = receipts.get(&(p.id, e.event_id)) {
            let same = previous == &fingerprint;
            let status = if same { "duplicate" } else { "conflict" };
            batch::increment(&mut counters, p.id, status, 1);
            results.push(result(
                e.event_id,
                status,
                if same {
                    None
                } else {
                    Some("event_id_conflict")
                },
                if same { *cursor } else { None },
            ));
            continue;
        }
        let schema_key = format!("{}:{}:{}", e.property_id, e.name, e.schema_version);
        if !schemas.contains_key(&schema_key) {
            let schema =
                match object::<EventSchemaDefinition>(&mut tx, "schemas", &schema_key).await {
                    Ok(s) => Some(s),
                    Err(error) if error.status.is_client_error() => None,
                    Err(error) => return Err(error),
                };
            schemas.insert(schema_key.clone(), schema);
        }
        let Some(schema) = schemas[&schema_key].as_ref() else {
            batch::increment(&mut counters, p.id, "schema_unknown", 1);
            results.push(result(e.event_id, "rejected", Some("schema_unknown"), None));
            continue;
        };
        let invalid = validate(&e, &p, schema, browser).err().or(
            if gpc && p.policy.respect_global_privacy_control {
                Some("global_privacy_control")
            } else {
                None
            },
        );
        if let Some(code) = invalid {
            batch::increment(&mut counters, p.id, code, 1);
            results.push(result(e.event_id, "rejected", Some(code), None));
            continue;
        }
        if let Some(t) = e.touch_id
            && resolve_touch(
                &mut tx,
                t,
                p.id,
                e.visit_id.as_deref(),
                e.visitor_id.as_deref(),
                now,
            )
            .await?
            .is_none()
        {
            batch::increment(&mut counters, p.id, "invalid_touch", 1);
            results.push(result(e.event_id, "rejected", Some("invalid_touch"), None));
            continue;
        }
        if let Some(subject) = &e.subject {
            let previous=sqlx::query("SELECT subject_namespace,subject_id FROM ingest_bindings WHERE property_id=$1 AND visit_id=$2 AND visitor_id=$3 AND ended_at IS NULL AND expires_at>clock_timestamp()")
                .bind(p.id).bind(e.visit_id.as_deref().unwrap_or("")).bind(e.visitor_id.as_deref().unwrap_or("")).fetch_optional(&mut *tx).await?;
            if let Some(row) = previous
                && (row.get::<String, _>("subject_namespace") != subject.namespace
                    || row.get::<String, _>("subject_id") != subject.id)
            {
                batch::increment(&mut counters, p.id, "identity_conflict", 1);
                results.push(result(
                    e.event_id,
                    "rejected",
                    Some("identity_conflict"),
                    None,
                ));
                continue;
            }
        }
        // A verified proof creates bindings. Public envelope subjects remain forbidden.
        if browser && e.collection.profile == CollectionProfile::Identified
            && let Some(row)=sqlx::query("SELECT subject_namespace,subject_id FROM ingest_bindings WHERE property_id=$1 AND visit_id=$2 AND visitor_id=$3 AND expires_at>$4 AND ended_at IS NULL")
                .bind(p.id).bind(e.visit_id.as_deref().unwrap_or("")).bind(e.visitor_id.as_deref().unwrap_or("")).bind(now).fetch_optional(&mut *tx).await? {
                e.subject=Some(NamespacedId {namespace:row.get("subject_namespace"),id:row.get("subject_id")});
                if suppressed(&mut tx,p.id,None,None,e.subject.as_ref()).await? {results.push(result(e.event_id,"rejected",Some("suppressed"),None));continue;}
            }
        let creates_touch = e.campaign.as_ref().is_some_and(|c| {
            c.utm_campaign.is_some() || c.experiment_id.is_some() || c.utm_source.is_some()
        }) && (e.visit_id.is_some() || e.visitor_id.is_some());
        if !creates_touch && e.category != EventCategory::Signup {
            pending.push(batch::pending(&e, &p, &fingerprint, now, ordinal));
            receipts.insert((p.id, e.event_id), (fingerprint, None));
            batch::increment(&mut counters, p.id, "accepted", 1);
            batch::increment(
                &mut counters,
                p.id,
                "client_reported_dropped",
                e.collection.dropped_events as i64,
            );
            results.push(result(e.event_id, "accepted", None, None));
            continue;
        }
        // Flush preceding ordinary events before interdependent touch/signup work.
        batch::flush(&mut tx, &mut pending, &mut receipts).await?;
        // Per-event savepoint gives semantic rejections an independent rollback.
        sqlx::query("SAVEPOINT event_write")
            .execute(&mut *tx)
            .await?;
        let cursor:i64=sqlx::query_scalar("INSERT INTO ingest_events(property_id,event_id,payload,fingerprint,occurred_at,producer,category,policy_version,profile,visit_id,visitor_id,subject_namespace,subject_id,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14) RETURNING receipt_cursor")
            .bind(p.id).bind(e.event_id).bind(serde_json::to_value(&e).unwrap()).bind(&fingerprint).bind(e.occurred_at)
            .bind(if browser {"browser"}else{"server"}).bind(serde_json::to_value(&e.category).unwrap().as_str().unwrap())
            .bind(p.policy.version).bind(profile_name(&e.collection.profile)).bind(&e.visit_id).bind(&e.visitor_id)
            .bind(e.subject.as_ref().map(|s|&s.namespace)).bind(e.subject.as_ref().map(|s|&s.id))
            .bind(now+Duration::days(p.policy.event_retention_days as i64)).fetch_one(&mut *tx).await?;
        if let Some(c) = &e.campaign
            && (c.utm_campaign.is_some() || c.experiment_id.is_some() || c.utm_source.is_some())
            && (e.visit_id.is_some() || e.visitor_id.is_some())
        {
            sqlx::query("INSERT INTO ingest_touches(touch_id,property_id,event_id,occurred_at,visit_id,visitor_id,campaign,policy_version,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                    .bind(touch_uuid(p.id,e.event_id)).bind(p.id).bind(e.event_id).bind(e.occurred_at).bind(&e.visit_id).bind(&e.visitor_id).bind(serde_json::to_value(c).unwrap()).bind(p.policy.version)
                    .bind(now+Duration::days(p.policy.attribution_window_days as i64)).execute(&mut *tx).await?;
        }
        if let Err(error) = attribute(&mut tx, &e, &p, now).await {
            if !error.status.is_client_error() {
                return Err(error);
            }
            sqlx::query("ROLLBACK TO SAVEPOINT event_write")
                .execute(&mut *tx)
                .await?;
            batch::increment(&mut counters, p.id, "identity_conflict", 1);
            results.push(result(
                e.event_id,
                "rejected",
                Some("identity_conflict"),
                None,
            ));
            continue;
        }
        sqlx::query("RELEASE SAVEPOINT event_write")
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO ingest_receipts(property_id,event_id,fingerprint,receipt_cursor,expires_at,subject_hash,visit_hash,visitor_hash) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(p.id).bind(e.event_id).bind(&fingerprint).bind(cursor).bind(now+Duration::days(p.policy.identity_retention_days.max(p.policy.event_retention_days).max(365) as i64))
            .bind(e.subject.as_ref().map(subject_hash)).bind(e.visit_id.as_deref().map(hash)).bind(e.visitor_id.as_deref().map(hash)).execute(&mut *tx).await?;
        batch::increment(&mut counters, p.id, "accepted", 1);
        batch::increment(
            &mut counters,
            p.id,
            "client_reported_dropped",
            e.collection.dropped_events as i64,
        );
        receipts.insert((p.id, e.event_id), (fingerprint, Some(cursor)));
        results.push(result(e.event_id, "accepted", None, Some(cursor)));
    }
    batch::flush(&mut tx, &mut pending, &mut receipts).await?;
    batch::flush_counters(&mut tx, counters).await?;
    for (ack, key) in results.iter_mut().zip(result_keys) {
        if matches!(ack.status.as_str(), "accepted" | "duplicate") && ack.receipt_cursor.is_none() {
            ack.receipt_cursor = Some(
                receipts
                    .get(&key)
                    .and_then(|r| r.1)
                    .ok_or_else(ApiError::internal)?
                    .to_string(),
            );
        }
    }
    tx.commit().await?;
    Ok(IngestAcknowledgement {
        results,
        committed_at: Utc::now(),
    })
}
