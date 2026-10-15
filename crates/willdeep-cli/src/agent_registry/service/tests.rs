use super::*;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use willdeep_core::agent_definition::AgentDefinition;

#[derive(Default)]
struct Database {
    item: Option<RemoteItem>,
    sequence: u32,
    put_keys: Vec<String>,
    after_list: Option<Box<dyn FnOnce() + Send>>,
    before_put: Option<RemoteItem>,
    versions: BTreeMap<(String, String), String>,
}
type DatabaseState = Arc<Mutex<Database>>;
fn authorized(headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .is_some_and(|v| v == "Bearer customer-test-session")
}
async fn identity(headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({"data":{"user_id":"user-1","tenant_id":"tenant-1"}})).into_response()
}
async fn list(State(db): State<DatabaseState>, headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut db = db.lock().await;
    let items = db.item.iter().cloned().collect::<Vec<_>>();
    if let Some(hook) = db.after_list.take() {
        hook();
    }
    Json(json!({"data":{"items":items,"cursor":"snapshot"}})).into_response()
}
async fn put(
    State(db): State<DatabaseState>,
    headers: HeaderMap,
    Json(package): Json<AgentPackage>,
) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut db = db.lock().await;
    if let Some(item) = db.before_put.take() {
        db.item = Some(item);
    }
    let valid = match &db.item {
        None => headers.get("if-none-match").is_some_and(|v| v == "*"),
        Some(item) => {
            headers.get("if-match").and_then(|v| v.to_str().ok()) == Some(item.etag.as_str())
        }
    };
    if !valid {
        return StatusCode::PRECONDITION_FAILED.into_response();
    }
    let version_key = (
        package.definition.definition_id.clone(),
        package.definition.version.clone(),
    );
    if db
        .versions
        .get(&version_key)
        .is_some_and(|sha| sha != &package.sha256)
    {
        return StatusCode::CONFLICT.into_response();
    }
    db.versions.insert(version_key, package.sha256.clone());
    let Some(key) = headers.get("idempotency-key").and_then(|v| v.to_str().ok()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    db.put_keys.push(key.into());
    db.sequence += 1;
    let item = RemoteItem {
        definition_id: package.definition.definition_id.clone(),
        sha256: package.sha256.clone(),
        version: package.definition.version.clone(),
        deleted: false,
        etag: format!("etag-{}", db.sequence),
        package: Some(package),
    };
    db.item = Some(item.clone());
    Json(json!({"data":item})).into_response()
}
async fn delete(State(db): State<DatabaseState>, headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut db = db.lock().await;
    let Some(item) = db.item.as_mut() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if headers.get("if-match").and_then(|v| v.to_str().ok()) != Some(item.etag.as_str()) {
        return StatusCode::PRECONDITION_FAILED.into_response();
    }
    item.deleted = true;
    item.package = None;
    item.etag = "tombstone-1".into();
    Json(json!({"data":item})).into_response()
}
async fn server() -> (String, DatabaseState, tokio::task::JoinHandle<()>) {
    let db = Arc::new(Mutex::new(Database::default()));
    let router = Router::new()
        .route("/api/v1/customer/agent-identity", get(identity))
        .route("/api/v1/customer/agents", get(list).put(put).delete(delete))
        .with_state(db.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, db, task)
}
fn home() -> PathBuf {
    std::env::temp_dir().join(format!("agents-sync-test-{}", uuid::Uuid::new_v4()))
}
#[path = "resolution_tests.rs"]
mod resolution_tests;
fn package() -> AgentPackage {
    AgentPackage::local(
        AgentDefinition::parse(include_bytes!(
            "../../../../../docs/examples/agent-definition.v1.json"
        ))
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn two_clients_sync_conflict_and_tombstone_preserve_local_changes() {
    let (origin, db, task) = server().await;
    let service = Service::new(&origin).unwrap();
    let session = service.identify("customer-test-session").await.unwrap();
    let a = home();
    let b = home();
    session.save(&a).unwrap();
    session.save(&b).unwrap();
    let original = package();
    session
        .library(&a)
        .install(&original, Origin::Private, &BTreeMap::new())
        .unwrap();
    assert_eq!(
        synchronize(&a).await.unwrap().uploaded,
        vec![original.definition.definition_id.clone()]
    );
    assert_eq!(
        synchronize(&b).await.unwrap().downloaded,
        vec![original.definition.definition_id.clone()]
    );
    assert_eq!(
        session.library(&b).package(&original.sha256).unwrap(),
        original
    );
    // Both devices edit the same remote base. B's edit must survive A's upload.
    let mut next = original.definition.clone();
    next.version = "1.0.0-rc2".into();
    next.instructions.push_str(" Device A.");
    let a_next = AgentPackage::local(next.clone()).unwrap();
    next.instructions.push_str(" Device B.");
    let b_next = AgentPackage::local(next).unwrap();
    for (home, package) in [(&a, &a_next), (&b, &b_next)] {
        let library = session.library(home);
        let etag = library.installations().unwrap()[&original.definition.definition_id]
            .remote_etag
            .clone()
            .unwrap();
        library
            .install(package, Origin::Private, &BTreeMap::new())
            .unwrap();
        library
            .set_remote_base(&original.definition.definition_id, etag)
            .unwrap();
    }
    synchronize(&a).await.unwrap();
    assert_eq!(
        synchronize(&b).await.unwrap().conflicts,
        vec![original.definition.definition_id.clone()]
    );
    assert_eq!(
        session.library(&b).installations().unwrap()[&original.definition.definition_id].sha256,
        b_next.sha256
    );
    let item = db.lock().await.item.clone().unwrap();
    service
        .delete(&session, &item.definition_id, &item.etag, "delete-request")
        .await
        .unwrap();
    assert_eq!(
        synchronize(&a).await.unwrap().deleted,
        vec![item.definition_id.clone()]
    );
    assert_eq!(
        synchronize(&b).await.unwrap().conflicts,
        vec![item.definition_id]
    );
    assert_eq!(db.lock().await.put_keys.len(), 2);
    task.abort();
    std::fs::remove_dir_all(a).unwrap();
    std::fs::remove_dir_all(b).unwrap();
}

#[tokio::test]
async fn unauthorized_api_key_is_not_treated_as_customer_identity() {
    let (origin, _, task) = server().await;
    let error = Service::new(&origin)
        .unwrap()
        .identify("inference-test-key")
        .await
        .err()
        .unwrap();
    assert!(!error.to_string().contains("inference-test-key"));
    assert!(error.to_string().contains("401"));
    task.abort();
}

#[tokio::test]
async fn official_download_pins_hash_and_rejects_tamper_without_installing() {
    let golden: Value = serde_json::from_slice(include_bytes!(
        "../../../../../docs/examples/agent-contract-golden.json"
    ))
    .unwrap();
    let keys = golden["keys"].clone();
    let package = golden["packages"][2].clone();
    let router = Router::new()
        .route(
            "/api/v1/public/agents/keys",
            get(move || {
                let keys = keys.clone();
                async move { Json(json!({"data":{"keys":keys}})) }
            }),
        )
        .route(
            "/api/v1/public/agents/package",
            get(move || {
                let package = package.clone();
                async move { Json(json!({"data":package})) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let home = home();
    let value = &golden["packages"][2];
    let id = value["definition"]["definition_id"].as_str().unwrap();
    let revision = value["sha256"].as_str().unwrap();
    let service = Service::new(&origin).unwrap();
    assert_eq!(
        service.official(id, revision, &home).await.unwrap().sha256,
        revision
    );
    assert!(AgentLibrary::new(&home).installations().unwrap().is_empty());
    assert!(service.official(id, &"0".repeat(64), &home).await.is_err());
    task.abort();
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn redirects_cannot_change_trusted_origin() {
    let router = Router::new().route(
        "/api/v1/public/agents/keys",
        get(|| async { axum::response::Redirect::temporary("http://other.invalid/keys") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    assert!(
        Service::new(&origin)
            .unwrap()
            .keys()
            .await
            .unwrap_err()
            .to_string()
            .contains("307")
    );
    task.abort();
}

fn rotation_package(
    seed: u8,
    key_id_override: Option<&str>,
) -> (BTreeMap<String, String>, AgentPackage) {
    use ring::signature::KeyPair;
    use willdeep_core::agent_definition::{PackageSignature, SIGNING_DOMAIN, digest};
    let golden: Value = serde_json::from_slice(include_bytes!(
        "../../../../../docs/examples/agent-contract-golden.json"
    ))
    .unwrap();
    let mut package =
        AgentPackage::parse(&serde_json::to_vec(&golden["packages"][2]).unwrap()).unwrap();
    let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
    let public = pair.public_key().as_ref();
    let derived = digest(public)[..16].to_owned();
    let key_id = key_id_override.unwrap_or(&derived).to_owned();
    let public_hex = public.iter().map(|byte| format!("{byte:02x}")).collect();
    let value = pair
        .sign(format!("{SIGNING_DOMAIN}{}", package.sha256).as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    package.signature = Some(PackageSignature {
        key_id: key_id.clone(),
        value,
    });
    (BTreeMap::from([(key_id, public_hex)]), package)
}

type RotationState = Arc<Mutex<(BTreeMap<String, String>, AgentPackage)>>;
async fn rotating_keys(State(state): State<RotationState>) -> Json<Value> {
    Json(json!({"data":{"keys":state.lock().await.0}}))
}
async fn rotating_package(State(state): State<RotationState>) -> Json<Value> {
    Json(json!({"data":state.lock().await.1}))
}
async fn rotation_server(
    initial: (BTreeMap<String, String>, AgentPackage),
) -> (String, RotationState, tokio::task::JoinHandle<()>) {
    let state = Arc::new(Mutex::new(initial));
    let router = Router::new()
        .route("/api/v1/public/agents/keys", get(rotating_keys))
        .route("/api/v1/public/agents/package", get(rotating_package))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, state, task)
}

#[tokio::test]
async fn same_origin_rotation_keeps_old_key_for_offline_pinned_import() {
    let first = rotation_package(0, None);
    let second = rotation_package(1, None);
    assert_ne!(first.0.keys().next(), second.0.keys().next());
    assert_eq!(first.1.sha256, second.1.sha256);
    let (origin, state, task) = rotation_server(first.clone()).await;
    let home = home();
    let service = Service::new(&origin).unwrap();
    let id = first.1.definition.definition_id.clone();
    let revision = first.1.sha256.clone();
    let original = service.official(&id, &revision, &home).await.unwrap();
    super::super::operate(&home, "import", json!({"package":original}))
        .await
        .unwrap();
    let library = AgentLibrary::new(&home);
    library.set_pinned(&id, true).unwrap();
    *state.lock().await = second.clone();
    service.official(&id, &revision, &home).await.unwrap();
    let trust: Value =
        serde_json::from_slice(&std::fs::read(library.root().join("trusted-keys.json")).unwrap())
            .unwrap();
    assert_eq!(trust["keys"].as_object().unwrap().len(), 2);
    task.abort();
    super::super::operate(&home, "import", json!({"package":original}))
        .await
        .unwrap();
    let installation = &library.installations().unwrap()[&id];
    assert!(installation.pinned && installation.enabled);
    assert_eq!(installation.sha256, revision);
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn reused_key_id_with_different_public_key_is_rejected_without_cache_mutation() {
    let first = rotation_package(0, None);
    let key_id = first.0.keys().next().unwrap().clone();
    let collision = rotation_package(1, Some(&key_id));
    let (origin, state, task) = rotation_server(first.clone()).await;
    let home = home();
    let service = Service::new(&origin).unwrap();
    let id = &first.1.definition.definition_id;
    let revision = &first.1.sha256;
    service.official(id, revision, &home).await.unwrap();
    let path = AgentLibrary::new(&home).root().join("trusted-keys.json");
    let previous = std::fs::read(&path).unwrap();
    *state.lock().await = collision;
    let error = service.official(id, revision, &home).await.unwrap_err();
    assert!(error.to_string().contains("key ID changed public key"));
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    task.abort();
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn different_origins_never_merge_signing_keys() {
    let first = rotation_package(0, None);
    let second = rotation_package(1, None);
    let (origin_a, _, task_a) = rotation_server(first.clone()).await;
    let (origin_b, _, task_b) = rotation_server(second.clone()).await;
    let home = home();
    Service::new(&origin_a)
        .unwrap()
        .official(&first.1.definition.definition_id, &first.1.sha256, &home)
        .await
        .unwrap();
    Service::new(&origin_b)
        .unwrap()
        .official(&second.1.definition.definition_id, &second.1.sha256, &home)
        .await
        .unwrap();
    let trust: Value = serde_json::from_slice(
        &std::fs::read(AgentLibrary::new(&home).root().join("trusted-keys.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(trust["origin"], format!("{origin_b}/"));
    assert_eq!(
        serde_json::from_value::<BTreeMap<String, String>>(trust["keys"].clone()).unwrap(),
        second.0
    );
    task_a.abort();
    task_b.abort();
    std::fs::remove_dir_all(home).unwrap();
}
