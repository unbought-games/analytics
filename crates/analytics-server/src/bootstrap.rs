use crate::{
    admin::ADMIN_LOCK,
    auth::{audit, insert_credential},
};
use analytics_core::*;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};
use uuid::Uuid;

/// Refuse symlinks and group/world-readable deployment secrets.
pub fn read_secret(path: &Path) -> std::io::Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::other("Secret must be a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::other(
                "Secret file permissions must be 0600 or stricter",
            ));
        }
    }
    let value = std::fs::read_to_string(path)?;
    Ok(value.trim().to_owned())
}
struct HandoffFile {
    path: std::path::PathBuf,
    file: File,
    committed: bool,
}
impl HandoffFile {
    fn create(path: &Path) -> ApiResult<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let meta = std::fs::symlink_metadata(parent)
            .map_err(|_| ApiError::bad_request("Handoff directory must exist"))?;
        if !meta.is_dir() {
            return Err(ApiError::bad_request("Handoff parent must be a directory"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err(ApiError::bad_request(
                    "Handoff directory must have mode 0700",
                ));
            }
        }
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path).map_err(|_| {
            ApiError::bad_request("Cannot create protected handoff; path must not exist")
        })?;
        Ok(Self {
            path: path.into(),
            file,
            committed: false,
        })
    }
    fn write(&mut self, value: Value) -> ApiResult<()> {
        serde_json::to_writer(&mut self.file, &value).map_err(|_| ApiError::internal())?;
        self.file
            .write_all(b"\n")
            .map_err(|_| ApiError::internal())?;
        self.file.sync_all().map_err(|_| ApiError::internal())?;
        if let Some(p) = self.path.parent() {
            File::open(p)
                .and_then(|f| f.sync_all())
                .map_err(|_| ApiError::internal())?;
        }
        Ok(())
    }
}
impl Drop for HandoffFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
pub async fn bootstrap(
    db: &PgPool,
    owner: &str,
    agent: &str,
    path: &Path,
    days: i64,
) -> ApiResult<(Uuid, Uuid)> {
    if owner.is_empty()
        || agent.is_empty()
        || owner.len() > 120
        || agent.len() > 120
        || !(1..=366).contains(&days)
    {
        return Err(ApiError::bad_request("Names and expiry are invalid"));
    }
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_LOCK)
        .execute(&mut *tx)
        .await?;
    let initialized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM installation)")
        .fetch_one(&mut *tx)
        .await?;
    if initialized {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "already_initialized",
            "Installation already initialized; use authorized credential management or explicit local recovery",
        ));
    }
    let mut file = HandoffFile::create(path)?;
    sqlx::query("INSERT INTO installation(id) VALUES(true)")
        .execute(&mut *tx)
        .await?;
    let owner_id = Uuid::new_v4();
    let agent_id = Uuid::new_v4();
    for (id, name, kind) in [(owner_id, owner, "human"), (agent_id, agent, "agent")] {
        sqlx::query("INSERT INTO principals(id,name,kind) VALUES($1,$2,$3)")
            .bind(id)
            .bind(name)
            .bind(kind)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO grants(principal_id,action,resource_kind,resource_id) VALUES($1,'*','installation','default')").bind(id).execute(&mut *tx).await?;
    }
    let credential = insert_credential(
        &mut tx,
        &CredentialRequest {
            principal_id: agent_id,
            name: "initial-agent".into(),
            expires_at: Utc::now() + Duration::days(days),
            grants: vec![Grant {
                action: "*".into(),
                resource: Resource::installation(),
            }],
        },
    )
    .await?;
    audit(&mut tx, None, "bootstrap", &owner_id.to_string(), "success").await?;
    file.write(json!({"owner_id":owner_id,"agent_id":agent_id,"credential":credential}))?;
    tx.commit().await?;
    file.committed = true;
    Ok((owner_id, agent_id))
}
pub async fn recover(
    db: &PgPool,
    principal: Uuid,
    path: &Path,
    days: i64,
    revoke_all: bool,
) -> ApiResult<()> {
    if !revoke_all || !(1..=366).contains(&days) {
        return Err(ApiError::bad_request(
            "Recovery requires explicit revoke-all and expiry in 1..366 days",
        ));
    }
    let mut tx = db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(ADMIN_LOCK)
        .execute(&mut *tx)
        .await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM principals WHERE id=$1 AND delegated_by IS NULL)",
    )
    .bind(principal)
    .fetch_one(&mut *tx)
    .await?;
    if !exists {
        return Err(ApiError::bad_request(
            "Recovery requires an existing nondelegated principal",
        ));
    }
    let mut file = HandoffFile::create(path)?;
    sqlx::query("UPDATE credentials SET revoked_at=COALESCE(revoked_at,now())")
        .execute(&mut *tx)
        .await?;
    // Revoke external bindings too; otherwise lost/compromised OIDC access survives local recovery.
    sqlx::query("DELETE FROM oidc_bindings")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE principals SET enabled=true WHERE id=$1")
        .bind(principal)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO grants(principal_id,action,resource_kind,resource_id) VALUES($1,'*','installation','default') ON CONFLICT DO NOTHING").bind(principal).execute(&mut *tx).await?;
    let credential = insert_credential(
        &mut tx,
        &CredentialRequest {
            principal_id: principal,
            name: "local-recovery".into(),
            expires_at: Utc::now() + Duration::days(days),
            grants: vec![Grant {
                action: "*".into(),
                resource: Resource::installation(),
            }],
        },
    )
    .await?;
    audit(
        &mut tx,
        None,
        "local_recovery:revoke_all",
        &principal.to_string(),
        "success",
    )
    .await?;
    file.write(json!({"principal_id":principal,"credential":credential,"revoked":"all credentials and OIDC bindings"}))?;
    tx.commit().await?;
    file.committed = true;
    Ok(())
}
