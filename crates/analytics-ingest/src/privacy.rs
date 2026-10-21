use crate::{
    journey::{BrowserChoiceRequest, identifiers},
    store::*,
};
use analytics_core::*;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn browser_choice(
    state: &AppState,
    r: BrowserChoiceRequest,
    reset: bool,
) -> ApiResult<Value> {
    identifiers(r.visit_id.as_deref(), r.visitor_id.as_deref())?;
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    let reason = if reset { "reset" } else { "withdrawal" };
    if let Some(v) = &r.visit_id {
        suppress(&mut tx, r.property_id, "visit", hash(v), reason).await?;
    }
    if let Some(v) = &r.visitor_id {
        suppress(&mut tx, r.property_id, "visitor", hash(v), reason).await?;
    }
    sqlx::query("DELETE FROM ingest_bindings WHERE property_id=$1 AND (($2::text IS NOT NULL AND visit_id=$2) OR ($3::text IS NOT NULL AND visitor_id=$3))")
        .bind(r.property_id).bind(&r.visit_id).bind(&r.visitor_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM ingest_proofs WHERE property_id=$1 AND (($2::text IS NOT NULL AND visit_id=$2) OR ($3::text IS NOT NULL AND visitor_id=$3))")
        .bind(r.property_id).bind(&r.visit_id).bind(&r.visitor_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM ingest_handoffs WHERE (property_id=$1 AND (($2::text IS NOT NULL AND visit_id=$2) OR ($3::text IS NOT NULL AND visitor_id=$3))) OR (audience=$1 AND (($2::text IS NOT NULL AND redeemed_visit_id=$2) OR ($3::text IS NOT NULL AND redeemed_visitor_id=$3)))")
        .bind(r.property_id).bind(&r.visit_id).bind(&r.visitor_id).execute(&mut *tx).await?;
    count(&mut tx, r.property_id, reason, 1).await?;
    tx.commit().await?;
    Ok(done())
}
pub async fn withdraw(
    state: &AppState,
    actor: &AuthContext,
    r: WithdrawalRequest,
) -> ApiResult<Value> {
    state
        .auth
        .authorize(actor, "privacy:delete", &Resource::property(r.property_id))
        .await?;
    if r.subject.is_none() && r.visitor_id.is_none() {
        return Err(ApiError::bad_request("Withdrawal target required"));
    }
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    if let Some(s) = &r.subject {
        suppress(
            &mut tx,
            r.property_id,
            "subject",
            subject_hash(s),
            "withdrawal",
        )
        .await?;
    }
    if let Some(v) = &r.visitor_id {
        suppress(&mut tx, r.property_id, "visitor", hash(v), "withdrawal").await?;
    }
    audit(
        &mut tx,
        actor,
        r.property_id,
        "withdrawal",
        json!({"target":"suppressed"}),
    )
    .await?;
    tx.commit().await?;
    Ok(done())
}
pub async fn delete(state: &AppState, actor: &AuthContext, r: DeletionRequest) -> ApiResult<Value> {
    state
        .auth
        .authorize(actor, "privacy:delete", &Resource::property(r.property_id))
        .await?;
    if r.subject.is_none() && r.visitor_id.is_none()
        || !crate::validation::bounded_text(&r.reason, 256)
    {
        return Err(ApiError::bad_request(
            "Deletion target and bounded reason required",
        ));
    }
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    // Verified mappings form a namespaced graph. Traverse both directions within
    // the authorized property; UUID equality across namespaces is never evidence.
    let mut subjects: Vec<(String, String)> = Vec::new();
    if let Some(s) = &r.subject {
        subjects.push((s.namespace.clone(), s.id.clone()));
    }
    if let Some(visitor) = &r.visitor_id {
        let rows=sqlx::query("SELECT DISTINCT subject_namespace,subject_id FROM ingest_events WHERE property_id=$1 AND visitor_id=$2 AND subject_id IS NOT NULL UNION SELECT subject_namespace,subject_id FROM ingest_bindings WHERE property_id=$1 AND visitor_id=$2")
            .bind(r.property_id).bind(visitor).fetch_all(&mut *tx).await?;
        for row in rows {
            subjects.push((row.get("subject_namespace"), row.get("subject_id")));
        }
    }
    let mut i = 0;
    while i < subjects.len() {
        if subjects.len() > 10000 {
            return Err(ApiError::bad_request(
                "Identity graph exceeds deletion budget; use scoped maintenance",
            ));
        }
        let (ns, id) = &subjects[i];
        let rows=sqlx::query("SELECT subject_namespace,subject_id,target_namespace,target_id FROM ingest_identity_links WHERE property_id=$1 AND ((subject_namespace=$2 AND subject_id=$3) OR (target_namespace=$2 AND target_id=$3))")
            .bind(r.property_id).bind(ns).bind(id).fetch_all(&mut *tx).await?;
        for row in rows {
            for value in [
                (
                    row.get::<String, _>("subject_namespace"),
                    row.get::<String, _>("subject_id"),
                ),
                (row.get("target_namespace"), row.get("target_id")),
            ] {
                if !subjects.contains(&value) {
                    subjects.push(value);
                }
            }
        }
        i += 1;
    }
    let mut visitors: Vec<String> = r.visitor_id.clone().into_iter().collect();
    let mut visits: Vec<String> = Vec::new();
    for (ns, id) in &subjects {
        let s = NamespacedId {
            namespace: ns.clone(),
            id: id.clone(),
        };
        suppress(
            &mut tx,
            r.property_id,
            "subject",
            subject_hash(&s),
            "deletion",
        )
        .await?;
        let rows=sqlx::query("SELECT DISTINCT visit_id,visitor_id FROM ingest_events WHERE property_id=$1 AND subject_namespace=$2 AND subject_id=$3 UNION SELECT visit_id,visitor_id FROM ingest_bindings WHERE property_id=$1 AND subject_namespace=$2 AND subject_id=$3")
            .bind(r.property_id).bind(ns).bind(id).fetch_all(&mut *tx).await?;
        for row in rows {
            if let Some(v) = row.get::<Option<String>, _>("visit_id") {
                if !v.is_empty() && !visits.contains(&v) {
                    visits.push(v);
                }
            }
            if let Some(v) = row.get::<Option<String>, _>("visitor_id") {
                if !v.is_empty() && !visitors.contains(&v) {
                    visitors.push(v);
                }
            }
        }
        for table in [
            "ingest_events",
            "ingest_acquisitions",
            "ingest_bindings",
            "ingest_proofs",
        ] {
            sqlx::query(&format!("DELETE FROM {table} WHERE property_id=$1 AND subject_namespace=$2 AND subject_id=$3"))
                .bind(r.property_id).bind(ns).bind(id).execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM ingest_identity_links WHERE property_id=$1 AND ((subject_namespace=$2 AND subject_id=$3) OR (target_namespace=$2 AND target_id=$3))")
            .bind(r.property_id).bind(ns).bind(id).execute(&mut *tx).await?;
    }
    for v in &visitors {
        suppress(&mut tx, r.property_id, "visitor", hash(v), "deletion").await?;
    }
    for v in &visits {
        suppress(&mut tx, r.property_id, "visit", hash(v), "deletion").await?;
    }
    for table in [
        "ingest_events",
        "ingest_touches",
        "ingest_touch_access",
        "ingest_bindings",
        "ingest_proofs",
    ] {
        sqlx::query(&format!(
            "DELETE FROM {table} WHERE property_id=$1 AND (visitor_id=ANY($2) OR visit_id=ANY($3))"
        ))
        .bind(r.property_id)
        .bind(&visitors)
        .bind(&visits)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM ingest_handoffs WHERE (property_id=$1 AND (visitor_id=ANY($2) OR visit_id=ANY($3))) OR (audience=$1 AND (redeemed_visitor_id=ANY($2) OR redeemed_visit_id=ANY($3)))")
        .bind(r.property_id).bind(&visitors).bind(&visits).execute(&mut *tx).await?;
    invalidate_exports(&mut tx, r.property_id).await?;
    // Audit excludes submitted reason (which may contain personal information).
    audit(&mut tx,actor,r.property_id,"privacy_delete",json!({"verified_identities":subjects.len(),"visitor_boundaries":visitors.len(),"reason_recorded":true})).await?;
    tx.commit().await?;
    Ok(done())
}
pub(crate) async fn invalidate_exports(
    db: &mut sqlx::PgConnection,
    property: Uuid,
) -> ApiResult<()> {
    let present: bool = sqlx::query_scalar("SELECT to_regclass('export_jobs') IS NOT NULL")
        .fetch_one(&mut *db)
        .await?;
    if present {
        sqlx::query("UPDATE export_jobs SET status='expired',manifest=NULL WHERE property_id=$1")
            .bind(property)
            .execute(db)
            .await?;
    }
    Ok(())
}
pub async fn retain(state: &AppState, actor: &AuthContext, property_id: Uuid) -> ApiResult<Value> {
    state
        .auth
        .authorize(actor, "privacy:delete", &Resource::property(property_id))
        .await?;
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    let p = property(&mut tx, property_id).await?;
    let mut removed = 0u64;
    for (table, time, days) in [
        (
            "ingest_events",
            "received_at",
            p.policy.event_retention_days,
        ),
        (
            "ingest_touches",
            "received_at",
            p.policy.attribution_window_days,
        ),
        (
            "ingest_acquisitions",
            "signed_up_at",
            p.policy.identity_retention_days,
        ),
        (
            "ingest_identity_links",
            "created_at",
            p.policy.identity_retention_days,
        ),
        (
            "ingest_bindings",
            "created_at",
            p.policy.identity_retention_days,
        ),
    ] {
        removed+=sqlx::query(&format!("DELETE FROM {table} WHERE ctid IN (SELECT ctid FROM {table} WHERE property_id=$1 AND (expires_at<=clock_timestamp() OR {time}<clock_timestamp()-make_interval(days=>$2)) LIMIT 10000)"))
            .bind(property_id).bind(days).execute(&mut *tx).await?.rows_affected();
    }
    for table in [
        "ingest_proofs",
        "ingest_handoffs",
        "ingest_touch_access",
        "ingest_receipts",
    ] {
        removed+=sqlx::query(&format!("DELETE FROM {table} WHERE ctid IN (SELECT ctid FROM {table} WHERE property_id=$1 AND expires_at<=clock_timestamp() LIMIT 10000)"))
            .bind(property_id).execute(&mut *tx).await?.rows_affected();
    }
    if removed > 0 {
        invalidate_exports(&mut tx, property_id).await?;
    }
    audit(
        &mut tx,
        actor,
        property_id,
        "retention",
        json!({"removed":removed}),
    )
    .await?;
    tx.commit().await?;
    Ok(json!({"removed":removed,"batch_limit_per_table":10000}))
}
