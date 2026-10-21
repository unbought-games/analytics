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
    let row = sqlx::query("SELECT delivered_at,last_error_code FROM analytics_outbox")
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
        sqlx::query_scalar("SELECT delivered_at IS NOT NULL FROM analytics_outbox")
            .fetch_one(&s.db)
            .await
            .unwrap();
    assert!(delivered);
    // Simulate crash after remote commit but before local delivery marking.
    sqlx::query(
        "UPDATE analytics_outbox SET delivered_at=NULL,available_at=now(),lease_until=NULL",
    )
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
