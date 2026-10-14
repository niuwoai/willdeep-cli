use super::*;

fn fixture() -> AgentDefinition {
    AgentDefinition::parse(include_bytes!(
        "../../../../docs/examples/agent-definition.v1.json"
    ))
    .unwrap()
}
fn home() -> PathBuf {
    std::env::temp_dir().join(format!("agent-registry-test-{}", uuid::Uuid::new_v4()))
}

#[tokio::test]
async fn signed_incompatible_official_update_preserves_active_installation() {
    use willdeep_core::agent_definition::{PackageSignature, SIGNING_DOMAIN};
    let golden: Value = serde_json::from_slice(include_bytes!(
        "../../../../docs/examples/agent-contract-golden.json"
    ))
    .unwrap();
    let current =
        AgentPackage::parse(&serde_json::to_vec(&golden["packages"][2]).unwrap()).unwrap();
    let keys: BTreeMap<String, String> = serde_json::from_value(golden["keys"].clone()).unwrap();
    let home = home();
    let library = AgentLibrary::new(&home);
    private_write(
        &library.root().join("trusted-keys.json"),
        &serde_json::to_vec(&json!({"origin":service::DEFAULT_ORIGIN,"keys":keys})).unwrap(),
    )
    .unwrap();
    operate(&home, "import", json!({"package":current}))
        .await
        .unwrap();
    let id = &current.definition.definition_id;
    library
        .set_binding(
            id,
            willdeep_core::agent_definition::store::ModelBinding {
                provider_profile: Some("fixture".into()),
                model: Some("fixed-model".into()),
            },
        )
        .unwrap();
    let mut definition = current.definition.clone();
    definition.version = "1.0.0-rc2".into();
    definition.input_modalities.push("image".into());
    let mut candidate = AgentPackage::local(definition).unwrap();
    let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[0u8; 32]).unwrap();
    let value = pair
        .sign(format!("{SIGNING_DOMAIN}{}", candidate.sha256).as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    candidate.signature = Some(PackageSignature {
        key_id: "golden-test-only".into(),
        value,
    });
    candidate.verify_official(&keys).unwrap();
    for pinned in [false, true] {
        library.set_pinned(id, pinned).unwrap();
        let before = serde_json::to_value(library.installations().unwrap()).unwrap();
        let error = operate(&home, "import", json!({"package":candidate}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("incompatible"));
        assert!(error.to_string().contains("does not support image"));
        assert_eq!(
            serde_json::to_value(library.installations().unwrap()).unwrap(),
            before
        );
        assert_eq!(library.installations().unwrap()[id].sha256, current.sha256);
        assert!(library.installations().unwrap()[id].enabled);
        assert_eq!(library.history(id).unwrap().len(), 1);
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn library_crud_pin_rollback_and_fork_use_immutable_revisions() {
    let home = home();
    let first = fixture();
    let id = first.definition_id.clone();
    let saved = operate(&home, "save", json!({"definition":first}))
        .await
        .unwrap();
    let revision = saved["sha256"].as_str().unwrap().to_owned();
    operate(&home, "pin", json!({"id":id,"value":true}))
        .await
        .unwrap();
    let mut second = fixture();
    second.version = "1.0.0-rc2".into();
    second.instructions.push_str("Second revision.");
    assert!(
        operate(&home, "save", json!({"definition":second}))
            .await
            .is_err()
    );
    operate(&home, "pin", json!({"id":id,"value":false}))
        .await
        .unwrap();
    operate(&home, "save", json!({"definition":second}))
        .await
        .unwrap();
    assert_eq!(
        operate(&home, "history", json!({"id":id}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    operate(&home, "rollback", json!({"id":id,"revision":revision}))
        .await
        .unwrap();
    assert_eq!(
        operate(&home, "export", json!({"id":id})).await.unwrap()["sha256"],
        saved["sha256"]
    );
    let fork = operate(&home, "fork", json!({"id":id,"new_id":"user/my-fork"}))
        .await
        .unwrap();
    assert_eq!(fork["definition"]["forked_from"], saved["sha256"]);
    assert!(fork["signature"].is_null());
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn incompatible_import_is_visible_disabled_and_cannot_be_enabled() {
    let home = home();
    let mut definition = fixture();
    definition.input_modalities.push("image".into());
    let id = definition.definition_id.clone();
    operate(&home, "save", json!({"definition":definition}))
        .await
        .unwrap();
    let list = operate(&home, "list", json!({})).await.unwrap();
    assert_eq!(list["items"][0]["compatible"], false);
    assert_eq!(list["items"][0]["installation"]["enabled"], false);
    assert!(
        operate(&home, "enable", json!({"id":id,"value":true}))
            .await
            .is_err()
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn private_account_is_hidden_on_logout_without_exporting_credentials() {
    let home = home();
    let session = CustomerSession {
        origin: service::DEFAULT_ORIGIN.into(),
        token: "never-display-this-session".into(),
        user_id: "u1".into(),
        tenant_id: "t1".into(),
    };
    session.save(&home).unwrap();
    let package = AgentPackage::local(fixture()).unwrap();
    session
        .library(&home)
        .install(&package, Origin::Private, &BTreeMap::new())
        .unwrap();
    let list = operate(&home, "list", json!({})).await.unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert!(
        !serde_json::to_string(&list)
            .unwrap()
            .contains(&session.token)
    );
    operate(&home, "logout", json!({})).await.unwrap();
    assert_eq!(
        operate(&home, "list", json!({})).await.unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert!(session.library(&home).package(&package.sha256).is_ok());
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn independent_local_models_preserve_definition_hash_and_permissions() {
    let home = home();
    let first = fixture();
    let id = first.definition_id.clone();
    operate(&home, "save", json!({"definition":first}))
        .await
        .unwrap();
    operate(
        &home,
        "fork",
        json!({"id":id,"new_id":"user/second-reviewer"}),
    )
    .await
    .unwrap();
    let before = operate(&home, "export", json!({"id":id})).await.unwrap();
    for (id, model) in [
        (id.as_str(), "review-model-a"),
        ("user/second-reviewer", "review-model-b"),
    ] {
        operate(
            &home,
            "bind",
            json!({"id":id,"binding":{"provider_profile":"fixture","model":model}}),
        )
        .await
        .unwrap();
    }
    let file: crate::config::ConfigFile=toml::from_str("version=1\ndefault_provider='fixture'\n[providers.fixture]\nprovider='openai-compatible'\napi_base='http://127.0.0.1:1/v1'\napi_key='fixture-only'\nmodel='parent-model'\n").unwrap();
    let parent = crate::provider_config_from_profile(&file, "fixture").unwrap();
    let bases =
        willdeep_core::builtin_profiles(willdeep_core::build_provider(parent.clone()).unwrap());
    let profiles = runtime_profiles(&home, &bases, &file, &parent).unwrap();
    assert_eq!(
        profiles
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .model
            .as_deref(),
        Some("review-model-a")
    );
    assert_eq!(
        profiles
            .iter()
            .find(|p| p.id == "user/second-reviewer")
            .unwrap()
            .model
            .as_deref(),
        Some("review-model-b")
    );
    assert!(profiles.iter().all(|p| p.definition_model_bound
        && !p.hosted_job_prompt
        && p.write_scope == willdeep_core::SubagentWriteScope::None));
    let after = operate(&home, "export", json!({"id":id})).await.unwrap();
    assert_eq!(before, after);
    assert!(after.get("binding").is_none());
    let session = CustomerSession {
        origin: service::DEFAULT_ORIGIN.into(),
        token: "fixture-session".into(),
        user_id: "u1".into(),
        tenant_id: "t1".into(),
    };
    session.save(&home).unwrap();
    operate(&home, "share", json!({"id":id})).await.unwrap();
    assert_eq!(
        session.library(&home).installations().unwrap()[&id]
            .binding
            .model
            .as_deref(),
        Some("review-model-a")
    );
    operate(
        &home,
        "bind",
        json!({"id":id,"binding":{"provider_profile":null,"model":null}}),
    )
    .await
    .unwrap();
    let inherited = runtime_profiles(&home, &bases, &file, &parent).unwrap();
    assert!(
        !inherited
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .definition_model_bound
    );
    std::fs::remove_dir_all(home).unwrap();
}
