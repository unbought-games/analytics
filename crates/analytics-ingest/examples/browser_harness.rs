//! Loopback-only synthetic browser integration harness, never a production binary.
use analytics_core::*;
use analytics_ingest::*;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::sync::Arc;
use uuid::Uuid;
struct Deny;
#[async_trait]
impl Authorization for Deny {
    async fn authenticate(&self, _: &str) -> ApiResult<AuthContext> {
        Err(ApiError::unauthorized())
    }
    async fn authorize(&self, _: &AuthContext, _: &str, _: &Resource) -> ApiResult<()> {
        Err(ApiError::forbidden())
    }
    async fn grants(&self, _: &AuthContext) -> ApiResult<Vec<Grant>> {
        Ok(vec![])
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("INGEST_TEST_DATABASE_URL")?;
    let base = PgPool::connect(&url).await?;
    let schema = format!("browser_test_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&base)
        .await?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .after_connect(move |connection, _| {
            let schema = schema.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {schema}"))
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&url)
        .await?;
    sqlx::raw_sql(include_str!("../../../migrations/0001_foundation.sql"))
        .execute(&pool)
        .await?;
    sqlx::raw_sql(include_str!("../../../migrations/0002_ingestion.sql"))
        .execute(&pool)
        .await?;
    let property_id = Uuid::new_v4();
    let origin = std::env::var("BROWSER_TEST_ORIGIN").unwrap_or("http://127.0.0.1:4173".into());
    let p = PropertyDefinition {
        id: property_id,
        name: "Synthetic browser harness".into(),
        allowed_origins: vec![origin],
        handoff_audiences: vec![],
        enabled: true,
        policy: CollectionPolicy {
            profile: CollectionProfile::Identified,
            version: 1,
            effective_at: Utc::now() - Duration::days(1),
            cross_property: false,
            event_retention_days: 30,
            identity_retention_days: 365,
            attribution_window_days: 30,
            allowed_query_keys: vec![],
            respect_global_privacy_control: true,
        },
    };
    sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES('properties',$1,'property',$1,$2)").bind(property_id.to_string()).bind(serde_json::to_value(p)?).execute(&pool).await?;
    let mut schemas: Vec<Value> = if let Ok(path) = std::env::var("BROWSER_SCHEMA_FILE") {
        serde_json::from_str(&std::fs::read_to_string(path)?)?
    } else {
        vec![
            json!({"name":"page_view","version":1,"authoritative":false,"category":"navigation","properties":{}}),
        ]
    };
    for schema in &mut schemas {
        schema["property_id"] = json!(property_id);
        let schema: EventSchemaDefinition = serde_json::from_value(schema.clone())?;
        sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES('schemas',$1,'property',$2,$3)").bind(format!("{}:{}:{}",property_id,schema.name,schema.version)).bind(property_id.to_string()).bind(serde_json::to_value(schema)?).execute(&pool).await?;
    }
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    use sha2::{Digest, Sha256};
    let verifier = format!("{:x}", Sha256::digest(token.as_bytes()));
    sqlx::query("INSERT INTO ingest_browser_tokens(id,property_id,verifier,expires_at) VALUES($1,$2,$3,now()+interval '1 hour')").bind(Uuid::new_v4()).bind(property_id).bind(verifier).execute(&pool).await?;
    let state = AppState {
        db: pool,
        auth: Arc::new(Deny),
        config: Arc::new(ServiceConfig {
            public_base_url: "http://127.0.0.1".into(),
            export_directory: "/tmp/synthetic-exports".into(),
            secret_directory: "/tmp/synthetic-secrets".into(),
            max_body_bytes: 262144,
        }),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    println!(
        "{}",
        json!({"endpoint":format!("http://{}",listener.local_addr()?),"property_id":property_id,"token":token})
    );
    let app = IngestionExtension
        .router()
        .route(
            "/__test/events",
            axum::routing::get(
                |axum::extract::State(s): axum::extract::State<AppState>| async move {
                    let rows: Vec<Value> = sqlx::query_scalar(
                        "SELECT payload FROM ingest_events ORDER BY receipt_cursor",
                    )
                    .fetch_all(&s.db)
                    .await
                    .unwrap();
                    axum::Json(rows)
                },
            ),
        )
        .with_state(state);
    axum::serve(listener, app).await?;
    Ok(())
}
