use analytics_core::*;
use chrono::{TimeZone, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct OidcVerifier {
    pub issuer: String,
    pub audience: String,
    pub metadata: Value,
    keys: Arc<RwLock<(JwkSet, Instant)>>,
    client: reqwest::Client,
    jwks_uri: String,
}
#[derive(Clone, Deserialize)]
struct AccessClaims {
    sub: String,
    exp: i64,
}
fn safe_url(url: &str) -> ApiResult<()> {
    let u = url::Url::parse(url).map_err(|_| ApiError::bad_request("Invalid issuer URL"))?;
    if u.scheme() != "https"
        && !(u.scheme() == "http"
            && matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")))
    {
        return Err(ApiError::bad_request(
            "Issuer endpoints require HTTPS (loopback HTTP allowed for local fixtures)",
        ));
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(ApiError::bad_request(
            "Issuer URL must not contain credentials",
        ));
    }
    Ok(())
}
impl OidcVerifier {
    pub async fn discover(issuer: String, audience: String) -> ApiResult<Self> {
        safe_url(&issuer)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ApiError::internal())?;
        let response = client
            .get(format!(
                "{}/.well-known/openid-configuration",
                issuer.trim_end_matches('/')
            ))
            .send()
            .await
            .map_err(|_| ApiError::internal())?;
        if !response.status().is_success() {
            return Err(ApiError::bad_request("OIDC discovery failed"));
        }
        let metadata: Value = response
            .json()
            .await
            .map_err(|_| ApiError::bad_request("Invalid OIDC discovery"))?;
        if metadata["issuer"].as_str() != Some(&issuer) {
            return Err(ApiError::bad_request("OIDC discovery issuer mismatch"));
        }
        let jwks = metadata["jwks_uri"]
            .as_str()
            .ok_or_else(|| ApiError::bad_request("OIDC discovery missing JWKS URI"))?;
        safe_url(jwks)?;
        for endpoint in ["authorization_endpoint", "token_endpoint"] {
            safe_url(
                metadata[endpoint]
                    .as_str()
                    .ok_or_else(|| ApiError::bad_request("Missing OIDC endpoint"))?,
            )?;
        }
        let jwks_uri = jwks.to_owned();
        let response = client
            .get(jwks)
            .send()
            .await
            .map_err(|_| ApiError::internal())?;
        if !response.status().is_success() {
            return Err(ApiError::bad_request("JWKS unavailable"));
        }
        let keys: JwkSet = response
            .json()
            .await
            .map_err(|_| ApiError::bad_request("Invalid JWKS"))?;
        Ok(Self {
            issuer,
            audience,
            metadata,
            keys: Arc::new(RwLock::new((keys, Instant::now()))),
            client,
            jwks_uri,
        })
    }
    pub async fn authenticate(&self, db: &PgPool, token: &str) -> ApiResult<AuthContext> {
        let header = decode_header(token).map_err(|_| ApiError::unauthorized())?;
        // RFC 9068 explicit access-token type excludes OIDC ID tokens and algorithm confusion.
        if header.alg != Algorithm::RS256
            || !matches!(header.typ.as_deref(), Some("at+jwt" | "application/at+jwt"))
        {
            return Err(ApiError::unauthorized());
        }
        let kid = header.kid.as_deref().ok_or_else(ApiError::unauthorized)?;
        let refresh = {
            let cache = self.keys.read().await;
            cache.1.elapsed() > Duration::from_secs(300)
                || (cache.0.find(kid).is_none() && cache.1.elapsed() > Duration::from_secs(10))
        };
        if refresh {
            let mut cache = self.keys.write().await;
            if cache.1.elapsed() > Duration::from_secs(10) {
                let response = self
                    .client
                    .get(&self.jwks_uri)
                    .send()
                    .await
                    .map_err(|_| ApiError::unauthorized())?;
                if !response.status().is_success() {
                    return Err(ApiError::unauthorized());
                }
                let keys: JwkSet = response
                    .json()
                    .await
                    .map_err(|_| ApiError::unauthorized())?;
                *cache = (keys, Instant::now());
            }
        }
        let cache = self.keys.read().await;
        let key = cache.0.find(kid).ok_or_else(ApiError::unauthorized)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        validation.leeway = 0;
        validation.validate_nbf = true;
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let claims = decode::<AccessClaims>(
            token,
            &DecodingKey::from_jwk(key).map_err(|_| ApiError::unauthorized())?,
            &validation,
        )
        .map_err(|_| ApiError::unauthorized())?
        .claims;
        drop(cache);
        let row=sqlx::query("SELECT p.id,p.delegated_by FROM oidc_bindings b JOIN principals p ON p.id=b.principal_id WHERE b.issuer=$1 AND b.subject=$2 AND p.enabled").bind(&self.issuer).bind(&claims.sub).fetch_optional(db).await?.ok_or_else(ApiError::unauthorized)?;
        let principal_id: uuid::Uuid = row.get("id");
        let expiry = Utc
            .timestamp_opt(claims.exp, 0)
            .single()
            .ok_or_else(ApiError::unauthorized)?;
        let session_id:uuid::Uuid=sqlx::query_scalar("INSERT INTO credentials(id,principal_id,name,verifier,expires_at,ceiling,oidc_issuer,oidc_subject) VALUES($1,$2,'oidc-access',$3,$4,$5,$6,$7) ON CONFLICT(verifier) DO UPDATE SET verifier=excluded.verifier RETURNING id")
            .bind(uuid::Uuid::new_v4()).bind(principal_id).bind(crate::auth::verifier(token)).bind(expiry)
            .bind(serde_json::json!([{"action":"*","resource":{"kind":"installation","id":"default"}}]))
            .bind(&self.issuer).bind(&claims.sub).fetch_one(db).await?;
        Ok(AuthContext {
            principal_id,
            credential_id: Some(session_id),
            delegated_by: row.get("delegated_by"),
            expires_at: Some(expiry),
        })
    }
}
