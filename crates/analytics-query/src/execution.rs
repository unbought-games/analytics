use crate::{
    http_sources::{HttpSource, PrometheusQuery, SentryQuery, SentryScope},
    postgres::PostgresSource,
    safety::Relations,
    source::*,
};
use analytics_core::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub async fn validate_enabled(state: &AppState, query: &QueryRequest) -> ApiResult<()> {
    if let Some(id) = query.source_id {
        let s: SourceDefinition = load_object(&state.db, "sources", &id.to_string()).await?;
        if !s.enabled {
            return Err(ApiError::forbidden());
        }
    }
    if let Some(id) = query.property_id {
        let p: PropertyDefinition = load_object(&state.db, "properties", &id.to_string()).await?;
        if !p.enabled {
            return Err(ApiError::forbidden());
        }
    }
    Ok(())
}
pub async fn budget(state: &AppState, query: &QueryRequest) -> ApiResult<Budget> {
    let mut b = Budget {
        max_rows: query.budget.max_rows,
        max_bytes: query.budget.max_bytes,
        timeout_ms: query.budget.timeout_ms,
        ..Default::default()
    };
    if let Some(id) = query.source_id {
        let source: SourceDefinition = load_object(&state.db, "sources", &id.to_string()).await?;
        b.max_rows = b.max_rows.min(source.budget.max_rows);
        b.max_bytes = b.max_bytes.min(source.budget.max_bytes);
        b.timeout_ms = b.timeout_ms.min(source.budget.timeout_ms);
    }
    b.validate()
        .map_err(|_| ApiError::bad_request("Invalid query budget"))?;
    if !(1..=4).contains(&query.budget.max_concurrency) {
        return Err(ApiError::bad_request("Concurrency must be 1..4"));
    }
    Ok(b)
}
pub async fn validate_request(state: &AppState, query: &QueryRequest) -> ApiResult<()> {
    crate::export::scope(query)?;
    validate_enabled(state, query).await?;
    budget(state, query).await?;
    if query.definition_version.is_empty()
        || query.definition_version.len() > 128
        || serde_json::to_vec(query)
            .map_err(|_| ApiError::internal())?
            .len()
            > 128 * 1024
    {
        return Err(ApiError::bad_request("Invalid query definition"));
    }
    if let Some(w) = &query.window
        && (w.end < w.start || w.end > query.cutoff)
    {
        return Err(ApiError::bad_request("Window must end at or before cutoff"));
    }
    if query.property_id.is_some() {
        if query.kind != "analytics" {
            return Err(ApiError::bad_request(
                "Property queries must use analytics kind",
            ));
        }
        crate::analytics::parse(&query.query)?;
    }
    if let Some(id) = query.source_id {
        let s: SourceDefinition = load_object(&state.db, "sources", &id.to_string()).await?;
        match s.kind.as_str() {
            "postgres" if query.kind == "postgres" => {
                let sql = sql(query)?;
                crate::safety::validate_sql(sql, &relations(&s)?).map_err(ApiError::bad_request)?;
            }
            "prometheus" if query.kind.starts_with("prometheus_") => {
                prometheus_query(query)?;
            }
            "sentry" if query.kind.starts_with("sentry_") => {
                sentry_query(query)?;
            }
            _ => return Err(ApiError::bad_request("Query kind does not match source")),
        }
    }
    // SQL parameters are deliberately unavailable until typed binding can be
    // reproduced safely; use literals in validated SQL, never substitution.
    if !query.parameters.is_empty()
        && (query.kind != "postgres"
            || query.parameters.len() > 64
            || (1..=query.parameters.len()).any(|i| {
                query
                    .parameters
                    .get(&format!("p{i}"))
                    .is_none_or(|v| !v.is_string() && !v.is_null())
            }))
    {
        return Err(ApiError::bad_request(
            "PostgreSQL parameters p1..pN must be strings or null; cast explicitly in SQL",
        ));
    }
    Ok(())
}
fn relations(s: &SourceDefinition) -> ApiResult<Relations> {
    let mut result = Relations::new();
    for table in &s.allowed_tables {
        let (schema, _) = table
            .split_once('.')
            .ok_or_else(|| ApiError::bad_request("Source tables require schema qualification"))?;
        if !s.allowed_schemas.iter().any(|v| v == schema)
            || table
                .bytes()
                .any(|b| !b.is_ascii_alphanumeric() && !matches!(b, b'_' | b'.'))
        {
            return Err(ApiError::bad_request("Source relation not allowed"));
        }
        let columns = s
            .allowed_columns
            .get(table)
            .ok_or_else(|| ApiError::bad_request("Explicit source column allowlist required"))?;
        if columns.is_empty() {
            return Err(ApiError::bad_request("Empty column scope"));
        }
        result.insert(table.clone(), columns.iter().cloned().collect());
    }
    Ok(result)
}
async fn secret(
    state: &AppState,
    reference: &Option<String>,
) -> Result<Option<String>, SourceError> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    if reference.is_empty()
        || reference.len() > 128
        || !reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        || reference == "."
        || reference == ".."
    {
        return Err(SourceError::Invalid("secret_reference"));
    }
    let path = state.config.secret_directory.join(reference);
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| SourceError::Unavailable)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16384 {
        return Err(SourceError::Denied);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(SourceError::Denied);
        }
    }
    Ok(Some(
        tokio::fs::read_to_string(path)
            .await
            .map_err(|_| SourceError::Unavailable)?
            .trim()
            .into(),
    ))
}
fn sql(query: &QueryRequest) -> ApiResult<&str> {
    query
        .query
        .get("sql")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("query.sql required"))
}
fn prometheus_query(q: &QueryRequest) -> ApiResult<PrometheusQuery> {
    let v = q.query.clone();
    let expression = || {
        v.get("expression")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| ApiError::bad_request("expression required"))
    };
    let window = || {
        q.window
            .as_ref()
            .ok_or_else(|| ApiError::bad_request("window required"))
    };
    match q.kind.as_str() {
        "prometheus_instant" => Ok(PrometheusQuery::Instant {
            expression: expression()?,
            time: q.cutoff,
        }),
        "prometheus_range" => {
            let w = window()?;
            Ok(PrometheusQuery::Range {
                expression: expression()?,
                start: w.start,
                end: w.end,
                step_seconds: v
                    .get("step_seconds")
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| ApiError::bad_request("step_seconds required"))?,
            })
        }
        "prometheus_discovery" => {
            let w = window()?;
            match v
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("metrics")
            {
                "metrics" => Ok(PrometheusQuery::Metrics {
                    start: w.start,
                    end: w.end,
                }),
                "labels" => Ok(PrometheusQuery::Labels {
                    selector: v
                        .get("selector")
                        .and_then(Value::as_str)
                        .unwrap_or("{__name__=~\".+\"}")
                        .into(),
                    start: w.start,
                    end: w.end,
                }),
                "label_values" => Ok(PrometheusQuery::LabelValues {
                    label: v
                        .get("label")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ApiError::bad_request("label required"))?
                        .into(),
                    selector: v
                        .get("selector")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ApiError::bad_request("selector required"))?
                        .into(),
                    start: w.start,
                    end: w.end,
                }),
                _ => Err(ApiError::bad_request("Unknown discovery operation")),
            }
        }
        _ => Err(ApiError::bad_request("Unknown Prometheus query")),
    }
}
fn sentry_query(q: &QueryRequest) -> ApiResult<SentryQuery> {
    let v = &q.query;
    let project = v
        .get("project")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("project required"))?
        .into();
    let cursor = v.get("cursor").and_then(Value::as_str).map(str::to_owned);
    if q.kind == "sentry_event" {
        return Ok(SentryQuery::Detail {
            project,
            issue: v
                .get("issue")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::bad_request("issue required"))?
                .into(),
            event: v
                .get("event")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::bad_request("event required"))?
                .into(),
        });
    }
    let w = q
        .window
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("window required"))?;
    match q.kind.as_str() {
        "sentry_issues" => Ok(SentryQuery::Issues {
            project,
            start: w.start,
            end: w.end,
            query: v.get("filter").and_then(Value::as_str).unwrap_or("").into(),
            cursor,
        }),
        "sentry_events" => Ok(SentryQuery::Events {
            project,
            start: w.start,
            end: w.end,
            cursor,
        }),
        _ => Err(ApiError::bad_request("Unknown Sentry query")),
    }
}
pub async fn execute(
    state: &AppState,
    query: &QueryRequest,
    budget: &Budget,
    sink: &mut dyn RowSink,
    cancel: &CancellationToken,
) -> Result<ReadSummary, SourceError> {
    if query.property_id.is_some() {
        return crate::analytics::run(state, query, budget, sink, cancel).await;
    }
    let s: SourceDefinition = load_object(
        &state.db,
        "sources",
        &query.source_id.ok_or(SourceError::Denied)?.to_string(),
    )
    .await
    .map_err(|_| SourceError::Denied)?;
    if !s.enabled {
        return Err(SourceError::Denied);
    }
    let secret = secret(state, &s.secret_ref).await?;
    match s.kind.as_str() {
        "postgres" => {
            let dsn = secret.ok_or(SourceError::Invalid("postgres_secret_required"))?;
            PostgresSource::new(dsn, relations(&s).map_err(|_| SourceError::Denied)?)
                .run(
                    sql(query).map_err(|_| SourceError::Response)?,
                    &query.parameters,
                    budget,
                    sink,
                    cancel,
                )
                .await
        }
        "prometheus" => {
            HttpSource::new(&s.endpoint, secret)?
                .prometheus(
                    &prometheus_query(query).map_err(|_| SourceError::Response)?,
                    budget,
                    sink,
                    cancel,
                )
                .await
        }
        "sentry" => {
            HttpSource::new(&s.endpoint, secret)?
                .sentry(
                    &SentryScope {
                        organization: s
                            .semantics
                            .get("organization")
                            .and_then(Value::as_str)
                            .ok_or(SourceError::Invalid("organization_required"))?
                            .into(),
                        projects: s.allowed_projects.into_iter().collect::<BTreeSet<_>>(),
                    },
                    &sentry_query(query).map_err(|_| SourceError::Response)?,
                    budget,
                    sink,
                    cancel,
                )
                .await
        }
        _ => Err(SourceError::Invalid("unsupported_source")),
    }
}
pub async fn discover(state: &AppState, actor: &AuthContext, id: Uuid) -> ApiResult<Value> {
    static DISCOVERY: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    let _permit = DISCOVERY
        .get_or_init(|| tokio::sync::Semaphore::new(4))
        .try_acquire()
        .map_err(|_| {
            ApiError::new(
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                "discovery_busy",
                "Discovery capacity reached",
            )
        })?;
    state
        .auth
        .authorize(actor, "sources:read", &Resource::source(id))
        .await?;
    state
        .auth
        .authorize(actor, "query:read", &Resource::source(id))
        .await?;
    let s: SourceDefinition = load_object(&state.db, "sources", &id.to_string()).await?;
    if !s.enabled {
        return Err(ApiError::forbidden());
    }
    let details = if s.kind == "postgres" {
        let connection = secret(state, &s.secret_ref)
            .await
            .map_err(|_| ApiError::internal())?
            .ok_or_else(ApiError::internal)?;
        tokio::time::timeout(
            std::time::Duration::from_millis(s.budget.timeout_ms.min(30_000)),
            PostgresSource::new(connection, relations(&s)?).discover(),
        )
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
    } else {
        json!({"operations":if s.kind=="prometheus"{vec!["prometheus_instant","prometheus_range","prometheus_discovery"]}else{vec!["sentry_issues","sentry_events","sentry_event"]},"projects":s.allowed_projects})
    };
    Ok(
        json!({"id":id,"name":s.name,"kind":s.kind,"environment":s.environment,"details":details,"budget":s.budget}),
    )
}

