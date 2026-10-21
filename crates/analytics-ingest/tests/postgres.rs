use analytics_core::*;
use analytics_ingest::*;
use async_trait::async_trait;
use axum::{body::Body, http::Request};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::{PgPool, Row, postgres::PgPoolOptions};
use std::{collections::BTreeMap, sync::Arc};
use tower::ServiceExt;
use uuid::Uuid;

struct TestAuthorization;
#[async_trait]
impl Authorization for TestAuthorization {
    async fn authenticate(&self, bearer: &str) -> ApiResult<AuthContext> {
        if bearer != "synthetic-test-server" {
            return Err(ApiError::unauthorized());
        }
        Ok(actor())
    }
    async fn authorize(&self, a: &AuthContext, _: &str, _: &Resource) -> ApiResult<()> {
        if a.principal_id.is_nil() {
            Err(ApiError::forbidden())
        } else {
            Ok(())
        }
    }
    async fn grants(&self, _: &AuthContext) -> ApiResult<Vec<Grant>> {
        Ok(vec![])
    }
}
fn actor() -> AuthContext {
    AuthContext {
        principal_id: Uuid::from_u128(1),
        credential_id: None,
        delegated_by: None,
        expires_at: None,
    }
}
async fn state() -> AppState {
    let url = std::env::var("INGEST_TEST_DATABASE_URL")
        .expect("Set INGEST_TEST_DATABASE_URL to a disposable PostgreSQL instance");
    let base = PgPool::connect(&url).await.unwrap();
    let schema = format!("ingest_test_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&base)
        .await
        .unwrap();
    base.close().await;
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
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../../../migrations/0001_foundation.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../../../migrations/0002_ingestion.sql"))
        .execute(&pool)
        .await
        .unwrap();
    AppState {
        db: pool,
        auth: Arc::new(TestAuthorization),
        config: Arc::new(ServiceConfig {
            public_base_url: "http://127.0.0.1".into(),
            export_directory: "/tmp/synthetic-exports".into(),
            secret_directory: "/tmp/synthetic-secrets".into(),
            max_body_bytes: 262144,
        }),
    }
}
async fn configure(s: &AppState, profile: CollectionProfile) -> (PropertyDefinition, String) {
    let p = PropertyDefinition {
        id: Uuid::new_v4(),
        name: "Synthetic property".into(),
        allowed_origins: vec!["https://site.example".into()],
        handoff_audiences: vec![],
        enabled: true,
        policy: CollectionPolicy {
            profile,
            version: 1,
            effective_at: Utc::now() - Duration::days(1),
            cross_property: true,
            event_retention_days: 30,
            identity_retention_days: 365,
            attribution_window_days: 30,
            allowed_query_keys: vec![],
            respect_global_privacy_control: true,
        },
    };
    save(s, &p).await;
    for (name, category, authoritative) in [
        ("page_view", EventCategory::Navigation, false),
        ("signup", EventCategory::Signup, true),
        ("enroll", EventCategory::Enrollment, true),
        ("action", EventCategory::HumanAction, true),
        ("effect", EventCategory::Scheduled, true),
        ("login", EventCategory::Custom, true),
    ] {
        let schema = EventSchemaDefinition {
            property_id: p.id,
            name: name.into(),
            version: 1,
            category,
            authoritative,
            properties: [
                ("target_namespace", "string"),
                ("target_id", "string"),
                ("world", "string"),
                ("duration_ms", "integer"),
            ]
            .into_iter()
            .map(|(k, t)| {
                (
                    k.into(),
                    PropertyType {
                        kind: t.into(),
                        required: false,
                        max_length: Some(128),
                        unit: None,
                    },
                )
            })
            .collect(),
        };
        sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES('schemas',$1,'property',$2,$3)").bind(format!("{}:{name}:1",p.id)).bind(p.id.to_string()).bind(serde_json::to_value(schema).unwrap()).execute(&s.db).await.unwrap();
    }
    let token = issue_token(
        s,
        &actor(),
        TokenRequest {
            property_id: p.id,
            expires_at: Utc::now() + Duration::days(1),
        },
    )
    .await
    .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    (p, token)
}
async fn save(s: &AppState, p: &PropertyDefinition) {
    sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES('properties',$1,'property',$1,$2) ON CONFLICT(kind,id) DO UPDATE SET value=EXCLUDED.value").bind(p.id.to_string()).bind(serde_json::to_value(p).unwrap()).execute(&s.db).await.unwrap();
}
fn event(p: &PropertyDefinition, name: &str) -> EventEnvelope {
    let (category, producer, clock) = match name {
        "signup" => (EventCategory::Signup, Producer::Server, "server"),
        "enroll" => (EventCategory::Enrollment, Producer::Server, "server"),
        "action" => (EventCategory::HumanAction, Producer::Server, "server"),
        "effect" => (EventCategory::Scheduled, Producer::Server, "game"),
        "login" => (EventCategory::Custom, Producer::Server, "server"),
        _ => (EventCategory::Navigation, Producer::Browser, "client"),
    };
    EventEnvelope {
        event_id: Uuid::new_v4(),
        property_id: p.id,
        name: name.into(),
        schema_version: 1,
        producer,
        category,
        occurred_at: Utc::now() - Duration::minutes(1),
        clock: clock.into(),
        logical_time: None,
        visit_id: if p.policy.profile == CollectionProfile::Minimal {
            None
        } else {
            Some("visit-1".into())
        },
        visitor_id: if p.policy.profile == CollectionProfile::Identified {
            Some("visitor-1".into())
        } else {
            None
        },
        subject: None,
        context: EventContext {
            environment: "test".into(),
            release: None,
            page: Some("https://site.example/play".into()),
            referrer: None,
            control: None,
            correlation_id: None,
        },
        campaign: None,
        touch_id: None,
        attribution_rule: None,
        collection: CollectionContext {
            profile: p.policy.profile.clone(),
            policy_version: 1,
            consent: true,
            sample_rate: 1.,
            dropped_events: 0,
        },
        properties: BTreeMap::new(),
    }
}
fn campaign(name: &str) -> Campaign {
    Campaign {
        utm_campaign: Some(name.into()),
        utm_source: Some("newsletter".into()),
        utm_medium: None,
        utm_term: None,
        utm_content: None,
        experiment_id: None,
    }
}
fn subject() -> NamespacedId {
    NamespacedId {
        namespace: "synthetic-issuer".into(),
        id: "issuer-subject-7".into(),
    }
}
async fn browser(s: &AppState, token: &str, events: Vec<EventEnvelope>) -> Value {
    request(
        s,
        "/v1/ingest/browser",
        token,
        json!({"events":events}),
        false,
    )
    .await
    .1
}
async fn request(
    s: &AppState,
    path: &str,
    token: &str,
    body: Value,
    trusted: bool,
) -> (u16, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("origin", "https://site.example");
    req = if trusted {
        req.header("authorization", format!("Bearer {token}"))
    } else {
        req.header("x-property-token", token)
    };
    let response = IngestionExtension
        .router()
        .with_state(s.clone())
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let code = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (code, serde_json::from_slice(&bytes).unwrap_or(json!(null)))
}
async fn accepted(s: &AppState, e: EventEnvelope) -> IngestAcknowledgement {
    let ack = ingest_server(s, &actor(), IngestBatch { events: vec![e] })
        .await
        .unwrap();
    assert_eq!(ack.results[0].status, "accepted", "{ack:?}");
    ack
}

#[tokio::test]
async fn durable_ack_dedup_conflict_late_arrival_and_scoped_touch_ids() {
    let s = state().await;
    let (p, t) = configure(&s, CollectionProfile::Identified).await;
    let (q, u) = configure(&s, CollectionProfile::Identified).await;
    let mut e = event(&p, "page_view");
    e.campaign = Some(campaign("first"));
    let a = browser(&s, &t, vec![e.clone()]).await;
    assert_eq!(a["results"][0]["status"], "accepted");
    assert_eq!(
        browser(&s, &t, vec![e.clone()]).await["results"][0]["status"],
        "duplicate"
    );
    let mut conflict = e.clone();
    conflict.context.control = Some("changed".into());
    assert_eq!(
        browser(&s, &t, vec![conflict]).await["results"][0]["status"],
        "conflict"
    );
    let mut scoped = e.clone();
    scoped.property_id = q.id;
    assert_eq!(
        browser(&s, &u, vec![scoped]).await["results"][0]["status"],
        "accepted"
    );
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT touch_id FROM ingest_touches WHERE event_id=$1")
            .bind(e.event_id)
            .fetch_all(&s.db)
            .await
            .unwrap();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    let mut late = event(&p, "page_view");
    late.occurred_at -= Duration::days(180);
    let b = browser(&s, &t, vec![late]).await;
    assert_eq!(b["results"][0]["status"], "accepted");
    assert!(
        b["results"][0]["receipt_cursor"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
            > a["results"][0]["receipt_cursor"]
                .as_str()
                .unwrap()
                .parse::<i64>()
                .unwrap()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_events")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(count, 3);
    let denied = AuthContext {
        principal_id: Uuid::nil(),
        credential_id: None,
        delegated_by: None,
        expires_at: None,
    };
    assert!(
        ingest_server(
            &s,
            &denied,
            IngestBatch {
                events: vec![event(&p, "action")]
            }
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn policy_profiles_authority_origin_and_prohibited_properties() {
    let s = state().await;
    for profile in [
        CollectionProfile::Minimal,
        CollectionProfile::Journey,
        CollectionProfile::Identified,
    ] {
        let (p, t) = configure(&s, profile).await;
        assert_eq!(
            browser(&s, &t, vec![event(&p, "page_view")]).await["results"][0]["status"],
            "accepted"
        );
        let mut e = event(&p, "page_view");
        e.subject = Some(subject());
        assert_eq!(
            browser(&s, &t, vec![e]).await["results"][0]["status"],
            "rejected"
        );
        let mut e = event(&p, "page_view");
        e.collection.consent = false;
        assert_eq!(
            browser(&s, &t, vec![e]).await["results"][0]["code"],
            "collection_disabled"
        );
        let mut e = event(&p, "page_view");
        e.properties
            .insert("password".into(), json!("not-persisted"));
        assert_eq!(
            browser(&s, &t, vec![e]).await["results"][0]["status"],
            "rejected"
        );
        let mut e = event(&p, "page_view");
        e.context.page = Some("https://site.example/?secret=x".into());
        assert_eq!(
            browser(&s, &t, vec![e]).await["results"][0]["code"],
            "unsafe_url"
        );
        let mut e = event(&p, "signup");
        e.subject = Some(subject());
        assert_eq!(
            browser(&s, &t, vec![e]).await["results"][0]["status"],
            "rejected"
        );
        let e = event(&p, "page_view");
        let req = Request::builder()
            .method("POST")
            .uri("/v1/ingest/browser")
            .header("content-type", "application/json")
            .header("origin", "https://hostile.example")
            .header("x-property-token", &t)
            .body(Body::from(json!({"events":[e]}).to_string()))
            .unwrap();
        assert_eq!(
            IngestionExtension
                .router()
                .with_state(s.clone())
                .oneshot(req)
                .await
                .unwrap()
                .status(),
            403
        );
        let e = event(&p, "page_view");
        let req = Request::builder()
            .method("POST")
            .uri("/v1/ingest/browser")
            .header("content-type", "application/json")
            .header("origin", "https://site.example")
            .header("sec-gpc", "1")
            .header("x-property-token", &t)
            .body(Body::from(json!({"events":[e]}).to_string()))
            .unwrap();
        let response = IngestionExtension
            .router()
            .with_state(s.clone())
            .oneshot(req)
            .await
            .unwrap();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["results"][0]["code"], "global_privacy_control");
    }
}

#[tokio::test]
async fn campaign_handoff_retry_expiry_immutable_signup_and_scheduled_semantics() {
    let s = state().await;
    let (mut p, t) = configure(&s, CollectionProfile::Identified).await;
    let (q, _) = configure(&s, CollectionProfile::Identified).await;
    p.handoff_audiences.push(q.id);
    save(&s, &p).await;
    let mut first = event(&p, "page_view");
    first.campaign = Some(campaign("acquired"));
    first.occurred_at -= Duration::days(2);
    let mut last = event(&p, "page_view");
    last.campaign = Some(campaign("last"));
    last.occurred_at -= Duration::days(1);
    assert_eq!(
        browser(
            &s,
            &t,
            vec![last.clone(), first.clone(), event(&p, "page_view")]
        )
        .await["results"][0]["status"],
        "accepted"
    );
    let (_,h)=request(&s,"/v1/journey/handoff",&t,json!({"property_id":p.id,"audience":q.id,"visit_id":"visit-1","visitor_id":"visitor-1","touch_id":first.event_id}),false).await;
    let redeem_request = RedeemHandoffRequest {
        handoff: h["handoff"].as_str().unwrap().into(),
        audience: q.id,
        visit_id: Some("destination-visit".into()),
        visitor_id: Some("destination-visitor".into()),
    };
    redeem(&s, &actor(), redeem_request.clone()).await.unwrap();
    assert_eq!(
        redeem(&s, &actor(), redeem_request.clone()).await.unwrap()["duplicate"],
        true
    );
    let mut wrong = redeem_request.clone();
    wrong.visitor_id = Some("wrong".into());
    assert!(redeem(&s, &actor(), wrong).await.is_err());
    let mut signup = event(&q, "signup");
    signup.subject = Some(subject());
    signup.visit_id = redeem_request.visit_id;
    signup.visitor_id = redeem_request.visitor_id;
    accepted(&s, signup.clone()).await;
    let row=sqlx::query("SELECT campaign,first_touch_id,last_touch_id FROM ingest_acquisitions WHERE property_id=$1").bind(q.id).fetch_one(&s.db).await.unwrap();
    assert_eq!(row.get::<Value, _>("campaign")["utm_campaign"], "acquired");
    assert_ne!(
        row.get::<Uuid, _>("first_touch_id"),
        row.get::<Uuid, _>("last_touch_id")
    );
    let mut again = signup.clone();
    again.event_id = Uuid::new_v4();
    again.campaign = Some(campaign("later"));
    accepted(&s, again).await;
    let campaign: Value =
        sqlx::query_scalar("SELECT campaign FROM ingest_acquisitions WHERE property_id=$1")
            .bind(q.id)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(campaign["utm_campaign"], "acquired");
    let mut login = event(&q, "login");
    login.visit_id = Some("existing-account-visit".into());
    login.visitor_id = Some("existing-account-visitor".into());
    login.subject = Some(NamespacedId {
        namespace: "synthetic-issuer".into(),
        id: "existing-account".into(),
    });
    accepted(&s, login).await;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ingest_acquisitions WHERE property_id=$1")
            .bind(q.id)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(count, 1);
    let mut effect = event(&q, "effect");
    effect.logical_time = Some("9007199254740993000".into());
    accepted(&s, effect).await;
    let category: String =
        sqlx::query_scalar("SELECT category FROM ingest_events WHERE payload->>'name'='effect'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(category, "scheduled");
    sqlx::query("UPDATE ingest_handoffs SET expires_at=now()-interval '1 second'")
        .execute(&s.db)
        .await
        .unwrap();
    let expired = RedeemHandoffRequest {
        handoff: h["handoff"].as_str().unwrap().into(),
        audience: q.id,
        visit_id: Some("destination-visit".into()),
        visitor_id: Some("destination-visitor".into()),
    };
    assert!(redeem(&s, &actor(), expired).await.is_err());
}

#[tokio::test]
async fn verified_identity_multiple_worlds_reset_withdrawal_and_deletion() {
    let s = state().await;
    let (p, t) = configure(&s, CollectionProfile::Identified).await;
    let mut signup = event(&p, "signup");
    signup.subject = Some(subject());
    signup
        .properties
        .insert("target_namespace".into(), json!("app-account"));
    signup
        .properties
        .insert("target_id".into(), json!("different-account-uuid"));
    accepted(&s, signup.clone()).await;
    identity_link(
        &s,
        &actor(),
        IdentityLinkRequest {
            property_id: p.id,
            subject: subject(),
            target: NamespacedId {
                namespace: "app-account".into(),
                id: "different-account-uuid".into(),
            },
            relationship: "account".into(),
            world: None,
            evidence_event_id: signup.event_id,
        },
    )
    .await
    .unwrap();
    for world in ["world-a", "world-b"] {
        let mut enroll = event(&p, "enroll");
        enroll.subject = Some(subject());
        enroll
            .properties
            .insert("target_namespace".into(), json!("game-player"));
        enroll
            .properties
            .insert("target_id".into(), json!("player-9"));
        enroll.properties.insert("world".into(), json!(world));
        accepted(&s, enroll.clone()).await;
        identity_link(
            &s,
            &actor(),
            IdentityLinkRequest {
                property_id: p.id,
                subject: subject(),
                target: NamespacedId {
                    namespace: "game-player".into(),
                    id: "player-9".into(),
                },
                relationship: "player".into(),
                world: Some(world.into()),
                evidence_event_id: enroll.event_id,
            },
        )
        .await
        .unwrap();
    }
    let proof = issue_proof(
        &s,
        &actor(),
        IssueProofRequest {
            property_id: p.id,
            visit_id: signup.visit_id.clone(),
            visitor_id: signup.visitor_id.clone(),
            evidence_event_id: signup.event_id,
        },
    )
    .await
    .unwrap();
    let (code,_)=request(&s,"/v1/identity/identify",&t,json!({"property_id":p.id,"visit_id":"visit-1","visitor_id":"visitor-1","proof":proof["proof"]}),false).await;
    assert_eq!(code, 200);
    let e = event(&p, "page_view");
    let id = e.event_id;
    browser(&s, &t, vec![e]).await;
    let linked: Option<String> =
        sqlx::query_scalar("SELECT subject_id FROM ingest_events WHERE event_id=$1")
            .bind(id)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(linked.as_deref(), Some("issuer-subject-7"));
    let (code, _) = request(
        &s,
        "/v1/identity/reset",
        &t,
        json!({"property_id":p.id,"visit_id":"visit-1","visitor_id":"visitor-1"}),
        false,
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(
        browser(&s, &t, vec![event(&p, "page_view")]).await["results"][0]["code"],
        "suppressed"
    );
    let mut fresh = event(&p, "page_view");
    fresh.visit_id = Some("new-visit".into());
    fresh.visitor_id = Some("new-visitor".into());
    let fresh_id = fresh.event_id;
    browser(&s, &t, vec![fresh]).await;
    let linked: Option<String> =
        sqlx::query_scalar("SELECT subject_id FROM ingest_events WHERE event_id=$1")
            .bind(fresh_id)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert!(linked.is_none());
    sqlx::raw_sql("CREATE TABLE export_jobs(property_id uuid,status text,manifest jsonb)")
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO export_jobs VALUES($1,'complete','{}')")
        .bind(p.id)
        .execute(&s.db)
        .await
        .unwrap();
    delete(
        &s,
        &actor(),
        DeletionRequest {
            property_id: p.id,
            subject: Some(subject()),
            visitor_id: None,
            reason: "synthetic deletion".into(),
        },
    )
    .await
    .unwrap();
    for table in ["ingest_acquisitions", "ingest_identity_links"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&s.db)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    let row = sqlx::query("SELECT status,manifest FROM export_jobs")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "expired");
    assert!(row.get::<Option<Value>, _>("manifest").is_none());
    signup.event_id = Uuid::new_v4();
    assert_eq!(
        ingest_server(
            &s,
            &actor(),
            IngestBatch {
                events: vec![signup]
            }
        )
        .await
        .unwrap()
        .results[0]
            .code
            .as_deref(),
        Some("suppressed")
    );
    let (journey, jt) = configure(&s, CollectionProfile::Journey).await;
    request(
        &s,
        "/v1/privacy/withdraw",
        &jt,
        json!({"property_id":journey.id,"visit_id":"visit-1"}),
        false,
    )
    .await;
    assert_eq!(
        browser(&s, &jt, vec![event(&journey, "page_view")]).await["results"][0]["code"],
        "suppressed"
    );
}

#[allow(dead_code)]
#[path = "../../../examples/producers/outbox.rs"]
mod producer;
#[tokio::test]
async fn transactional_outbox_rollback_outage_and_retry_recovery() {
    let s = state().await;
    let (p, _) = configure(&s, CollectionProfile::Identified).await;
    sqlx::raw_sql(include_str!("../../../examples/producers/outbox.sql"))
        .execute(&s.db)
        .await
        .unwrap();
    let e = event(&p, "action");
    let mut tx = s.db.begin().await.unwrap();
    producer::capture(&mut tx, &e, Uuid::new_v4())
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM synthetic_commands")
            .fetch_one(&s.db)
            .await
            .unwrap(),
        0
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM analytics_outbox")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let mut tx = s.db.begin().await.unwrap();
    producer::capture(&mut tx, &e, Uuid::new_v4())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    producer::dispatch(&s.db, "http://127.0.0.1:1", "synthetic-test-server")
        .await
        .unwrap();
    let row = sqlx::query("SELECT c.analytics_delivered_at AS delivered_at,o.last_error_code FROM analytics_outbox o JOIN synthetic_commands c ON c.command_id=o.event_id")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert!(
        row.get::<Option<chrono::DateTime<Utc>>, _>("delivered_at")
            .is_none()
    );
    assert_eq!(
        row.get::<String, _>("last_error_code"),
        "transport_unavailable"
    );
    sqlx::query("UPDATE analytics_outbox SET available_at=now()")
        .execute(&s.db)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = IngestionExtension.router().with_state(s.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    producer::dispatch(&s.db, &endpoint, "synthetic-test-server")
        .await
        .unwrap();
    let delivered: bool =
        sqlx::query_scalar("SELECT analytics_delivered_at IS NOT NULL FROM synthetic_commands")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert!(delivered);
    // Simulate crash after remote commit but before local delivery marking.
    sqlx::query("UPDATE synthetic_commands SET analytics_delivered_at=NULL")
        .execute(&s.db)
        .await
        .unwrap();
    producer::dispatch(&s.db, &endpoint, "synthetic-test-server")
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_events WHERE event_id=$1")
        .bind(e.event_id)
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    task.abort();
}

#[tokio::test]
async fn concurrent_receipts_retention_and_no_resurrection_after_horizon() {
    let s = state().await;
    let (p, t) = configure(&s, CollectionProfile::Identified).await;
    let mut e = event(&p, "page_view");
    e.campaign = Some(campaign("concurrent"));
    let (a, b) = tokio::join!(
        browser(&s, &t, vec![e.clone()]),
        browser(&s, &t, vec![e.clone()])
    );
    let mut statuses = vec![
        a["results"][0]["status"].as_str().unwrap(),
        b["results"][0]["status"].as_str().unwrap(),
    ];
    statuses.sort();
    assert_eq!(statuses, vec!["accepted", "duplicate"]);
    let mut follow = event(&p, "page_view");
    follow.touch_id = Some(e.event_id);
    assert_eq!(
        browser(&s, &t, vec![follow]).await["results"][0]["status"],
        "accepted"
    );
    let mut first = event(&p, "page_view");
    first.campaign = Some(campaign("same-batch"));
    let mut next = event(&p, "page_view");
    next.touch_id = Some(first.event_id);
    let ack = browser(&s, &t, vec![first, next]).await;
    assert!(
        ack["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["status"] == "accepted")
    );
    sqlx::query("UPDATE ingest_events SET expires_at=now()-interval '1 second'")
        .execute(&s.db)
        .await
        .unwrap();
    let retained = retain(&s, &actor(), p.id).await.unwrap();
    assert!(retained["removed"].as_u64().unwrap() >= 4);
    assert_eq!(
        browser(&s, &t, vec![e.clone()]).await["results"][0]["status"],
        "duplicate"
    );
    sqlx::query("UPDATE ingest_receipts SET expires_at=now()-interval '1 second'")
        .execute(&s.db)
        .await
        .unwrap();
    retain(&s, &actor(), p.id).await.unwrap();
    let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_receipts")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(receipts, 0);
    e.occurred_at = Utc::now() - Duration::days(366);
    assert_eq!(
        browser(&s, &t, vec![e]).await["results"][0]["code"],
        "outside_dedupe_horizon"
    );
    let mut quota = event(&p, "page_view");
    quota.collection.dropped_events = 7;
    browser(&s, &t, vec![quota.clone()]).await;
    browser(&s, &t, vec![quota]).await;
    let drops: i64 = sqlx::query_scalar(
        "SELECT total FROM ingest_counters WHERE property_id=$1 AND code='client_reported_dropped'",
    )
    .bind(p.id)
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(drops, 7);
    sqlx::query("UPDATE ingest_quotas SET window_start=date_trunc('minute',now()),events=6000 WHERE property_id=$1").bind(p.id).execute(&s.db).await.unwrap();
    assert_eq!(
        browser(&s, &t, vec![event(&p, "page_view")]).await["results"][0]["code"],
        "rate_limited"
    );
}

#[tokio::test]
async fn deletion_after_reset_and_event_retention_removes_original_touches() {
    let s = state().await;
    let (mut p, t) = configure(&s, CollectionProfile::Identified).await;
    let (destination, _) = configure(&s, CollectionProfile::Identified).await;
    p.handoff_audiences.push(destination.id);
    save(&s, &p).await;
    let mut touch = event(&p, "page_view");
    touch.campaign = Some(campaign("privacy"));
    browser(&s, &t, vec![touch]).await;
    let mut signup = event(&p, "signup");
    signup.subject = Some(subject());
    accepted(&s, signup.clone()).await;
    let mut switch = signup.clone();
    switch.event_id = Uuid::new_v4();
    switch.subject.as_mut().unwrap().id = "other-account".into();
    assert_eq!(
        ingest_server(
            &s,
            &actor(),
            IngestBatch {
                events: vec![switch]
            }
        )
        .await
        .unwrap()
        .results[0]
            .code
            .as_deref(),
        Some("identity_conflict")
    );
    let (_,handoff)=request(&s,"/v1/journey/handoff",&t,json!({"property_id":p.id,"audience":destination.id,"visit_id":"visit-1","visitor_id":"visitor-1"}),false).await;
    redeem(
        &s,
        &actor(),
        RedeemHandoffRequest {
            handoff: handoff["handoff"].as_str().unwrap().into(),
            audience: destination.id,
            visit_id: Some("other-visit".into()),
            visitor_id: Some("other-visitor".into()),
        },
    )
    .await
    .unwrap();
    request(
        &s,
        "/v1/identity/reset",
        &t,
        json!({"property_id":p.id,"visit_id":"visit-1","visitor_id":"visitor-1"}),
        false,
    )
    .await;
    sqlx::query("UPDATE ingest_events SET expires_at=now()-interval '1 second'")
        .execute(&s.db)
        .await
        .unwrap();
    retain(&s, &actor(), p.id).await.unwrap();
    delete(
        &s,
        &actor(),
        DeletionRequest {
            property_id: p.id,
            subject: Some(subject()),
            visitor_id: None,
            reason: "synthetic request".into(),
        },
    )
    .await
    .unwrap();
    let touches: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_touches")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(touches, 0);
    let access: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_touch_access")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(access, 0);
    let receipts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ingest_receipts WHERE property_id=$1")
            .bind(p.id)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(receipts, 0);
    assert_eq!(
        ingest_server(
            &s,
            &actor(),
            IngestBatch {
                events: vec![signup.clone()]
            }
        )
        .await
        .unwrap()
        .results[0]
            .code
            .as_deref(),
        Some("deleted_event")
    );
    let bindings: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_bindings")
        .fetch_one(&s.db)
        .await
        .unwrap();
    assert_eq!(bindings, 0);
}

#[tokio::test]
async fn policy_change_retry_partial_ack_revocation_and_attribution_boundaries() {
    let s = state().await;
    let (mut p, t) = configure(&s, CollectionProfile::Identified).await;
    let e = event(&p, "page_view");
    browser(&s, &t, vec![e.clone()]).await;
    p.policy.version = 2;
    save(&s, &p).await;
    assert_eq!(
        browser(&s, &t, vec![e]).await["results"][0]["status"],
        "duplicate"
    );
    let mut stale = event(&p, "page_view");
    let mut valid = stale.clone();
    valid.event_id = Uuid::new_v4();
    valid.collection.policy_version = 2;
    let ack = browser(&s, &t, vec![valid, stale.clone()]).await;
    assert_eq!(ack["results"][0]["status"], "accepted");
    assert_eq!(ack["results"][1]["code"], "policy_mismatch");
    p.policy.version = 1;
    save(&s, &p).await;
    let signup_at = Utc::now() - Duration::minutes(1);
    let mut touches = Vec::new();
    for (name, time) in [
        (
            "too-old",
            signup_at - Duration::days(30) - Duration::seconds(1),
        ),
        ("boundary", signup_at - Duration::days(30)),
        ("at-signup", signup_at),
        ("too-new", signup_at + Duration::seconds(1)),
    ] {
        let mut e = event(&p, "page_view");
        e.campaign = Some(campaign(name));
        e.occurred_at = time;
        e.visit_id = Some("boundary-visit".into());
        e.visitor_id = Some("boundary-visitor".into());
        touches.push(e);
    }
    browser(&s, &t, touches).await;
    let mut signup = event(&p, "signup");
    signup.subject = Some(subject());
    signup.visit_id = Some("boundary-visit".into());
    signup.visitor_id = Some("boundary-visitor".into());
    signup.occurred_at = signup_at;
    accepted(&s, signup.clone()).await;
    let campaign: Value =
        sqlx::query_scalar("SELECT campaign FROM ingest_acquisitions WHERE property_id=$1")
            .bind(p.id)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(campaign["utm_campaign"], "boundary");
    let last_campaign:Value=sqlx::query_scalar("SELECT t.campaign FROM ingest_touches t JOIN ingest_acquisitions a ON a.last_touch_id=t.touch_id WHERE a.property_id=$1").bind(p.id).fetch_one(&s.db).await.unwrap();
    assert_eq!(last_campaign["utm_campaign"], "at-signup");
    correct_acquisition(
        &s,
        &actor(),
        CorrectionRequest {
            property_id: p.id,
            subject: subject(),
            first_touch_id: None,
            reason: "synthetic invalidation".into(),
        },
    )
    .await
    .unwrap();
    let row =
        sqlx::query("SELECT campaign,rule_version FROM ingest_acquisitions WHERE property_id=$1")
            .bind(p.id)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert!(row.get::<Option<Value>, _>("campaign").is_none());
    assert_eq!(
        row.get::<String, _>("rule_version"),
        "audited-correction-v1"
    );
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ingest_audit WHERE action='acquisition_correction'",
    )
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(audit, 1);
    let mut effect = event(&p, "effect");
    effect.subject = Some(subject());
    effect.visit_id = signup.visit_id;
    effect.visitor_id = signup.visitor_id;
    accepted(&s, effect.clone()).await;
    assert!(
        issue_proof(
            &s,
            &actor(),
            IssueProofRequest {
                property_id: p.id,
                visit_id: effect.visit_id,
                visitor_id: effect.visitor_id,
                evidence_event_id: effect.event_id
            }
        )
        .await
        .is_err()
    );
    let id: Uuid = sqlx::query_scalar("SELECT id FROM ingest_browser_tokens WHERE property_id=$1")
        .bind(p.id)
        .fetch_one(&s.db)
        .await
        .unwrap();
    revoke_token(
        &s,
        &actor(),
        RevokeTokenRequest {
            property_id: p.id,
            token_id: id,
        },
    )
    .await
    .unwrap();
    stale.collection.policy_version = 1;
    assert_eq!(
        request(
            &s,
            "/v1/ingest/browser",
            &t,
            json!({"events":[stale]}),
            false
        )
        .await
        .0,
        401
    );
}

#[tokio::test]
async fn public_quota_cannot_consume_authoritative_delivery_budget() {
    let s = state().await;
    let (p, token) = configure(&s, CollectionProfile::Identified).await;
    browser(&s, &token, vec![event(&p, "page_view")]).await;
    sqlx::query("UPDATE ingest_quotas SET window_start=date_trunc('minute',now()),events=6000 WHERE property_id=$1 AND bucket='public'").bind(p.id).execute(&s.db).await.unwrap();
    assert_eq!(
        browser(&s, &token, vec![event(&p, "page_view")]).await["results"][0]["code"],
        "rate_limited"
    );
    accepted(&s, event(&p, "action")).await;
    let used: i32 = sqlx::query_scalar(
        "SELECT events FROM ingest_quotas WHERE property_id=$1 AND bucket='server'",
    )
    .bind(p.id)
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(used, 1);
}

#[tokio::test]
async fn journal_projects_bounded_queue_and_recovers_more_than_capacity() {
    let s = state().await;
    let (p, _) = configure(&s, CollectionProfile::Identified).await;
    sqlx::raw_sql(include_str!("../../../examples/producers/outbox.sql"))
        .execute(&s.db)
        .await
        .unwrap();
    let first = event(&p, "action");
    let player = Uuid::new_v4();
    let mut tx = s.db.begin().await.unwrap();
    for i in 0..10000 {
        let mut e = first.clone();
        if i > 0 {
            e.event_id = Uuid::new_v4();
        }
        producer::capture(&mut tx, &e, player).await.unwrap();
    }
    tx.commit().await.unwrap();
    while producer::project(&s.db).await.unwrap() > 0 {}
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM analytics_outbox")
            .fetch_one(&s.db)
            .await
            .unwrap(),
        10000
    );
    producer::dispatch(&s.db, "http://127.0.0.1:1", "synthetic-test-server")
        .await
        .unwrap();
    // New business facts commit despite a completely full queue and failed delivery.
    let mut tx = s.db.begin().await.unwrap();
    for _ in 0..25 {
        producer::capture(&mut tx, &event(&p, "action"), player)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM synthetic_commands")
            .fetch_one(&s.db)
            .await
            .unwrap(),
        10025
    );
    assert_eq!(producer::project(&s.db).await.unwrap(), 0);
    // Queue loss is recoverable from original retained facts without a cursor gap.
    sqlx::query("DELETE FROM analytics_outbox")
        .execute(&s.db)
        .await
        .unwrap();
    // Model an uncertain ACK: receiver already committed this event; journal has no ACK.
    accepted(&s, first).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let app = IngestionExtension.router().with_state(s.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut sent = 0;
    loop {
        let batch = producer::dispatch(&s.db, &endpoint, "synthetic-test-server")
            .await
            .unwrap();
        assert!(batch <= 50);
        let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM analytics_outbox")
            .fetch_one(&s.db)
            .await
            .unwrap();
        assert!(queued <= 10000);
        if batch == 0 {
            break;
        }
        sent += batch;
    }
    assert_eq!(sent, 10025);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ingest_events")
            .fetch_one(&s.db)
            .await
            .unwrap(),
        10025
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM synthetic_commands WHERE analytics_delivered_at IS NOT NULL"
        )
        .fetch_one(&s.db)
        .await
        .unwrap(),
        10025
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM analytics_outbox")
            .fetch_one(&s.db)
            .await
            .unwrap(),
        0
    );
    // Independent business-history retention explicitly reports unavailable facts.
    let mut tx = s.db.begin().await.unwrap();
    let lost = event(&p, "action");
    producer::capture(&mut tx, &lost, player).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query(
        "UPDATE synthetic_commands SET applied_at=now()-interval '2 days' WHERE command_id=$1",
    )
    .bind(lost.event_id)
    .execute(&s.db)
    .await
    .unwrap();
    producer::project(&s.db).await.unwrap();
    assert_eq!(
        producer::prune_journal(&s.db, Utc::now() - Duration::days(1))
            .await
            .unwrap(),
        (1, 1)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sum(event_count)::bigint FROM producer_coverage_gaps")
            .fetch_one(&s.db)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM analytics_outbox")
            .fetch_one(&s.db)
            .await
            .unwrap(),
        0
    );
    task.abort();
}

#[tokio::test]
async fn projection_preserves_late_commits_and_bounds_large_payload_batches() {
    let s = state().await;
    let (p, _) = configure(&s, CollectionProfile::Identified).await;
    sqlx::raw_sql(include_str!("../../../examples/producers/outbox.sql"))
        .execute(&s.db)
        .await
        .unwrap();
    let mut earlier = s.db.begin().await.unwrap();
    let early = event(&p, "action");
    producer::capture(&mut earlier, &early, Uuid::new_v4())
        .await
        .unwrap();
    let mut later = s.db.begin().await.unwrap();
    for _ in 0..6 {
        let mut e = event(&p, "action");
        e.properties
            .insert("synthetic_padding".into(), json!("x".repeat(50000)));
        producer::capture(&mut later, &e, Uuid::new_v4())
            .await
            .unwrap();
    }
    later.commit().await.unwrap();
    assert_eq!(producer::project(&s.db).await.unwrap(), 6);
    earlier.commit().await.unwrap();
    assert_eq!(producer::project(&s.db).await.unwrap(), 1);
    let mut tx = s.db.begin().await.unwrap();
    let rows = sqlx::query(include_str!("../../../examples/producers/claim.sql"))
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(rows.len() < 7);
    let total: usize = rows
        .iter()
        .map(|r| r.get::<Value, _>("payload").to_string().len() + 1)
        .sum();
    assert!(total <= 240000);
    tx.rollback().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM analytics_outbox WHERE event_id=$1")
            .bind(early.event_id)
            .fetch_one(&s.db)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn batched_writes_preserve_duplicate_conflict_order_and_mid_batch_signup() {
    let s = state().await;
    let (p, _) = configure(&s, CollectionProfile::Identified).await;
    let first = event(&p, "action");
    let duplicate = first.clone();
    let mut conflict = first.clone();
    conflict.context.control = Some("changed".into());
    let mut touch = event(&p, "action");
    touch.campaign = Some(campaign("ordered"));
    let mut signup = event(&p, "signup");
    signup.subject = Some(subject());
    signup.occurred_at = touch.occurred_at;
    let mut last = event(&p, "action");
    last.subject = Some(subject());
    last.touch_id = Some(touch.event_id);
    last.collection.dropped_events = 3;
    let mut rejected = last.clone();
    rejected.event_id = Uuid::new_v4();
    rejected.collection.policy_version = 99;
    let ack = ingest_server(
        &s,
        &actor(),
        IngestBatch {
            events: vec![
                first.clone(),
                duplicate,
                conflict,
                touch.clone(),
                signup.clone(),
                rejected,
                last.clone(),
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(
        ack.results
            .iter()
            .map(|a| a.status.as_str())
            .collect::<Vec<_>>(),
        vec![
            "accepted",
            "duplicate",
            "conflict",
            "accepted",
            "accepted",
            "rejected",
            "accepted"
        ]
    );
    assert_eq!(ack.results[0].receipt_cursor, ack.results[1].receipt_cursor);
    let cursors: Vec<i64> = [0usize, 3, 4, 6]
        .iter()
        .map(|i| {
            ack.results[*i]
                .receipt_cursor
                .as_ref()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    assert!(cursors.windows(2).all(|v| v[0] < v[1]));
    let ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT event_id FROM ingest_events ORDER BY receipt_cursor")
            .fetch_all(&s.db)
            .await
            .unwrap();
    assert_eq!(
        ids,
        vec![
            first.event_id,
            touch.event_id,
            signup.event_id,
            last.event_id
        ]
    );
    let campaign: Value =
        sqlx::query_scalar("SELECT campaign FROM ingest_acquisitions WHERE property_id=$1")
            .bind(p.id)
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(campaign["utm_campaign"], "ordered");
    let counters = status(&s, &actor(), p.id).await.unwrap();
    assert_eq!(counters["counters"]["accepted"], "4");
    assert_eq!(counters["counters"]["duplicate"], "1");
    assert_eq!(counters["counters"]["conflict"], "1");
    assert_eq!(counters["counters"]["policy_mismatch"], "1");
    assert_eq!(counters["counters"]["client_reported_dropped"], "3");
}

#[tokio::test]
async fn batch_quota_reservation_preserves_partial_capacity_without_regranting() {
    let s = state().await;
    let (p, t) = configure(&s, CollectionProfile::Minimal).await;
    sqlx::query("INSERT INTO ingest_quotas(property_id,bucket,window_start,events) VALUES($1,'public',date_trunc('minute',now()),5998)").bind(p.id).execute(&s.db).await.unwrap();
    let ack = browser(&s, &t, (0..5).map(|_| event(&p, "page_view")).collect()).await;
    assert_eq!(
        ack["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["status"] == "accepted")
            .count(),
        2
    );
    assert_eq!(
        ack["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["code"] == "rate_limited")
            .count(),
        3
    );
    let next = browser(&s, &t, (0..100).map(|_| event(&p, "page_view")).collect()).await;
    assert!(
        next["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["code"] == "rate_limited")
    );
    let counters = status(&s, &actor(), p.id).await.unwrap();
    assert_eq!(counters["counters"]["accepted"], "2");
    assert_eq!(counters["counters"]["rate_limited"], "103");
}

#[tokio::test]
async fn bulk_database_failure_never_acknowledges_partial_writes() {
    let s = state().await;
    let (p, _) = configure(&s, CollectionProfile::Minimal).await;
    sqlx::query("ALTER TABLE ingest_events ADD CONSTRAINT synthetic_write_failure CHECK(payload#>>'{context,control}' IS DISTINCT FROM 'synthetic-database-failure')").execute(&s.db).await.unwrap();
    let first = event(&p, "action");
    let mut second = event(&p, "action");
    second.context.control = Some("synthetic-database-failure".into());
    let batch = IngestBatch {
        events: vec![first, second],
    };
    assert!(ingest_server(&s, &actor(), batch.clone()).await.is_err());
    for table in [
        "ingest_events",
        "ingest_receipts",
        "ingest_counters",
        "ingest_quotas",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&s.db)
                .await
                .unwrap(),
            0
        );
    }
    sqlx::query("ALTER TABLE ingest_events DROP CONSTRAINT synthetic_write_failure")
        .execute(&s.db)
        .await
        .unwrap();
    let ack = ingest_server(&s, &actor(), batch).await.unwrap();
    assert!(
        ack.results
            .iter()
            .all(|r| r.status == "accepted" && r.receipt_cursor.is_some())
    );
}

struct PoolAuthorization {
    pool: PgPool,
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl Authorization for PoolAuthorization {
    async fn authenticate(&self, _: &str) -> ApiResult<AuthContext> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(actor())
    }
    async fn authorize(&self, _: &AuthContext, _: &str, _: &Resource) -> ApiResult<()> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(())
    }
    async fn grants(&self, _: &AuthContext) -> ApiResult<Vec<Grant>> {
        Ok(vec![])
    }
}
#[tokio::test]
async fn authorization_uses_unique_scopes_before_single_connection_transaction() {
    let s = state().await;
    let (p, _) = configure(&s, CollectionProfile::Minimal).await;
    let schema: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(&s.db)
        .await
        .unwrap();
    let url = std::env::var("INGEST_TEST_DATABASE_URL").unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(1)
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
        .await
        .unwrap();
    let auth = Arc::new(PoolAuthorization {
        pool: pool.clone(),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let state = AppState {
        db: pool,
        auth: auth.clone(),
        config: s.config.clone(),
    };
    let batch = IngestBatch {
        events: (0..20).map(|_| event(&p, "action")).collect(),
    };
    let ack = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        ingest_server(&state, &actor(), batch),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(ack.results.iter().all(|r| r.status == "accepted"));
    assert_eq!(auth.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn same_origin_config_post_uses_query_token_and_origin_without_writes() {
    let s = state().await;
    let (mut property, token) = configure(&s, CollectionProfile::Identified).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    property.allowed_origins = vec![origin.clone()];
    save(&s, &property).await;
    let app = IngestionExtension.router().with_state(s.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let before: Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(a) ORDER BY kind,id) FROM admin_objects a")
            .fetch_one(&s.db)
            .await
            .unwrap();
    for path in ["/v1/ingest/config", "/v1/ingest/policy"] {
        let url = format!("{origin}{path}?property_id={}", property.id);
        // Same-origin GET can omit Origin; the trust boundary must stay strict.
        assert_eq!(
            client
                .get(&url)
                .header("X-Property-Token", &token)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .post(&url)
                .header("X-Property-Token", &token)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .post(&url)
                .header("X-Property-Token", &token)
                .header("Origin", "null")
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .post(&url)
                .header("X-Property-Token", &token)
                .header("Origin", "https://unapproved.example")
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .post(&url)
                .header("X-Property-Token", "invalid-public-token")
                .header("Origin", &origin)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        let post = client
            .post(&url)
            .header("X-Property-Token", &token)
            .header("Origin", &origin)
            .send()
            .await
            .unwrap();
        assert_eq!(post.status(), 200);
        let config: Value = post.json().await.unwrap();
        assert_eq!(config["property_id"], property.id.to_string());
        assert!(
            config["schemas"]
                .as_array()
                .unwrap()
                .iter()
                .all(|schema| schema["authoritative"] == false)
        );
        let get = client
            .get(&url)
            .header("X-Property-Token", &token)
            .header("Origin", &origin)
            .send()
            .await
            .unwrap();
        assert_eq!(get.status(), 200);
        assert_eq!(get.json::<Value>().await.unwrap(), config);
        let missing_query = client
            .post(format!("{origin}{path}"))
            .header("X-Property-Token", &token)
            .header("Origin", &origin)
            .send()
            .await
            .unwrap();
        assert_eq!(missing_query.status(), 400);
    }
    let after: Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(a) ORDER BY kind,id) FROM admin_objects a")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert_eq!(before, after);
    for table in [
        "ingest_events",
        "ingest_receipts",
        "ingest_counters",
        "ingest_quotas",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&s.db)
                .await
                .unwrap(),
            0
        );
    }
    server.abort();
}
