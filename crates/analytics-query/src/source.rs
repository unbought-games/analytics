use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Budget {
    pub max_rows: u64,
    pub max_bytes: u64,
    pub timeout_ms: u64,
    pub max_page_bytes: usize,
    pub max_pages: u32,
    pub max_series: usize,
    pub max_samples: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            max_rows: 100_000,
            max_bytes: 64 * 1024 * 1024,
            timeout_ms: 30_000,
            max_page_bytes: 4 * 1024 * 1024,
            max_pages: 100,
            max_series: 10_000,
            max_samples: 100_000,
        }
    }
}
impl Budget {
    pub fn validate(&self) -> Result<(), SourceError> {
        if self.max_rows == 0
            || self.max_rows > 10_000_000
            || self.max_bytes == 0
            || self.max_bytes > 4 * 1024 * 1024 * 1024
            || !(1..=300_000).contains(&self.timeout_ms)
            || !(1024..=16 * 1024 * 1024).contains(&self.max_page_bytes)
            || !(1..=1000).contains(&self.max_pages)
            || !(1..=100_000).contains(&self.max_series)
            || !(1..=1_000_000).contains(&self.max_samples)
        {
            return Err(SourceError::Invalid("invalid_budget"));
        }
        Ok(())
    }
}

// Error values intentionally never contain vendor response bodies, URLs,
// database errors, SQL parameters or credentials.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("source_unavailable")]
    Unavailable,
    #[error("source_denied")]
    Denied,
    #[error("source_response_invalid")]
    Response,
    #[error("cancelled")]
    Cancelled,
    #[error("timeout")]
    Timeout,
    #[error("budget_exceeded")]
    Limit,
    #[error("rate_limited")]
    RateLimited { retry_after: Option<u64> },
    #[error("storage_failure")]
    Storage,
}
impl From<sqlx::Error> for SourceError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
impl From<std::io::Error> for SourceError {
    fn from(_: std::io::Error) -> Self {
        Self::Storage
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub encoding: String,
    pub namespace: Option<String>,
    pub unit: Option<String>,
}
#[derive(Default, Debug)]
pub struct ReadSummary {
    pub columns: Vec<Column>,
    pub warnings: Vec<String>,
    pub continuation: Option<Value>,
    pub snapshot: Option<String>,
}
#[async_trait]
pub trait RowSink: Send {
    async fn row(&mut self, row: Value) -> Result<(), SourceError>;
    async fn columns(&mut self, _columns: Vec<Column>) -> Result<(), SourceError> {
        Ok(())
    }
}

pub async fn cancellable<T>(
    cancel: &CancellationToken,
    future: impl std::future::Future<Output = Result<T, SourceError>>,
) -> Result<T, SourceError> {
    tokio::select! { biased; _ = cancel.cancelled() => Err(SourceError::Cancelled), result = future => result }
}

/// JSON number tokens become decimal strings, including arbitrary precision
/// values nested inside JSON columns; no intermediate IEEE-754 conversion.
pub fn lossless_numbers(value: &mut Value) {
    match value {
        Value::Number(n) => *value = Value::String(n.to_string()),
        Value::Array(a) => a.iter_mut().for_each(lossless_numbers),
        Value::Object(o) => o.values_mut().for_each(lossless_numbers),
        _ => (),
    }
}
