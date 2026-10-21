use crate::{
    safety::{Relations, validate_sql},
    source::*,
};
use futures_util::TryStreamExt;
use serde_json::Value;
use sqlx::{
    Column as SqlColumn, Connection, Executor, Row, TypeInfo,
    postgres::{PgConnectOptions, PgConnection},
};
use std::{collections::BTreeMap, str::FromStr, time::Duration};
use tokio_util::sync::CancellationToken;

/// Connection material is never Debug/Serialize. Callers resolve deployment
/// secrets only after authorization; there is no request-supplied DSN.
pub struct PostgresSource {
    connection: String,
    pub relations: Relations,
}
impl PostgresSource {
    pub fn new(connection: String, relations: Relations) -> Self {
        Self {
            connection,
            relations,
        }
    }
    async fn connect(&self) -> Result<PgConnection, SourceError> {
        let opts = PgConnectOptions::from_str(&self.connection)
            .map_err(|_| SourceError::Invalid("source_configuration"))?
            .options([
                ("statement_timeout", "30000"),
                ("lock_timeout", "1000"),
                ("client_connection_check_interval", "1000"),
                ("application_name", "analytics_source"),
            ]);
        let mut connection = PgConnection::connect_with(&opts).await?;
        // Reject inherited/elevated roles. A dedicated source role is mandatory.
        let elevated: bool = sqlx::query_scalar("SELECT rolsuper OR rolcreaterole OR rolcreatedb OR rolreplication OR rolbypassrls OR EXISTS(SELECT 1 FROM pg_auth_members WHERE member = r.oid) FROM pg_roles r WHERE rolname=current_user").fetch_one(&mut connection).await?;
        if elevated {
            return Err(SourceError::Denied);
        }
        for (table, allowed) in &self.relations {
            let rows = sqlx::query("SELECT a.attname, t.typname, n.nspname AS type_schema, has_column_privilege(current_user,c.oid,a.attnum,'SELECT') AS can_read, c.relowner=(SELECT oid FROM pg_roles WHERE rolname=current_user) AS owns FROM pg_class c JOIN pg_attribute a ON a.attrelid=c.oid JOIN pg_type t ON t.oid=a.atttypid JOIN pg_namespace n ON n.oid=t.typnamespace WHERE c.oid=to_regclass($1) AND a.attnum>0 AND NOT a.attisdropped").bind(table).fetch_all(&mut connection).await?;
            if rows.is_empty()
                || allowed
                    .iter()
                    .any(|a| !rows.iter().any(|r| r.get::<String, _>("attname") == *a))
            {
                return Err(SourceError::Denied);
            }
            for row in rows {
                let name: String = row.get("attname");
                let ty: String = row.get("typname");
                let schema: String = row.get("type_schema");
                let read: bool = row.get("can_read");
                let owns: bool = row.get("owns");
                if owns
                    || (read && !allowed.contains(&name))
                    || (allowed.contains(&name)
                        && (!read
                            || schema != "pg_catalog"
                            || !matches!(
                                ty.as_str(),
                                "bool"
                                    | "int2"
                                    | "int4"
                                    | "int8"
                                    | "float4"
                                    | "float8"
                                    | "numeric"
                                    | "text"
                                    | "varchar"
                                    | "bpchar"
                                    | "uuid"
                                    | "date"
                                    | "timestamp"
                                    | "timestamptz"
                                    | "json"
                                    | "jsonb"
                            )))
                {
                    return Err(SourceError::Denied);
                }
            }
        }
        Ok(connection)
    }
    pub async fn discover(&self) -> Result<Value, SourceError> {
        let mut c = self.connect().await?;
        let mut tables = Vec::new();
        for (table, allowed) in &self.relations {
            let rows = sqlx::query("SELECT a.attname, format_type(a.atttypid,a.atttypmod) AS data_type, NOT a.attnotnull AS nullable FROM pg_attribute a WHERE a.attrelid=to_regclass($1) AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum").bind(table).fetch_all(&mut c).await?;
            let columns: Vec<Value> = rows.into_iter().filter(|r|allowed.contains(r.get::<String,_>("attname").as_str())).map(|r|serde_json::json!({"name":r.get::<String,_>("attname"),"data_type":r.get::<String,_>("data_type"),"nullable":r.get::<bool,_>("nullable")})).collect();
            tables.push(serde_json::json!({"relation":table,"columns":columns}));
        }
        Ok(
            serde_json::json!({"relations":tables,"ordering":"explicit ORDER BY with a unique tie-breaker required","snapshot":"repeatable_read","number_encoding":"decimal_string"}),
        )
    }
    pub async fn run(
        &self,
        sql: &str,
        parameters: &BTreeMap<String, Value>,
        budget: &Budget,
        sink: &mut dyn RowSink,
        cancel: &CancellationToken,
    ) -> Result<ReadSummary, SourceError> {
        budget.validate()?;
        let safe = validate_sql(sql, &self.relations).map_err(SourceError::Invalid)?;
        // Closing this dedicated connection on cancellation interrupts the source
        // backend; statement_timeout independently bounds abandoned work.
        cancellable(cancel, async {
            tokio::time::timeout(Duration::from_millis(budget.timeout_ms), async {
                let mut c = self.connect().await?;
                c.execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY").await?;
                c.execute(format!("SET LOCAL statement_timeout = {}; SET LOCAL lock_timeout = 1000; SET LOCAL idle_in_transaction_session_timeout = 5000; SET LOCAL search_path = pg_catalog; SET LOCAL timezone = 'UTC'; SET LOCAL datestyle = 'ISO, YMD'",budget.timeout_ms).as_str()).await?;
                let description = c.describe(safe.as_str()).await?;
                let columns: Vec<Column> = description.columns().iter().map(|c|Column{name:c.name().into(),data_type:c.type_info().name().into(),nullable:true,encoding:"json; all numeric tokens encoded as decimal strings; timestamps UTC".into(),namespace:None,unit:None}).collect();
                if columns.iter().map(|c|&c.name).collect::<std::collections::BTreeSet<_>>().len()!=columns.len(){return Err(SourceError::Invalid("duplicate_columns"));}
                sink.columns(columns.clone()).await?;
                let snapshot: String = sqlx::query_scalar("SELECT pg_current_snapshot()::text").fetch_one(&mut c).await?;
                let wrapped = format!("SELECT CASE WHEN octet_length(row_to_json(__export)::text) <= {} THEN row_to_json(__export)::text ELSE NULL END AS row FROM ({safe}) AS __export LIMIT {}",budget.max_page_bytes,budget.max_rows.saturating_add(1));
                let mut statement=sqlx::query(&wrapped);
                for i in 1..=parameters.len() { let value=parameters.get(&format!("p{i}")).ok_or(SourceError::Invalid("parameter_sequence"))?; statement=statement.bind(value.as_str()); }
                let mut rows = statement.fetch(&mut c);
                while let Some(row) = rows.try_next().await? {
                    let raw: Option<String> = row.get(0);
                    let raw=raw.ok_or(SourceError::Limit)?;
                    if raw.len() > budget.max_page_bytes {return Err(SourceError::Limit);}
                    let mut value: Value = serde_json::from_str(&raw).map_err(|_|SourceError::Response)?;
                    lossless_numbers(&mut value);
                    sink.row(value).await?;
                }
                drop(rows);
                c.execute("COMMIT").await?;
                Ok(ReadSummary {columns,snapshot:Some(snapshot),..Default::default()})
            }).await.map_err(|_|SourceError::Timeout)?
        }).await
    }
}
