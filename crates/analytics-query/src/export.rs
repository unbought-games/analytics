use crate::{execution, source::*};
use analytics_core::*;
use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::path::PathBuf;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub fn scope(query: &QueryRequest) -> ApiResult<Resource> {
    match (query.source_id, query.property_id) {
        (Some(s), None) => Ok(Resource::source(s)),
        (None, Some(p)) => Ok(Resource::property(p)),
        _ => Err(ApiError::bad_request(
            "Exactly one property/source scope is required",
        )),
    }
}
fn api_source(_: SourceError) -> ApiError {
    ApiError::internal()
}
fn encode<T: serde::Serialize>(value: &T) -> ApiResult<Value> {
    serde_json::to_value(value).map_err(|_| ApiError::internal())
}

pub async fn create(
    state: &AppState,
    actor: &AuthContext,
    request: ExportRequest,
) -> ApiResult<ExportStatus> {
    let resource = scope(&request.query)?;
    state.auth.authorize(actor, "query:read", &resource).await?;
    state
        .auth
        .authorize(actor, "exports:create", &resource)
        .await?;
    if request.format != "jsonl" || request.retention_hours == 0 || request.retention_hours > 168 {
        return Err(ApiError::bad_request(
            "JSONL and retention of 1..168 hours required",
        ));
    }
    execution::validate_request(state, &request.query).await?;
    let revision = configuration_revision(state, &request.query).await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
        .bind(format!("export-admission:{}", actor.principal_id))
        .execute(&mut *tx)
        .await?;
    let queued:i64=sqlx::query_scalar("SELECT count(*) FROM export_jobs WHERE owner_principal_id=$1 AND status IN ('queued','running')").bind(actor.principal_id).fetch_one(&mut *tx).await?;
    if queued >= 100 {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "queue_full",
            "Export queue limit reached",
        ));
    }
    let id = Uuid::new_v4();
    let created_at = Utc::now();
    let expires_at = created_at + Duration::hours(request.retention_hours.into());
    sqlx::query("INSERT INTO export_jobs(id,owner_principal_id,actor,source_id,property_id,request,status,created_at,expires_at,configuration_revision) VALUES($1,$2,$3,$4,$5,$6,'queued',$7,$8,$9)")
        .bind(id).bind(actor.principal_id).bind(encode(actor)?).bind(request.query.source_id).bind(request.query.property_id).bind(encode(&request)?).bind(created_at).bind(expires_at).bind(revision).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit_records(principal_id,delegated_by,action,target,outcome) VALUES($1,$2,'exports:create',$3,'success')").bind(actor.principal_id).bind(actor.delegated_by).bind(id.to_string()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(ExportStatus {
        id,
        status: "queued".into(),
        created_at,
        expires_at,
        manifest_url: None,
        error: None,
    })
}

