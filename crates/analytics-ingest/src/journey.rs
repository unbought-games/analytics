use crate::{store::*, validation::*};
use analytics_core::*;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BrowserChoiceRequest {
    pub property_id: Uuid,
    pub visit_id: Option<String>,
    pub visitor_id: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IssueProofRequest {
    pub property_id: Uuid,
    pub visit_id: Option<String>,
    pub visitor_id: Option<String>,
    pub evidence_event_id: Uuid,
}
#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdentifyRequest {
    pub property_id: Uuid,
    pub visit_id: Option<String>,
    pub visitor_id: Option<String>,
    pub proof: String,
}

pub(crate) fn identifiers(visit: Option<&str>, visitor: Option<&str>) -> ApiResult<()> {
    if visit.is_none() && visitor.is_none()
        || [visit, visitor]
            .iter()
            .flatten()
            .any(|v| !bounded_text(v, 128))
    {
        return Err(ApiError::bad_request(
            "At least one bounded visit/visitor identifier required",
        ));
    }
    Ok(())
}
pub(crate) async fn handoff(state: &AppState, r: HandoffRequest) -> ApiResult<HandoffResponse> {
    identifiers(r.visit_id.as_deref(), r.visitor_id.as_deref())?;
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    let source = property(&mut tx, r.property_id).await?;
    let audience = property(&mut tx, r.audience).await?;
    if !source.policy.cross_property
        || !audience.policy.cross_property
        || !source.handoff_audiences.contains(&r.audience)
        || source.policy.profile == CollectionProfile::Minimal
        || audience.policy.profile == CollectionProfile::Minimal
        || (r.visitor_id.is_some() && source.policy.profile != CollectionProfile::Identified)
    {
        return Err(ApiError::forbidden());
    }
    if suppressed(
        &mut tx,
        r.property_id,
        r.visit_id.as_deref(),
        r.visitor_id.as_deref(),
        None,
    )
    .await?
    {
        return Err(ApiError::forbidden());
    }
    let now = Utc::now();
    let touch = if let Some(id) = r.touch_id {
        Some(
            resolve_touch(
                &mut tx,
                id,
                r.property_id,
                r.visit_id.as_deref(),
                r.visitor_id.as_deref(),
                now,
            )
            .await?
            .ok_or_else(ApiError::forbidden)?,
        )
    } else {
        sqlx::query_scalar("SELECT touch_id FROM ingest_touches WHERE property_id=$1 AND expires_at>$4 AND (($2::text IS NOT NULL AND visit_id=$2) OR ($3::text IS NOT NULL AND visitor_id=$3)) ORDER BY occurred_at,touch_id LIMIT 1")
            .bind(r.property_id).bind(&r.visit_id).bind(&r.visitor_id).bind(now).fetch_optional(&mut *tx).await?
    };
    let raw = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let expires_at = now + Duration::minutes(10);
    sqlx::query("INSERT INTO ingest_handoffs(verifier,property_id,audience,visit_id,visitor_id,touch_id,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(hash(&raw)).bind(r.property_id).bind(r.audience).bind(&r.visit_id).bind(&r.visitor_id).bind(touch).bind(expires_at).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(HandoffResponse {
        handoff: raw,
        expires_at,
    })
}
pub async fn redeem(
    state: &AppState,
    actor: &AuthContext,
    r: RedeemHandoffRequest,
) -> ApiResult<Value> {
    state
        .auth
        .authorize(actor, "identity:manage", &Resource::property(r.audience))
        .await?;
    identifiers(r.visit_id.as_deref(), r.visitor_id.as_deref())?;
    if !bounded_text(&r.handoff, 128) {
        return Err(ApiError::bad_request("Invalid handoff"));
    }
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    let p = property(&mut tx, r.audience).await?;
    if !p.policy.cross_property
        || p.policy.profile == CollectionProfile::Minimal
        || (r.visitor_id.is_some() && p.policy.profile != CollectionProfile::Identified)
    {
        return Err(ApiError::forbidden());
    }
    let row=sqlx::query("SELECT * FROM ingest_handoffs WHERE verifier=$1 AND audience=$2 AND expires_at>clock_timestamp() FOR UPDATE")
        .bind(hash(&r.handoff)).bind(r.audience).fetch_optional(&mut *tx).await?.ok_or_else(ApiError::forbidden)?;
    let source_id: Uuid = row.get("property_id");
    let source = property(&mut tx, source_id).await?;
    if !source.policy.cross_property || !source.handoff_audiences.contains(&r.audience) {
        return Err(ApiError::forbidden());
    }
    let source_visit: Option<String> = row.get("visit_id");
    let source_visitor: Option<String> = row.get("visitor_id");
    if suppressed(
        &mut tx,
        source_id,
        source_visit.as_deref(),
        source_visitor.as_deref(),
        None,
    )
    .await?
        || suppressed(
            &mut tx,
            r.audience,
            r.visit_id.as_deref(),
            r.visitor_id.as_deref(),
            None,
        )
        .await?
    {
        return Err(ApiError::forbidden());
    }
    if row
        .get::<Option<chrono::DateTime<Utc>>, _>("redeemed_at")
        .is_some()
    {
        if row.get::<Option<String>, _>("redeemed_visit_id") != r.visit_id
            || row.get::<Option<String>, _>("redeemed_visitor_id") != r.visitor_id
        {
            return Err(ApiError::forbidden());
        }
        tx.commit().await?;
        return Ok(json!({"ok":true,"duplicate":true}));
    }
    // Transfer all eligible touch evidence, preserving first and last facts through multiple hops.
    sqlx::query("INSERT INTO ingest_touch_access(touch_id,property_id,visit_id,visitor_id,expires_at) SELECT t.touch_id,$1,$2,$3,LEAST(t.expires_at,clock_timestamp()+make_interval(days=>$7)) FROM ingest_touches t WHERE t.expires_at>clock_timestamp() AND ((t.property_id=$4 AND (($5::text IS NOT NULL AND t.visit_id=$5) OR ($6::text IS NOT NULL AND t.visitor_id=$6))) OR EXISTS(SELECT 1 FROM ingest_touch_access a WHERE a.touch_id=t.touch_id AND a.property_id=$4 AND a.expires_at>clock_timestamp() AND (($5::text IS NOT NULL AND a.visit_id=$5) OR ($6::text IS NOT NULL AND a.visitor_id=$6))))")
        .bind(r.audience).bind(&r.visit_id).bind(&r.visitor_id).bind(source_id).bind(source_visit).bind(source_visitor).bind(p.policy.attribution_window_days).execute(&mut *tx).await?;
    sqlx::query("UPDATE ingest_handoffs SET redeemed_at=clock_timestamp(),redeemed_visit_id=$2,redeemed_visitor_id=$3 WHERE verifier=$1")
        .bind(hash(&r.handoff)).bind(&r.visit_id).bind(&r.visitor_id).execute(&mut *tx).await?;
    audit(
        &mut tx,
        actor,
        r.audience,
        "handoff_redeem",
        json!({"source_property":source_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(done())
}
pub async fn identity_link(
    state: &AppState,
    actor: &AuthContext,
    r: IdentityLinkRequest,
) -> ApiResult<Value> {
    state
        .auth
        .authorize(actor, "identity:manage", &Resource::property(r.property_id))
        .await?;
    for s in [&r.subject, &r.target] {
        if !safe_key(&s.namespace) || !bounded_text(&s.id, 256) {
            return Err(ApiError::bad_request("Invalid namespaced identity"));
        }
    }
    if !matches!(r.relationship.as_str(), "account" | "player")
        || r.world.as_ref().is_some_and(|s| !bounded_text(s, 128))
    {
        return Err(ApiError::bad_request(
            "Relationship must be account or player",
        ));
    }
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    let p = property(&mut tx, r.property_id).await?;
    if p.policy.profile != CollectionProfile::Identified
        || suppressed(&mut tx, p.id, None, None, Some(&r.subject)).await?
        || suppressed(&mut tx, p.id, None, None, Some(&r.target)).await?
    {
        return Err(ApiError::forbidden());
    }
    let evidence:Option<Value>=sqlx::query_scalar("SELECT payload FROM ingest_events WHERE property_id=$1 AND event_id=$2 AND producer='server' AND expires_at>clock_timestamp()")
        .bind(p.id).bind(r.evidence_event_id).fetch_optional(&mut *tx).await?;
    let e: EventEnvelope =
        parse(evidence.ok_or_else(|| ApiError::bad_request("Missing authoritative evidence"))?)?;
    let s = e.subject.ok_or_else(ApiError::forbidden)?;
    if s.namespace != r.subject.namespace
        || s.id != r.subject.id
        || !matches!(
            e.category,
            EventCategory::Signup | EventCategory::Enrollment
        )
        || e.properties.get("target_namespace").and_then(Value::as_str) != Some(&r.target.namespace)
        || e.properties.get("target_id").and_then(Value::as_str) != Some(&r.target.id)
        || e.properties.get("world").and_then(Value::as_str) != r.world.as_deref()
    {
        return Err(ApiError::bad_request(
            "Mapping does not match authoritative evidence",
        ));
    }
    let existing=sqlx::query("SELECT subject_namespace,subject_id FROM ingest_identity_links WHERE property_id=$1 AND target_namespace=$2 AND target_id=$3 AND relationship=$4 AND world=$5")
        .bind(p.id).bind(&r.target.namespace).bind(&r.target.id).bind(&r.relationship).bind(r.world.as_deref().unwrap_or("")).fetch_optional(&mut *tx).await?;
    if let Some(row) = existing {
        if row.get::<String, _>("subject_namespace") != r.subject.namespace
            || row.get::<String, _>("subject_id") != r.subject.id
        {
            return Err(ApiError::bad_request(
                "Identity mapping conflicts with existing account",
            ));
        }
    }
    sqlx::query("INSERT INTO ingest_identity_links(id,property_id,subject_namespace,subject_id,target_namespace,target_id,relationship,world,evidence_event_id,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT DO NOTHING")
        .bind(Uuid::new_v4()).bind(p.id).bind(&r.subject.namespace).bind(&r.subject.id).bind(&r.target.namespace).bind(&r.target.id).bind(&r.relationship).bind(r.world.unwrap_or_default()).bind(r.evidence_event_id).bind(Utc::now()+Duration::days(p.policy.identity_retention_days as i64)).execute(&mut *tx).await?;
    audit(
        &mut tx,
        actor,
        p.id,
        "identity_link",
        json!({"evidence_event_id":r.evidence_event_id,"relationship":r.relationship}),
    )
    .await?;
    tx.commit().await?;
    Ok(done())
}
pub async fn issue_proof(
    state: &AppState,
    actor: &AuthContext,
    r: IssueProofRequest,
) -> ApiResult<Value> {
    state
        .auth
        .authorize(actor, "identity:manage", &Resource::property(r.property_id))
        .await?;
    identifiers(r.visit_id.as_deref(), r.visitor_id.as_deref())?;
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    let p = property(&mut tx, r.property_id).await?;
    if p.policy.profile != CollectionProfile::Identified {
        return Err(ApiError::forbidden());
    }
    let e:Value=sqlx::query_scalar("SELECT payload FROM ingest_events WHERE property_id=$1 AND event_id=$2 AND producer='server' AND subject_id IS NOT NULL AND expires_at>clock_timestamp()")
        .bind(p.id).bind(r.evidence_event_id).fetch_optional(&mut *tx).await?.ok_or_else(ApiError::forbidden)?;
    let e: EventEnvelope = parse(e)?;
    let subject = e.subject.unwrap();
    if e.visit_id != r.visit_id
        || e.visitor_id != r.visitor_id
        || suppressed(
            &mut tx,
            p.id,
            r.visit_id.as_deref(),
            r.visitor_id.as_deref(),
            Some(&subject),
        )
        .await?
    {
        return Err(ApiError::forbidden());
    }
    let raw = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let expires_at = Utc::now() + Duration::minutes(5);
    sqlx::query("INSERT INTO ingest_proofs(verifier,property_id,visit_id,visitor_id,subject_namespace,subject_id,evidence_event_id,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(hash(&raw)).bind(p.id).bind(r.visit_id).bind(r.visitor_id).bind(subject.namespace).bind(subject.id).bind(r.evidence_event_id).bind(expires_at).execute(&mut *tx).await?;
    audit(
        &mut tx,
        actor,
        p.id,
        "identity_proof_issue",
        json!({"evidence_event_id":r.evidence_event_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(json!({"proof":raw,"expires_at":expires_at}))
}
pub(crate) async fn identify(state: &AppState, r: IdentifyRequest) -> ApiResult<Value> {
    identifiers(r.visit_id.as_deref(), r.visitor_id.as_deref())?;
    if !bounded_text(&r.proof, 128) {
        return Err(ApiError::forbidden());
    }
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    let p = property(&mut tx, r.property_id).await?;
    if p.policy.profile != CollectionProfile::Identified {
        return Err(ApiError::forbidden());
    }
    let row=sqlx::query("SELECT * FROM ingest_proofs WHERE verifier=$1 AND property_id=$2 AND visit_id IS NOT DISTINCT FROM $3 AND visitor_id IS NOT DISTINCT FROM $4 AND expires_at>clock_timestamp() FOR UPDATE")
        .bind(hash(&r.proof)).bind(p.id).bind(&r.visit_id).bind(&r.visitor_id).fetch_optional(&mut *tx).await?.ok_or_else(ApiError::forbidden)?;
    let subject = NamespacedId {
        namespace: row.get("subject_namespace"),
        id: row.get("subject_id"),
    };
    if suppressed(
        &mut tx,
        p.id,
        r.visit_id.as_deref(),
        r.visitor_id.as_deref(),
        Some(&subject),
    )
    .await?
    {
        return Err(ApiError::forbidden());
    }
    let previous=sqlx::query("SELECT subject_namespace,subject_id FROM ingest_bindings WHERE property_id=$1 AND visit_id=$2 AND visitor_id=$3")
        .bind(p.id).bind(r.visit_id.as_deref().unwrap_or("")).bind(r.visitor_id.as_deref().unwrap_or("")).fetch_optional(&mut *tx).await?;
    if let Some(row) = previous {
        if row.get::<String, _>("subject_namespace") != subject.namespace
            || row.get::<String, _>("subject_id") != subject.id
        {
            return Err(ApiError::bad_request(
                "Reset required before account switching",
            ));
        }
    }
    sqlx::query("INSERT INTO ingest_bindings(property_id,visit_id,visitor_id,subject_namespace,subject_id,evidence_event_id,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING")
        .bind(p.id).bind(r.visit_id.unwrap_or_default()).bind(r.visitor_id.unwrap_or_default()).bind(subject.namespace).bind(subject.id).bind(row.get::<Uuid,_>("evidence_event_id"))
        .bind(Utc::now()+Duration::days(p.policy.identity_retention_days as i64)).execute(&mut *tx).await?;
    sqlx::query("UPDATE ingest_proofs SET redeemed_at=clock_timestamp() WHERE verifier=$1")
        .bind(hash(&r.proof))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(done())
}
