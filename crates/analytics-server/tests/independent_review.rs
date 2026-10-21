//! Targeted composed security regressions. Use a disposable PostgreSQL database.
//! Run explicitly with REVIEW_DATABASE_URL and --ignored; no production data.
use analytics_core::*;
use analytics_query::QueryExtension;
use analytics_server::{MIGRATOR, auth::DatabaseAuthorization, bootstrap};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, Uri},
    routing::get,
};
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

type Captures = Arc<Mutex<Vec<(String, String)>>>;
async fn vendor(State(captures): State<Captures>, uri: Uri, headers: HeaderMap) -> Json<Value> {
    captures.lock().unwrap().push((
        uri.path().to_owned(),
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    ));
    Json(
        json!({"status":"success","data":{"resultType":"vector","result":[{"metric":{"fixture":"synthetic"},"value":[1,"2"]}]}}),
    )
}

async fn admin(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    kind: &str,
    operation: &str,
    id: Uuid,
    value: Option<Value>,
) -> reqwest::Response {
    client
        .post(format!("{base}/v1/admin/execute"))
        .bearer_auth(token)
        .json(&json!({"resource_kind":kind,"operation":operation,"resource_id":id,"value":value}))
        .send()
        .await
        .unwrap()
}

async fn completed(client: &reqwest::Client, base: &str, token: &str, id: Uuid) -> Value {
    for _ in 0..100 {
        let response = client
            .get(format!("{base}/v1/exports/{id}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert!(
            response.status().is_success(),
            "status HTTP {}",
            response.status()
        );
        let value: Value = response.json().await.unwrap();
        if !matches!(value["status"].as_str(), Some("queued" | "running")) {
            return value;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("export did not terminate");
}

async fn create(client: &reqwest::Client, base: &str, token: &str, query: Value) -> Uuid {
    let response = client
        .post(format!("{base}/v1/exports"))
        .bearer_auth(token)
        .json(&json!({"query":query,"format":"jsonl","retention_hours":1}))
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "create HTTP {}",
        response.status()
    );
    response.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn query(source: Uuid) -> Value {
    json!({"source_id":source,"property_id":null,"kind":"prometheus_instant","query":{"expression":"synthetic_metric"},"parameters":{},"window":null,"cutoff":Utc::now(),"definition_version":"review/1","budget":{"timeout_ms":5000,"max_rows":100,"max_bytes":100000,"max_concurrency":1}})
}

#[tokio::test]
#[ignore = "requires explicitly supplied disposable REVIEW_DATABASE_URL"]
async fn composed_source_secret_isolation_and_definition_lifecycle() {
    let url = std::env::var("REVIEW_DATABASE_URL").expect("explicit disposable database required");
    let root = sqlx::PgPool::connect(&url).await.unwrap();
    let database = format!("independent_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {database}"))
        .execute(&root)
        .await
        .unwrap();
    let mut parsed = url::Url::parse(&url).unwrap();
    parsed.set_path(&format!("/{database}"));
    let db = sqlx::PgPool::connect(parsed.as_str()).await.unwrap();
    MIGRATOR.run(&db).await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let handoff = directory.path().join("bootstrap.json");
    bootstrap::bootstrap(&db, "Synthetic owner", "Synthetic agent", &handoff, 1)
        .await
        .unwrap();
    let bootstrap: Value = serde_json::from_slice(&std::fs::read(handoff).unwrap()).unwrap();
    let owner = bootstrap["credential"]["token"].as_str().unwrap();
    let secrets = directory.path().join("secrets");
    std::fs::create_dir(&secrets).unwrap();
    let canary_a = format!("synthetic-a-{}", Uuid::new_v4());
    let canary_b = format!("synthetic-b-{}", Uuid::new_v4());
    for (name, token) in [("source-a", &canary_a), ("source-b", &canary_b)] {
        let path = secrets.join(name);
        std::fs::write(&path, token).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let captures: Captures = Arc::new(Mutex::new(vec![]));
    let fixture_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture_base = format!("http://{}", fixture_listener.local_addr().unwrap());
    let fixture = Router::new()
        .route("/{prefix}/api/v1/query", get(vendor))
        .with_state(captures.clone());
    let fixture_task = tokio::spawn(axum::serve(fixture_listener, fixture).into_future());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let state = AppState {
        db: db.clone(),
        auth: Arc::new(DatabaseAuthorization {
            db: db.clone(),
            oidc: None,
        }),
        config: Arc::new(ServiceConfig {
            public_base_url: base.clone(),
            export_directory: directory.path().join("exports"),
            secret_directory: secrets,
            max_body_bytes: 1024 * 1024,
        }),
    };
    let app = analytics_server::router(
        state.clone(),
        vec![
            Arc::new(QueryExtension),
            Arc::new(analytics_ingest::IngestionExtension),
        ],
        None,
    );
    let server = tokio::spawn(axum::serve(listener, app).into_future());
    let client = reqwest::Client::new();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let c = Uuid::new_v4();
    let source = |id, prefix: &str, secret: &str| json!({"id":id,"name":"Synthetic source","kind":"prometheus","environment":"test","endpoint":format!("{fixture_base}/{prefix}/"),"secret_ref":secret,"allowed_schemas":[],"allowed_tables":[],"allowed_columns":{},"allowed_projects":[],"budget":{"timeout_ms":5000,"max_rows":100,"max_bytes":100000,"max_concurrency":1},"semantics":{},"enabled":true});
    let definition_a = source(a, "safe", "source-a");
    for (id, definition) in [
        (a, definition_a.clone()),
        (b, source(b, "protected", "source-b")),
    ] {
        assert!(
            admin(
                &client,
                &base,
                owner,
                "sources",
                "put",
                id,
                Some(definition)
            )
            .await
            .status()
            .is_success()
        );
    }
    let manager = Uuid::new_v4();
    assert!(admin(&client,&base,owner,"principals","put",manager,Some(json!({"id":manager,"name":"Scoped synthetic manager","kind":"agent","enabled":true,"delegated_by":null}))).await.status().is_success());
    let mut grants: Vec<Value> = [
        "sources:manage",
        "sources:read",
        "query:read",
        "exports:create",
        "exports:read",
        "exports:delete",
        "definitions:manage",
        "definitions:read",
    ]
    .iter()
    .map(|action| json!({"action":action,"resource":{"kind":"source","id":a}}))
    .collect();
    grants.push(json!({"action":"sources:manage","resource":{"kind":"source","id":c}}));
    assert!(
        admin(
            &client,
            &base,
            owner,
            "grants",
            "put",
            manager,
            Some(json!(grants))
        )
        .await
        .status()
        .is_success()
    );
    let credential = admin(&client,&base,owner,"credentials","issue",manager,Some(json!({"principal_id":manager,"name":"Synthetic scoped credential","expires_at":Utc::now()+Duration::hours(1),"grants":grants}))).await;
    assert!(credential.status().is_success());
    let credential: Value = credential.json().await.unwrap();
    let token = credential["token"].as_str().unwrap();
    for (field, value) in [
        ("secret_ref", json!("source-b")),
        ("endpoint", json!(format!("{fixture_base}/trap/"))),
        ("kind", json!("sentry")),
    ] {
        let mut changed = definition_a.clone();
        changed[field] = value;
        assert_eq!(
            admin(&client, &base, token, "sources", "put", a, Some(changed))
                .await
                .status(),
            403,
            "scoped manager changed {field}"
        );
    }
    assert_eq!(
        admin(
            &client,
            &base,
            token,
            "sources",
            "put",
            c,
            Some(source(c, "trap", "source-b"))
        )
        .await
        .status(),
        403
    );
    let worker = analytics_query::start_worker(state.clone());
    let export = create(&client, &base, token, query(a)).await;
    assert_eq!(
        completed(&client, &base, token, export).await["status"],
        "complete"
    );
    let manifest_url = format!("{base}/v1/exports/{export}/manifest");
    let manifest: Value = client
        .get(&manifest_url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let response = client
        .get(manifest["datasets"][0]["download_url"].as_str().unwrap())
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert!(!response.bytes().await.unwrap().is_empty());
    {
        let seen = captures.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "/safe/api/v1/query");
        assert!(
            seen[0].1 == format!("Bearer {canary_a}"),
            "wrong synthetic source credential used"
        );
        assert!(
            seen.iter().all(|(_, header)| !header.contains(&canary_b)),
            "other-source canary escaped"
        );
    }
    // Administration and query reproduction must share current authority/history.
    let saved = Uuid::new_v4();
    assert!(admin(&client,&base,token,"definitions","put",saved,Some(json!({"id":saved,"name":"Synthetic definition","version":"1","description":"review","query":query(a)}))).await.status().is_success());
    let reproduce = || {
        client
            .post(format!("{base}/v1/query/reproduce"))
            .bearer_auth(token)
            .json(&json!({"id":saved,"version":"1","cutoff":Utc::now(),"retention_hours":1}))
    };
    let response = reproduce().send().await.unwrap();
    assert!(response.status().is_success());
    let reproduced: Value = response.json().await.unwrap();
    let reproduced: Uuid = reproduced["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        completed(&client, &base, token, reproduced).await["status"],
        "complete"
    );
    assert!(
        admin(&client, &base, token, "definitions", "delete", saved, None)
            .await
            .status()
            .is_success()
    );
    assert!(!reproduce().send().await.unwrap().status().is_success());
    // A deleted source must not poison the worker queue for unrelated sources.
    worker.abort();
    let orphan = create(&client, &base, token, query(a)).await;
    assert!(
        admin(&client, &base, owner, "sources", "delete", a, None)
            .await
            .status()
            .is_success()
    );
    assert!(
        !client
            .get(&manifest_url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    let unrelated = create(&client, &base, owner, query(b)).await;
    let worker = analytics_query::start_worker(state.clone());
    assert_eq!(
        completed(&client, &base, owner, unrelated).await["status"],
        "complete"
    );
    let orphan_status: String = sqlx::query_scalar("SELECT status FROM export_jobs WHERE id=$1")
        .bind(orphan)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(orphan_status, "failed");
    // Recreating a source must never revive a previously issued artifact.
    assert!(
        admin(
            &client,
            &base,
            owner,
            "sources",
            "put",
            a,
            Some(definition_a)
        )
        .await
        .status()
        .is_success()
    );
    let revived = client
        .get(&manifest_url)
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .status()
        .is_success();
    worker.abort();
    server.abort();
    fixture_task.abort();
    db.close().await;
    sqlx::query(&format!("DROP DATABASE {database} WITH (FORCE)"))
        .execute(&root)
        .await
        .unwrap();
    assert!(
        !revived,
        "source recreation revived an old artifact (configuration generation reused)"
    );
}
