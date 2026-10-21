pub mod dto;
use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{FromRequestParts, State},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
pub use dto::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
pub const API_VERSION: &str = "1";

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
}
impl ApiError {
    pub fn new(status: StatusCode, code: &str, message: &str) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
        }
    }
    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "Permission denied")
    }
    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Valid access credential required",
        )
    }
    pub fn bad_request(message: &str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Operation failed",
        )
    }
}
impl From<sqlx::Error> for ApiError {
    fn from(_: sqlx::Error) -> Self {
        Self::internal()
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({"error":{"code":self.code,"message":self.message}})),
        )
            .into_response()
    }
}
pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthContext {
    pub principal_id: Uuid,
    pub credential_id: Option<Uuid>,
    pub delegated_by: Option<Uuid>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Installation,
    Property,
    Source,
    Export,
    Principal,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct Resource {
    pub kind: ResourceKind,
    pub id: String,
}
impl Resource {
    pub fn installation() -> Self {
        Self {
            kind: ResourceKind::Installation,
            id: "default".into(),
        }
    }
    pub fn property(id: Uuid) -> Self {
        Self {
            kind: ResourceKind::Property,
            id: id.to_string(),
        }
    }
    pub fn source(id: Uuid) -> Self {
        Self {
            kind: ResourceKind::Source,
            id: id.to_string(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct Grant {
    pub action: String,
    pub resource: Resource,
}
#[async_trait]
pub trait Authorization: Send + Sync {
    async fn authenticate(&self, bearer: &str) -> ApiResult<AuthContext>;
    /// MUST revalidate credential/principal and current grants, including after job queue delays.
    async fn authorize(
        &self,
        actor: &AuthContext,
        action: &str,
        resource: &Resource,
    ) -> ApiResult<()>;
    async fn grants(&self, actor: &AuthContext) -> ApiResult<Vec<Grant>>;
}
#[derive(Clone, Debug)]
pub struct ServiceConfig {
    pub public_base_url: String,
    pub export_directory: std::path::PathBuf,
    pub secret_directory: std::path::PathBuf,
    pub max_body_bytes: usize,
}
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub auth: Arc<dyn Authorization>,
    pub config: Arc<ServiceConfig>,
}
impl FromRequestParts<AppState> for AuthContext {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> ApiResult<Self> {
        let value = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(ApiError::unauthorized)?;
        state.auth.authenticate(value).await
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}
#[async_trait]
pub trait Extension: Send + Sync {
    fn name(&self) -> &'static str;
    fn router(&self) -> Router<AppState>;
    fn tools(&self) -> Vec<ToolDefinition>;
    async fn call(
        &self,
        state: &AppState,
        actor: &AuthContext,
        name: &str,
        arguments: Value,
    ) -> ApiResult<Value>;
}
/// Health is intentionally unauthenticated; readiness proves database connectivity.
pub async fn ready(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    Ok(Json(serde_json::json!({"ready":true})))
}

/// Configuration storage contract: extension code consumes validated versioned JSON DTOs.
pub async fn load_object<T: serde::de::DeserializeOwned>(
    db: &PgPool,
    kind: &str,
    id: &str,
) -> ApiResult<T> {
    let value: Value =
        sqlx::query_scalar("SELECT value FROM admin_objects WHERE kind=$1 AND id=$2")
            .bind(kind)
            .bind(id)
            .fetch_optional(db)
            .await?
            .ok_or_else(|| {
                ApiError::new(StatusCode::NOT_FOUND, "not_found", "Resource not found")
            })?;
    serde_json::from_value(value).map_err(|_| ApiError::internal())
}