pub async fn metadata(
    state: &AppState,
    q: &QueryRequest,
    columns: &mut [Column],
) -> ApiResult<(Coverage, Value)> {
    let unknown = Coverage {
        complete: false,
        history_start: None,
        history_end: None,
        limitations: vec![
            "Successful retrieval does not establish retained history completeness".into(),
        ],
        sample_rate: None,
        policy_versions: Vec::new(),
    };
    if let Some(id) = q.source_id {
        let s: SourceDefinition = load_object(&state.db, "sources", &id.to_string()).await?;
        let mut coverage: Coverage = s
            .semantics
            .get("coverage")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or(unknown);
        if coverage.history_start.is_none()
            || coverage.history_end.is_none()
            || coverage.history_start > coverage.history_end
            || coverage.sample_rate != Some(1.)
        {
            coverage.complete = false;
        }
        if let Some(w) = &q.window
            && (coverage.history_start.is_some_and(|t| t > w.start)
                || coverage.history_end.is_some_and(|t| t < w.end))
        {
            coverage.complete = false;
            coverage
                .limitations
                .push("Requested window exceeds declared retained history".into());
        }
        for c in columns {
            if let Some(v) = s.semantics.get("columns").and_then(|v| v.get(&c.name)) {
                c.namespace = v
                    .get("namespace")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                c.unit = v.get("unit").and_then(Value::as_str).map(str::to_owned);
            }
        }
        Ok((
            coverage,
            json!({"source_id":id,"environment":s.environment,"kind":s.kind,"coverage_basis":"administrator-declared source history; not inferred from row count","numeric_encoding":"decimal_string_recursive","null_semantics":"null distinct from absent and zero","snapshot_semantics":"independent source reads; PostgreSQL repeatable-read transaction; vendor pages independent"}),
        ))
    } else {
        let id = q.property_id.ok_or_else(ApiError::forbidden)?;
        let p: PropertyDefinition = load_object(&state.db, "properties", &id.to_string()).await?;
        let mut coverage = unknown;
        coverage.policy_versions = vec![p.policy.version];
        coverage.limitations.push(format!(
            "Collection profile {:?}; detailed facts retained {} days; identity facts {} days",
            p.policy.profile, p.policy.event_retention_days, p.policy.identity_retention_days
        ));
        Ok((
            coverage,
            json!({"property_id":id,"numeric_encoding":"decimal_string_recursive","null_semantics":"null distinct from absent and zero","receipt_watermark":"maximum committed cursor at repeatable-read snapshot"}),
        ))
    }
}
