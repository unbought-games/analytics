use analytics_core::*;
use analytics_query::{QueryExtension, start_worker};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use uuid::Uuid;

// Deliberately controlled authorization fixture. Production Authorization lives
// in analytics-server and is exercised by the composed acceptance suite.
struct FixtureAuth {
    actor: AuthContext,
    revoked: AtomicBool,
    source: Uuid,
}
#[async_trait]
impl Authorization for FixtureAuth {
    async fn authenticate(&self, bearer: &str) -> ApiResult<AuthContext> {
        if bearer != "fixture-query-token" || self.revoked.load(Ordering::SeqCst) {
            return Err(ApiError::unauthorized());
        }
        Ok(self.actor.clone())
    }
    async fn authorize(&self, a: &AuthContext, _: &str, r: &Resource) -> ApiResult<()> {
        if self.revoked.load(Ordering::SeqCst)
            || a.principal_id != self.actor.principal_id
            || r.kind != ResourceKind::Source
            || r.id != self.source.to_string()
        {
            Err(ApiError::forbidden())
        } else {
            Ok(())
        }
    }
    async fn grants(&self, _: &AuthContext) -> ApiResult<Vec<Grant>> {
        Ok(vec![])
    }
}
async fn until_done(client: &reqwest::Client, base: &str, id: Uuid) -> ExportStatus {
    for _ in 0..600 {
        let response = client
            .get(format!("{base}/v1/exports/{id}"))
            .bearer_auth("fixture-query-token")
            .send()
            .await
            .unwrap();
        let status: ExportStatus = response.json().await.unwrap();
        if !matches!(status.status.as_str(), "queued" | "running") {
            return status;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("export did not terminate")
}
#[tokio::test]
async fn durable_remote_export_reconnect_scope_partial_failure_cancel_expiry_revocation() {
    let url = std::env::var("QUERY_TEST_DATABASE_URL")
        .expect("QUERY_TEST_DATABASE_URL must reference a disposable PostgreSQL database");
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let dbname = format!("exports_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {dbname}"))
        .execute(&admin)
        .await
        .unwrap();
    let mut parsed = reqwest::Url::parse(&url).unwrap();
    parsed.set_path(&format!("/{dbname}"));
    let dburl = parsed.to_string();
    let db = sqlx::PgPool::connect(&dburl).await.unwrap();
    sqlx::raw_sql(include_str!("../../../migrations/0001_foundation.sql"))
        .execute(&db)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../../../migrations/0003_exports.sql"))
        .execute(&db)
        .await
        .unwrap();
    let role = format!("reader_{}", Uuid::new_v4().simple());
    sqlx::raw_sql(
        &include_str!("../../../tests/fixtures/sources/postgres.sql")
            .replace("fixture_reader", &role),
    )
    .execute(&db)
    .await
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let secrets = directory.path().join("secrets");
    tokio::fs::create_dir(&secrets).await.unwrap();
    let source = Uuid::new_v4();
    let principal = Uuid::new_v4();
    let credential = Uuid::new_v4();
    sqlx::query("INSERT INTO principals(id,name,kind) VALUES($1,'fixture analyst','agent')")
        .bind(principal)
        .execute(&db)
        .await
        .unwrap();
    let password = Uuid::new_v4().simple().to_string();
    sqlx::query(&format!("ALTER ROLE {role} PASSWORD '{password}'"))
        .execute(&db)
        .await
        .unwrap();
    let mut parsed = reqwest::Url::parse(&dburl).unwrap();
    parsed.set_username(&role).unwrap();
    parsed.set_password(Some(&password)).unwrap();
    let source_dsn = parsed.to_string();
    let secret = secrets.join("fixture-postgres");
    tokio::fs::write(&secret, source_dsn).await.unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600))
            .await
            .unwrap();
    }
    let definition = SourceDefinition {
        id: source,
        name: "Manufactured".into(),
        kind: "postgres".into(),
        environment: "test".into(),
        endpoint: "postgresql://fixture.invalid/synthetic".into(),
        secret_ref: Some("fixture-postgres".into()),
        allowed_schemas: vec!["reporting".into()],
        allowed_tables: vec!["reporting.activity".into(), "reporting.large".into()],
        allowed_columns: BTreeMap::from([
            (
                "reporting.activity".into(),
                vec![
                    "id".into(),
                    "amount".into(),
                    "occurred_at".into(),
                    "payload".into(),
                ],
            ),
            ("reporting.large".into(), vec!["id".into(), "value".into()]),
        ]),
        allowed_projects: vec![],
        budget: QueryBudget {
            timeout_ms: 10000,
            max_rows: 250000,
            max_bytes: 64 * 1024 * 1024,
            max_concurrency: 2,
        },
        semantics: json!({"coverage":{"complete":true,"history_start":"2026-01-01T00:00:00Z","history_end":"2026-07-01T00:00:00Z","sample_rate":1,"policy_versions":[],"limitations":[]}}),
        enabled: true,
    };
    sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES('sources',$1,'source',$1,$2)").bind(source.to_string()).bind(serde_json::to_value(&definition).unwrap()).execute(&db).await.unwrap();
    let auth = Arc::new(FixtureAuth {
        actor: AuthContext {
            principal_id: principal,
            credential_id: Some(credential),
            delegated_by: None,
            expires_at: Some(Utc::now() + Duration::hours(1)),
        },
        revoked: AtomicBool::new(false),
        source,
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let state = AppState {
        db: db.clone(),
        auth: auth.clone(),
        config: Arc::new(ServiceConfig {
            public_base_url: base.clone(),
            export_directory: directory.path().join("exports"),
            secret_directory: secrets,
            max_body_bytes: 1024 * 1024,
        }),
    };
    let server = tokio::spawn(
        axum::serve(listener, QueryExtension.router().with_state(state.clone())).into_future(),
    );
    let request = ExportRequest {
        query: QueryRequest {
            source_id: Some(source),
            property_id: None,
            kind: "postgres".into(),
            query: json!({"sql":"SELECT id,amount,payload FROM reporting.activity ORDER BY id","dataset":"activity"}),
            parameters: BTreeMap::new(),
            window: None,
            cutoff: Utc::now(),
            definition_version: "fixture/1".into(),
            budget: QueryBudget {
                timeout_ms: 10000,
                max_rows: 10000,
                max_bytes: 1024 * 1024,
                max_concurrency: 2,
            },
        },
        format: "jsonl".into(),
        retention_hours: 1,
    };
    let client = reqwest::Client::new();
    let create = |r: ExportRequest| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let response = client
                .post(format!("{base}/v1/exports"))
                .bearer_auth("fixture-query-token")
                .json(&r)
                .send()
                .await
                .unwrap();
            assert!(
                response.status().is_success(),
                "{}",
                response.text().await.unwrap()
            );
            response.json::<ExportStatus>().await.unwrap()
        }
    };
    let queued = create(request.clone()).await;
    assert_eq!(queued.status, "queued");
    // Job is durable before a worker exists; starting/reconnecting is independent.
    let worker = start_worker(state.clone());
    let status = until_done(&client, &base, queued.id).await;
    assert_eq!(status.status, "complete", "{:?}", status);
    let m: ExportManifest = client
        .get(status.manifest_url.unwrap())
        .bearer_auth("fixture-query-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(m.datasets[0].row_count, 2);
    assert!(m.coverage.complete);
    let reconnect = reqwest::Client::new();
    let data = reconnect
        .get(&m.datasets[0].download_url)
        .bearer_auth("fixture-query-token")
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(m.datasets[0].sha256, format!("{:x}", Sha256::digest(&data)));
    assert_eq!(m.datasets[0].bytes, data.len() as u64);
    assert!(String::from_utf8_lossy(&data).contains("9007199254740993"));
    assert_eq!(
        client
            .get(&m.datasets[0].download_url)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let mut partial = request.clone();
    partial.query.budget.max_rows = 1;
    let job = create(partial).await;
    assert_eq!(until_done(&client, &base, job.id).await.status, "partial");
    let mut failed = request.clone();
    failed.query.query = json!({"sql":"SELECT hidden FROM reporting.activity ORDER BY 1"});
    let job = create(failed).await;
    let failed = until_done(&client, &base, job.id).await;
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.error.as_deref(), Some("source_unavailable"));
    let mut forbidden = request.clone();
    forbidden.query.source_id = Some(Uuid::new_v4());
    assert_eq!(
        client
            .post(format!("{base}/v1/exports"))
            .bearer_auth("fixture-query-token")
            .json(&forbidden)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut large = request.clone();
    large.query.query = json!({"sql":"SELECT id,value FROM reporting.large ORDER BY id"});
    large.query.budget.max_rows = 250000;
    large.query.budget.max_bytes = 64 * 1024 * 1024;
    let large = create(large).await;
    let completed = until_done(&client, &base, large.id).await;
    assert_eq!(completed.status, "complete");
    let manifest: ExportManifest = client
        .get(completed.manifest_url.unwrap())
        .bearer_auth("fixture-query-token")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(manifest.datasets[0].row_count, 200000);
    let mut stream = client
        .get(&manifest.datasets[0].download_url)
        .bearer_auth("fixture-query-token")
        .send()
        .await
        .unwrap()
        .bytes_stream();
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut lines = 0usize;
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        bytes += chunk.len() as u64;
        lines += chunk.iter().filter(|b| **b == b'\n').count();
        digest.update(&chunk);
    }
    assert_eq!(lines, 200000);
    assert_eq!(bytes, manifest.datasets[0].bytes);
    assert_eq!(
        format!("{:x}", digest.finalize()),
        manifest.datasets[0].sha256
    );
    let definition_id = Uuid::new_v4();
    let saved = SavedDefinition {
        id: definition_id,
        name: "Reproducible fixture".into(),
        version: "1".into(),
        query: request.query.clone(),
        description: "synthetic".into(),
    };
    QueryExtension
        .call(
            &state,
            &auth.actor,
            "query_save",
            serde_json::to_value(&saved).unwrap(),
        )
        .await
        .unwrap();
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT value FROM admin_objects WHERE kind='definitions' AND id=$1")
            .bind(definition_id.to_string())
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(stored["version"], "1");
    let mut version2 = saved.clone();
    version2.version = "2".into();
    let mut tx = db.begin().await.unwrap();
    sqlx::query("INSERT INTO admin_objects(kind,id,scope_kind,scope_id,value) VALUES('definition_versions',$1,'source',$2,$3)").bind(format!("{definition_id}:2")).bind(source.to_string()).bind(serde_json::to_value(&version2).unwrap()).execute(&mut *tx).await.unwrap();
    sqlx::query("UPDATE admin_objects SET value=$2 WHERE kind='definitions' AND id=$1")
        .bind(definition_id.to_string())
        .bind(serde_json::to_value(&version2).unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let reproduced = QueryExtension
        .call(
            &state,
            &auth.actor,
            "query_reproduce",
            json!({"id":definition_id,"version":"1","cutoff":request.query.cutoff}),
        )
        .await
        .unwrap();
    let reproduced: ExportStatus = serde_json::from_value(reproduced).unwrap();
    assert_eq!(
        until_done(&client, &base, reproduced.id).await.status,
        "complete"
    );
    sqlx::query("DELETE FROM admin_objects WHERE kind='definitions' AND id=$1")
        .bind(definition_id.to_string())
        .execute(&db)
        .await
        .unwrap();
    assert!(
        QueryExtension
            .call(
                &state,
                &auth.actor,
                "query_reproduce",
                json!({"id":definition_id,"version":"1","cutoff":request.query.cutoff})
            )
            .await
            .is_err()
    );
    worker.abort();
    let cancelled = create(request.clone()).await;
    client
        .post(format!("{base}/v1/exports/{}/cancel", cancelled.id))
        .bearer_auth("fixture-query-token")
        .send()
        .await
        .unwrap();
    assert_eq!(
        until_done(&client, &base, cancelled.id).await.status,
        "cancelled"
    );
    sqlx::query("UPDATE export_jobs SET expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(queued.id)
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        client
            .get(&m.datasets[0].download_url)
            .bearer_auth("fixture-query-token")
            .send()
            .await
            .unwrap()
            .status(),
        410
    );
    // A deleted source at the oldest queued job must not block another source.
    let poison = create(request.clone()).await;
    let missing = Uuid::new_v4();
    let mut poison_request = request.clone();
    poison_request.query.source_id = Some(missing);
    sqlx::query("UPDATE export_jobs SET source_id=$2,request=$3 WHERE id=$1")
        .bind(poison.id)
        .bind(missing)
        .bind(serde_json::to_value(&poison_request).unwrap())
        .execute(&db)
        .await
        .unwrap();
    let good = create(request.clone()).await;
    let temporary_worker = start_worker(state.clone());
    assert_eq!(until_done(&client, &base, good.id).await.status, "complete");
    let poison_status: String = sqlx::query_scalar("SELECT status FROM export_jobs WHERE id=$1")
        .bind(poison.id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(poison_status, "failed");
    let mut long = request.clone();
    long.query.query = json!({"sql":"SELECT sum(a.id*b.id) FROM reporting.large a CROSS JOIN reporting.large b ORDER BY 1"});
    let running = create(long).await;
    for _ in 0..100 {
        let status: ExportStatus = client
            .get(format!("{base}/v1/exports/{}", running.id))
            .bearer_auth("fixture-query-token")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if status.status == "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    client
        .post(format!("{base}/v1/exports/{}/cancel", running.id))
        .bearer_auth("fixture-query-token")
        .send()
        .await
        .unwrap();
    assert_eq!(
        until_done(&client, &base, running.id).await.status,
        "cancelled"
    );
    temporary_worker.abort();
    let revoked = create(request.clone()).await;
    auth.revoked.store(true, Ordering::SeqCst);
    let worker = start_worker(state.clone());
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let row = sqlx::query("SELECT status FROM export_jobs WHERE id=$1")
        .bind(revoked.id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(row.get::<String, _>("status"), "failed");
    assert_eq!(
        client
            .get(format!("{base}/v1/exports/{}/manifest", revoked.id))
            .bearer_auth("fixture-query-token")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    auth.revoked.store(false, Ordering::SeqCst);
    let created = create(request.clone()).await;
    let completed = until_done(&client, &base, created.id).await;
    assert_eq!(completed.status, "complete");
    sqlx::query("UPDATE admin_objects SET revision=revision+1 WHERE kind='sources'")
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        client
            .get(format!("{base}/v1/exports/{}/manifest", created.id))
            .bearer_auth("fixture-query-token")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    worker.abort();
    server.abort();
    db.close().await;
    sqlx::query(&format!("DROP DATABASE {dbname} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("DROP ROLE {role}"))
        .execute(&admin)
        .await
        .unwrap();
}
