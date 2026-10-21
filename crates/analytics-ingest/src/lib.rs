//! Durable ingestion and verified journey/privacy operations.
mod ingest;
mod journey;
mod privacy;
mod store;
mod transport;
mod validation;
pub use ingest::ingest_server;
pub use journey::{
    BrowserChoiceRequest, IdentifyRequest, IssueProofRequest, identity_link, issue_proof, redeem,
};
pub use privacy::{delete, retain, withdraw};

use analytics_core::*;
use async_trait::async_trait;
use axum::Router;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use store::*;
use uuid::Uuid;

#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PropertyRequest {
    pub property_id: Uuid,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenRequest {
    pub property_id: Uuid,
    pub expires_at: chrono::DateTime<Utc>,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RevokeTokenRequest {
    pub property_id: Uuid,
    pub token_id: Uuid,
}
#[derive(Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CorrectionRequest {
    pub property_id: Uuid,
    pub subject: NamespacedId,
    pub first_touch_id: Option<Uuid>,
    pub reason: String,
}

pub async fn issue_token(
    state: &AppState,
    actor: &AuthContext,
    r: TokenRequest,
) -> ApiResult<Value> {
    state
        .auth
        .authorize(
            actor,
            "properties:manage",
            &Resource::property(r.property_id),
        )
        .await?;
    if r.expires_at <= Utc::now() || r.expires_at > Utc::now() + Duration::days(90) {
        return Err(ApiError::bad_request(
            "Public token expiry must be within 90 days",
        ));
    }
    let mut tx = state.db.begin().await?;
    property(&mut tx, r.property_id).await?;
    let raw = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO ingest_browser_tokens(id,property_id,verifier,expires_at) VALUES($1,$2,$3,$4)",
    )
    .bind(id)
    .bind(r.property_id)
    .bind(hash(&raw))
    .bind(r.expires_at)
    .execute(&mut *tx)
    .await?;
    audit(
        &mut tx,
        actor,
        r.property_id,
        "browser_token_issue",
        json!({"token_id":id}),
    )
    .await?;
    tx.commit().await?;
    Ok(json!({"id":id,"token":raw,"expires_at":r.expires_at}))
}
pub async fn revoke_token(
    state: &AppState,
    actor: &AuthContext,
    r: RevokeTokenRequest,
) -> ApiResult<Value> {
    state
        .auth
        .authorize(
            actor,
            "properties:manage",
            &Resource::property(r.property_id),
        )
        .await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("UPDATE ingest_browser_tokens SET revoked_at=clock_timestamp() WHERE id=$1 AND property_id=$2").bind(r.token_id).bind(r.property_id).execute(&mut *tx).await?;
    audit(
        &mut tx,
        actor,
        r.property_id,
        "browser_token_revoke",
        json!({"token_id":r.token_id}),
    )
    .await?;
    tx.commit().await?;
    Ok(done())
}
pub async fn status(state: &AppState, actor: &AuthContext, property: Uuid) -> ApiResult<Value> {
    state
        .auth
        .authorize(actor, "status:read", &Resource::property(property))
        .await?;
    let rows =
        sqlx::query("SELECT code,total FROM ingest_counters WHERE property_id=$1 ORDER BY code")
            .bind(property)
            .fetch_all(&state.db)
            .await?;
    let counters: serde_json::Map<String, Value> = rows
        .iter()
        .map(|r| (r.get("code"), json!(r.get::<i64, _>("total").to_string())))
        .collect();
    let watermark: Option<i64> =
        sqlx::query_scalar("SELECT max(receipt_cursor) FROM ingest_events WHERE property_id=$1")
            .bind(property)
            .fetch_one(&state.db)
            .await?;
    Ok(
        json!({"property_id":property,"counters":counters,"receipt_watermark":watermark.map(|v|v.to_string()),"queue":"synchronous durable batch; no volatile server queue","coverage":"public traffic is unverified; counters measure requests, not people"}),
    )
}
pub async fn correct_acquisition(
    state: &AppState,
    actor: &AuthContext,
    r: CorrectionRequest,
) -> ApiResult<Value> {
    state
        .auth
        .authorize(actor, "identity:manage", &Resource::property(r.property_id))
        .await?;
    if !validation::bounded_text(&r.reason, 256) {
        return Err(ApiError::bad_request("Bounded correction reason required"));
    }
    let mut tx = state.db.begin().await?;
    lock(&mut tx).await?;
    let previous:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(a) FROM ingest_acquisitions a WHERE property_id=$1 AND subject_namespace=$2 AND subject_id=$3 FOR UPDATE")
        .bind(r.property_id).bind(&r.subject.namespace).bind(&r.subject.id).fetch_optional(&mut *tx).await?;
    let previous = previous.ok_or_else(|| ApiError::bad_request("Unknown acquisition"))?;
    let campaign = if let Some(id) = r.first_touch_id {
        let campaign:Option<Value>=sqlx::query_scalar("SELECT campaign FROM ingest_touches WHERE touch_id=$1 AND property_id=$2 AND expires_at>clock_timestamp()")
            .bind(id).bind(r.property_id).fetch_optional(&mut *tx).await?;
        Some(
            campaign.ok_or_else(|| {
                ApiError::bad_request("Correction touch not in authorized property")
            })?,
        )
    } else {
        None
    };
    sqlx::query("UPDATE ingest_acquisitions SET first_touch_id=$4,campaign=$5,rule_version='audited-correction-v1' WHERE property_id=$1 AND subject_namespace=$2 AND subject_id=$3")
        .bind(r.property_id).bind(&r.subject.namespace).bind(&r.subject.id).bind(r.first_touch_id).bind(campaign).execute(&mut *tx).await?;
    audit(&mut tx,actor,r.property_id,"acquisition_correction",json!({"signup_event_id":previous["signup_event_id"],"previous_touch":previous["first_touch_id"],"new_touch":r.first_touch_id,"reason":r.reason})).await?;
    privacy::invalidate_exports(&mut tx, r.property_id).await?;
    tx.commit().await?;
    Ok(done())
}
fn tool<T: schemars::JsonSchema>(name: &str, description: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.into(),
        description: description.into(),
        input_schema: serde_json::to_value(schemars::schema_for!(T)).unwrap(),
    }
}
#[derive(Default)]
pub struct IngestionExtension;
#[async_trait]
impl Extension for IngestionExtension {
    fn name(&self) -> &'static str {
        "ingestion"
    }
    fn router(&self) -> Router<AppState> {
        transport::router()
    }
    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            tool::<IngestBatch>(
                "ingest_server",
                "Commit a bounded authoritative event batch; per-event durable acknowledgement",
            ),
            tool::<TokenRequest>(
                "ingest_token_issue",
                "Issue a property-scoped public token; returns raw value once",
            ),
            tool::<RevokeTokenRequest>("ingest_token_revoke", "Revoke a public ingestion token"),
            tool::<HandoffRequest>(
                "ingest_handoff",
                "Create an allowlisted audience-bound journey capability",
            ),
            tool::<RedeemHandoffRequest>(
                "ingest_redeem",
                "Redeem a journey capability through a trusted application",
            ),
            tool::<IdentityLinkRequest>(
                "ingest_identity_link",
                "Link namespaced identities backed by an authoritative enrollment/signup event",
            ),
            tool::<IssueProofRequest>(
                "ingest_identity_proof",
                "Issue a bounded browser identity capability from authoritative evidence",
            ),
            tool::<BrowserChoiceRequest>(
                "ingest_identity_reset",
                "Suppress old browser identifiers and end their verified bindings",
            ),
            tool::<WithdrawalRequest>(
                "ingest_withdraw",
                "Suppress future collection for subject or visitor",
            ),
            tool::<DeletionRequest>(
                "ingest_delete",
                "Delete scoped history, identities and invalidate existing exports",
            ),
            tool::<PropertyRequest>(
                "ingest_retention",
                "Apply configured retention in bounded batches",
            ),
            tool::<PropertyRequest>(
                "ingest_status",
                "Inspect committed acceptance/rejection and reported loss counters",
            ),
            tool::<CorrectionRequest>(
                "ingest_acquisition_correct",
                "Apply an explicit audited correction to immutable acquisition",
            ),
        ]
    }
    async fn call(
        &self,
        state: &AppState,
        actor: &AuthContext,
        name: &str,
        args: Value,
    ) -> ApiResult<Value> {
        match name {
            "ingest_server" => json(ingest_server(state, actor, parse(args)?).await?),
            "ingest_token_issue" => issue_token(state, actor, parse(args)?).await,
            "ingest_token_revoke" => revoke_token(state, actor, parse(args)?).await,
            "ingest_handoff" => {
                let r: HandoffRequest = parse(args)?;
                state
                    .auth
                    .authorize(actor, "identity:manage", &Resource::property(r.property_id))
                    .await?;
                json(journey::handoff(state, r).await?)
            }
            "ingest_redeem" => redeem(state, actor, parse(args)?).await,
            "ingest_identity_link" => identity_link(state, actor, parse(args)?).await,
            "ingest_identity_proof" => issue_proof(state, actor, parse(args)?).await,
            "ingest_identity_reset" => {
                let r: BrowserChoiceRequest = parse(args)?;
                state
                    .auth
                    .authorize(actor, "identity:manage", &Resource::property(r.property_id))
                    .await?;
                journey_reset(state, r).await
            }
            "ingest_withdraw" => withdraw(state, actor, parse(args)?).await,
            "ingest_delete" => delete(state, actor, parse(args)?).await,
            "ingest_retention" => {
                retain(state, actor, parse::<PropertyRequest>(args)?.property_id).await
            }
            "ingest_status" => {
                status(state, actor, parse::<PropertyRequest>(args)?.property_id).await
            }
            "ingest_acquisition_correct" => correct_acquisition(state, actor, parse(args)?).await,
            _ => Err(ApiError::bad_request("Unknown ingestion operation")),
        }
    }
}
async fn journey_reset(state: &AppState, r: BrowserChoiceRequest) -> ApiResult<Value> {
    privacy::browser_choice(state, r, true).await
}
