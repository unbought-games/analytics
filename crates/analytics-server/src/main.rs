use analytics_core::*;
use analytics_server::{MIGRATOR, auth::DatabaseAuthorization, bootstrap, oidc::OidcVerifier};
use clap::{Parser, Subcommand};
use std::{path::PathBuf, sync::Arc, time::Duration};
use uuid::Uuid;
#[derive(Parser)]
#[command(
    name = "analytics-server",
    version,
    about = "Headless analytics service"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Migrate,
    Bootstrap {
        #[arg(long)]
        owner: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value_t = 30)]
        expires_days: i64,
    },
    Recover {
        #[arg(long)]
        principal: Uuid,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        revoke_all: bool,
        #[arg(long, default_value_t = 30)]
        expires_days: i64,
    },
    Serve {
        #[arg(long, default_value = "127.0.0.1:8080")]
        bind: String,
    },
}
fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::new(env("RUST_LOG", "info")))
        .init();
    if let Err(e) = run().await {
        eprintln!("{}: {}", e.code, e.message);
        std::process::exit(1);
    }
}
async fn run() -> ApiResult<()> {
    let cli = Cli::parse();
    let dsn = match std::env::var_os("ANALYTICS_DATABASE_URL_FILE") {
        Some(path) => bootstrap::read_secret(&PathBuf::from(path))
            .map_err(|_| ApiError::bad_request("Cannot read protected database secret file"))?,
        None => std::env::var("DATABASE_URL").map_err(|_| {
            ApiError::bad_request("Set DATABASE_URL or ANALYTICS_DATABASE_URL_FILE")
        })?,
    };
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(12)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&dsn)
        .await
        .map_err(|_| ApiError::internal())?;
    // Use the same account for schema changes and normal application access.
    // SQLx serializes concurrent migrators with a PostgreSQL advisory lock and
    // verifies applied checksums. Never start serving after a migration failure.
    MIGRATOR.run(&db).await.map_err(|_| ApiError {
        status: axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        code: "migration_failed".into(),
        message: "Database migration failed; startup aborted".into(),
    })?;
    match cli.command {
        Command::Migrate => {
            println!("Migrations applied");
        }
        Command::Bootstrap {
            owner,
            agent,
            output,
            expires_days,
        } => {
            let (owner_id, agent_id) =
                bootstrap::bootstrap(&db, &owner, &agent, &output, expires_days).await?;
            println!(
                "Initialized owner {owner_id} and agent {agent_id}; credential written to protected handoff file"
            );
        }
        Command::Recover {
            principal,
            output,
            revoke_all,
            expires_days,
        } => {
            bootstrap::recover(&db, principal, &output, expires_days, revoke_all).await?;
            println!(
                "Recovered principal {principal}; all prior credentials and OIDC bindings revoked; new credential written to protected handoff file"
            );
        }
        Command::Serve { bind } => {
            let public_base_url = env("ANALYTICS_PUBLIC_BASE_URL", "http://127.0.0.1:8080")
                .trim_end_matches('/')
                .to_owned();
            let url = url::Url::parse(&public_base_url)
                .map_err(|_| ApiError::bad_request("Invalid public base URL"))?;
            if url.query().is_some()
                || url.fragment().is_some()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || (url.scheme() != "https"
                    && !(url.scheme() == "http"
                        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
            {
                return Err(ApiError::bad_request(
                    "Public base URL must be an HTTPS origin, or loopback HTTP for local use",
                ));
            }
            let oidc = if let Ok(issuer) = std::env::var("ANALYTICS_OIDC_ISSUER") {
                Some(OidcVerifier::discover(issuer, format!("{public_base_url}/mcp")).await?)
            } else {
                None
            };
            let auth = Arc::new(DatabaseAuthorization {
                db: db.clone(),
                oidc: oidc.clone(),
            });
            let state = AppState {
                db,
                auth,
                config: Arc::new(ServiceConfig {
                    public_base_url,
                    export_directory: PathBuf::from(env("ANALYTICS_EXPORT_DIRECTORY", "exports")),
                    secret_directory: PathBuf::from(env("ANALYTICS_SECRET_DIRECTORY", "secrets")),
                    max_body_bytes: 1024 * 1024,
                }),
            };
            let _export_worker = analytics_query::start_worker(state.clone());
            let extensions: Vec<Arc<dyn Extension>> = vec![
                Arc::new(analytics_query::QueryExtension),
                Arc::new(analytics_ingest::IngestionExtension),
            ];
            let max_in_flight = env("ANALYTICS_MAX_IN_FLIGHT_REQUESTS", "64")
                .parse::<usize>()
                .map_err(|_| {
                    ApiError::bad_request(
                        "ANALYTICS_MAX_IN_FLIGHT_REQUESTS must be an integer in 1..4096",
                    )
                })?;
            let request_timeout = env("ANALYTICS_REQUEST_TIMEOUT_MS", "30000")
                .parse::<u64>()
                .map_err(|_| {
                    ApiError::bad_request(
                        "ANALYTICS_REQUEST_TIMEOUT_MS must be a positive integer at most 600000",
                    )
                })?;
            let stream_timeout = env("ANALYTICS_STREAM_TIMEOUT_MS", "300000")
                .parse::<u64>()
                .map_err(|_| {
                    ApiError::bad_request(
                        "ANALYTICS_STREAM_TIMEOUT_MS must be a positive integer at most 3600000",
                    )
                })?;
            let app = analytics_server::router_with_admission_limits(
                state,
                extensions,
                oidc,
                analytics_server::AdmissionLimits {
                    max_in_flight,
                    request_timeout: Duration::from_millis(request_timeout),
                    stream_timeout: Duration::from_millis(stream_timeout),
                },
            )?;
            let listener = tokio::net::TcpListener::bind(bind)
                .await
                .map_err(|_| ApiError::internal())?;
            tracing::info!("analytics service listening");
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
                .map_err(|_| ApiError::internal())?;
        }
    }
    Ok(())
}
