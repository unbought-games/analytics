use analytics_query::{http_sources::*, postgres::PostgresSource, source::*};
use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio_util::sync::CancellationToken;
#[derive(Default)]
struct Sink {
    rows: Vec<Value>,
    count: u64,
    collect: bool,
}
#[async_trait]
impl RowSink for Sink {
    async fn row(&mut self, row: Value) -> Result<(), SourceError> {
        self.count += 1;
        if self.collect {
            self.rows.push(row);
        }
        Ok(())
    }
}
fn scope() -> BTreeMap<String, BTreeSet<String>> {
    BTreeMap::from([
        (
            "reporting.activity".into(),
            ["id", "amount", "occurred_at", "payload"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        ),
        (
            "reporting.large".into(),
            ["id", "value"].into_iter().map(str::to_owned).collect(),
        ),
    ])
}
#[tokio::test]
async fn postgres_restricted_streaming_lossless_and_cancellation() {
    let url = std::env::var("QUERY_TEST_DATABASE_URL")
        .expect("QUERY_TEST_DATABASE_URL must reference a disposable PostgreSQL database");
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let dbname = format!("source_{suffix}");
    let role = format!("reader_{suffix}");
    sqlx::query(&format!("CREATE DATABASE {dbname}"))
        .execute(&admin)
        .await
        .unwrap();
    let mut parsed = reqwest::Url::parse(&url).unwrap();
    parsed.set_path(&format!("/{dbname}"));
    let url = parsed.to_string();
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    sqlx::raw_sql(
        &include_str!("../../../tests/fixtures/sources/postgres.sql")
            .replace("fixture_reader", &role),
    )
    .execute(&pool)
    .await
    .unwrap();
    let password = uuid::Uuid::new_v4().simple().to_string();
    sqlx::query(&format!("ALTER ROLE {role} PASSWORD '{password}'"))
        .execute(&pool)
        .await
        .unwrap();
    let mut parsed = reqwest::Url::parse(&url).unwrap();
    parsed.set_username(&role).unwrap();
    parsed.set_password(Some(&password)).unwrap();
    let restricted = parsed.to_string();
    let source = PostgresSource::new(restricted.clone(), scope());
    let discovered = source.discover().await.unwrap();
    assert!(!discovered.to_string().contains("hidden"));
    let mut sink = Sink {
        collect: true,
        ..Default::default()
    };
    source
        .run(
            "SELECT id,amount,occurred_at,payload FROM reporting.activity ORDER BY id",
            &BTreeMap::new(),
            &Budget::default(),
            &mut sink,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(sink.rows[0]["id"], "9007199254740993");
    assert_eq!(
        sink.rows[0]["amount"],
        "123456789012345678901234567890.1234567890"
    );
    assert_eq!(sink.rows[0]["payload"]["nested"], "9007199254740993");
    assert!(sink.rows[1]["amount"].is_null());
    for query in [
        "SELECT 1 AS x, 2 AS x ORDER BY 1",
        "SELECT hidden FROM reporting.activity ORDER BY id",
        "SELECT * FROM reporting.private ORDER BY id",
        "SELECT reporting.unsafe() ORDER BY 1",
        "WITH pg_roles AS (SELECT * FROM pg_roles) SELECT * FROM pg_roles ORDER BY 1",
        "SELECT (WITH pg_roles AS (SELECT 1) SELECT 1) FROM pg_roles ORDER BY 1",
    ] {
        assert!(
            source
                .run(
                    query,
                    &BTreeMap::new(),
                    &Budget::default(),
                    &mut Sink::default(),
                    &CancellationToken::new()
                )
                .await
                .is_err(),
            "{query}"
        );
    }
    let direct = sqlx::PgPool::connect(&restricted).await.unwrap();
    assert!(
        sqlx::query("UPDATE reporting.activity SET amount=0")
            .execute(&direct)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("SELECT hidden FROM reporting.activity")
            .fetch_all(&direct)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("SELECT reporting.unsafe()")
            .fetch_one(&direct)
            .await
            .is_err()
    );
    let mut count = Sink::default();
    let started = Instant::now();
    source
        .run(
            "SELECT id,value FROM reporting.large ORDER BY id",
            &BTreeMap::new(),
            &Budget {
                max_rows: 250000,
                ..Default::default()
            },
            &mut count,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(count.count, 200000);
    eprintln!(
        "streamed {} rows in {:?} without collecting",
        count.count,
        started.elapsed()
    );
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        trigger.cancel();
    });
    let start = Instant::now();
    let error = source
        .run(
            "SELECT sum(a.id*b.id) FROM reporting.large a CROSS JOIN reporting.large b ORDER BY 1",
            &BTreeMap::new(),
            &Budget::default(),
            &mut Sink::default(),
            &cancel,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SourceError::Cancelled));
    assert!(start.elapsed().as_secs() < 2);
    let error = source
        .run(
            "SELECT sum(a.id*b.id) FROM reporting.large a CROSS JOIN reporting.large b ORDER BY 1",
            &BTreeMap::new(),
            &Budget {
                timeout_ms: 50,
                ..Default::default()
            },
            &mut Sink::default(),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SourceError::Timeout | SourceError::Unavailable
    ));
    let mut selected = Sink {
        collect: true,
        ..Default::default()
    };
    source
        .run(
            "SELECT id FROM reporting.activity WHERE id=CAST($1 AS bigint) ORDER BY id",
            &BTreeMap::from([("p1".into(), json!("9007199254740993"))]),
            &Budget::default(),
            &mut selected,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(selected.rows.len(), 1);
    let mut overly_broad = scope();
    overly_broad
        .get_mut("reporting.activity")
        .unwrap()
        .remove("amount");
    assert!(
        PostgresSource::new(restricted, overly_broad)
            .discover()
            .await
            .is_err()
    );
    for _ in 0..30 {
        let running:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE usename=$1 AND application_name='analytics_source' AND state='active'").bind(&role).fetch_one(&admin).await.unwrap();
        if running == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let running:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE usename=$1 AND application_name='analytics_source' AND state='active'").bind(&role).fetch_one(&admin).await.unwrap();
    assert_eq!(running, 0, "cancelled source backend still active");
    direct.close().await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {dbname} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE {role}"))
        .execute(&admin)
        .await
        .unwrap();
}
type FixtureRequest = (String, BTreeMap<String, String>);
#[derive(Clone, Default)]
struct Fixture {
    requests: Arc<Mutex<Vec<FixtureRequest>>>,
}
async fn handler(
    State(f): State<Fixture>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    Query(params): Query<BTreeMap<String, String>>,
) -> Response {
    assert_eq!(
        headers.get("authorization").unwrap(),
        "Bearer synthetic-token"
    );
    f.requests
        .lock()
        .unwrap()
        .push((uri.path().into(), params.clone()));
    if params.get("query").map(String::as_str) == Some("rate_limited") {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("retry-after", "3")],
            "sensitive body",
        )
            .into_response();
    }
    if params.get("query").map(String::as_str) == Some("unavailable") {
        return (StatusCode::BAD_GATEWAY, "sensitive body").into_response();
    }
    if uri.path().contains("api/v1/query_range") {
        return Json(json!({"status":"success","data":{"resultType":"matrix","result":[{"metric":{"__name__":"requests_total","release":"synthetic-r1"},"values":[[1767225600,"NaN"],[1767225660,"+Inf"]]}]},"warnings":["fixture synthetic-token warning"]})).into_response();
    }
    if uri.path().contains("api/v1/query") {
        return Json(json!({"status":"success","data":{"resultType":"vector","result":[]}}))
            .into_response();
    }
    if uri.path().contains("api/v1/") {
        return Json(json!({"status":"success","data":["requests_total"]})).into_response();
    }
    if uri
        .path()
        .contains("/events/0123456789abcdef0123456789abcdef/")
    {
        return Json(json!({"id":"0123456789abcdef0123456789abcdef","projectID":101,"release":{"version":"synthetic-r1"},"user":{"email":"redacted@example.invalid"},"request":{"headers":["secret"]},"tags":[{"key":"email","value":"private"},{"key":"request_id","value":"request-1"}]})).into_response();
    }
    let body = json!([{"id":if params.contains_key("cursor"){ "issue2" }else{"issue1"},"count":"9007199254740993","project":{"id":"101","slug":"manufactured"},"user":{"email":"private@example.invalid"},"title":"secret context"}]);
    if !params.contains_key("cursor") {
        return ([("link","<https://hostile.invalid/steal?project=-1&cursor=next-page>; rel=\"next\"; results=\"true\"")],Json(body)).into_response();
    }
    Json(body).into_response()
}
#[tokio::test]
async fn vendor_fixtures_scope_pagination_redaction_missing_rate_limits() {
    let f = Fixture::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(
        axum::serve(
            listener,
            Router::new().fallback(get(handler)).with_state(f.clone()),
        )
        .into_future(),
    );
    let source = HttpSource::new(&endpoint, Some("synthetic-token".into())).unwrap();
    let b = Budget::default();
    let c = CancellationToken::new();
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let end = start + chrono::Duration::hours(1);
    let mut sink = Sink {
        collect: true,
        ..Default::default()
    };
    let summary = source
        .prometheus(
            &PrometheusQuery::Range {
                expression: "requests_total".into(),
                start,
                end,
                step_seconds: 60,
            },
            &b,
            &mut sink,
            &c,
        )
        .await
        .unwrap();
    assert_eq!(summary.warnings, vec!["fixture [redacted] warning"]);
    assert_eq!(sink.rows[0]["series"]["values"][0][1], "NaN");
    assert_eq!(sink.rows[0]["series"]["values"][0][0], "1767225600");
    let mut empty = Sink::default();
    source
        .prometheus(
            &PrometheusQuery::Instant {
                expression: "missing_metric".into(),
                time: end,
            },
            &b,
            &mut empty,
            &c,
        )
        .await
        .unwrap();
    assert_eq!(empty.count, 0);
    source
        .prometheus(&PrometheusQuery::Metrics { start, end }, &b, &mut empty, &c)
        .await
        .unwrap();
    assert_eq!(empty.count, 1);
    assert!(matches!(
        source
            .prometheus(
                &PrometheusQuery::Instant {
                    expression: "rate_limited".into(),
                    time: end
                },
                &b,
                &mut empty,
                &c
            )
            .await
            .unwrap_err(),
        SourceError::RateLimited {
            retry_after: Some(3)
        }
    ));
    assert!(matches!(
        source
            .prometheus(
                &PrometheusQuery::Instant {
                    expression: "unavailable".into(),
                    time: end
                },
                &b,
                &mut empty,
                &c
            )
            .await
            .unwrap_err(),
        SourceError::Unavailable
    ));
    let scope = SentryScope {
        organization: "synthetic".into(),
        projects: BTreeSet::from(["101".into()]),
    };
    let query = SentryQuery::Issues {
        project: "101".into(),
        start,
        end,
        query: "".into(),
        cursor: None,
    };
    let mut issues = Sink {
        collect: true,
        ..Default::default()
    };
    source
        .sentry(&scope, &query, &b, &mut issues, &c)
        .await
        .unwrap();
    assert_eq!(issues.rows.len(), 2);
    assert!(
        !serde_json::to_string(&issues.rows)
            .unwrap()
            .contains("secret")
    );
    assert!(
        !serde_json::to_string(&issues.rows)
            .unwrap()
            .contains("email")
    );
    let partial = source
        .sentry(
            &scope,
            &query,
            &Budget {
                max_pages: 1,
                ..b.clone()
            },
            &mut Sink::default(),
            &c,
        )
        .await
        .unwrap();
    assert_eq!(partial.continuation.unwrap()["cursor"], "next-page");
    let bad = SentryQuery::Issues {
        project: "-1".into(),
        start,
        end,
        query: "".into(),
        cursor: None,
    };
    assert!(matches!(
        source
            .sentry(&scope, &bad, &b, &mut Sink::default(), &c)
            .await
            .unwrap_err(),
        SourceError::Denied
    ));
    let mut detail = Sink {
        collect: true,
        ..Default::default()
    };
    source
        .sentry(
            &scope,
            &SentryQuery::Detail {
                project: "101".into(),
                issue: "issue1".into(),
                event: "0123456789abcdef0123456789abcdef".into(),
            },
            &b,
            &mut detail,
            &c,
        )
        .await
        .unwrap();
    assert_eq!(detail.rows[0]["tags"].as_array().unwrap().len(), 1);
    assert!(detail.rows[0].get("request").is_none());
    assert!(f.requests.lock().unwrap().iter().all(
        |(path, params)| !path.contains("steal") && params.get("project") != Some(&"-1".into())
    ));
    let slug_scope = SentryScope {
        organization: "synthetic".into(),
        projects: BTreeSet::from(["manufactured".into()]),
    };
    source
        .sentry(
            &slug_scope,
            &SentryQuery::Detail {
                project: "manufactured".into(),
                issue: "issue1".into(),
                event: "0123456789abcdef0123456789abcdef".into(),
            },
            &b,
            &mut Sink::default(),
            &c,
        )
        .await
        .unwrap();
    server.abort();
}
