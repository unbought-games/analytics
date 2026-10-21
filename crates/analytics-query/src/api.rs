use crate::{analytics, execution, export};
use analytics_core::*;
use axum::{
    Json, Router,
    extract::{Path, State},
    response::Response,
    routing::{get, post},
};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Default)]
pub struct QueryExtension;
#[async_trait::async_trait]
impl Extension for QueryExtension {
    fn name(&self) -> &'static str {
        "query"
    }
    fn router(&self) -> Router<AppState> {
        Router::new()
            .route("/v1/exports", post(create))
            .route("/v1/exports/{id}", get(status).delete(delete))
            .route("/v1/exports/{id}/cancel", post(cancel))
            .route("/v1/exports/{id}/manifest", get(manifest))
            .route("/v1/exports/{id}/files/{file}", get(download))
            .route("/v1/sources/{id}/discover", get(discover))
            .route("/v1/properties/{id}/analytics", get(analytics_discover))
            .route("/v1/query/definitions", post(save))
            .route("/v1/query/reproduce", post(reproduce))
    }
    fn tools(&self) -> Vec<ToolDefinition> {
        [
            ("export_create","Create an authorized durable JSONL export; returns a queued job and retrievable manifest",serde_json::to_value(schemars::schema_for!(ExportRequest)).expect("serializable schema")),
            ("export_status","Inspect export state",id_schema()),("export_manifest","Retrieve typed export manifest",id_schema()),("export_cancel","Cancel a queued/running export",id_schema()),("export_delete","Expire and delete an export",id_schema()),
            ("query_source_discover","Describe only authorized source schemas/capabilities",id_schema()),("query_analytics_discover","Describe retained analytics and policy limitations",id_schema()),
            ("query_save","Save an immutable versioned query definition",serde_json::to_value(schemars::schema_for!(SavedDefinition)).expect("serializable schema")),
            ("query_reproduce","Run an exact saved definition/version at a specified cutoff",json!({"type":"object","required":["id","version","cutoff"],"properties":{"id":{"type":"string"},"version":{"type":"string"},"cutoff":{"type":"string"},"retention_hours":{"type":"integer"}},"additionalProperties":false})),
        ].into_iter().map(|(n,d,s)|ToolDefinition{name:n.into(),description:d.into(),input_schema:s}).collect()
    }
    async fn call(
        &self,
        state: &AppState,
        actor: &AuthContext,
        name: &str,
        args: Value,
    ) -> ApiResult<Value> {
        match name {
            "export_create" => value(export::create(state, actor, parse(args)?).await?),
            "export_status" => value(export::status(state, actor, id(&args)?).await?),
            "export_manifest" => value(export::manifest(state, actor, id(&args)?).await?),
            "export_cancel" => value(export::cancel(state, actor, id(&args)?).await?),
            "export_delete" => export::delete(state, actor, id(&args)?).await,
            "query_source_discover" => execution::discover(state, actor, id(&args)?).await,
            "query_analytics_discover" => analytics::discover(state, actor, id(&args)?).await,
            "query_save" => save_definition(state, actor, parse(args)?).await,
            "query_reproduce" => value(reproduce_definition(state, actor, parse(args)?).await?),
            _ => Err(ApiError::bad_request("Unknown query tool")),
        }
    }
}
fn id_schema() -> Value {
    json!({"type":"object","required":["id"],"properties":{"id":{"type":"string","format":"uuid"}},"additionalProperties":false})
}
fn id(v: &Value) -> ApiResult<Uuid> {
    v.get("id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| ApiError::bad_request("UUID id required"))
}
fn parse<T: serde::de::DeserializeOwned>(v: Value) -> ApiResult<T> {
    serde_json::from_value(v).map_err(|_| ApiError::bad_request("Invalid query arguments"))
}
fn value<T: serde::Serialize>(v: T) -> ApiResult<Value> {
    serde_json::to_value(v).map_err(|_| ApiError::internal())
}
async fn create(
    State(s): State<AppState>,
    a: AuthContext,
    Json(r): Json<ExportRequest>,
) -> ApiResult<Json<ExportStatus>> {
    Ok(Json(export::create(&s, &a, r).await?))
}
async fn status(
    State(s): State<AppState>,
    a: AuthContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ExportStatus>> {
    Ok(Json(export::status(&s, &a, id).await?))
}
async fn cancel(
    State(s): State<AppState>,
    a: AuthContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ExportStatus>> {
    Ok(Json(export::cancel(&s, &a, id).await?))
}
async fn delete(
    State(s): State<AppState>,
    a: AuthContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    Ok(Json(export::delete(&s, &a, id).await?))
}
async fn manifest(
    State(s): State<AppState>,
    a: AuthContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<ExportManifest>> {
    Ok(Json(export::manifest(&s, &a, id).await?))
}
async fn download(
    State(s): State<AppState>,
    a: AuthContext,
    Path((id, file)): Path<(Uuid, String)>,
) -> ApiResult<Response> {
    export::download(&s, &a, id, &file).await
}
async fn discover(
    State(s): State<AppState>,
    a: AuthContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    Ok(Json(execution::discover(&s, &a, id).await?))
}
async fn analytics_discover(
    State(s): State<AppState>,
    a: AuthContext,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    Ok(Json(analytics::discover(&s, &a, id).await?))
}
async fn save(
    State(s): State<AppState>,
    a: AuthContext,
    Json(r): Json<SavedDefinition>,
) -> ApiResult<Json<Value>> {
    Ok(Json(save_definition(&s, &a, r).await?))
}
async fn reproduce(
    State(s): State<AppState>,
    a: AuthContext,
    Json(r): Json<Reproduction>,
) -> ApiResult<Json<ExportStatus>> {
    Ok(Json(reproduce_definition(&s, &a, r).await?))
}
pub async fn save_definition(
    s: &AppState,
    a: &AuthContext,
    d: SavedDefinition,
) -> ApiResult<Value> {
    let scope = export::scope(&d.query)?;
    s.auth.authorize(a, "definitions:manage", &scope).await?;
    s.auth.authorize(a, "query:read", &scope).await?;
    execution::validate_request(s, &d.query).await?;
    if d.version.is_empty()
        || d.version.len() > 120
        || d.version.contains(':')
        || d.name.len() > 256
        || d.description.len() > 4096
    {
        return Err(ApiError::bad_request("Invalid definition metadata"));
    }
    let mut tx = s.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(d.id.to_string())
        .execute(&mut *tx)
        .await?;
    let existing: Option<Value> =
        sqlx::query_scalar("SELECT value FROM admin_objects WHERE kind='definitions' AND id=$1")
            .bind(d.id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
    if let Some(existing) = existing {
        let old: SavedDefinition = parse(existing)?;
        if export::scope(&old.query)? != scope {
            return Err(ApiError::bad_request("Definition scope is immutable"));
        }
    }
    let history:Option<Value>=sqlx::query_scalar("SELECT value FROM admin_objects WHERE kind='definition_versions' AND split_part(id,':',1)=$1 LIMIT 1").bind(d.id.to_string()).fetch_optional(&mut *tx).await?;
    if let Some(history) = history {
        let old: SavedDefinition = parse(history)?;
        if export::scope(&old.query)? != scope {
            return Err(ApiError::bad_request(
                "Definition scope is immutable across deletion",
            ));
        }
    }
    let version_id = format!("{}:{}", d.id, d.version);
    let version: Option<Value> = sqlx::query_scalar(
        "SELECT value FROM admin_objects WHERE kind='definition_versions' AND id=$1",
    )
    .bind(&version_id)
    .fetch_optional(&mut *tx)
    .await?;
    let encoded = value(&d)?;
    if version.is_some_and(|v| v != encoded) {
        return Err(ApiError::bad_request("Definition version is immutable"));
    }
    let kind = if d.query.source_id.is_some() {
        "source"
    } else {
        "property"
    };
    sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES('definition_versions',$1,$2,$3,$4) ON CONFLICT(kind,id) DO NOTHING").bind(version_id).bind(kind).bind(&scope.id).bind(&encoded).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES('definitions',$1,$2,$3,$4) ON CONFLICT(kind,id) DO UPDATE SET value=excluded.value,revision=admin_objects.revision+1,updated_at=now()")
        .bind(d.id.to_string()).bind(kind).bind(&scope.id).bind(&encoded).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit_records(principal_id,delegated_by,action,target,outcome) VALUES($1,$2,'definitions:manage',$3,'success')").bind(a.principal_id).bind(a.delegated_by).bind(d.id.to_string()).execute(&mut *tx).await?;
    tx.commit().await?;
    value(d)
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reproduction {
    id: Uuid,
    version: String,
    cutoff: chrono::DateTime<chrono::Utc>,
    #[serde(default = "retention")]
    retention_hours: u32,
}
fn retention() -> u32 {
    24
}
pub async fn reproduce_definition(
    s: &AppState,
    a: &AuthContext,
    r: Reproduction,
) -> ApiResult<ExportStatus> {
    let current: SavedDefinition = load_object(&s.db, "definitions", &r.id.to_string()).await?;
    let current_scope = export::scope(&current.query)?;
    s.auth
        .authorize(a, "definitions:read", &current_scope)
        .await?;
    let mut d: SavedDefinition = load_object(
        &s.db,
        "definition_versions",
        &format!("{}:{}", r.id, r.version),
    )
    .await?;
    if export::scope(&d.query)? != current_scope {
        return Err(ApiError::forbidden());
    }
    s.auth
        .authorize(a, "definitions:read", &export::scope(&d.query)?)
        .await?;
    d.query.cutoff = r.cutoff;
    export::create(
        s,
        a,
        ExportRequest {
            query: d.query,
            format: "jsonl".into(),
            retention_hours: r.retention_hours,
        },
    )
    .await
}
