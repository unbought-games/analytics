//! v1 wire contract. IDs are UUID strings; wall time is RFC3339 UTC; logical time is a decimal string.
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;
macro_rules! dto { ($($item:item)*) => {$ (#[derive(Clone,Debug,Serialize,Deserialize,JsonSchema)] #[serde(deny_unknown_fields)] $item)*} }
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollectionProfile {
    Minimal,
    Journey,
    Identified,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Producer {
    Browser,
    Server,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    Navigation,
    Interaction,
    Form,
    Visibility,
    Signup,
    Enrollment,
    HumanAction,
    Scheduled,
    System,
    Custom,
}
dto! {
 pub struct NamespacedId { pub namespace:String, pub id:String }
 pub struct Campaign { pub utm_source:Option<String>,pub utm_medium:Option<String>,pub utm_campaign:Option<String>,pub utm_term:Option<String>,pub utm_content:Option<String>,pub experiment_id:Option<String> }
 pub struct EventContext { pub environment:String,pub release:Option<String>,pub page:Option<String>,pub referrer:Option<String>,pub control:Option<String>,pub correlation_id:Option<String> }
 pub struct CollectionContext { pub profile:CollectionProfile,pub policy_version:i32,pub consent:bool,pub sample_rate:f64,pub dropped_events:u64 }
 pub struct EventEnvelope { pub event_id:Uuid,pub property_id:Uuid,pub name:String,pub schema_version:i32,pub producer:Producer,pub category:EventCategory,pub occurred_at:DateTime<Utc>,pub clock:String,pub logical_time:Option<String>,pub visit_id:Option<String>,pub visitor_id:Option<String>,pub subject:Option<NamespacedId>,pub context:EventContext,pub campaign:Option<Campaign>,pub touch_id:Option<Uuid>,pub attribution_rule:Option<String>,pub collection:CollectionContext,pub properties:BTreeMap<String,Value> }
 pub struct IngestBatch { pub events:Vec<EventEnvelope> }
 pub struct EventAcknowledgement { pub event_id:Uuid,pub status:String,pub code:Option<String>,pub receipt_cursor:Option<String> }
 pub struct IngestAcknowledgement { pub results:Vec<EventAcknowledgement>,pub committed_at:DateTime<Utc> }
 pub struct CollectionPolicy { pub profile:CollectionProfile,pub version:i32,pub effective_at:DateTime<Utc>,pub cross_property:bool,pub event_retention_days:i32,pub identity_retention_days:i32,pub attribution_window_days:i32,pub allowed_query_keys:Vec<String>,pub respect_global_privacy_control:bool }
 pub struct PropertyDefinition { pub id:Uuid,pub name:String,pub allowed_origins:Vec<String>,pub policy:CollectionPolicy,pub handoff_audiences:Vec<Uuid>,pub enabled:bool }
 pub struct EventSchemaDefinition { pub property_id:Uuid,pub name:String,pub version:i32,pub authoritative:bool,pub category:EventCategory,pub properties:BTreeMap<String,PropertyType> }
 pub struct PropertyType { pub kind:String,pub required:bool,pub max_length:Option<usize>,pub unit:Option<String> }
 pub struct HandoffRequest { pub property_id:Uuid,pub audience:Uuid,pub visit_id:Option<String>,pub visitor_id:Option<String>,pub touch_id:Option<Uuid> }
 pub struct HandoffResponse { pub handoff:String,pub expires_at:DateTime<Utc> }
 pub struct RedeemHandoffRequest { pub handoff:String,pub audience:Uuid,pub visit_id:Option<String>,pub visitor_id:Option<String> }
 pub struct IdentityLinkRequest { pub property_id:Uuid,pub subject:NamespacedId,pub target:NamespacedId,pub relationship:String,pub world:Option<String>,pub evidence_event_id:Uuid }
 pub struct IdentityResetRequest { pub property_id:Uuid,pub visit_id:Option<String>,pub visitor_id:Option<String> }
 pub struct WithdrawalRequest { pub property_id:Uuid,pub subject:Option<NamespacedId>,pub visitor_id:Option<String> }
 pub struct DeletionRequest { pub property_id:Uuid,pub subject:Option<NamespacedId>,pub visitor_id:Option<String>,pub reason:String }
 pub struct QueryBudget { pub timeout_ms:u64,pub max_rows:u64,pub max_bytes:u64,pub max_concurrency:u32 }
 pub struct SourceDefinition { pub id:Uuid,pub name:String,pub kind:String,pub environment:String,pub endpoint:String,pub secret_ref:Option<String>,pub allowed_schemas:Vec<String>,pub allowed_tables:Vec<String>,pub allowed_columns:BTreeMap<String,Vec<String>>,pub allowed_projects:Vec<String>,pub budget:QueryBudget,pub semantics:Value,pub enabled:bool }
 pub struct TimeWindow { pub start:DateTime<Utc>,pub end:DateTime<Utc> }
 pub struct QueryRequest { pub source_id:Option<Uuid>,pub property_id:Option<Uuid>,pub kind:String,pub query:Value,pub parameters:BTreeMap<String,Value>,pub window:Option<TimeWindow>,pub cutoff:DateTime<Utc>,pub definition_version:String,pub budget:QueryBudget }
 pub struct SavedDefinition { pub id:Uuid,pub name:String,pub version:String,pub query:QueryRequest,pub description:String }
 pub struct ExportRequest { pub query:QueryRequest,pub format:String,pub retention_hours:u32 }
 pub struct ExportStatus { pub id:Uuid,pub status:String,pub created_at:DateTime<Utc>,pub expires_at:DateTime<Utc>,pub manifest_url:Option<String>,pub error:Option<String> }
 pub struct ColumnDefinition { pub name:String,pub data_type:String,pub nullable:bool,pub unit:Option<String>,pub namespace:Option<String>,pub description:Option<String> }
 pub struct DatasetManifest { pub name:String,pub file_name:String,pub media_type:String,pub bytes:u64,pub sha256:String,pub row_count:u64,pub columns:Vec<ColumnDefinition>,pub download_url:String,pub ordering:Vec<String> }
 pub struct Coverage { pub complete:bool,pub history_start:Option<DateTime<Utc>>,pub history_end:Option<DateTime<Utc>>,pub limitations:Vec<String>,pub sample_rate:Option<f64>,pub policy_versions:Vec<i32> }
 pub struct ExportManifest { pub export_id:Uuid,pub format_version:String,pub status:String,pub datasets:Vec<DatasetManifest>,pub query:QueryRequest,pub created_at:DateTime<Utc>,pub expires_at:DateTime<Utc>,pub source_read_at:Vec<DateTime<Utc>>,pub watermark:Option<String>,pub coverage:Coverage,pub continuation:Option<Value>,pub provenance:Value }
 pub struct PrincipalDefinition { pub id:Uuid,pub name:String,pub kind:String,pub enabled:bool,pub delegated_by:Option<Uuid> }
 pub struct OidcBinding { pub principal_id:Uuid,pub issuer:String,pub subject:String }
 pub struct CredentialRequest { pub principal_id:Uuid,pub name:String,pub expires_at:DateTime<Utc>,pub grants:Vec<crate::Grant> }
 pub struct CredentialCreated { pub id:Uuid,pub token:String,pub expires_at:DateTime<Utc> }
 pub struct AdminRequest { pub operation:String,pub resource_kind:String,pub resource_id:Option<String>,pub value:Option<Value> }
}
