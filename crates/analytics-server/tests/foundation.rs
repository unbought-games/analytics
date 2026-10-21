use analytics_core::*;
use analytics_server::{
    MIGRATOR, admin, auth::DatabaseAuthorization, bootstrap, oidc::OidcVerifier,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rsa::{RsaPrivateKey, pkcs8::EncodePrivateKey, traits::PublicKeyParts};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

async fn request(
    state: &AppState,
    actor: &AuthContext,
    kind: &str,
    op: &str,
    key: Option<String>,
    value: Option<Value>,
) -> ApiResult<Value> {
    admin::execute(
        state,
        actor,
        AdminRequest {
            resource_kind: kind.into(),
            operation: op.into(),
            resource_id: key,
            value,
        },
    )
    .await
}
fn grant(action: &str, resource: Resource) -> Grant {
    Grant {
        action: action.into(),
        resource,
    }
}
async fn add_principal(
    state: &AppState,
    actor: &AuthContext,
    kind: &str,
    grants: Vec<Grant>,
) -> (Uuid, String) {
    let id = Uuid::new_v4();
    request(state,actor,"principals","put",Some(id.to_string()),Some(json!({"id":id,"name":"synthetic principal","kind":kind,"enabled":true,"delegated_by":null}))).await.unwrap();
    request(
        state,
        actor,
        "grants",
        "put",
        Some(id.to_string()),
        Some(json!(grants)),
    )
    .await
    .unwrap();
    let issued=request(state,actor,"credentials","issue",None,Some(json!({"principal_id":id,"name":"test","expires_at":Utc::now()+Duration::hours(1),"grants":grants}))).await.unwrap();
    (id, issued["token"].as_str().unwrap().into())
}
#[tokio::test]
async fn postgres_foundation_security_and_protocol() {
    let dsn=std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL must point to a disposable PostgreSQL database; this test creates a private schema");
    let root = sqlx::PgPool::connect(&dsn).await.unwrap();
    let schema = format!("foundation_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&root)
        .await
        .unwrap();
    let search = schema.clone();
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(12)
        .after_connect(move |conn, _| {
            let query = format!("SET search_path TO {search}");
            Box::pin(async move {
                sqlx::query(&query).execute(conn).await?;
                Ok(())
            })
        })
        .connect(&dsn)
        .await
        .unwrap();
    MIGRATOR.run(&db).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let occupied = dir.path().join("occupied.json");
    std::fs::write(&occupied, "unrelated existing content").unwrap();
    assert!(
        bootstrap::bootstrap(&db, "owner", "agent", &occupied, 30)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(&occupied).unwrap(),
        "unrelated existing content"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM installation")
            .fetch_one(&db)
            .await
            .unwrap(),
        0
    );
    let a = dir.path().join("a.json");
    let b = dir.path().join("b.json");
    let (first, second) = tokio::join!(
        bootstrap::bootstrap(&db, "synthetic owner", "synthetic agent", &a, 30),
        bootstrap::bootstrap(&db, "synthetic owner", "synthetic agent", &b, 30)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let handoff = if first.is_ok() { &a } else { &b };
    let content = std::fs::read_to_string(handoff).unwrap();
    let value: Value = serde_json::from_str(&content).unwrap();
    let token = value["credential"]["token"].as_str().unwrap();
    let owner: Uuid = serde_json::from_value(value["owner_id"].clone()).unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM principals")
            .fetch_one(&db)
            .await
            .unwrap(),
        2
    );
    assert!(
        bootstrap::bootstrap(&db, "other", "other", &dir.path().join("again"), 30)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(handoff).unwrap(), content);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(handoff).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let auth = Arc::new(DatabaseAuthorization {
        db: db.clone(),
        oidc: None,
    });
    let state = AppState {
        db: db.clone(),
        auth: auth.clone(),
        config: Arc::new(ServiceConfig {
            public_base_url: "http://127.0.0.1:8080".into(),
            export_directory: dir.path().join("exports"),
            secret_directory: dir.path().join("secrets"),
            max_body_bytes: 1024 * 1024,
        }),
    };
    let actor = auth.authenticate(token).await.unwrap();
    let property = Uuid::new_v4();
    let other = Uuid::new_v4();
    let policy = json!({"profile":"minimal","version":1,"effective_at":Utc::now(),"cross_property":false,"event_retention_days":30,"identity_retention_days":190,"attribution_window_days":30,"allowed_query_keys":["utm_campaign"],"respect_global_privacy_control":true});
    for id in [property, other] {
        request(&state,&actor,"properties","put",Some(id.to_string()),Some(json!({"id":id,"name":"Synthetic property","allowed_origins":["https://web.example"],"policy":policy,"handoff_audiences":[],"enabled":true}))).await.unwrap();
    }
    let grants = vec![
        grant("properties:read", Resource::property(property)),
        grant("properties:manage", Resource::property(property)),
    ];
    let (human_id, human_token) = add_principal(&state, &actor, "human", grants.clone()).await;
    let (agent_id, agent_token) = add_principal(&state, &actor, "agent", grants.clone()).await;
    let human = auth.authenticate(&human_token).await.unwrap();
    let agent = auth.authenticate(&agent_token).await.unwrap();
    for p in [&human, &agent] {
        let listed = request(&state, p, "properties", "list", None, None)
            .await
            .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["id"], json!(property));
        assert!(
            request(
                &state,
                p,
                "properties",
                "get",
                Some(other.to_string()),
                None
            )
            .await
            .is_err()
        );
        assert!(
            request(
                &state,
                p,
                "grants",
                "put",
                Some(p.principal_id.to_string()),
                Some(json!([grant("*", Resource::installation())]))
            )
            .await
            .is_err()
        );
    }
    // Scoped management cannot mint capabilities above the caller's current authority.
    let manager_grants = vec![
        grant(
            "credentials:manage",
            Resource {
                kind: ResourceKind::Principal,
                id: agent_id.to_string(),
            },
        ),
        grant(
            "grants:manage",
            Resource {
                kind: ResourceKind::Principal,
                id: agent_id.to_string(),
            },
        ),
        grant(
            "principals:manage",
            Resource {
                kind: ResourceKind::Principal,
                id: owner.to_string(),
            },
        ),
        grant("properties:read", Resource::property(property)),
    ];
    let (_, manager_token) = add_principal(&state, &actor, "agent", manager_grants).await;
    let manager = auth.authenticate(&manager_token).await.unwrap();
    assert!(request(&state,&manager,"credentials","issue",None,Some(json!({"principal_id":agent_id,"name":"escalation","expires_at":Utc::now()+Duration::hours(1),"grants":[grant("*",Resource::installation())]}))).await.is_err());
    assert!(request(&state,&manager,"oidc_bindings","put",Some(owner.to_string()),Some(json!({"principal_id":owner,"issuer":"https://identity.example","subject":"attacker"}))).await.is_err());
    // A narrowed token of an owner cannot recover its broader principal grants.
    let narrowed=request(&state,&actor,"credentials","issue",None,Some(json!({"principal_id":actor.principal_id,"name":"narrow","expires_at":Utc::now()+Duration::hours(1),"grants":[grant("properties:read",Resource::property(property))]}))).await.unwrap();
    let narrow = auth
        .authenticate(narrowed["token"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        auth.grants(&narrow).await.unwrap(),
        vec![grant("properties:read", Resource::property(property))]
    );
    assert!(
        auth.authorize(&narrow, "properties:read", &Resource::property(other))
            .await
            .is_err()
    );
    // Action and resource intersections are independent dimensions.
    let (cross_id, cross_token) = add_principal(
        &state,
        &actor,
        "agent",
        vec![grant("*", Resource::installation())],
    )
    .await;
    let cross_actor = auth.authenticate(&cross_token).await.unwrap();
    let cross_credential=request(&state,&actor,"credentials","issue",None,Some(json!({"principal_id":cross_id,"name":"cross dimensions","expires_at":Utc::now()+Duration::hours(1),"grants":[grant("query:read",Resource::installation())]}))).await.unwrap();
    request(
        &state,
        &actor,
        "grants",
        "put",
        Some(cross_id.to_string()),
        Some(json!([grant("*", Resource::source(property))])),
    )
    .await
    .unwrap();
    let cross = auth
        .authenticate(cross_credential["token"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(
        auth.grants(&cross).await.unwrap(),
        vec![grant("query:read", Resource::source(property))]
    );
    auth.authorize(&cross, "query:read", &Resource::source(property))
        .await
        .unwrap();
    assert!(
        auth.authorize(&cross, "query:read", &Resource::source(other))
            .await
            .is_err()
    );
    // Delegated agents retain separate attribution and lose access when parent grants disappear.
    let delegated_id = Uuid::new_v4();
    request(&state,&actor,"principals","put",Some(delegated_id.to_string()),Some(json!({"id":delegated_id,"name":"delegated synthetic agent","kind":"agent","enabled":true,"delegated_by":cross_id}))).await.unwrap();
    request(
        &state,
        &actor,
        "grants",
        "put",
        Some(delegated_id.to_string()),
        Some(json!([grant("*", Resource::installation())])),
    )
    .await
    .unwrap();
    let delegated_token=request(&state,&actor,"credentials","issue",None,Some(json!({"principal_id":delegated_id,"name":"delegated","expires_at":Utc::now()+Duration::hours(1),"grants":[grant("*",Resource::installation())]}))).await.unwrap();
    let delegated = auth
        .authenticate(delegated_token["token"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(delegated.delegated_by, Some(cross_id));
    auth.authorize(&delegated, "query:read", &Resource::source(property))
        .await
        .unwrap();
    assert!(
        auth.authorize(&delegated, "query:read", &Resource::source(other))
            .await
            .is_err()
    );
    request(
        &state,
        &actor,
        "grants",
        "delete",
        Some(cross_id.to_string()),
        None,
    )
    .await
    .unwrap();
    assert!(
        auth.authorize(&delegated, "query:read", &Resource::source(property))
            .await
            .is_err()
    );
    assert!(
        auth.authorize(&cross_actor, "query:read", &Resource::source(property))
            .await
            .is_err()
    );
    assert!(
        request(
            &state,
            &delegated,
            "properties",
            "get",
            Some(property.to_string()),
            None
        )
        .await
        .is_err()
    );
    // Administrative source validation never permits inline connection secrets.
    let source_id = Uuid::new_v4();
    let mut source = json!({"id":source_id,"name":"synthetic source","kind":"postgres","environment":"test","endpoint":"postgresql://db.example/readonly","secret_ref":"source-dsn","allowed_schemas":["public"],"allowed_tables":["public.activity"],"allowed_columns":{},"allowed_projects":[],"budget":{"timeout_ms":1000,"max_rows":100,"max_bytes":10000,"max_concurrency":1},"semantics":{},"enabled":true});
    request(
        &state,
        &actor,
        "sources",
        "put",
        Some(source_id.to_string()),
        Some(source.clone()),
    )
    .await
    .unwrap();
    let (_, source_manager_token) = add_principal(
        &state,
        &actor,
        "agent",
        vec![
            grant("sources:manage", Resource::source(source_id)),
            grant("sources:read", Resource::source(source_id)),
        ],
    )
    .await;
    let source_manager = auth.authenticate(&source_manager_token).await.unwrap();
    let mut label_only = source.clone();
    label_only["name"] = json!("renamed synthetic source");
    request(
        &state,
        &source_manager,
        "sources",
        "put",
        Some(source_id.to_string()),
        Some(label_only),
    )
    .await
    .unwrap();
    for (field, replacement) in [
        ("endpoint", json!("https://controlled.example")),
        ("secret_ref", json!("other-source-secret")),
        ("kind", json!("prometheus")),
    ] {
        let mut changed = source.clone();
        changed[field] = replacement;
        assert!(
            request(
                &state,
                &source_manager,
                "sources",
                "put",
                Some(source_id.to_string()),
                Some(changed)
            )
            .await
            .is_err(),
            "scoped source manager changed {field}"
        );
    }
    let old_source_revision: i64 =
        sqlx::query_scalar("SELECT revision FROM admin_objects WHERE kind='sources' AND id=$1")
            .bind(source_id.to_string())
            .fetch_one(&db)
            .await
            .unwrap();
    request(
        &state,
        &actor,
        "sources",
        "delete",
        Some(source_id.to_string()),
        None,
    )
    .await
    .unwrap();
    assert!(
        request(
            &state,
            &source_manager,
            "sources",
            "put",
            Some(source_id.to_string()),
            Some(source.clone())
        )
        .await
        .is_err()
    );
    request(
        &state,
        &actor,
        "sources",
        "put",
        Some(source_id.to_string()),
        Some(source.clone()),
    )
    .await
    .unwrap();
    let recreated_revision: i64 =
        sqlx::query_scalar("SELECT revision FROM admin_objects WHERE kind='sources' AND id=$1")
            .bind(source_id.to_string())
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(
        recreated_revision > old_source_revision,
        "delete/recreate must not revive old export generation"
    );
    let mut inline_secret_url = url::Url::parse("postgresql://db.example/readonly").unwrap();
    inline_secret_url.set_username("synthetic-user").unwrap();
    inline_secret_url
        .set_password(Some(&Uuid::new_v4().to_string()))
        .unwrap();
    source["endpoint"] = json!(inline_secret_url.as_str());
    assert!(
        request(
            &state,
            &actor,
            "sources",
            "put",
            Some(source_id.to_string()),
            Some(source)
        )
        .await
        .is_err()
    );
    let schema_id = format!("{property}:action:1");
    let mut schema_value = json!({"property_id":property,"name":"action","version":1,"authoritative":false,"category":"interaction","properties":{"control":{"kind":"string","required":true,"max_length":40,"unit":null}}});
    request(
        &state,
        &actor,
        "schemas",
        "put",
        Some(schema_id.clone()),
        Some(schema_value.clone()),
    )
    .await
    .unwrap();
    schema_value["properties"]["control"]["max_length"] = json!(50);
    assert!(
        request(
            &state,
            &actor,
            "schemas",
            "put",
            Some(schema_id),
            Some(schema_value)
        )
        .await
        .is_err()
    );
    let definition_id = Uuid::new_v4();
    let mut definition = json!({"id":definition_id,"name":"synthetic definition","version":"1","description":"synthetic test","query":{"source_id":source_id,"property_id":null,"kind":"postgres","query":{"sql":"SELECT 1"},"parameters":{},"window":null,"cutoff":Utc::now(),"definition_version":"1","budget":{"timeout_ms":1000,"max_rows":100,"max_bytes":10000,"max_concurrency":1}}});
    request(
        &state,
        &actor,
        "definitions",
        "put",
        Some(definition_id.to_string()),
        Some(definition.clone()),
    )
    .await
    .unwrap();
    definition["description"] = json!("changed without version");
    assert!(
        request(
            &state,
            &actor,
            "definitions",
            "put",
            Some(definition_id.to_string()),
            Some(definition.clone())
        )
        .await
        .is_err()
    );
    definition["version"] = json!("2");
    request(
        &state,
        &actor,
        "definitions",
        "put",
        Some(definition_id.to_string()),
        Some(definition.clone()),
    )
    .await
    .unwrap();
    let first_version: SavedDefinition =
        load_object(&db, "definition_versions", &format!("{definition_id}:1"))
            .await
            .unwrap();
    assert_eq!(first_version.description, "synthetic test");
    request(
        &state,
        &actor,
        "definitions",
        "delete",
        Some(definition_id.to_string()),
        None,
    )
    .await
    .unwrap();
    assert!(
        load_object::<SavedDefinition>(&db, "definitions", &definition_id.to_string())
            .await
            .is_err()
    );
    definition["version"] = json!("3");
    definition["query"]["source_id"] = json!(Uuid::new_v4());
    assert!(
        request(
            &state,
            &actor,
            "definitions",
            "put",
            Some(definition_id.to_string()),
            Some(definition)
        )
        .await
        .is_err()
    );
    // Real remote HTTP + MCP requests share mutation and read paths.
    let app = analytics_server::router(state.clone(), vec![], None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{address}");
    let client = reqwest::Client::new();
    let unauth=client.post(format!("{base}/mcp")).json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":MCP_PROTOCOL_VERSION}})).send().await.unwrap();
    assert_eq!(unauth.status(), 401);
    assert!(
        unauth.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("oauth-protected-resource")
    );
    let initialized:Value=client.post(format!("{base}/mcp")).bearer_auth(token).header("Accept","application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":MCP_PROTOCOL_VERSION}})).send().await.unwrap().json().await.unwrap();
    assert_eq!(
        initialized["result"]["protocolVersion"],
        MCP_PROTOCOL_VERSION
    );
    for t in [&human_token, &agent_token] {
        let result:Value=client.post(format!("{base}/mcp")).bearer_auth(t).json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"admin_execute","arguments":{"operation":"get","resource_kind":"properties","resource_id":property}}})).send().await.unwrap().json().await.unwrap();
        assert_eq!(result["result"]["structuredContent"]["id"], json!(property));
    }
    assert_eq!(
        client
            .post(format!("{base}/mcp"))
            .bearer_auth(token)
            .header("Origin", "https://evil.example")
            .json(&json!({"jsonrpc":"2.0","id":3,"method":"ping"}))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    // A mutation through MCP is visible to HTTP using the same dispatcher.
    let mut changed_policy = policy.clone();
    changed_policy["version"] = json!(2);
    let mutation:Value=client.post(format!("{base}/mcp")).bearer_auth(token).json(&json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"admin_execute","arguments":{"operation":"put","resource_kind":"policies","resource_id":property,"value":changed_policy}}})).send().await.unwrap().json().await.unwrap();
    assert_eq!(mutation["result"]["isError"], false);
    let via_http: Value = client
        .post(format!("{base}/v1/admin/execute"))
        .bearer_auth(token)
        .json(&json!({"operation":"get","resource_kind":"policies","resource_id":property}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(via_http["version"], 2);
    let listed = request(
        &state,
        &actor,
        "credentials",
        "list",
        Some(actor.principal_id.to_string()),
        None,
    )
    .await
    .unwrap()
    .to_string();
    assert!(!listed.contains(token));
    assert!(!listed.contains("verifier"));
    // Rotation is atomic and revocation applies to an actor already held by queued work.
    let rotated=request(&state,&actor,"credentials","rotate",Some(agent.credential_id.unwrap().to_string()),Some(json!({"principal_id":agent_id,"name":"rotated","expires_at":Utc::now()+Duration::hours(1),"grants":grants}))).await.unwrap();
    assert!(auth.authenticate(&agent_token).await.is_err());
    assert!(
        auth.authorize(&agent, "properties:read", &Resource::property(property))
            .await
            .is_err()
    );
    assert!(
        auth.authenticate(rotated["token"].as_str().unwrap())
            .await
            .is_ok()
    );
    assert_eq!(
        client
            .get(format!("{base}/v1/me"))
            .bearer_auth(&agent_token)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    // Controlled OIDC fixture serves a generated test-only RSA key and discovery metadata.
    let key = RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
    let public = key.to_public_key();
    let pem = key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap();
    let signing = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
    let oidc_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", oidc_listener.local_addr().unwrap());
    let issuer_for_route = issuer.clone();
    let jwks = json!({"keys":[{"kty":"RSA","kid":"fixture-key","use":"sig","alg":"RS256","n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),"e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())}]});
    let fixture=axum::Router::new().route("/.well-known/openid-configuration",axum::routing::get(move||{let issuer=issuer_for_route.clone();async move{axum::Json(json!({"issuer":issuer,"jwks_uri":format!("{issuer}/jwks"),"authorization_endpoint":format!("{issuer}/authorize"),"token_endpoint":format!("{issuer}/token"),"code_challenge_methods_supported":["S256"]}))}})).route("/jwks",axum::routing::get(move||{let keys=jwks.clone();async move{axum::Json(keys)}}));
    let fixture_task =
        tokio::spawn(async move { axum::serve(oidc_listener, fixture).await.unwrap() });
    let audience = "https://analytics.example/mcp";
    let oidc = OidcVerifier::discover(issuer.clone(), audience.into())
        .await
        .unwrap();
    let oidc_auth = DatabaseAuthorization {
        db: db.clone(),
        oidc: Some(oidc),
    };
    request(
        &state,
        &actor,
        "oidc_bindings",
        "put",
        Some(human_id.to_string()),
        Some(json!({"principal_id":human_id,"issuer":issuer,"subject":"synthetic-human-subject"})),
    )
    .await
    .unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("fixture-key".into());
    header.typ = Some("at+jwt".into());
    let claims = json!({"iss":issuer,"aud":audience,"sub":"synthetic-human-subject","exp":(Utc::now()+Duration::minutes(10)).timestamp(),"iat":Utc::now().timestamp(),"client_id":"synthetic-client","jti":Uuid::new_v4().to_string()});
    let jwt = encode(&header, &claims, &signing).unwrap();
    let mut tampered = jwt.clone().into_bytes();
    let signature_start = jwt.rfind('.').unwrap() + 1;
    tampered[signature_start] = if tampered[signature_start] == b'A' {
        b'B'
    } else {
        b'A'
    };
    assert!(
        oidc_auth
            .authenticate(&String::from_utf8(tampered).unwrap())
            .await
            .is_err()
    );
    let external = oidc_auth.authenticate(&jwt).await.unwrap();
    assert_eq!(external.principal_id, human_id);
    oidc_auth
        .authorize(&external, "properties:read", &Resource::property(property))
        .await
        .unwrap();
    for (field, v) in [
        ("iss", json!("https://wrong.example")),
        ("aud", json!("https://other.example/mcp")),
        (
            "exp",
            json!((Utc::now() - Duration::seconds(1)).timestamp()),
        ),
        ("sub", json!("unbound")),
        ("nbf", json!((Utc::now() + Duration::hours(1)).timestamp())),
    ] {
        let mut bad = claims.clone();
        bad[field] = v;
        assert!(
            oidc_auth
                .authenticate(&encode(&header, &bad, &signing).unwrap())
                .await
                .is_err(),
            "accepted invalid {field}"
        );
    }
    header.typ = Some("JWT".into());
    assert!(
        oidc_auth
            .authenticate(&encode(&header, &claims, &signing).unwrap())
            .await
            .is_err()
    );
    request(
        &state,
        &actor,
        "credentials",
        "revoke",
        Some(external.credential_id.unwrap().to_string()),
        None,
    )
    .await
    .unwrap();
    assert!(oidc_auth.authenticate(&jwt).await.is_err());
    assert!(
        oidc_auth
            .authorize(&external, "properties:read", &Resource::property(property))
            .await
            .is_err()
    );
    let mut fresh_claims = claims.clone();
    fresh_claims["jti"] = json!(Uuid::new_v4().to_string());
    header.typ = Some("at+jwt".into());
    let fresh_jwt = encode(&header, &fresh_claims, &signing).unwrap();
    let held_external = oidc_auth.authenticate(&fresh_jwt).await.unwrap();
    request(
        &state,
        &actor,
        "oidc_bindings",
        "delete",
        Some(human_id.to_string()),
        None,
    )
    .await
    .unwrap();
    assert!(oidc_auth.authenticate(&fresh_jwt).await.is_err());
    assert!(
        oidc_auth
            .authorize(
                &held_external,
                "properties:read",
                &Resource::property(property)
            )
            .await
            .is_err()
    );
    // An expired local verifier blocks both reauthentication and held jobs.
    sqlx::query("UPDATE credentials SET expires_at=now()-interval '1 second' WHERE id=$1")
        .bind(human.credential_id)
        .execute(&db)
        .await
        .unwrap();
    assert!(auth.authenticate(&human_token).await.is_err());
    assert!(
        auth.authorize(&human, "properties:read", &Resource::property(property))
            .await
            .is_err()
    );
    // Local recovery is explicit, revokes all prior credentials and OIDC bindings, and audits.
    let recovery = dir.path().join("recovery.json");
    assert!(
        bootstrap::recover(&db, owner, &recovery, 30, false)
            .await
            .is_err()
    );
    bootstrap::recover(&db, owner, &recovery, 30, true)
        .await
        .unwrap();
    assert!(auth.authenticate(token).await.is_err());
    assert!(oidc_auth.authenticate(&jwt).await.is_err());
    let recovered: Value =
        serde_json::from_str(&std::fs::read_to_string(recovery).unwrap()).unwrap();
    assert!(
        auth.authenticate(recovered["credential"]["token"].as_str().unwrap())
            .await
            .is_ok()
    );
    let audits: Vec<Value> =
        sqlx::query_scalar("SELECT to_jsonb(audit_records) FROM audit_records")
            .fetch_all(&db)
            .await
            .unwrap();
    let serialized = serde_json::to_string(&audits).unwrap();
    assert!(!serialized.contains(token));
    assert!(!serialized.contains(&jwt));
    assert!(serialized.contains("local_recovery:revoke_all"));
    assert!(
        audits
            .iter()
            .any(|r| r["principal_id"] == json!(actor.principal_id)
                && r["action"] == "credentials:rotate")
    );
    assert!(
        audits
            .iter()
            .any(|r| r["principal_id"] == json!(delegated_id)
                && r["delegated_by"] == json!(cross_id))
    );
    server.abort();
    fixture_task.abort();
    db.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&root)
        .await
        .unwrap();
    root.close().await;
}
