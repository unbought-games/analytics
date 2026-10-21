use analytics_core::*;
use serde_json::{Value, json};
fn main() {
    let mut schemas = serde_json::Map::new();
    macro_rules! add { ($($t:ty),*)=>{$(let value=serde_json::to_value(schemars::schema_for!($t)).unwrap();schemas.insert(stringify!($t).into(),value);)*}; }
    add!(
        EventEnvelope,
        IngestBatch,
        IngestAcknowledgement,
        CollectionPolicy,
        PropertyDefinition,
        EventSchemaDefinition,
        HandoffRequest,
        HandoffResponse,
        RedeemHandoffRequest,
        IdentityLinkRequest,
        IdentityResetRequest,
        WithdrawalRequest,
        DeletionRequest,
        SourceDefinition,
        QueryRequest,
        SavedDefinition,
        ExportRequest,
        ExportStatus,
        ExportManifest,
        PrincipalDefinition,
        OidcBinding,
        CredentialRequest,
        CredentialCreated,
        AdminRequest
    );
    // Hoist definitions to OpenAPI components and rewrite JSON schema references.
    let mut extras = serde_json::Map::new();
    for schema in schemas.values_mut() {
        if let Some(defs) = schema.as_object_mut().unwrap().remove("$defs") {
            extras.extend(defs.as_object().unwrap().clone());
        }
        schema.as_object_mut().unwrap().remove("$schema");
    }
    schemas.extend(extras);
    let mut paths = serde_json::Map::new();
    for (path, request, response, public) in [
        (
            "/v1/ingest/browser",
            "IngestBatch",
            "IngestAcknowledgement",
            true,
        ),
        (
            "/v1/ingest/server",
            "IngestBatch",
            "IngestAcknowledgement",
            false,
        ),
        (
            "/v1/journey/handoff",
            "HandoffRequest",
            "HandoffResponse",
            true,
        ),
        (
            "/v1/journey/redeem",
            "RedeemHandoffRequest",
            "HandoffResponse",
            false,
        ),
        ("/v1/identity/link", "IdentityLinkRequest", "", false),
        ("/v1/identity/reset", "IdentityResetRequest", "", true),
        ("/v1/privacy/withdraw", "WithdrawalRequest", "", false),
        ("/v1/privacy/delete", "DeletionRequest", "", false),
        ("/v1/query", "QueryRequest", "ExportStatus", false),
        ("/v1/exports", "ExportRequest", "ExportStatus", false),
        ("/v1/admin/execute", "AdminRequest", "", false),
    ] {
        let mut op = json!({"operationId":path.replace('/','_'.to_string().as_str()),"responses":{"200":{"description":"Success"},"400":{"description":"Invalid request"},"401":{"description":"Invalid credential"},"403":{"description":"Permission denied"}},"requestBody":{"required":true,"content":{"application/json":{"schema":{"$ref":format!("#/components/schemas/{request}")}}}},"security":[{"bearerAuth":[]}]});
        if !response.is_empty() {
            op["responses"]["200"]["content"] = json!({"application/json":{"schema":{"$ref":format!("#/components/schemas/{response}")}}});
        }
        if public {
            op["security"] = json!([{"propertyToken":[]}]);
        }
        paths.insert(path.into(), json!({"post":op}));
    }
    for path in [
        "/healthz",
        "/readyz",
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-authorization-server",
        "/v1/me",
        "/v1/admin/status",
        "/v1/sources",
        "/v1/sources/{id}",
        "/v1/exports/{id}",
        "/v1/exports/{id}/manifest",
        "/v1/exports/{id}/files/{name}",
    ] {
        let mut op = json!({"responses":{"200":{"description":"Success"}}});
        if path.starts_with("/v1/") {
            op["security"] = json!([{"bearerAuth":[]}]);
        }
        let params:Vec<Value>=path.split('/').filter(|s|s.starts_with('{')).map(|s|json!({"name":s.trim_matches(['{','}']),"in":"path","required":true,"schema":{"type":"string"}})).collect();
        op["parameters"] = json!(params);
        paths.insert(path.into(), json!({"get":op}));
    }
    paths.insert("/mcp".into(),json!({"post":{"description":"MCP 2025-11-25 Streamable HTTP; JSON-RPC initialize, ping, tools/list, tools/call. Stateless JSON responses.","security":[{"bearerAuth":[]}],"responses":{"200":{"description":"JSON-RPC response"},"202":{"description":"Notification accepted"},"401":{"description":"OAuth resource metadata challenge"}}}}));
    let value = json!({"openapi":"3.1.0","info":{"title":"Analytics Engine","version":"1.0.0"},"paths":paths,"components":{"schemas":schemas,"securitySchemes":{"bearerAuth":{"type":"http","scheme":"bearer"},"propertyToken":{"type":"apiKey","in":"header","name":"X-Property-Token"}}}});
    println!(
        "{}",
        serde_json::to_string_pretty(&value)
            .unwrap()
            .replace("#/$defs/", "#/components/schemas/")
    );
}