async fn configuration_revision(state: &AppState, query: &QueryRequest) -> ApiResult<i64> {
    let (kind, id) = if let Some(id) = query.source_id {
        ("sources", id)
    } else {
        (
            "properties",
            query.property_id.ok_or_else(ApiError::forbidden)?,
        )
    };
    sqlx::query_scalar("SELECT revision FROM admin_objects WHERE kind=$1 AND id=$2")
        .bind(kind)
        .bind(id.to_string())
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::forbidden)
}
async fn authorized_job(
    state: &AppState,
    actor: &AuthContext,
    id: Uuid,
    action: &str,
) -> ApiResult<sqlx::postgres::PgRow> {
    let row = sqlx::query("SELECT * FROM export_jobs WHERE id=$1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "not_found", "Export not found"))?;
    let request: ExportRequest =
        serde_json::from_value(row.get("request")).map_err(|_| ApiError::internal())?;
    let owner: AuthContext =
        serde_json::from_value(row.get("actor")).map_err(|_| ApiError::internal())?;
    let resource = scope(&request.query)?;
    // Original credential revocation also invalidates a queued job and every
    // subsequently retrieved artifact, even through a replacement credential.
    state
        .auth
        .authorize(&owner, "query:read", &resource)
        .await?;
    state
        .auth
        .authorize(&owner, "exports:create", &resource)
        .await?;
    state.auth.authorize(actor, "query:read", &resource).await?;
    if actor.principal_id != owner.principal_id {
        state
            .auth
            .authorize(actor, action, &Resource::installation())
            .await?;
    }
    if state
        .auth
        .authorize(
            actor,
            action,
            &Resource {
                kind: ResourceKind::Export,
                id: id.to_string(),
            },
        )
        .await
        .is_err()
    {
        state.auth.authorize(actor, action, &resource).await?;
    }
    execution::validate_enabled(state, &request.query).await?;
    if row.get::<i64, _>("configuration_revision")
        != configuration_revision(state, &request.query).await?
    {
        return Err(ApiError::forbidden());
    }
    Ok(row)
}
fn status_from(state: &AppState, row: &sqlx::postgres::PgRow) -> ExportStatus {
    let id: Uuid = row.get("id");
    let expires_at = row.get("expires_at");
    let status: String = if expires_at <= Utc::now() {
        "expired".into()
    } else {
        row.get("status")
    };
    let has_manifest: Option<Value> = row.get("manifest");
    ExportStatus {
        id,
        status: status.clone(),
        created_at: row.get("created_at"),
        expires_at,
        manifest_url: if has_manifest.is_some() && status != "expired" && status != "cancelled" {
            Some(format!(
                "{}/v1/exports/{id}/manifest",
                state.config.public_base_url.trim_end_matches('/')
            ))
        } else {
            None
        },
        error: row.get("error"),
    }
}
pub async fn status(state: &AppState, actor: &AuthContext, id: Uuid) -> ApiResult<ExportStatus> {
    Ok(status_from(
        state,
        &authorized_job(state, actor, id, "exports:read").await?,
    ))
}
pub async fn manifest(
    state: &AppState,
    actor: &AuthContext,
    id: Uuid,
) -> ApiResult<ExportManifest> {
    let row = authorized_job(state, actor, id, "exports:read").await?;
    let status = status_from(state, &row);
    if matches!(status.status.as_str(), "expired" | "cancelled") {
        return Err(ApiError::new(
            StatusCode::GONE,
            "unavailable",
            "Export no longer available",
        ));
    }
    let value: Option<Value> = row.get("manifest");
    serde_json::from_value(
        value
            .ok_or_else(|| ApiError::new(StatusCode::CONFLICT, "pending", "Export is not ready"))?,
    )
    .map_err(|_| ApiError::internal())
}
pub async fn cancel(state: &AppState, actor: &AuthContext, id: Uuid) -> ApiResult<ExportStatus> {
    authorized_job(state, actor, id, "exports:delete").await?;
    sqlx::query("UPDATE export_jobs SET status='cancelled',manifest=NULL,error=NULL WHERE id=$1 AND status IN ('queued','running')").bind(id).execute(&state.db).await?;
    status(state, actor, id).await
}
pub async fn delete(state: &AppState, actor: &AuthContext, id: Uuid) -> ApiResult<Value> {
    authorized_job(state, actor, id, "exports:delete").await?;
    sqlx::query("UPDATE export_jobs SET status='expired',manifest=NULL WHERE id=$1")
        .bind(id)
        .execute(&state.db)
        .await?;
    remove_files(state, id).await;
    Ok(json!({"id":id,"status":"expired"}))
}
async fn remove_files(state: &AppState, id: Uuid) {
    let _ = fs::remove_dir_all(state.config.export_directory.join(id.to_string())).await;
}

pub async fn download(
    state: &AppState,
    actor: &AuthContext,
    id: Uuid,
    file: &str,
) -> ApiResult<Response> {
    let manifest = manifest(state, actor, id).await?;
    if file != "data.jsonl" || !manifest.datasets.iter().any(|d| d.file_name == file) {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "Dataset not found",
        ));
    }
    let file = fs::File::open(
        state
            .config
            .export_directory
            .join(id.to_string())
            .join(file),
    )
    .await
    .map_err(|_| ApiError::internal())?;
    // Revalidate current authorization/status on every bounded chunk, including
    // mid-download revocation. A revoked connection terminates without new data.
    let stream = futures_util::stream::try_unfold(
        (file, state.clone(), actor.clone()),
        move |(mut file, state, actor)| async move {
            let row = authorized_job(&state, &actor, id, "exports:read")
                .await
                .map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access_revoked")
                })?;
            if !matches!(
                status_from(&state, &row).status.as_str(),
                "complete" | "partial"
            ) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "artifact_unavailable",
                ));
            }
            let mut buffer = vec![0u8; 64 * 1024];
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                return Ok(None);
            }
            buffer.truncate(n);
            Ok(Some((buffer, (file, state, actor))))
        },
    );
    Ok((
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "private, no-store"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"data.jsonl\"",
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

pub struct FileSink {
    file: tokio::io::BufWriter<fs::File>,
    hash: Sha256,
    pub rows: u64,
    pub bytes: u64,
    budget: Budget,
    pub columns: Vec<Column>,
}
impl FileSink {
    pub async fn new(path: PathBuf, budget: Budget) -> Result<Self, SourceError> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        Ok(Self {
            file: tokio::io::BufWriter::with_capacity(64 * 1024, options.open(path).await?),
            hash: Sha256::new(),
            rows: 0,
            bytes: 0,
            budget,
            columns: Vec::new(),
        })
    }
    pub async fn finish(mut self) -> Result<(u64, u64, String, Vec<Column>), SourceError> {
        self.file.flush().await?;
        self.file.get_ref().sync_all().await?;
        Ok((
            self.rows,
            self.bytes,
            format!("{:x}", self.hash.finalize()),
            self.columns,
        ))
    }
}
#[async_trait::async_trait]
impl RowSink for FileSink {
    async fn row(&mut self, row: Value) -> Result<(), SourceError> {
        let mut line = serde_json::to_vec(&row).map_err(|_| SourceError::Response)?;
        line.push(b'\n');
        if self.rows >= self.budget.max_rows
            || self.bytes + line.len() as u64 > self.budget.max_bytes
            || line.len() > self.budget.max_page_bytes
        {
            return Err(SourceError::Limit);
        }
        if self.columns.is_empty()
            && let Some(object) = row.as_object()
        {
            self.columns = object
                .iter()
                .map(|(k, v)| Column {
                    name: k.clone(),
                    data_type: match v {
                        Value::Null => "unknown",
                        Value::Bool(_) => "boolean",
                        Value::String(_) => "string",
                        Value::Array(_) => "array",
                        Value::Object(_) => "object",
                        Value::Number(_) => "number",
                    }
                    .into(),
                    nullable: true,
                    encoding: "lossless JSON; numeric strings".into(),
                    namespace: None,
                    unit: None,
                })
                .collect();
        }
        self.file.write_all(&line).await?;
        self.hash.update(&line);
        self.rows += 1;
        self.bytes += line.len() as u64;
        Ok(())
    }
    async fn columns(&mut self, columns: Vec<Column>) -> Result<(), SourceError> {
        self.columns = columns;
        Ok(())
    }
}

/// Start once per service process after migrations. Database claims enforce a
/// cluster-wide maximum of four active exports and configured source concurrency.
pub fn start_worker(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let _ = maintenance(&state).await;
            for _ in 0..4 {
                match claim(&state).await {
                    Ok(Some(id)) => {
                        let s = state.clone();
                        tokio::spawn(async move {
                            if run(&s, id).await.is_err() {
                                let _=sqlx::query("UPDATE export_jobs SET status='failed',error='export_failure' WHERE id=$1 AND status='running'").bind(id).execute(&s.db).await;
                            }
                            let _ =
                                sqlx::query("UPDATE export_jobs SET lease_until=now() WHERE id=$1")
                                    .bind(id)
                                    .execute(&s.db)
                                    .await;
                        });
                    }
                    _ => break,
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
    })
}
async fn maintenance(state: &AppState) -> ApiResult<()> {
    sqlx::query("UPDATE export_jobs SET status='failed',error='worker_interrupted' WHERE status='running' AND lease_until < now()").execute(&state.db).await?;
    sqlx::query("UPDATE export_jobs SET status='expired',manifest=NULL WHERE id IN (SELECT id FROM export_jobs WHERE expires_at<now() AND status<>'expired' ORDER BY expires_at LIMIT 1000)").execute(&state.db).await?;
    // A cancelled worker may still hold/create its temporary file. Its lease is
    // released when run() exits; process crashes are bounded by the lease expiry.
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM export_jobs WHERE status IN ('expired','cancelled','failed') AND cleaned_at IS NULL AND (lease_until IS NULL OR lease_until<=now()) ORDER BY created_at,id LIMIT 1000").fetch_all(&state.db).await?;
    for id in ids {
        let removed =
            match fs::remove_dir_all(state.config.export_directory.join(id.to_string())).await {
                Ok(()) => true,
                Err(e) => e.kind() == std::io::ErrorKind::NotFound,
            };
        if removed {
            sqlx::query("UPDATE export_jobs SET cleaned_at=now() WHERE id=$1 AND status IN ('expired','cancelled','failed')").bind(id).execute(&state.db).await?;
        }
    }
    Ok(())
}
async fn claim(state: &AppState) -> ApiResult<Option<Uuid>> {
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(782153921)")
        .execute(&mut *tx)
        .await?;
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM export_jobs WHERE status='running'")
        .fetch_one(&mut *tx)
        .await?;
    if active >= 4 {
        return Ok(None);
    }
    let rows=sqlx::query("SELECT id,request,source_id FROM export_jobs WHERE status='queued' AND expires_at>now() ORDER BY created_at LIMIT 100 FOR UPDATE SKIP LOCKED").fetch_all(&mut *tx).await?;
    for row in rows {
        let request: ExportRequest = match serde_json::from_value(row.get("request")) {
            Ok(r) => r,
            Err(_) => {
                sqlx::query("UPDATE export_jobs SET status='failed',error='invalid_persisted_request' WHERE id=$1").bind(row.get::<Uuid,_>("id")).execute(&mut *tx).await?;
                continue;
            }
        };
        let max = if let Some(source) = request.query.source_id {
            let raw: Option<Value> = sqlx::query_scalar(
                "SELECT value FROM admin_objects WHERE kind='sources' AND id=$1",
            )
            .bind(source.to_string())
            .fetch_optional(&mut *tx)
            .await?;
            let definition: SourceDefinition = match raw
                .and_then(|v| serde_json::from_value(v).ok())
            {
                Some(s) => s,
                None => {
                    sqlx::query("UPDATE export_jobs SET status='failed',error='source_unavailable' WHERE id=$1").bind(row.get::<Uuid,_>("id")).execute(&mut *tx).await?;
                    continue;
                }
            };
            definition
                .budget
                .max_concurrency
                .min(request.query.budget.max_concurrency)
                .clamp(1, 4)
        } else {
            request.query.budget.max_concurrency.clamp(1, 4)
        };
        let running:i64=sqlx::query_scalar("SELECT count(*) FROM export_jobs WHERE status='running' AND source_id IS NOT DISTINCT FROM $1 AND property_id IS NOT DISTINCT FROM $2").bind(request.query.source_id).bind(request.query.property_id).fetch_one(&mut *tx).await?;
        if running >= max as i64 {
            continue;
        }
        let id: Uuid = row.get("id");
        sqlx::query("UPDATE export_jobs SET status='running',started_at=now(),lease_until=now()+interval '310 seconds' WHERE id=$1").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(Some(id));
    }
    tx.commit().await?;
    Ok(None)
}
async fn run(state: &AppState, id: Uuid) -> ApiResult<()> {
    let row =
        sqlx::query("SELECT actor,request,created_at,expires_at FROM export_jobs WHERE id=$1")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    let actor: AuthContext =
        serde_json::from_value(row.get("actor")).map_err(|_| ApiError::internal())?;
    let request: ExportRequest =
        serde_json::from_value(row.get("request")).map_err(|_| ApiError::internal())?;
    authorized_job(state, &actor, id, "exports:read").await?;
    let budget = execution::budget(state, &request.query).await?;
    let directory = state.config.export_directory.join(id.to_string());
    fs::create_dir_all(&directory)
        .await
        .map_err(|_| ApiError::internal())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|_| ApiError::internal())?;
    }
    let temp = directory.join("data.partial");
    let mut sink = FileSink::new(temp.clone(), budget.clone())
        .await
        .map_err(api_source)?;
    let cancel = CancellationToken::new();
    let monitor_cancel = cancel.clone();
    let monitor_state = state.clone();
    let monitor_actor = actor.clone();
    let monitor = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            match authorized_job(&monitor_state, &monitor_actor, id, "exports:read").await {
                Ok(row) if status_from(&monitor_state, &row).status == "running" => (),
                _ => {
                    monitor_cancel.cancel();
                    break;
                }
            }
        }
    });
    let read_at = Utc::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(budget.timeout_ms),
        execution::execute(state, &request.query, &budget, &mut sink, &cancel),
    )
    .await
    .unwrap_or(Err(SourceError::Timeout));
    monitor.abort();
    let (rows, bytes, sha256, columns) = sink.finish().await.map_err(api_source)?;
    let (status, error, summary) = match result {
        Ok(summary) => (
            if summary.continuation.is_some() {
                "partial"
            } else {
                "complete"
            },
            None,
            summary,
        ),
        Err(SourceError::Cancelled) => (
            "cancelled",
            Some("cancelled".into()),
            ReadSummary::default(),
        ),
        Err(error) => (
            if rows > 0 { "partial" } else { "failed" },
            Some(error.to_string()),
            ReadSummary {
                continuation: match &error {
                    SourceError::RateLimited { retry_after } => Some(
                        json!({"retry_after_seconds":retry_after,"resume":"rerun and deduplicate IDs; partial last page"}),
                    ),
                    _ => Some(
                        json!({"resume":"rerun with a narrower window or larger authorized budget; deduplicate IDs"}),
                    ),
                },
                ..Default::default()
            },
        ),
    };
    let available = matches!(status, "complete" | "partial");
    if available {
        fs::rename(&temp, directory.join("data.jsonl"))
            .await
            .map_err(|_| ApiError::internal())?;
        fs::File::open(&directory)
            .await
            .map_err(|_| ApiError::internal())?
            .sync_all()
            .await
            .map_err(|_| ApiError::internal())?;
    }
    let base = state.config.public_base_url.trim_end_matches('/');
    let mut metadata = if summary.columns.is_empty() {
        columns
    } else {
        summary.columns
    };
    let (mut coverage, provenance) =
        execution::metadata(state, &request.query, &mut metadata).await?;
    coverage.complete &= status == "complete";
    coverage.limitations.extend(summary.warnings);
    coverage.limitations.extend(error.clone());
    let datasets = if available {
        vec![DatasetManifest {
            name: request
                .query
                .query
                .get("dataset")
                .and_then(Value::as_str)
                .unwrap_or("data")
                .into(),
            file_name: "data.jsonl".into(),
            media_type: "application/x-ndjson".into(),
            bytes,
            sha256,
            row_count: rows,
            columns: metadata
                .into_iter()
                .map(|c| ColumnDefinition {
                    name: c.name,
                    data_type: c.data_type,
                    nullable: c.nullable,
                    unit: c.unit,
                    namespace: c.namespace,
                    description: Some(c.encoding),
                })
                .collect(),
            download_url: format!("{base}/v1/exports/{id}/files/data.jsonl"),
            ordering: vec!["query ORDER BY or vendor response order; see provenance".into()],
        }]
    } else {
        Vec::new()
    };
    let manifest = ExportManifest {
        export_id: id,
        format_version: "1".into(),
        status: status.into(),
        datasets,
        query: request.query.clone(),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        source_read_at: vec![read_at, Utc::now()],
        watermark: summary.snapshot,
        coverage,
        continuation: summary.continuation,
        provenance,
    };
    // The status predicate closes cancellation/deletion races. Authorization is
    // checked again before publishing and at every later retrieval.
    authorized_job(state, &actor, id, "exports:read").await?;
    sqlx::query("UPDATE export_jobs SET status=$2,manifest=$3,error=$4 WHERE id=$1 AND status='running' AND expires_at>now()")
        .bind(id).bind(status).bind(encode(&manifest)?).bind(error).execute(&state.db).await?;
    Ok(())
}
