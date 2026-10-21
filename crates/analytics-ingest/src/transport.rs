use crate::{store::*, *};
use analytics_core::*;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

async fn browser_auth(
    state: &AppState,
    headers: &HeaderMap,
    id: Uuid,
    choice: bool,
) -> ApiResult<PropertyDefinition> {
    let raw = headers
        .get("x-property-token")
        .and_then(|h| h.to_str().ok())
        .filter(|s| s.len() <= 128)
        .ok_or_else(ApiError::unauthorized)?;
    let origin = headers
        .get("origin")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(ApiError::forbidden)?;
    let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ingest_browser_tokens WHERE property_id=$1 AND verifier=$2 AND revoked_at IS NULL AND expires_at>clock_timestamp())")
        .bind(id).bind(hash(raw)).fetch_one(&state.db).await?;
    if !valid {
        return Err(ApiError::unauthorized());
    }
    let mut conn = state.db.acquire().await?;
    let p = property(&mut conn, id).await?;
    if !p.allowed_origins.iter().any(|s| s == origin) {
        return Err(ApiError::forbidden());
    }
    if !choice
        && p.policy.respect_global_privacy_control
        && headers.get("sec-gpc").is_some_and(|v| v == "1")
    {
        return Err(ApiError::forbidden());
    }
    Ok(p)
}
async fn browser(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(batch): Json<IngestBatch>,
) -> ApiResult<Json<IngestAcknowledgement>> {
    let id = batch
        .events
        .first()
        .ok_or_else(|| ApiError::bad_request("Empty batch"))?
        .property_id;
    if batch.events.iter().any(|e| e.property_id != id) {
        return Err(ApiError::bad_request(
            "Public batch must contain one property",
        ));
    }
    browser_auth(&s, &headers, id, true).await?;
    Ok(Json(
        ingest::ingest(
            &s,
            batch,
            true,
            headers.get("sec-gpc").is_some_and(|v| v == "1"),
        )
        .await?,
    ))
}
async fn server(
    State(s): State<AppState>,
    a: AuthContext,
    Json(r): Json<IngestBatch>,
) -> ApiResult<Json<IngestAcknowledgement>> {
    Ok(Json(ingest_server(&s, &a, r).await?))
}
async fn config(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(r): Query<PropertyRequest>,
) -> ApiResult<Json<Value>> {
    let p = browser_auth(&s, &headers, r.property_id, true).await?;
    let rows=sqlx::query("SELECT value FROM admin_objects WHERE kind='schemas' AND scope_kind='property' AND scope_id=$1 ORDER BY id LIMIT 500").bind(r.property_id.to_string()).fetch_all(&s.db).await?;
    let schemas: Vec<Value> = rows
        .into_iter()
        .filter_map(|r| {
            let v: Value = r.get("value");
            if v["authoritative"] == false {
                Some(v)
            } else {
                None
            }
        })
        .collect();
    Ok(Json(
        json!({"property_id":p.id,"policy":p.policy,"handoff_audiences":p.handoff_audiences,"schemas":schemas,"limitations":["Public observations are unverified; not unique people or committed actions","Identified browser linkage requires authoritative server evidence","Missing campaign is unknown, never inferred direct traffic"]}),
    ))
}
async fn public_handoff(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(r): Json<HandoffRequest>,
) -> ApiResult<Json<HandoffResponse>> {
    browser_auth(&s, &headers, r.property_id, false).await?;
    Ok(Json(journey::handoff(&s, r).await?))
}
async fn public_identify(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(r): Json<IdentifyRequest>,
) -> ApiResult<Json<Value>> {
    browser_auth(&s, &headers, r.property_id, false).await?;
    Ok(Json(journey::identify(&s, r).await?))
}
async fn reset(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(r): Json<BrowserChoiceRequest>,
) -> ApiResult<Json<Value>> {
    browser_auth(&s, &headers, r.property_id, true).await?;
    Ok(Json(privacy::browser_choice(&s, r, true).await?))
}
async fn public_withdraw(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(r): Json<BrowserChoiceRequest>,
) -> ApiResult<Json<Value>> {
    browser_auth(&s, &headers, r.property_id, true).await?;
    Ok(Json(privacy::browser_choice(&s, r, false).await?))
}
macro_rules! protected {
    ($name:ident,$ty:ty,$operation:ident) => {
        async fn $name(
            State(s): State<AppState>,
            a: AuthContext,
            Json(r): Json<$ty>,
        ) -> ApiResult<Json<Value>> {
            Ok(Json($operation(&s, &a, r).await?))
        }
    };
}
protected!(redeem_http, RedeemHandoffRequest, redeem);
protected!(link_http, IdentityLinkRequest, identity_link);
protected!(proof_http, IssueProofRequest, issue_proof);
protected!(delete_http, DeletionRequest, delete);
protected!(withdraw_http, WithdrawalRequest, withdraw);
protected!(token_http, TokenRequest, issue_token);
protected!(revoke_http, RevokeTokenRequest, revoke_token);
protected!(correct_http, CorrectionRequest, correct_acquisition);
async fn retain_http(
    State(s): State<AppState>,
    a: AuthContext,
    Json(r): Json<PropertyRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(retain(&s, &a, r.property_id).await?))
}
async fn status_http(
    State(s): State<AppState>,
    a: AuthContext,
    Query(r): Query<PropertyRequest>,
) -> ApiResult<Json<Value>> {
    Ok(Json(status(&s, &a, r.property_id).await?))
}
async fn cors(request: Request<axum::body::Body>, next: Next) -> Response {
    let origin = request.headers().get("origin").cloned();
    let is_options = request.method() == axum::http::Method::OPTIONS;
    let mut response = if is_options {
        Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap()
    } else {
        next.run(request).await
    };
    // No ambient credentials: every actual public operation validates exact
    // configured origin + property token. Reflection alone never permits data.
    if let Some(origin) = origin {
        response
            .headers_mut()
            .insert("access-control-allow-origin", origin);
        response
            .headers_mut()
            .insert("vary", HeaderValue::from_static("Origin"));
        response.headers_mut().insert(
            "access-control-allow-methods",
            HeaderValue::from_static("GET, POST, OPTIONS"),
        );
        response.headers_mut().insert(
            "access-control-allow-headers",
            HeaderValue::from_static("content-type, x-property-token, sec-gpc"),
        );
    }
    response
}
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/ingest/browser", post(browser))
        .route("/v1/ingest/server", post(server))
        .route("/v1/ingest/config", get(config))
        .route("/v1/ingest/policy", get(config))
        .route("/v1/ingest/token", post(token_http))
        .route("/v1/ingest/token/revoke", post(revoke_http))
        .route("/v1/ingest/status", get(status_http))
        .route("/v1/ingest/retention", post(retain_http))
        .route("/v1/journey/handoff", post(public_handoff))
        .route("/v1/journey/redeem", post(redeem_http))
        .route("/v1/identity/link", post(link_http))
        .route("/v1/identity/proof", post(proof_http))
        .route("/v1/identity/identify", post(public_identify))
        .route("/v1/identity/reset", post(reset))
        .route("/v1/privacy/withdraw", post(public_withdraw))
        .route("/v1/privacy/withdraw-subject", post(withdraw_http))
        .route("/v1/privacy/delete", post(delete_http))
        .route("/v1/attribution/correct", post(correct_http))
        .layer(DefaultBodyLimit::max(262144))
        .layer(middleware::from_fn(cors))
}
