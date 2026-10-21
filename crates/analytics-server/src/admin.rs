use crate::auth::{audit, grant_subset, insert_credential, resource_kind};
use analytics_core::*;
use axum::http::StatusCode;
use chrono::Utc;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

// One lock serializes authorization-changing administration and local recovery/bootstrap.
pub const ADMIN_LOCK: i64 = 746021014;
pub fn parse<T: DeserializeOwned>(v: Value) -> ApiResult<T> {
    serde_json::from_value(v).map_err(|_| ApiError::bad_request("Invalid request shape"))
}
fn id(req: &AdminRequest) -> ApiResult<&str> {
    req.resource_id
        .as_deref()
        .filter(|s| !s.is_empty() && s.len() < 512)
        .ok_or_else(|| ApiError::bad_request("resource_id required"))
}
fn uuid(req: &AdminRequest) -> ApiResult<Uuid> {
    Uuid::parse_str(id(req)?).map_err(|_| ApiError::bad_request("UUID resource_id required"))
}
fn value(req: &AdminRequest) -> ApiResult<Value> {
    req.value
        .clone()
        .ok_or_else(|| ApiError::bad_request("value required"))
}
fn principal(id: Uuid) -> Resource {
    Resource {
        kind: ResourceKind::Principal,
        id: id.to_string(),
    }
}
fn action(kind: &str, write: bool) -> ApiResult<String> {
    if !matches!(
        kind,
        "properties" | "policies" | "schemas" | "sources" | "definitions"
    ) {
        return Err(ApiError::bad_request("Unknown resource kind"));
    }
    Ok(format!("{kind}:{}", if write { "manage" } else { "read" }))
}
fn validate_grants(grants: &[Grant]) -> ApiResult<()> {
    const ACTIONS: &[&str] = &[
        "*",
        "properties:read",
        "properties:manage",
        "policies:read",
        "policies:manage",
        "schemas:read",
        "schemas:manage",
        "sources:read",
        "sources:manage",
        "principals:read",
        "principals:manage",
        "grants:manage",
        "credentials:manage",
        "definitions:read",
        "definitions:manage",
        "status:read",
        "audit:read",
        "ingest:server",
        "identity:manage",
        "privacy:delete",
        "query:read",
        "exports:create",
        "exports:read",
        "exports:delete",
    ];
    if grants.len() > 128 {
        return Err(ApiError::bad_request("Too many grants"));
    }
    for g in grants {
        if !ACTIONS.contains(&g.action.as_str())
            || (g.resource.kind == ResourceKind::Installation && g.resource.id != "default")
            || (g.resource.kind != ResourceKind::Installation
                && Uuid::parse_str(&g.resource.id).is_err())
        {
            return Err(ApiError::bad_request("Invalid action or resource"));
        }
    }
    Ok(())
}
fn policy(p: &CollectionPolicy) -> ApiResult<()> {
    if p.version < 1
        || !(1..=3650).contains(&p.event_retention_days)
        || !(1..=3650).contains(&p.identity_retention_days)
        || !(1..=365).contains(&p.attribution_window_days)
        || p.effective_at > Utc::now() + chrono::Duration::minutes(1)
        || p.allowed_query_keys.iter().any(|s| {
            !matches!(
                s.as_str(),
                "utm_source"
                    | "utm_medium"
                    | "utm_campaign"
                    | "utm_term"
                    | "utm_content"
                    | "experiment_id"
            )
        })
    {
        return Err(ApiError::bad_request(
            "Invalid retention, effective date or allowed URL fields",
        ));
    }
    if p.profile != CollectionProfile::Identified && p.cross_property {
        return Err(ApiError::bad_request(
            "Cross-property collection requires identified profile",
        ));
    }
    Ok(())
}
fn endpoint(s: &str) -> ApiResult<()> {
    let u = url::Url::parse(s).map_err(|_| ApiError::bad_request("Invalid endpoint"))?;
    if !matches!(u.scheme(), "https" | "http" | "postgres" | "postgresql")
        || u.host_str().is_none()
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err(ApiError::bad_request(
            "Endpoint must be a host URL without credentials, query or fragment",
        ));
    }
    Ok(())
}
fn validate_object(kind: &str, key: &str, v: Value) -> ApiResult<(Resource, Value)> {
    match kind {
        "properties" => {
            let p: PropertyDefinition = parse(v)?;
            if p.id.to_string() != key
                || p.name.is_empty()
                || p.name.len() > 120
                || p.allowed_origins.len() > 64
                || p.handoff_audiences.len() > 64
            {
                return Err(ApiError::bad_request("Invalid property"));
            }
            policy(&p.policy)?;
            for o in &p.allowed_origins {
                endpoint(o)?;
                let u = url::Url::parse(o).unwrap();
                if !matches!(u.scheme(), "http" | "https") || u.path() != "/" {
                    return Err(ApiError::bad_request(
                        "Property origins must be HTTP origins",
                    ));
                }
            }
            Ok((Resource::property(p.id), json!(p)))
        }
        "schemas" => {
            let s: EventSchemaDefinition = parse(v)?;
            if key != format!("{}:{}:{}", s.property_id, s.name, s.version)
                || s.version < 1
                || s.name.is_empty()
                || s.name.len() > 120
                || s.properties.len() > 64
            {
                return Err(ApiError::bad_request("Invalid schema key or version"));
            }
            for (n, p) in &s.properties {
                if n.is_empty()
                    || n.len() > 64
                    || !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    || [
                        "password",
                        "token",
                        "secret",
                        "email",
                        "field_value",
                        "form_value",
                        "text",
                        "html",
                    ]
                    .contains(&n.to_lowercase().as_str())
                    || !matches!(p.kind.as_str(), "string" | "boolean" | "integer" | "number")
                    || p.max_length.is_some_and(|n| n > 2048)
                {
                    return Err(ApiError::bad_request(
                        "Invalid or prohibited schema property",
                    ));
                }
            }
            Ok((Resource::property(s.property_id), json!(s)))
        }
        "sources" => {
            let s: SourceDefinition = parse(v)?;
            if key != s.id.to_string()
                || s.name.is_empty()
                || s.name.len() > 120
                || !matches!(s.kind.as_str(), "postgres" | "prometheus" | "sentry")
                || s.budget.timeout_ms == 0
                || s.budget.timeout_ms > 120_000
                || s.budget.max_rows == 0
                || s.budget.max_rows > 1_000_000
                || s.budget.max_bytes == 0
                || s.budget.max_bytes > 1_073_741_824
                || s.budget.max_concurrency == 0
                || s.budget.max_concurrency > 32
            {
                return Err(ApiError::bad_request("Invalid source or workload budget"));
            }
            endpoint(&s.endpoint)?;
            if s.secret_ref.as_ref().is_some_and(|r| {
                r.is_empty()
                    || r.len() > 128
                    || !r
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                    || r == "."
                    || r == ".."
            }) {
                return Err(ApiError::bad_request(
                    "secret_ref must be a basename under the configured secret directory",
                ));
            }
            Ok((Resource::source(s.id), json!(s)))
        }
        "definitions" => {
            let d: SavedDefinition = parse(v)?;
            if key != d.id.to_string() || d.name.is_empty() || d.version.is_empty() {
                return Err(ApiError::bad_request("Invalid saved definition"));
            }
            let scope = match (d.query.property_id, d.query.source_id) {
                (Some(p), None) => Resource::property(p),
                (None, Some(s)) => Resource::source(s),
                _ => {
                    return Err(ApiError::bad_request(
                        "Definition requires exactly one scope",
                    ));
                }
            };
            Ok((scope, json!(d)))
        }
        _ => Err(ApiError::bad_request("Unknown resource kind")),
    }
}
fn row_scope(row: &sqlx::postgres::PgRow) -> ApiResult<Resource> {
    Ok(Resource {
        kind: parse(json!(row.get::<String, _>("scope_kind")))?,
        id: row.get("scope_id"),
    })
}
pub async fn execute(state: &AppState, actor: &AuthContext, req: AdminRequest) -> ApiResult<Value> {
    if !matches!(
        req.operation.as_str(),
        "list" | "get" | "put" | "delete" | "issue" | "rotate" | "revoke"
    ) {
        return Err(ApiError::bad_request("Unknown operation"));
    }
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_LOCK)
        .execute(&mut *tx)
        .await?;
    let result = execute_locked(state, actor, &req, &mut tx).await;
    match result {
        Ok(value) => {
            audit(
                &mut tx,
                Some(actor),
                &format!("{}:{}", req.resource_kind, req.operation),
                req.resource_id.as_deref().unwrap_or("collection"),
                "success",
            )
            .await?;
            tx.commit().await?;
            Ok(value)
        }
        Err(error) => {
            tx.rollback().await?;
            let mut audit_tx = state.db.begin().await?;
            audit(
                &mut audit_tx,
                Some(actor),
                "admin:denied",
                "administration",
                &error.code,
            )
            .await?;
            audit_tx.commit().await?;
            Err(error)
        }
    }
}
async fn execute_locked(
    state: &AppState,
    actor: &AuthContext,
    req: &AdminRequest,
    tx: &mut Transaction<'_, Postgres>,
) -> ApiResult<Value> {
    let write = !matches!(req.operation.as_str(), "list" | "get");
    match req.resource_kind.as_str() {
        "principals" => {
            if req.operation == "list" {
                let rows=sqlx::query("SELECT id,name,kind,enabled,delegated_by FROM principals ORDER BY id LIMIT 1000").fetch_all(&mut **tx).await?;
                let mut out = Vec::new();
                for r in rows {
                    if state
                        .auth
                        .authorize(actor, "principals:read", &principal(r.get("id")))
                        .await
                        .is_ok()
                    {
                        out.push(json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"kind":r.get::<String,_>("kind"),"enabled":r.get::<bool,_>("enabled"),"delegated_by":r.get::<Option<Uuid>,_>("delegated_by")}));
                    }
                }
                return Ok(json!(out));
            }
            let key = uuid(req)?;
            state
                .auth
                .authorize(
                    actor,
                    if write {
                        "principals:manage"
                    } else {
                        "principals:read"
                    },
                    &principal(key),
                )
                .await?;
            if req.operation == "get" {
                return sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',name,'kind',kind,'enabled',enabled,'delegated_by',delegated_by) FROM principals WHERE id=$1").bind(key).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::new(StatusCode::NOT_FOUND,"not_found","Principal not found"));
            }
            if req.operation == "delete" {
                sqlx::query("UPDATE principals SET enabled=false WHERE id=$1")
                    .bind(key)
                    .execute(&mut **tx)
                    .await?;
                return Ok(json!({"disabled":key}));
            }
            if req.operation != "put" {
                return Err(ApiError::bad_request("Unsupported principal operation"));
            }
            let p: PrincipalDefinition = parse(value(req)?)?;
            if p.id != key
                || p.name.is_empty()
                || p.name.len() > 120
                || !matches!(p.kind.as_str(), "human" | "agent" | "service")
                || p.delegated_by == Some(key)
            {
                return Err(ApiError::bad_request("Invalid principal"));
            }
            if let Some(parent) = p.delegated_by {
                state
                    .auth
                    .authorize(actor, "principals:manage", &principal(parent))
                    .await?;
                let depth: Option<Option<Uuid>> = sqlx::query_scalar(
                    "SELECT delegated_by FROM principals WHERE id=$1 AND enabled",
                )
                .bind(parent)
                .fetch_optional(&mut **tx)
                .await?;
                if depth != Some(None) {
                    return Err(ApiError::bad_request(
                        "Delegation must name an enabled nondelegated principal",
                    ));
                }
            }
            // Delegation parent is immutable: removing it could escalate existing grants.
            let existing: Option<Option<Uuid>> =
                sqlx::query_scalar("SELECT delegated_by FROM principals WHERE id=$1")
                    .bind(key)
                    .fetch_optional(&mut **tx)
                    .await?;
            if existing.is_some_and(|d| d != p.delegated_by) {
                return Err(ApiError::bad_request("Delegation parent is immutable"));
            }
            sqlx::query("INSERT INTO principals(id,name,kind,enabled,delegated_by) VALUES($1,$2,$3,$4,$5) ON CONFLICT(id) DO UPDATE SET name=excluded.name,kind=excluded.kind,enabled=excluded.enabled").bind(p.id).bind(&p.name).bind(&p.kind).bind(p.enabled).bind(p.delegated_by).execute(&mut **tx).await?;
            Ok(json!(p))
        }
        "grants" => {
            let key = uuid(req)?;
            state
                .auth
                .authorize(
                    actor,
                    if write {
                        "grants:manage"
                    } else {
                        "principals:read"
                    },
                    &principal(key),
                )
                .await?;
            if !write {
                return Ok(json!(crate::auth::principal_grants(&state.db, key).await?));
            }
            let grants: Vec<Grant> = if req.operation == "delete" {
                Vec::new()
            } else if req.operation == "put" {
                parse(value(req)?)?
            } else {
                return Err(ApiError::bad_request("Unsupported grants operation"));
            };
            validate_grants(&grants)?;
            let authority = state.auth.grants(actor).await?;
            if grants.iter().any(|g| !grant_subset(&authority, g)) {
                return Err(ApiError::forbidden());
            }
            // Removal also needs authority over removed grants; scoped managers cannot strip unrelated scopes.
            let previous = crate::auth::principal_grants(&state.db, key).await?;
            if previous.iter().any(|g| !grant_subset(&authority, g)) {
                return Err(ApiError::forbidden());
            }
            sqlx::query("DELETE FROM grants WHERE principal_id=$1")
                .bind(key)
                .execute(&mut **tx)
                .await?;
            for g in &grants {
                sqlx::query("INSERT INTO grants(principal_id,action,resource_kind,resource_id) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(key).bind(&g.action).bind(resource_kind(&g.resource.kind)).bind(&g.resource.id).execute(&mut **tx).await?;
            }
            Ok(json!(grants))
        }
        "oidc_bindings" => {
            let key = uuid(req)?;
            state
                .auth
                .authorize(
                    actor,
                    if write {
                        "principals:manage"
                    } else {
                        "principals:read"
                    },
                    &principal(key),
                )
                .await?;
            if !write {
                let values:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('principal_id',principal_id,'issuer',issuer,'subject',subject) FROM oidc_bindings WHERE principal_id=$1").bind(key).fetch_all(&mut **tx).await?;
                return Ok(json!(values));
            }
            if write {
                let authority = state.auth.grants(actor).await?;
                if crate::auth::principal_grants(&state.db, key)
                    .await?
                    .iter()
                    .any(|g| !grant_subset(&authority, g))
                {
                    return Err(ApiError::forbidden());
                }
            }
            if req.operation == "delete" {
                sqlx::query("DELETE FROM oidc_bindings WHERE principal_id=$1")
                    .bind(key)
                    .execute(&mut **tx)
                    .await?;
                return Ok(json!({"deleted":true}));
            }
            if req.operation != "put" {
                return Err(ApiError::bad_request("Unsupported binding operation"));
            }
            let b: OidcBinding = parse(value(req)?)?;
            if b.principal_id != key || b.subject.is_empty() || b.subject.len() > 512 {
                return Err(ApiError::bad_request("Invalid subject binding"));
            }
            endpoint(&b.issuer)?;
            sqlx::query("INSERT INTO oidc_bindings(issuer,subject,principal_id) VALUES($1,$2,$3) ON CONFLICT(issuer,subject) DO NOTHING").bind(&b.issuer).bind(&b.subject).bind(key).execute(&mut **tx).await?;
            let actual: Uuid = sqlx::query_scalar(
                "SELECT principal_id FROM oidc_bindings WHERE issuer=$1 AND subject=$2",
            )
            .bind(&b.issuer)
            .bind(&b.subject)
            .fetch_one(&mut **tx)
            .await?;
            if actual != key {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "binding_exists",
                    "Identity is already bound",
                ));
            }
            Ok(json!(b))
        }
        "credentials" => {
            if matches!(req.operation.as_str(), "issue" | "rotate") {
                let r: CredentialRequest = parse(value(req)?)?;
                state
                    .auth
                    .authorize(actor, "credentials:manage", &principal(r.principal_id))
                    .await?;
                validate_grants(&r.grants)?;
                let authority = state.auth.grants(actor).await?;
                let target = crate::auth::principal_grants(&state.db, r.principal_id).await?;
                if r.grants.is_empty()
                    || r.grants
                        .iter()
                        .any(|g| !grant_subset(&authority, g) || !grant_subset(&target, g))
                {
                    return Err(ApiError::forbidden());
                }
                if req.operation == "rotate" {
                    let old = uuid(req)?;
                    let changed=sqlx::query("UPDATE credentials SET revoked_at=now() WHERE id=$1 AND principal_id=$2 AND revoked_at IS NULL").bind(old).bind(r.principal_id).execute(&mut **tx).await?.rows_affected();
                    if changed != 1 {
                        return Err(ApiError::bad_request(
                            "Active credential not found for principal",
                        ));
                    }
                }
                return Ok(json!(insert_credential(tx, &r).await?));
            }
            if req.operation == "list" {
                let key = uuid(req)?;
                state
                    .auth
                    .authorize(actor, "credentials:manage", &principal(key))
                    .await?;
                let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'principal_id',principal_id,'name',name,'expires_at',expires_at,'revoked_at',revoked_at,'grants',ceiling) FROM credentials WHERE principal_id=$1 ORDER BY created_at LIMIT 1000").bind(key).fetch_all(&mut **tx).await?;
                return Ok(json!(rows));
            }
            let key = uuid(req)?;
            let owner: Uuid =
                sqlx::query_scalar("SELECT principal_id FROM credentials WHERE id=$1")
                    .bind(key)
                    .fetch_optional(&mut **tx)
                    .await?
                    .ok_or_else(ApiError::forbidden)?;
            state
                .auth
                .authorize(actor, "credentials:manage", &principal(owner))
                .await?;
            if matches!(req.operation.as_str(), "revoke" | "delete") {
                sqlx::query(
                    "UPDATE credentials SET revoked_at=COALESCE(revoked_at,now()) WHERE id=$1",
                )
                .bind(key)
                .execute(&mut **tx)
                .await?;
                Ok(json!({"revoked":key}))
            } else if req.operation == "get" {
                Ok(sqlx::query_scalar("SELECT jsonb_build_object('id',id,'principal_id',principal_id,'name',name,'expires_at',expires_at,'revoked_at',revoked_at,'grants',ceiling) FROM credentials WHERE id=$1").bind(key).fetch_one(&mut **tx).await?)
            } else {
                Err(ApiError::bad_request("Unsupported credential operation"))
            }
        }
        "audit" => {
            state
                .auth
                .authorize(actor, "audit:read", &Resource::installation())
                .await?;
            if write {
                return Err(ApiError::forbidden());
            }
            let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(a) FROM (SELECT sequence::text,principal_id,delegated_by,action,target,outcome,occurred_at FROM audit_records ORDER BY sequence DESC LIMIT 1000) a").fetch_all(&mut **tx).await?;
            Ok(json!(rows))
        }
        "policies" => {
            let key = uuid(req)?;
            state
                .auth
                .authorize(actor, &action("policies", write)?, &Resource::property(key))
                .await?;
            let mut p: PropertyDefinition =
                load_object(&state.db, "properties", &key.to_string()).await?;
            if !write {
                return Ok(json!(p.policy));
            }
            if req.operation != "put" {
                return Err(ApiError::bad_request(
                    "Policy updates require put; remove the property to delete configuration",
                ));
            }
            let next: CollectionPolicy = parse(value(req)?)?;
            policy(&next)?;
            if next.version <= p.policy.version {
                return Err(ApiError::bad_request("Policy version must increase"));
            }
            p.policy = next;
            sqlx::query("UPDATE admin_objects SET value=$1,revision=revision+1,updated_at=now() WHERE kind='properties' AND id=$2").bind(json!(p)).bind(key.to_string()).execute(&mut **tx).await?;
            Ok(json!(p.policy))
        }
        kind => {
            let permission = action(kind, write)?;
            if req.operation == "list" {
                let rows=sqlx::query("SELECT scope_kind,scope_id,value FROM admin_objects WHERE kind=$1 ORDER BY id LIMIT 1000").bind(kind).fetch_all(&mut **tx).await?;
                let mut out = Vec::new();
                for row in rows {
                    if state
                        .auth
                        .authorize(actor, &permission, &row_scope(&row)?)
                        .await
                        .is_ok()
                    {
                        out.push(row.get::<Value, _>("value"));
                    }
                }
                return Ok(json!(out));
            }
            let key = id(req)?;
            if kind == "definitions" {
                sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1)::bigint)")
                    .bind(key)
                    .execute(&mut **tx)
                    .await?;
            }
            let previous = sqlx::query(
                "SELECT scope_kind,scope_id,value FROM admin_objects WHERE kind=$1 AND id=$2",
            )
            .bind(kind)
            .bind(key)
            .fetch_optional(&mut **tx)
            .await?;
            if let Some(row) = &previous {
                state
                    .auth
                    .authorize(actor, &permission, &row_scope(row)?)
                    .await?;
            }
            if req.operation == "get" {
                return previous.map(|r| r.get("value")).ok_or_else(|| {
                    ApiError::new(StatusCode::NOT_FOUND, "not_found", "Resource not found")
                });
            }
            if req.operation == "delete" {
                if previous.is_none() {
                    return Err(ApiError::forbidden());
                }
                sqlx::query("DELETE FROM admin_objects WHERE kind=$1 AND id=$2")
                    .bind(kind)
                    .bind(key)
                    .execute(&mut **tx)
                    .await?;
                return Ok(json!({"deleted":true}));
            }
            if req.operation != "put" {
                return Err(ApiError::bad_request("Unsupported resource operation"));
            }
            let (scope, v) = validate_object(kind, key, value(req)?)?;
            if kind == "sources" {
                let previous_value = previous.as_ref().map(|r| r.get::<Value, _>("value"));
                let connection_changed = previous_value.as_ref().is_none_or(|old| {
                    ["kind", "endpoint", "secret_ref"]
                        .iter()
                        .any(|field| old[field] != v[field])
                });
                if connection_changed {
                    state
                        .auth
                        .authorize(actor, "sources:manage", &Resource::installation())
                        .await?;
                }
            }

            state.auth.authorize(actor, &permission, &scope).await?;
            if let Some(row) = &previous {
                if row_scope(row)? != scope {
                    return Err(ApiError::bad_request("Resource scope is immutable"));
                }
                let old: Value = row.get("value");
                if kind == "schemas" && old != v {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "immutable_schema",
                        "Publish a new schema version",
                    ));
                }
                if kind == "properties"
                    && old["policy"] != v["policy"]
                    && v["policy"]["version"].as_i64() <= old["policy"]["version"].as_i64()
                {
                    return Err(ApiError::bad_request(
                        "Changed policy requires increasing version",
                    ));
                }
            }
            if kind == "schemas" {
                let historical: Option<Value> = sqlx::query_scalar(
                    "SELECT value FROM admin_objects WHERE kind='schema_versions' AND id=$1",
                )
                .bind(key)
                .fetch_optional(&mut **tx)
                .await?;
                if historical.is_some_and(|old| old != v) {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "immutable_schema",
                        "Publish a new schema version",
                    ));
                }
                sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES('schema_versions',$1,$2,$3,$4) ON CONFLICT(kind,id) DO NOTHING").bind(key).bind(resource_kind(&scope.kind)).bind(&scope.id).bind(&v).execute(&mut **tx).await?;
            }
            if kind == "definitions" {
                let historical_scope=sqlx::query("SELECT scope_kind,scope_id FROM admin_objects WHERE kind='definition_versions' AND split_part(id,':',1)=$1 LIMIT 1").bind(key).fetch_optional(&mut **tx).await?;
                if let Some(row) = historical_scope
                    && row_scope(&row)? != scope
                {
                    return Err(ApiError::bad_request(
                        "Definition scope is immutable across deletion/recreation",
                    ));
                }

                let version = v["version"]
                    .as_str()
                    .ok_or_else(|| ApiError::bad_request("Missing definition version"))?;
                if version.len() > 120 || version.contains(':') {
                    return Err(ApiError::bad_request(
                        "Definition version must be 1..120 characters without colons",
                    ));
                }
                let version_key = format!("{key}:{version}");
                let existing: Option<Value> = sqlx::query_scalar(
                    "SELECT value FROM admin_objects WHERE kind='definition_versions' AND id=$1",
                )
                .bind(&version_key)
                .fetch_optional(&mut **tx)
                .await?;
                if existing.is_some_and(|old| old != v) {
                    return Err(ApiError::new(
                        StatusCode::CONFLICT,
                        "immutable_definition",
                        "Publish a new definition version",
                    ));
                }
                sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES('definition_versions',$1,$2,$3,$4) ON CONFLICT(kind,id) DO NOTHING").bind(&version_key).bind(resource_kind(&scope.kind)).bind(&scope.id).bind(&v).execute(&mut **tx).await?;
            }
            sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES($1,$2,$3,$4,$5) ON CONFLICT(kind,id) DO UPDATE SET value=excluded.value,revision=admin_objects.revision+1,updated_at=now()").bind(kind).bind(key).bind(resource_kind(&scope.kind)).bind(&scope.id).bind(&v).execute(&mut **tx).await?;
            Ok(v)
        }
    }
}
