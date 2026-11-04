use super::*;
use willdeep_core::agent_definition::store::ModelBinding;

struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("registry-cas-{}", uuid::Uuid::new_v4())))
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn definition() -> AgentDefinition {
    AgentDefinition::parse(include_bytes!(
        "../../../../docs/examples/agent-definition.v1.json"
    ))
    .unwrap()
}
fn params(home: &Path, value: Value, expected: Option<u64>) -> Value {
    let mut value = value;
    value["request_id"] = uuid::Uuid::new_v4().to_string().into();
    value["expected_scope"] = capture_scope(home).unwrap();
    if let Some(expected) = expected {
        value["expected_revision"] = expected.into();
    }
    value
}
async fn create(home: &Path) -> Value {
    operate(
        home,
        "create",
        params(home, json!({"definition":definition()}), None),
    )
    .await
    .unwrap()
}
async fn get(home: &Path, id: &str) -> Value {
    operate(home, "get", json!({"id":id})).await.unwrap()
}

#[tokio::test]
async fn public_contract_create_get_history_and_replayed_fork_keep_identity() {
    let home = Home::new();
    let home = &home.0;
    let definition = definition();
    let id = &definition.definition_id;
    let request = params(home, json!({"definition":definition}), None);
    let first = operate(home, "create", request.clone()).await.unwrap();
    assert_eq!(first["saved"], true);
    assert_eq!(first["target_scope"], "local");
    assert_eq!(first["definition_id"], id.as_str());
    let replay = operate(home, "create", request).await.unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        replay["installation_revision"],
        first["installation_revision"]
    );
    assert_eq!(replay["definition_id"], id.as_str());
    let loaded = get(home, id).await;
    assert_eq!(loaded["scope"], capture_scope(home).unwrap());
    assert_eq!(loaded["target_scope"], "local");
    assert_eq!(loaded["definition_revision"], first["sha256"]);
    assert_eq!(
        loaded["installation_revision"],
        loaded["installation"]["installation_revision"]
    );
    assert_eq!(loaded["package"]["definition"], first["definition"]);
    assert_eq!(
        operate(home, "list", json!({})).await.unwrap()["scope"],
        loaded["scope"]
    );
    assert!(
        operate(home, "history", json!({"id":id}))
            .await
            .unwrap()
            .is_array()
    );
    let fork = params(
        home,
        json!({"id":id,"new_id":"user/cas-fork"}),
        loaded["installation_revision"].as_u64(),
    );
    for replayed in [false, true] {
        let receipt = operate(home, "fork", fork.clone()).await.unwrap();
        assert_eq!(receipt["definition_id"], "user/cas-fork");
        assert_eq!(receipt["source_definition_id"], id.as_str());
        assert_eq!(receipt["replayed"], replayed);
        assert_eq!(receipt["definition"]["forked_from"], first["sha256"]);
    }
    assert!(loaded["package"].get("installation_revision").is_none());
}

