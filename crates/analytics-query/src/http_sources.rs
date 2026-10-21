use crate::source::*;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use tokio_util::sync::CancellationToken;

/// An endpoint is an administrator-selected trust boundary. PromQL is permitted
/// over this entire endpoint/tenant; use separate sources for separate tenants.
/// Never configure a multi-tenant endpoint with credentials broader than its grant.
pub struct HttpSource {
    client: Client,
    base: Url,
    token: Option<String>,
}
impl HttpSource {
    pub fn new(endpoint: &str, token: Option<String>) -> Result<Self, SourceError> {
        let base = Url::parse(endpoint).map_err(|_| SourceError::Invalid("source_endpoint"))?;
        if !matches!(base.scheme(), "https" | "http")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || !base.path().ends_with('/')
        {
            return Err(SourceError::Invalid("source_endpoint"));
        }
        if base.scheme() == "http"
            && !matches!(base.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        {
            return Err(SourceError::Invalid("https_required"));
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| SourceError::Unavailable)?;
        Ok(Self {
            client,
            base,
            token,
        })
    }
    async fn get(
        &self,
        path: &str,
        params: &[(String, String)],
        budget: &Budget,
        cancel: &CancellationToken,
    ) -> Result<(Value, Option<String>), SourceError> {
        let url = self
            .base
            .join(path)
            .map_err(|_| SourceError::Invalid("source_path"))?;
        cancellable(cancel, async {
            let mut request = self
                .client
                .get(url)
                .query(params)
                .timeout(Duration::from_millis(budget.timeout_ms));
            if let Some(token) = &self.token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await.map_err(|_| SourceError::Unavailable)?;
            if response.status().as_u16() == 429 {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse().ok())
                    .or_else(|| {
                        response
                            .headers()
                            .get("x-sentry-rate-limit-reset")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse::<i64>().ok())
                            .map(|t| (t - Utc::now().timestamp()).max(0) as u64)
                    });
                return Err(SourceError::RateLimited { retry_after });
            }
            if matches!(response.status().as_u16(), 401 | 403) {
                return Err(SourceError::Denied);
            }
            if !response.status().is_success() {
                return Err(SourceError::Unavailable);
            }
            if response
                .content_length()
                .is_some_and(|n| n > budget.max_page_bytes as u64)
            {
                return Err(SourceError::Limit);
            }
            let link = response
                .headers()
                .get("link")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| SourceError::Unavailable)?;
                if bytes.len() + chunk.len() > budget.max_page_bytes {
                    return Err(SourceError::Limit);
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok((
                serde_json::from_slice(&bytes).map_err(|_| SourceError::Response)?,
                link,
            ))
        })
        .await
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrometheusQuery {
    Instant {
        expression: String,
        time: DateTime<Utc>,
    },
    Range {
        expression: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        step_seconds: u32,
    },
    Metrics {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    Labels {
        selector: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    LabelValues {
        label: String,
        selector: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
}
fn interval(start: &DateTime<Utc>, end: &DateTime<Utc>) -> Result<(), SourceError> {
    if end < start || (*end - *start).num_days() > 366 {
        return Err(SourceError::Invalid("invalid_interval"));
    }
    Ok(())
}
fn bounded(s: &str) -> Result<(), SourceError> {
    if s.len() > 16 * 1024 {
        Err(SourceError::Invalid("query_too_large"))
    } else {
        Ok(())
    }
}
fn segment(s: &str) -> Result<(), SourceError> {
    if s.is_empty()
        || s.len() > 128
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        Err(SourceError::Invalid("invalid_identifier"))
    } else {
        Ok(())
    }
}
impl HttpSource {
    pub async fn prometheus(
        &self,
        query: &PrometheusQuery,
        budget: &Budget,
        sink: &mut dyn RowSink,
        cancel: &CancellationToken,
    ) -> Result<ReadSummary, SourceError> {
        budget.validate()?;
        let mut params = vec![(
            "timeout".into(),
            format!("{}s", budget.timeout_ms as f64 / 1000.),
        )];
        let path = match query {
            PrometheusQuery::Instant { expression, time } => {
                bounded(expression)?;
                params.extend([
                    ("query".into(), expression.clone()),
                    ("time".into(), time.to_rfc3339()),
                ]);
                "api/v1/query".into()
            }
            PrometheusQuery::Range {
                expression,
                start,
                end,
                step_seconds,
            } => {
                bounded(expression)?;
                interval(start, end)?;
                if *step_seconds == 0
                    || ((*end - *start).num_seconds() as u64 / *step_seconds as u64 + 1)
                        > budget.max_samples as u64
                {
                    return Err(SourceError::Invalid("sample_budget"));
                }
                params.extend([
                    ("query".into(), expression.clone()),
                    ("start".into(), start.to_rfc3339()),
                    ("end".into(), end.to_rfc3339()),
                    ("step".into(), step_seconds.to_string()),
                ]);
                "api/v1/query_range".into()
            }
            PrometheusQuery::Metrics { start, end } => {
                interval(start, end)?;
                params.extend([
                    ("start".into(), start.to_rfc3339()),
                    ("end".into(), end.to_rfc3339()),
                ]);
                "api/v1/label/__name__/values".into()
            }
            PrometheusQuery::Labels {
                selector,
                start,
                end,
            }
            | PrometheusQuery::LabelValues {
                selector,
                start,
                end,
                ..
            } => {
                bounded(selector)?;
                interval(start, end)?;
                params.extend([
                    ("match[]".into(), selector.clone()),
                    ("start".into(), start.to_rfc3339()),
                    ("end".into(), end.to_rfc3339()),
                ]);
                if let PrometheusQuery::LabelValues { label, .. } = query {
                    segment(label)?;
                    format!("api/v1/label/{label}/values")
                } else {
                    "api/v1/labels".into()
                }
            }
        };
        let (mut body, _) = self.get(&path, &params, budget, cancel).await?;
        if body["status"] != "success" {
            return Err(SourceError::Unavailable);
        }
        let warnings = body
            .get("warnings")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(20)
            .filter_map(Value::as_str)
            .map(|warning| {
                let sanitized = if let Some(secret) = &self.token {
                    warning.replace(secret, "[redacted]")
                } else {
                    warning.to_owned()
                };
                sanitized.chars().take(1024).collect()
            })
            .collect();
        let data = body.get_mut("data").ok_or(SourceError::Response)?;
        lossless_numbers(data);
        let mut samples = 0;
        if let Some(rows) = data.as_array() {
            if rows.len() > budget.max_series {
                return Err(SourceError::Limit);
            }
            for row in rows {
                sink.row(json!({"value":row})).await?;
            }
        } else {
            let result_type = data["resultType"]
                .as_str()
                .ok_or(SourceError::Response)?
                .to_owned();
            match result_type.as_str() {
                "matrix" | "vector" => {
                    let rows = data["result"].as_array().ok_or(SourceError::Response)?;
                    if rows.len() > budget.max_series {
                        return Err(SourceError::Limit);
                    }
                    for row in rows {
                        samples += row
                            .get("values")
                            .and_then(Value::as_array)
                            .map(Vec::len)
                            .unwrap_or(1);
                        samples += row
                            .get("histograms")
                            .and_then(Value::as_array)
                            .map(Vec::len)
                            .unwrap_or(0);
                        if samples > budget.max_samples {
                            return Err(SourceError::Limit);
                        }
                        sink.row(json!({"result_type":result_type,"series":row}))
                            .await?;
                    }
                }
                "scalar" | "string" => {
                    sink.row(json!({"result_type":result_type,"sample":data["result"]}))
                        .await?;
                }
                _ => return Err(SourceError::Response),
            }
        }
        Ok(ReadSummary {
            warnings,
            snapshot: Some(
                "independent Prometheus evaluation; missing samples are absent, not zero".into(),
            ),
            ..Default::default()
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum SentryQuery {
    Issues {
        project: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        query: String,
        #[serde(default)]
        cursor: Option<String>,
    },
    Events {
        project: String,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        #[serde(default)]
        cursor: Option<String>,
    },
    Detail {
        project: String,
        issue: String,
        event: String,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SentryScope {
    pub organization: String,
    pub projects: BTreeSet<String>,
}
impl HttpSource {
    pub async fn sentry(
        &self,
        scope: &SentryScope,
        query: &SentryQuery,
        budget: &Budget,
        sink: &mut dyn RowSink,
        cancel: &CancellationToken,
    ) -> Result<ReadSummary, SourceError> {
        budget.validate()?;
        segment(&scope.organization)?;
        let project = match query {
            SentryQuery::Issues { project, .. }
            | SentryQuery::Events { project, .. }
            | SentryQuery::Detail { project, .. } => project,
        };
        segment(project)?;
        if !scope.projects.contains(project) {
            return Err(SourceError::Denied);
        }
        let org = &scope.organization;
        let mut params = vec![("per_page".into(), "100".into())];
        let (path, mut cursor) = match query {
            SentryQuery::Issues {
                start,
                end,
                query,
                cursor,
                ..
            } => {
                interval(start, end)?;
                bounded(query)?;
                params.extend([
                    ("project".into(), project.clone()),
                    ("start".into(), start.to_rfc3339()),
                    ("end".into(), end.to_rfc3339()),
                    ("query".into(), query.clone()),
                    ("sort".into(), "date".into()),
                    ("shortIdLookup".into(), "0".into()),
                ]);
                (format!("api/0/organizations/{org}/issues/"), cursor.clone())
            }
            SentryQuery::Events {
                start, end, cursor, ..
            } => {
                interval(start, end)?;
                params.extend([
                    ("start".into(), start.to_rfc3339()),
                    ("end".into(), end.to_rfc3339()),
                    ("full".into(), "false".into()),
                ]);
                (
                    format!("api/0/projects/{org}/{project}/events/"),
                    cursor.clone(),
                )
            }
            SentryQuery::Detail { issue, event, .. } => {
                segment(issue)?;
                segment(event)?;
                if event.len() != 32 || !event.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return Err(SourceError::Invalid("explicit_event_id_required"));
                }
                (
                    format!("api/0/projects/{org}/{project}/events/{event}/"),
                    None,
                )
            }
        };
        let mut seen = BTreeSet::new();
        for _ in 0..budget.max_pages {
            let mut p = params.clone();
            if let Some(cursor) = &cursor {
                bounded(cursor)?;
                p.push(("cursor".into(), cursor.clone()));
            }
            let (body, link) = self.get(&path, &p, budget, cancel).await?;
            if let SentryQuery::Detail { .. } = query {
                // The issue-detail route is organization scoped. Verify project
                // membership from the response before any bytes enter the sink.
                let response_project = body.get("projectID").and_then(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .or_else(|| v.as_u64().map(|n| n.to_string()))
                });
                if response_project
                    .as_ref()
                    .is_none_or(|id| id.is_empty() || !id.bytes().all(|c| c.is_ascii_digit()))
                    || (project.bytes().all(|c| c.is_ascii_digit())
                        && response_project.as_deref() != Some(project.as_str()))
                {
                    return Err(SourceError::Denied);
                }
                sink.row(redact_sentry(&body)).await?;
                return Ok(ReadSummary::default());
            }
            let rows = body.as_array().ok_or(SourceError::Response)?;
            for row in rows {
                if matches!(query, SentryQuery::Issues { .. }) && !project_matches(row, project) {
                    return Err(SourceError::Denied);
                }
                sink.row(redact_sentry(row)).await?;
            }
            cursor = link.as_deref().map(next_cursor).transpose()?.flatten();
            if cursor.is_none() {
                return Ok(ReadSummary{snapshot:Some("Sentry pages are independent reads; deduplicate by id; counts are issue/event semantics".into()),..Default::default()});
            }
            if !seen.insert(cursor.clone()) {
                return Err(SourceError::Response);
            }
        }
        Ok(ReadSummary {
            continuation: Some(json!({"cursor":cursor})),
            warnings: vec!["page_budget_reached".into()],
            ..Default::default()
        })
    }
}
fn project_matches(row: &Value, project: &str) -> bool {
    row.get("projectID").and_then(Value::as_str) == Some(project)
        || row.pointer("/project/id").and_then(Value::as_str) == Some(project)
        || row.pointer("/project/slug").and_then(Value::as_str) == Some(project)
}
/// Positive field selection: raw messages, titles, URLs, user data, request
/// headers/bodies, breadcrumbs, stack vars and arbitrary tags never leave source.
fn redact_sentry(row: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in [
        "id",
        "eventID",
        "groupID",
        "projectID",
        "firstSeen",
        "lastSeen",
        "dateCreated",
        "dateReceived",
        "timestamp",
        "count",
        "userCount",
        "type",
        "platform",
        "level",
        "environment",
    ] {
        if let Some(v) = row.get(key)
            && (v.is_null() || v.is_string() || v.is_number() || v.is_boolean())
        {
            out.insert(key.into(), v.clone());
        }
    }
    if let Some(project) = row.get("project") {
        out.insert(
            "project".into(),
            json!({"id":project.get("id"),"slug":project.get("slug")}),
        );
    }
    if let Some(release) = row.get("release") {
        out.insert(
            "release".into(),
            if release.is_string() {
                release.clone()
            } else {
                json!({"version":release.get("version")})
            },
        );
    }
    if let Some(tags) = row.get("tags").and_then(Value::as_array) {
        out.insert(
            "tags".into(),
            Value::Array(
                tags.iter()
                    .filter(|t| {
                        matches!(
                            t["key"].as_str(),
                            Some("release" | "environment" | "request_id" | "application")
                        )
                    })
                    .map(|t| json!({"key":t["key"],"value":t["value"]}))
                    .collect(),
            ),
        );
    }
    let mut value = Value::Object(out);
    lossless_numbers(&mut value);
    value
}
fn next_cursor(link: &str) -> Result<Option<String>, SourceError> {
    for part in link.split(',') {
        if part.contains("rel=\"next\"") && part.contains("results=\"true\"") {
            let url = part
                .split('<')
                .nth(1)
                .and_then(|p| p.split('>').next())
                .ok_or(SourceError::Response)?;
            let url = Url::parse(url).map_err(|_| SourceError::Response)?;
            // Extract only cursor. Never follow the supplied URL or any changed scope.
            return url
                .query_pairs()
                .find(|(k, _)| k == "cursor")
                .map(|(_, v)| Some(v.into_owned()))
                .ok_or(SourceError::Response);
        }
    }
    Ok(None)
}
