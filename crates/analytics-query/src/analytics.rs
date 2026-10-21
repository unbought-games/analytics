use uuid::Uuid;
// Property-scoped composable filters/projections/aggregates over retained facts.
use crate::source::*;
use analytics_core::*;
use chrono::Utc;
use futures_util::TryStreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Executor, Postgres, QueryBuilder, Row};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalyticsQuery {
    pub dataset: String,
    #[serde(default)]
    pub select: Vec<String>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub group_by: Vec<String>,
    #[serde(default)]
    pub aggregates: Vec<Aggregate>,
    #[serde(default)]
    pub order_by: Vec<Order>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub field: String,
    pub op: String,
    pub value: Value,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    pub field: Option<String>,
    pub op: String,
    pub alias: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Order {
    pub field: String,
    #[serde(default)]
    pub descending: bool,
}
fn name(s: &str) -> bool {
    !s.is_empty() && s.len() < 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
pub fn parse(value: &Value) -> ApiResult<AnalyticsQuery> {
    let q: AnalyticsQuery = serde_json::from_value(value.clone())
        .map_err(|_| ApiError::bad_request("Invalid analytics query"))?;
    if !matches!(
        q.dataset.as_str(),
        "events" | "acquisition" | "identity_map"
    ) || q.select.len() > 64
        || q.filters.len() > 32
        || q.group_by.len() > 8
        || q.aggregates.len() > 16
        || q.order_by.len() > 8
    {
        return Err(ApiError::bad_request(
            "Invalid analytics dataset or complexity",
        ));
    }
    if q.select
        .iter()
        .chain(q.group_by.iter())
        .chain(q.filters.iter().map(|f| &f.field))
        .chain(q.order_by.iter().map(|o| &o.field))
        .any(|s| !name(s))
        || q.aggregates.iter().any(|a| {
            !name(&a.alias)
                || a.field.as_ref().is_some_and(|f| !name(f))
                || !matches!(a.op.as_str(), "count" | "sum" | "avg" | "min" | "max")
                || (a.op != "count" && a.field.is_none())
        })
        || q.filters.iter().any(|f| {
            !matches!(
                f.op.as_str(),
                "eq" | "ne" | "lt" | "le" | "gt" | "ge" | "is_null"
            )
        })
    {
        return Err(ApiError::bad_request(
            "Unsupported analytics field or operator",
        ));
    }
    if !q.aggregates.is_empty() && !q.select.is_empty() {
        return Err(ApiError::bad_request(
            "Aggregate projections use group_by and aggregates",
        ));
    }
    Ok(q)
}
fn field(field: &str) -> String {
    format!("data->'{field}'")
}
pub async fn run(
    state: &AppState,
    request: &QueryRequest,
    budget: &Budget,
    sink: &mut dyn RowSink,
    cancel: &CancellationToken,
) -> Result<ReadSummary, SourceError> {
    let q = parse(&request.query).map_err(|_| SourceError::Invalid("analytics_query"))?;
    let property = request.property_id.ok_or(SourceError::Denied)?;
    cancellable(cancel,async {
        let mut tx=state.db.begin().await?;
        tx.execute("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY").await?;
        tx.execute(format!("SET LOCAL statement_timeout={}; SET LOCAL lock_timeout=1000; SET LOCAL timezone='UTC'",budget.timeout_ms).as_str()).await?;
        let watermark:Option<i64>=sqlx::query_scalar("SELECT max(receipt_cursor) FROM ingest_events WHERE property_id=$1").bind(property).fetch_one(&mut *tx).await?;
        let (table,time,projection)=match q.dataset.as_str(){
            "events"=>("ingest_events","occurred_at","payload || jsonb_build_object('receipt_cursor',receipt_cursor,'received_at',to_char(received_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'))"),
            "acquisition"=>("ingest_acquisitions","signed_up_at","jsonb_build_object('event_id',signup_event_id::text,'kind','signup','campaign',coalesce(campaign->>'experiment_id',campaign->>'utm_campaign'),'occurred_at',to_char(signed_up_at AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'),'authoritative',true,'property',property_id::text,'visit_id',NULL,'issuer',subject_namespace,'subject',subject_id,'attribution_rule',rule_version,'touch_id',first_touch_id::text,'last_touch_id',last_touch_id::text,'campaign_facts',campaign)"),
            "identity_map"=>("ingest_identity_links","created_at","jsonb_build_object('id',id::text,'issuer',subject_namespace,'subject',subject_id,'application',target_namespace,'local_account_id',target_id,'status','verified','relationship',relationship,'world',world,'evidence_event_id',evidence_event_id::text)"),
            _=>return Err(SourceError::Invalid("dataset")),
        };
        let mut sql=QueryBuilder::<Postgres>::new(format!("WITH base AS (SELECT {projection} AS data FROM {table} WHERE property_id="));
        sql.push_bind(property).push(" AND expires_at > ").push_bind(Utc::now()).push(format!(" AND {time} < ")).push_bind(request.cutoff);
        if let Some(w)=&request.window {sql.push(format!(" AND {time} >= ")).push_bind(w.start).push(format!(" AND {time} < ")).push_bind(w.end);}
        sql.push("), filtered AS (SELECT data FROM base WHERE true");
        for f in &q.filters {
            sql.push(" AND ").push(field(&f.field));
            if f.op=="is_null" {sql.push(" IS NOT DISTINCT FROM 'null'::jsonb");}else{sql.push(match f.op.as_str(){"eq"=>" = ","ne"=>" <> ","lt"=>" < ","le"=>" <= ","gt"=>" > ","ge"=>" >= ",_=>unreachable!()}).push_bind(f.value.clone());}
        }
        sql.push("), projected AS (SELECT ");
        if !q.aggregates.is_empty(){
            sql.push("jsonb_build_object(");let mut first=true;
            for group in &q.group_by {if !first{sql.push(",");}first=false;sql.push(format!("'{group}',{}",field(group)));}
            for a in &q.aggregates {if !first{sql.push(",");}first=false;
                sql.push(format!("'{}',",a.alias));
                if a.op=="count" {if let Some(f)=&a.field{sql.push(format!("count(NULLIF({}, 'null'::jsonb))",field(f)));}else{sql.push("count(*)");}}
                else{let f=a.field.as_ref().unwrap();sql.push(format!("{}(CASE WHEN jsonb_typeof({})='number' THEN (data->>'{f}')::numeric ELSE NULL END)",a.op,field(f)));}
            }
            sql.push(") AS data FROM filtered");
            if !q.group_by.is_empty(){sql.push(" GROUP BY ").push(q.group_by.iter().map(|f|field(f)).collect::<Vec<_>>().join(","));}
        }else if q.select.is_empty(){sql.push("data FROM filtered");}else{sql.push("jsonb_build_object(").push(q.select.iter().map(|f|format!("'{f}',{}",field(f))).collect::<Vec<_>>().join(",")).push(") AS data FROM filtered");}
        sql.push(") SELECT CASE WHEN octet_length(data::text) <= ").push_bind(budget.max_page_bytes as i64).push(" THEN data::text ELSE NULL END AS row FROM projected ORDER BY ");
        for o in &q.order_by{sql.push(field(&o.field)).push(if o.descending{" DESC,"}else{" ASC,"});}
        // Full canonical row is a deterministic tie-breaker even for projections
        // and aggregates. Equal duplicate rows are byte-identical.
        sql.push("data ASC LIMIT ").push_bind(budget.max_rows.saturating_add(1) as i64);
        let mut rows=sql.build().fetch(&mut *tx);
        while let Some(row)=rows.try_next().await?{
            let raw:Option<String>=row.get(0);let raw=raw.ok_or(SourceError::Limit)?;
            let mut value:Value=serde_json::from_str(&raw).map_err(|_|SourceError::Response)?;lossless_numbers(&mut value);sink.row(value).await?;
        }
        drop(rows);tx.commit().await?;
        Ok(ReadSummary{snapshot:watermark.map(|v|v.to_string()),warnings:vec!["Retention and policy may remove earlier facts; current rows do not prove complete longitudinal history".into()],..Default::default()})
    }).await
}
pub async fn discover(state: &AppState, actor: &AuthContext, property: Uuid) -> ApiResult<Value> {
    state
        .auth
        .authorize(actor, "query:read", &Resource::property(property))
        .await?;
    let policy: PropertyDefinition =
        load_object(&state.db, "properties", &property.to_string()).await?;
    let rows=sqlx::query("SELECT min(occurred_at) AS first,max(occurred_at) AS last,count(*) AS rows,array_agg(DISTINCT policy_version) AS versions FROM ingest_events WHERE property_id=$1 AND expires_at>now()") .bind(property).fetch_one(&state.db).await?;
    Ok(
        json!({"property_id":property,"datasets":["events","acquisition","identity_map"],"operators":["eq","ne","lt","le","gt","ge","is_null"],"aggregates":["count","sum","avg","min","max"],"profile":policy.policy.profile,"policy_version":policy.policy.version,"retained_rows":rows.get::<i64,_>("rows").to_string(),"first_observed":rows.get::<Option<chrono::DateTime<Utc>>,_>("first"),"last_observed":rows.get::<Option<chrono::DateTime<Utc>>,_>("last"),"history_complete":false,"limitations":["Observed bounds do not establish continuous coverage","Minimal/journey profiles cannot establish person-level longitudinal retention"]}),
    )
}
