//! Additive OpenAPI fragment; merge with the foundation contract.
use analytics_core::*;
use schemars::JsonSchema;
use serde_json::{Map, Value, json};
fn rewrite(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, v) in map {
                if key == "$ref" {
                    if let Some(name) = v.as_str().and_then(|s| s.strip_prefix("#/$defs/")) {
                        *v = json!(format!("#/components/schemas/{name}"));
                    }
                } else {
                    rewrite(v)
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(rewrite),
        _ => (),
    }
}
fn schema<T: JsonSchema>(components: &mut Map<String, Value>) -> Value {
    let mut value = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    rewrite(&mut value);
    if let Some(defs) = value
        .as_object_mut()
        .and_then(|o| o.remove("$defs"))
        .and_then(|v| v.as_object().cloned())
    {
        components.extend(defs);
    }
    let name = std::any::type_name::<T>().rsplit("::").next().unwrap();
    value.as_object_mut().unwrap().remove("$schema");
    components.insert(name.into(), value);
    json!({"$ref":format!("#/components/schemas/{name}")})
}
fn operation(output: Value, request: Option<Value>, id: bool) -> Value {
    let mut op = json!({"security":[{"bearerAuth":[]}],"responses":{"200":{"description":"Authorized analytical result","content":{"application/json":{"schema":output}}},"400":{"description":"Invalid query"},"401":{"description":"Invalid credential"},"403":{"description":"Scope or credential denied"},"409":{"description":"Export not ready"},"410":{"description":"Artifact expired or cancelled"},"429":{"description":"Capacity exhausted"}}});
    if id {
        op["parameters"] = json!([{"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}}]);
    }
    if let Some(request) = request {
        op["requestBody"] =
            json!({"required":true,"content":{"application/json":{"schema":request}}});
    }
    op
}
fn main() {
    let mut components = Map::new();
    let request = schema::<ExportRequest>(&mut components);
    let status = schema::<ExportStatus>(&mut components);
    let manifest = schema::<ExportManifest>(&mut components);
    let definition = schema::<SavedDefinition>(&mut components);
    let mut paths = json!({});
    paths["/v1/exports"] = json!({"post":operation(status.clone(),Some(request),false)});
    paths["/v1/exports/{id}"] = json!({"get":operation(status.clone(),None,true),"delete":operation(json!({"type":"object"}),None,true)});
    paths["/v1/exports/{id}/cancel"] = json!({"post":operation(status,None,true)});
    paths["/v1/exports/{id}/manifest"] = json!({"get":operation(manifest,None,true)});
    paths["/v1/exports/{id}/files/{file}"] = json!({"get":{"security":[{"bearerAuth":[]}],"parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string","format":"uuid"}},{"name":"file","in":"path","required":true,"schema":{"const":"data.jsonl"}}],"responses":{"200":{"description":"Authenticated JSONL stream; access rechecked for every chunk","content":{"application/x-ndjson":{"schema":{"type":"string","format":"binary"}}}},"401":{"description":"Invalid credential"},"403":{"description":"Access revoked"},"410":{"description":"Expired or cancelled"}}}});
    for path in ["/v1/sources/{id}/discover", "/v1/properties/{id}/analytics"] {
        paths[path] = json!({"get":operation(json!({"type":"object"}),None,true)});
    }
    paths["/v1/query/definitions"] =
        json!({"post":operation(definition.clone(),Some(definition),false)});
    paths["/v1/query/reproduce"] = json!({"post":operation(schema::<ExportStatus>(&mut components),Some(json!({"type":"object","required":["id","version","cutoff"],"properties":{"id":{"type":"string","format":"uuid"},"version":{"type":"string"},"cutoff":{"type":"string","format":"date-time"},"retention_hours":{"type":"integer","default":24}},"additionalProperties":false})),false)});
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"paths":paths,"components":{"schemas":components}}))
            .unwrap()
    );
}
