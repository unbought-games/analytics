//! Additive OpenAPI fragment; merge paths with the foundation-generated contract.
use analytics_core::*;
use analytics_ingest::*;
use schemars::JsonSchema;
use serde_json::{Value, json};
fn rewrite(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "$ref" {
                    if let Some(reference) = value.as_str()
                        && let Some(name) = reference.strip_prefix("#/$defs/")
                    {
                        *value = json!(format!("#/components/schemas/{name}"));
                    }
                } else {
                    rewrite(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite(item);
            }
        }
        _ => {}
    }
}
fn schema<T: JsonSchema>(components: &mut serde_json::Map<String, Value>) -> Value {
    let mut value = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    rewrite(&mut value);
    if let Some(definitions) = value
        .as_object_mut()
        .and_then(|v| v.remove("$defs"))
        .and_then(|v| v.as_object().cloned())
    {
        components.extend(definitions);
    }
    let name = std::any::type_name::<T>()
        .rsplit("::")
        .next()
        .unwrap()
        .to_owned();
    if let Some(object) = value.as_object_mut() {
        object.remove("$schema");
    }
    components.insert(name.clone(), value);
    json!({"$ref":format!("#/components/schemas/{name}")})
}
fn operation<T: JsonSchema, R: JsonSchema>(
    public: bool,
    components: &mut serde_json::Map<String, Value>,
) -> Value {
    json!({"security":[if public {json!({"PropertyToken":[]})}else{json!({"bearerAuth":[]})}],"parameters":if public {vec![json!({"name":"Origin","in":"header","required":true,"schema":{"type":"string","format":"uri"}})]}else{vec![]},"requestBody":{"required":true,"content":{"application/json":{"schema":schema::<T>(components)}}},"responses":{"200":{"description":"Committed operation result","content":{"application/json":{"schema":schema::<R>(components)}}},"400":{"description":"Invalid input"},"401":{"description":"Missing/invalid credential"},"403":{"description":"Scope/origin/policy denied"},"429":{"description":"Rate limited"},"500":{"description":"Not acknowledged; safe to retry stable event IDs"}}})
}
fn main() {
    let mut paths = json!({});
    let mut components = serde_json::Map::new();
    macro_rules! route {($path:literal,$input:ty,$output:ty,$public:literal)=>{paths[$path]=json!({"post":operation::<$input,$output>($public,&mut components)});}}
    route!(
        "/v1/ingest/browser",
        IngestBatch,
        IngestAcknowledgement,
        true
    );
    route!(
        "/v1/ingest/server",
        IngestBatch,
        IngestAcknowledgement,
        false
    );
    route!("/v1/ingest/token", TokenRequest, Value, false);
    route!("/v1/ingest/token/revoke", RevokeTokenRequest, Value, false);
    route!("/v1/journey/handoff", HandoffRequest, HandoffResponse, true);
    route!("/v1/journey/redeem", RedeemHandoffRequest, Value, false);
    route!("/v1/identity/link", IdentityLinkRequest, Value, false);
    route!("/v1/identity/proof", IssueProofRequest, Value, false);
    route!("/v1/identity/identify", IdentifyRequest, Value, true);
    route!("/v1/identity/reset", BrowserChoiceRequest, Value, true);
    route!("/v1/privacy/withdraw", BrowserChoiceRequest, Value, true);
    route!(
        "/v1/privacy/withdraw-subject",
        WithdrawalRequest,
        Value,
        false
    );
    route!("/v1/privacy/delete", DeletionRequest, Value, false);
    route!("/v1/ingest/retention", PropertyRequest, Value, false);
    route!("/v1/attribution/correct", CorrectionRequest, Value, false);
    for (path, public) in [
        ("/v1/ingest/config", true),
        ("/v1/ingest/policy", true),
        ("/v1/ingest/status", false),
    ] {
        paths[path] = json!({"get":{"security":[if public {json!({"PropertyToken":[]})}else{json!({"bearerAuth":[]})}],"parameters":[{"name":"property_id","in":"query","required":true,"schema":{"type":"string","format":"uuid"}}],"responses":{"200":{"description":"Safe scoped configuration/status","content":{"application/json":{"schema":{"type":"object"}}}}}}});
    }
    println!("{}",serde_json::to_string_pretty(&json!({"paths":paths,"components":{"schemas":components,"securitySchemes":{"PropertyToken":{"type":"apiKey","in":"header","name":"X-Property-Token"}}}})).unwrap());
}
