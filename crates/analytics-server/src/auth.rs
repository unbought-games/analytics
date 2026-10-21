use crate::oidc::OidcVerifier;
use analytics_core::*;
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub struct DatabaseAuthorization {
    pub db: PgPool,
    pub oidc: Option<OidcVerifier>,
}
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("ae_{}", URL_SAFE_NO_PAD.encode(bytes))
}
pub fn verifier(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}
pub fn resource_kind(kind: &ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Installation => "installation",
        ResourceKind::Property => "property",
        ResourceKind::Source => "source",
        ResourceKind::Export => "export",
        ResourceKind::Principal => "principal",
    }
}
pub fn covers(grant: &Grant, action: &str, resource: &Resource) -> bool {
    (grant.action == "*" || grant.action == action)
        && (grant.resource == *resource || grant.resource == Resource::installation())
}
pub fn grant_subset(parent: &[Grant], child: &Grant) -> bool {
    parent
        .iter()
        .any(|g| covers(g, &child.action, &child.resource))
}
pub async fn principal_grants(db: &PgPool, id: Uuid) -> ApiResult<Vec<Grant>> {
    let rows =
        sqlx::query("SELECT action,resource_kind,resource_id FROM grants WHERE principal_id=$1")
            .bind(id)
            .fetch_all(db)
            .await?;
    rows.into_iter()
        .map(|r| {
            let kind: String = r.get("resource_kind");
            Ok(Grant {
                action: r.get("action"),
                resource: Resource {
                    kind: serde_json::from_value(Value::String(kind))
                        .map_err(|_| ApiError::internal())?,
                    id: r.get("resource_id"),
                },
            })
        })
        .collect()
}
impl DatabaseAuthorization {
    async fn valid(&self, actor: &AuthContext) -> ApiResult<Option<Vec<Grant>>> {
        if actor.expires_at.is_some_and(|e| e <= Utc::now()) {
            return Err(ApiError::unauthorized());
        }
        let row = sqlx::query("SELECT enabled,delegated_by FROM principals WHERE id=$1")
            .bind(actor.principal_id)
            .fetch_optional(&self.db)
            .await?
            .ok_or_else(ApiError::unauthorized)?;
        if !row.get::<bool, _>("enabled")
            || row.get::<Option<Uuid>, _>("delegated_by") != actor.delegated_by
        {
            return Err(ApiError::unauthorized());
        }
        if let Some(parent) = actor.delegated_by {
            let enabled: Option<bool> = sqlx::query_scalar(
                "SELECT enabled FROM principals WHERE id=$1 AND delegated_by IS NULL",
            )
            .bind(parent)
            .fetch_optional(&self.db)
            .await?;
            if enabled != Some(true) {
                return Err(ApiError::unauthorized());
            }
        }
        if let Some(id) = actor.credential_id {
            let value:Option<Value>=sqlx::query_scalar("SELECT ceiling FROM credentials WHERE id=$1 AND principal_id=$2 AND revoked_at IS NULL AND expires_at>now() AND (oidc_issuer IS NULL OR EXISTS(SELECT 1 FROM oidc_bindings b WHERE b.issuer=credentials.oidc_issuer AND b.subject=credentials.oidc_subject AND b.principal_id=credentials.principal_id))").bind(id).bind(actor.principal_id).fetch_optional(&self.db).await?;
            return Ok(Some(
                serde_json::from_value(value.ok_or_else(ApiError::unauthorized)?)
                    .map_err(|_| ApiError::internal())?,
            ));
        }
        Err(ApiError::unauthorized())
    }
}
#[async_trait]
impl Authorization for DatabaseAuthorization {
    async fn authenticate(&self, bearer: &str) -> ApiResult<AuthContext> {
        if bearer.len() > 16384 {
            return Err(ApiError::unauthorized());
        }
        let actor = if bearer.starts_with("ae_") {
            let row=sqlx::query("SELECT c.id,c.principal_id,c.expires_at,p.delegated_by FROM credentials c JOIN principals p ON p.id=c.principal_id WHERE verifier=$1 AND revoked_at IS NULL AND expires_at>now() AND enabled AND oidc_issuer IS NULL").bind(verifier(bearer)).fetch_optional(&self.db).await?.ok_or_else(ApiError::unauthorized)?;
            AuthContext {
                principal_id: row.get("principal_id"),
                credential_id: Some(row.get("id")),
                delegated_by: row.get("delegated_by"),
                expires_at: Some(row.get("expires_at")),
            }
        } else {
            self.oidc
                .as_ref()
                .ok_or_else(ApiError::unauthorized)?
                .authenticate(&self.db, bearer)
                .await?
        };
        self.valid(&actor).await?;
        Ok(actor)
    }
    async fn authorize(
        &self,
        actor: &AuthContext,
        action: &str,
        resource: &Resource,
    ) -> ApiResult<()> {
        let ceiling = self.valid(actor).await?;
        let grants = principal_grants(&self.db, actor.principal_id).await?;
        let mut allowed = grants.iter().any(|g| covers(g, action, resource));
        if let Some(ceiling) = ceiling {
            allowed &= ceiling.iter().any(|g| covers(g, action, resource));
        }
        if let Some(parent) = actor.delegated_by {
            allowed &= principal_grants(&self.db, parent)
                .await?
                .iter()
                .any(|g| covers(g, action, resource));
        }
        if allowed {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }
    async fn grants(&self, actor: &AuthContext) -> ApiResult<Vec<Grant>> {
        let ceiling = self.valid(actor).await?;
        let mut grants = principal_grants(&self.db, actor.principal_id).await?;
        // Intersect sets, retaining whichever member is narrower (important for scoped tokens of owners).
        fn intersect(a: Vec<Grant>, b: &[Grant]) -> Vec<Grant> {
            let mut out = Vec::new();
            for x in &a {
                for y in b {
                    let action = if x.action == "*" {
                        Some(y.action.clone())
                    } else if y.action == "*" || x.action == y.action {
                        Some(x.action.clone())
                    } else {
                        None
                    };
                    let resource = if x.resource == Resource::installation() {
                        Some(y.resource.clone())
                    } else if y.resource == Resource::installation() || x.resource == y.resource {
                        Some(x.resource.clone())
                    } else {
                        None
                    };
                    if let (Some(action), Some(resource)) = (action, resource) {
                        let g = Grant { action, resource };
                        if !out.contains(&g) {
                            out.push(g);
                        }
                    }
                }
            }
            out
        }
        if let Some(c) = ceiling {
            grants = intersect(grants, &c);
        }
        if let Some(p) = actor.delegated_by {
            grants = intersect(grants, &principal_grants(&self.db, p).await?);
        }
        Ok(grants)
    }
}
pub async fn insert_credential(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    request: &CredentialRequest,
) -> ApiResult<CredentialCreated> {
    if request.expires_at <= Utc::now()
        || request.expires_at > Utc::now() + chrono::Duration::days(366)
        || request.name.is_empty()
        || request.name.len() > 120
    {
        return Err(ApiError::bad_request(
            "Credential expiry must be in the next 366 days; name must be 1..120 characters",
        ));
    }
    let id = Uuid::new_v4();
    let token = generate_token();
    sqlx::query("INSERT INTO credentials(id,principal_id,name,verifier,expires_at,ceiling) VALUES($1,$2,$3,$4,$5,$6)").bind(id).bind(request.principal_id).bind(&request.name).bind(verifier(&token)).bind(request.expires_at).bind(serde_json::to_value(&request.grants).map_err(|_|ApiError::internal())?).execute(&mut **tx).await?;
    Ok(CredentialCreated {
        id,
        token,
        expires_at: request.expires_at,
    })
}
pub async fn audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor: Option<&AuthContext>,
    action: &str,
    target: &str,
    outcome: &str,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO audit_records(principal_id,delegated_by,action,target,outcome) VALUES($1,$2,$3,$4,$5)").bind(actor.map(|a|a.principal_id)).bind(actor.and_then(|a|a.delegated_by)).bind(action).bind(target).bind(outcome).execute(&mut **tx).await?;
    Ok(())
}