#[tokio::test]
async fn cli_adapter_passes_preconditions_but_raw_updates_require_them() {
    let home = Home::new();
    let home = &home.0;
    let first = create(home).await;
    let id = first["definition_id"].as_str().unwrap();
    for operation in ["bind", "enable", "pin", "rollback", "fork"] {
        let value = params(
            home,
            json!({"id":id,"binding":{},"value":false,"revision":first["sha256"],"new_id":"user/other"}),
            None,
        );
        assert!(
            operate(home, operation, value)
                .await
                .unwrap_err()
                .to_string()
                .contains("agent_revision_required")
        );
    }
    let mut changed = definition();
    changed.version = "1.0.0-rc2".into();
    assert!(
        operate(
            home,
            "save",
            params(home, json!({"definition":changed}), None)
        )
        .await
        .is_err()
    );
    assert!(
        operate(
            home,
            "update",
            params(home, json!({"definition":changed}), None)
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("agent_revision_required")
    );
    management::cli_write(
        home,
        "bind",
        json!({"id":id,"binding":{"model":"fixture-model"}}),
    )
    .await
    .unwrap();
    let current = get(home, id).await;
    assert!(
        current["installation_revision"].as_u64().unwrap()
            > first["installation_revision"].as_u64().unwrap()
    );
    assert_eq!(current["definition_revision"], first["definition_revision"]);
    assert!(
        operate(
            home,
            "pin",
            params(
                home,
                json!({"id":id,"value":true}),
                first["installation_revision"].as_u64()
            )
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("agent_revision_conflict")
    );
}

#[tokio::test]
async fn scope_is_stable_before_first_write_and_account_switch_rejects_writes_and_replays() {
    let home = Home::new();
    let home = &home.0;
    let before = capture_scope(home).unwrap();
    let request = params(home, json!({"definition":definition()}), None);
    create(home).await;
    assert_eq!(capture_scope(home).unwrap(), before);
    let mut session = CustomerSession {
        origin: service::DEFAULT_ORIGIN.into(),
        token: "synthetic-session-only".into(),
        user_id: "synthetic-a".into(),
        tenant_id: "synthetic-t".into(),
    };
    session.save(home).unwrap();
    assert!(
        operate(home, "create", request)
            .await
            .unwrap_err()
            .to_string()
            .contains("agent_scope_conflict")
    );
    let a = capture_scope(home).unwrap();
    session.token = "synthetic-refresh-only".into();
    session.save(home).unwrap();
    assert_eq!(capture_scope(home).unwrap(), a);
    let first = definition();
    let id = &first.definition_id;
    let loaded = get(home, id).await;
    let pin = params(
        home,
        json!({"id":id,"value":true}),
        loaded["installation_revision"].as_u64(),
    );
    operate(home, "pin", pin.clone()).await.unwrap();
    session.user_id = "synthetic-b".into();
    session.save(home).unwrap();
    assert!(
        operate(home, "pin", pin)
            .await
            .unwrap_err()
            .to_string()
            .contains("agent_scope_conflict")
    );
    let list = operate(home, "list", json!({})).await.unwrap().to_string();
    assert!(!list.contains("synthetic-refresh-only"));
    assert!(
        !capture_scope(home)
            .unwrap()
            .to_string()
            .contains("synthetic-refresh-only")
    );
}

#[tokio::test]
async fn body_update_and_rollback_preserve_history_and_request_payload_conflicts() {
    let home = Home::new();
    let home = &home.0;
    let first = create(home).await;
    let id = first["definition_id"].as_str().unwrap();
    let mut second = definition();
    second.version = "1.0.0-rc2".into();
    second.instructions.push_str(" Second.");
    let request = params(
        home,
        json!({"definition_text":serde_json::to_string(&second).unwrap()}),
        first["installation_revision"].as_u64(),
    );
    let saved = operate(home, "update", request.clone()).await.unwrap();
    let mut different = request.clone();
    different["definition_text"] = serde_json::to_string(&definition()).unwrap().into();
    assert!(
        operate(home, "update", different)
            .await
            .unwrap_err()
            .to_string()
            .contains("agent_request_conflict")
    );
    let rollback = operate(
        home,
        "rollback",
        params(
            home,
            json!({"id":id,"revision":first["sha256"]}),
            saved["installation_revision"].as_u64(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(rollback["definition_revision"], first["sha256"]);
    assert!(
        rollback["installation_revision"].as_u64().unwrap()
            > saved["installation_revision"].as_u64().unwrap()
    );
    assert_eq!(
        operate(home, "history", json!({"id":id}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        operate(home, "update", request).await.unwrap()["replayed"],
        true
    );
}

#[tokio::test]
async fn account_installations_receive_the_same_cas_and_receipts() {
    let home = Home::new();
    let home = &home.0;
    let session = CustomerSession {
        origin: service::DEFAULT_ORIGIN.into(),
        token: "fixture-only".into(),
        user_id: "u".into(),
        tenant_id: "t".into(),
    };
    session.save(home).unwrap();
    let package = AgentPackage::local(definition()).unwrap();
    let id = &package.definition.definition_id;
    let library = session.library(home);
    library
        .install(&package, Origin::Private, &BTreeMap::new())
        .unwrap();
    let before = get(home, id).await;
    assert_eq!(before["target_scope"], "private");
    let request = params(
        home,
        json!({"id":id,"binding":{"model":"local-model"}}),
        before["installation_revision"].as_u64(),
    );
    let saved = operate(home, "bind", request.clone()).await.unwrap();
    assert_eq!(saved["target_scope"], "private");
    assert_eq!(saved["replayed"], false);
    assert_eq!(
        operate(home, "bind", request).await.unwrap()["replayed"],
        true
    );
    assert_eq!(
        library.installations().unwrap()[id]
            .binding
            .model
            .as_deref(),
        Some("local-model")
    );
    assert!(AgentLibrary::new(home).installations().unwrap().is_empty());
}

#[tokio::test]
async fn cli_text_imports_and_saves_capture_existing_installation_revisions() {
    let home = Home::new();
    let home = &home.0;
    let first = create(home).await;
    let mut second = definition();
    second.version = "1.0.0-rc2".into();
    let imported = management::cli_write(
        home,
        "import",
        json!({"package_text":serde_json::to_string(&second).unwrap()}),
    )
    .await
    .unwrap();
    assert!(imported["installation_revision"].as_u64() > first["installation_revision"].as_u64());
    second.version = "1.0.0-rc3".into();
    let saved = management::cli_write(
        home,
        "save",
        json!({"definition_text":serde_json::to_string(&second).unwrap()}),
    )
    .await
    .unwrap();
    assert!(saved["installation_revision"].as_u64() > imported["installation_revision"].as_u64());
}

#[test]
fn cli_first_install_captures_scope_without_requiring_an_existing_id() {
    let home = Home::new();
    let params = management::cli_params(
        &home.0,
        "install",
        json!({"id":"official/fixture","revision":"a".repeat(64)}),
    )
    .unwrap();
    assert_eq!(params["expected_scope"], capture_scope(&home.0).unwrap());
    assert!(
        params["request_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert!(params.get("expected_revision").is_none());
}

#[tokio::test]
async fn account_save_and_logout_wait_for_the_transaction_scope_lock() {
    let home = Home::new();
    let session = CustomerSession {
        origin: service::DEFAULT_ORIGIN.into(),
        token: "synthetic-scope-lock-session".into(),
        user_id: "synthetic-user".into(),
        tenant_id: "synthetic-tenant".into(),
    };
    for logout in [false, true] {
        let library = AgentLibrary::new(&home.0);
        let lock = library
            .account_scope("synthetic-other-account")
            .lock_scope()
            .unwrap();
        let path = home.0.clone();
        let session = session.clone();
        let (started, start) = std::sync::mpsc::channel();
        let (finished, finish) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            started.send(()).unwrap();
            let result = if logout {
                tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap()
                    .block_on(operate(&path, "logout", json!({})))
                    .map(|_| ())
            } else {
                session.save(&path)
            };
            finished.send(result).unwrap();
        });
        start
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(
            finish
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err()
        );
        drop(lock);
        finish
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .unwrap();
        thread.join().unwrap();
    }
    assert!(CustomerSession::load(&home.0).is_err());
}

#[test]
fn provider_resolution_inherits_fallback_or_reuses_binding_configuration_without_inference() {
    struct NeverRun;
    #[async_trait::async_trait]
    impl willdeep_core::provider::Provider for NeverRun {
        async fn complete(
            &self,
            _: &[willdeep_core::Message],
            _: &[willdeep_core::types::ToolDefinition],
        ) -> std::result::Result<
            willdeep_core::types::Completion,
            willdeep_core::provider::ProviderError,
        > {
            panic!("test must not call a model")
        }
    }
    let file: crate::config::ConfigFile = toml::from_str("version=1\ndefault_provider='fixture'\n[providers.fixture]\nprovider='openai-compatible'\napi_base='http://127.0.0.1:1/v1'\napi_key='fixture-only'\nmodel='parent-model'\n").unwrap();
    let parent = crate::provider_config_from_profile(&file, "fixture").unwrap();
    let fallback: std::sync::Arc<dyn willdeep_core::provider::Provider> =
        std::sync::Arc::new(NeverRun);
    let resolved = resolve_binding_provider(
        Path::new("/unused"),
        &ModelBinding::default(),
        &file,
        &parent,
        fallback.clone(),
    )
    .unwrap();
    assert!(std::sync::Arc::ptr_eq(&fallback, &resolved));
    let binding = ModelBinding {
        provider_profile: Some("fixture".into()),
        model: Some("bound-model".into()),
        ..Default::default()
    };
    assert_eq!(
        binding_config(&binding, &file, &parent).unwrap().model,
        "bound-model"
    );
    assert!(
        resolve_binding_provider(Path::new("/unused"), &binding, &file, &parent, fallback).is_ok()
    );
}
