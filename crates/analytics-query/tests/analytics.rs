use analytics_core::*;
use analytics_query::{analytics, source::*};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
struct NoAuth;
#[async_trait]
impl Authorization for NoAuth {
    async fn authenticate(&self, _: &str) -> ApiResult<AuthContext> {
        Err(ApiError::unauthorized())
    }
    async fn authorize(&self, _: &AuthContext, _: &str, _: &Resource) -> ApiResult<()> {
        Err(ApiError::forbidden())
    }
    async fn grants(&self, _: &AuthContext) -> ApiResult<Vec<Grant>> {
        Err(ApiError::forbidden())
    }
}
#[derive(Default)]
struct Sink(Vec<Value>);
#[async_trait]
impl RowSink for Sink {
    async fn row(&mut self, v: Value) -> Result<(), SourceError> {
        self.0.push(v);
        Ok(())
    }
}
#[tokio::test]
async fn scoped_analytics_null_count_nested_aggregation_retention_visits_touch_origin() {
    let url = std::env::var("QUERY_TEST_DATABASE_URL")
        .expect("Disposable QUERY_TEST_DATABASE_URL required");
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let name = format!("analytics_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin)
        .await
        .unwrap();
    let mut parsed = reqwest::Url::parse(&url).unwrap();
    parsed.set_path(&format!("/{name}"));
    let db = sqlx::PgPool::connect(parsed.as_str()).await.unwrap();
    sqlx::raw_sql(include_str!("../../../migrations/0001_foundation.sql"))
        .execute(&db)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../../../migrations/0002_ingestion.sql"))
        .execute(&db)
        .await
        .unwrap();
    let state = AppState {
        db: db.clone(),
        auth: Arc::new(NoAuth),
        config: Arc::new(ServiceConfig {
            public_base_url: "http://localhost".into(),
            export_directory: "unused".into(),
            secret_directory: "unused".into(),
            max_body_bytes: 1024,
        }),
    };
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let other = Uuid::new_v4();
    let signup = Uuid::new_v4();
    let touch = Uuid::new_v4();
    let now = Utc::now();
    for (property, event, time, subject, amount, expired, category, visit) in [
        (
            a,
            first,
            now - Duration::hours(3),
            Value::Null,
            2,
            false,
            "navigation",
            "same-visit",
        ),
        (
            a,
            second,
            now - Duration::hours(2),
            json!({"id":"subject"}),
            3,
            false,
            "navigation",
            "same-visit",
        ),
        (
            b,
            other,
            now - Duration::hours(4),
            json!({"id":"other"}),
            40,
            false,
            "navigation",
            "same-visit",
        ),
        (
            a,
            Uuid::new_v4(),
            now - Duration::hours(4),
            json!({"id":"expired"}),
            90,
            true,
            "custom",
            "expired-visit",
        ),
        (
            b,
            signup,
            now - Duration::hours(1),
            json!({"id":"account"}),
            0,
            false,
            "signup",
            "signup-visit",
        ),
    ] {
        let payload = json!({"event_id":event,"subject":subject,"name":"observed","campaign":{"utm_campaign":"manufactured"},"properties":{"amount":amount}});
        sqlx::query("INSERT INTO ingest_events(property_id,event_id,payload,fingerprint,occurred_at,producer,category,policy_version,profile,visit_id,visitor_id,expires_at) VALUES($1,$2,$3,'fixture',$4,'browser',$5,1,'identified',$6,'visitor', $7)").bind(property).bind(event).bind(payload).bind(time).bind(category).bind(visit).bind(if expired{now-Duration::seconds(1)}else{now+Duration::days(30)}).execute(&db).await.unwrap();
    }
    sqlx::query("INSERT INTO ingest_touches(touch_id,property_id,event_id,occurred_at,visit_id,visitor_id,campaign,policy_version,expires_at) VALUES($1,$2,$3,$4,'same-visit','visitor',$5,1,$6)").bind(touch).bind(a).bind(second).bind(now-Duration::hours(2)).bind(json!({"utm_campaign":"manufactured"})).bind(now+Duration::days(30)).execute(&db).await.unwrap();
    sqlx::query("INSERT INTO ingest_acquisitions(property_id,subject_namespace,subject_id,signup_event_id,signed_up_at,first_touch_id,campaign,rule_version,policy_version,expires_at) VALUES($1,'issuer','account',$2,$3,$4,$5,'first/1',1,$6)").bind(b).bind(signup).bind(now-Duration::hours(1)).bind(touch).bind(json!({"utm_campaign":"manufactured"})).bind(now+Duration::days(30)).execute(&db).await.unwrap();
    let mut query = QueryRequest {
        source_id: None,
        property_id: Some(a),
        kind: "analytics".into(),
        query: json!({"dataset":"events","aggregates":[{"op":"count","field":"subject","alias":"counted"},{"op":"sum","field":"properties.amount","alias":"total"}]}),
        parameters: BTreeMap::new(),
        window: None,
        cutoff: now,
        definition_version: "cohort-input/1".into(),
        budget: QueryBudget {
            timeout_ms: 10000,
            max_rows: 1000,
            max_bytes: 1000000,
            max_concurrency: 1,
        },
    };
    let mut sink = Sink::default();
    analytics::run(
        &state,
        &query,
        &Budget::default(),
        &mut sink,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        sink.0,
        json!([{"counted":"1","total":"5"}])
            .as_array()
            .unwrap()
            .clone()
    );
    query.query = json!({"dataset":"events","filters":[{"field":"properties.amount","op":"ge","value":3}],"select":["event_id","properties.amount"]});
    let mut sink = Sink::default();
    analytics::run(
        &state,
        &query,
        &Budget::default(),
        &mut sink,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(sink.0.len(), 1);
    assert_eq!(sink.0[0]["event_id"], second.to_string());
    query.query = json!({"dataset":"visits"});
    let mut visits = Sink::default();
    analytics::run(
        &state,
        &query,
        &Budget::default(),
        &mut visits,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(visits.0.len(), 1);
    assert_eq!(visits.0[0]["event_id"], first.to_string());
    assert_eq!(visits.0[0]["property"], a.to_string());
    assert_eq!(visits.0[0]["linkage_permitted"], true);
    query.query = json!({"dataset":"touches"});
    let mut touches = Sink::default();
    analytics::run(
        &state,
        &query,
        &Budget::default(),
        &mut touches,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(touches.0.len(), 1);
    assert_eq!(touches.0[0]["touch_id"], touch.to_string());
    assert_eq!(touches.0[0]["linkage_permitted"], true);
    query.query = json!({"dataset":"acquisition"});
    let mut own = Sink::default();
    analytics::run(
        &state,
        &query,
        &Budget::default(),
        &mut own,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(own.0.is_empty());
    query.property_id = Some(b);
    let mut acquisitions = Sink::default();
    analytics::run(
        &state,
        &query,
        &Budget::default(),
        &mut acquisitions,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(acquisitions.0[0]["property"], b.to_string());
    assert_eq!(acquisitions.0[0]["visit_id"], "signup-visit");
    assert_eq!(acquisitions.0[0]["touch_id"], touch.to_string());
    assert!(acquisitions.0[0].get("origin_property").is_none());
    db.close().await;
    sqlx::query(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
}
