use super::*;

fn make_host(home: PathBuf, path: PathBuf) -> ChatConfigurationHost {
    use willdeep_core::provider::{ApiDialect, ProviderConfig, ProviderKind};
    let parent = ProviderConfig::new(
        ProviderKind::OpenAiCompatible,
        ApiDialect::ChatCompletions,
        "http://127.0.0.1:1/v1",
        "synthetic-fixture",
        "fixture-main",
    );
    let provider = willdeep_core::provider::build_provider(parent.clone()).unwrap();
    ChatConfigurationHost::new(home, path, parent, provider).unwrap()
}

fn fixture() -> (PathBuf, PathBuf, ChatConfigurationHost) {
    let root = std::env::temp_dir().join(format!("wd-chat-config-{}", uuid::Uuid::new_v4()));
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let path = root.join("custom 中文 config.toml");
    std::fs::write(&path, "version = 1\n# retained\n[providers.summary]\nprovider = \"openai-compatible\"\napi_base = \"http://127.0.0.1:1/v1\"\napi_key = \"synthetic-fixture\"\nmodel = \"fixture-model\"\ncontext_window = 65536\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(home.join("config.toml"), "# decoy\nversion = 1\n").unwrap();
    let host = make_host(home, path.clone());
    (root, path, host)
}

fn edits() -> Value {
    json!({"edits":[
        {"op":"set","section":"agent","key":"compressor_model","value":"fixture-summary"},
        {"op":"set","section":"agent","key":"compressor_profile","value":"summary"},
        {"op":"set","section":"agent","key":"compressor_context_window","value":32768}
    ]})
}

#[tokio::test]
async fn custom_path_preview_save_and_receipt_replay_preserve_default_and_backup() {
    let (root, path, host) = fixture();
    let before = std::fs::read(&path).unwrap();
    let plan = host.execute("plan", edits()).await.unwrap();
    assert_eq!(plan["ok"], true, "{plan}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let id = plan["plan_id"].as_str().unwrap();
    let description = host.approval_description(id).unwrap();
    // The host pins the loaded file through canonicalize, which on Windows adds
    // a verbatim prefix and expands short names, so compare the resolved path.
    let pinned = path.canonicalize().unwrap();
    assert!(description.contains(pinned.to_str().unwrap()));
    assert!(description.contains("compressor_profile"));
    let receipt = host.execute("apply", json!({"plan_id":id})).await.unwrap();
    assert_eq!(receipt["ok"], true, "{receipt}");
    assert_eq!(receipt["effective"], "next_runtime_turn");
    assert_eq!(
        std::fs::read(receipt["backupPath"].as_str().unwrap()).unwrap(),
        before
    );
    let loaded = crate::config::LoadedConfig::load(Some(&path)).unwrap();
    assert_eq!(
        loaded.file.agent.compressor_model.as_deref(),
        Some("fixture-summary")
    );
    assert_eq!(
        loaded.file.agent.compressor_profile.as_deref(),
        Some("summary")
    );
    assert_eq!(loaded.file.agent.compressor_context_window, Some(32768));
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("# retained")
    );
    assert_eq!(
        std::fs::read_to_string(host.home.join("config.toml")).unwrap(),
        "# decoy\nversion = 1\n"
    );
    assert_eq!(
        host.execute("apply", json!({"plan_id":id})).await.unwrap(),
        receipt
    );
    assert_eq!(
        std::fs::read_dir(host.home.join("plugin-data/willdeep-config/backups"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "bak"))
            .count(),
        1
    );
    assert!(!host.home.join("plugins").exists());
    assert!(!host.home.join("plugins.json").exists());
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stale_or_foreign_plans_cannot_write_and_require_a_new_preview() {
    let (root, path, host) = fixture();
    let plan = host.execute("plan", edits()).await.unwrap();
    let foreign = make_host(host.home.clone(), path.clone());
    assert!(
        foreign
            .approval_description(plan["plan_id"].as_str().unwrap())
            .is_err()
    );
    let changed = "version = 1\n# another writer\n";
    std::fs::write(&path, changed).unwrap();
    let result = host
        .execute("apply", json!({"plan_id":plan["plan_id"]}))
        .await
        .unwrap();
    assert_eq!(result["error"], "conflict_repreview_required");
    assert_eq!(std::fs::read_to_string(path).unwrap(), changed);
    assert!(
        !host
            .home
            .join("plugin-data/willdeep-config/backups")
            .exists()
    );
    drop(host);
    drop(foreign);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn only_compressor_edits_and_host_selected_paths_are_accepted() {
    let (root, path, host) = fixture();
    let before = std::fs::read(&path).unwrap();
    for operation in ["snapshot", "schema"] {
        assert!(
            host.execute(operation, json!({"path":"/another"}))
                .await
                .is_err()
        );
    }
    let mut overridden = edits();
    overridden["path"] = json!("/another");
    assert!(host.execute("plan", overridden).await.is_err());
    for key in ["approval", "api_key", "max_turns"] {
        assert!(
            host.execute(
                "plan",
                json!({"edits":[{"op":"set","section":"agent","key":key,"value":"arbitrary"}]})
            )
            .await
            .is_err()
        );
    }
    assert!(
        host.execute("apply", json!({"confirm":true,"edits":edits()["edits"]}))
            .await
            .is_err()
    );
    assert!(host.execute("reveal_secret", json!({})).await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(
        host.service.get().is_none(),
        "invalid calls must not even start the service"
    );
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn semantic_errors_are_rejected_before_a_plan_becomes_committable() {
    let (root, path, host) = fixture();
    for (key, value) in [
        ("compressor_profile", json!("missing")),
        ("compressor_context_window", json!(3999)),
    ] {
        let result = host
            .execute(
                "plan",
                json!({"edits":[{"op":"set","section":"agent","key":key,"value":value}]}),
            )
            .await
            .unwrap();
        assert_eq!(result["ok"], false);
        assert!(host.plan.lock().unwrap().is_none());
    }
    std::fs::write(&path, "version = 1\nunknown_runtime_key = true\n[providers.summary]\nprovider = \"openai-compatible\"\napi_base = \"http://127.0.0.1:1/v1\"\napi_key = \"synthetic-fixture\"\nmodel = \"fixture\"\n").unwrap();
    let result = host.execute("plan", edits()).await.unwrap();
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"], "runtime_validation_failed");
    assert!(host.plan.lock().unwrap().is_none());
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn snapshot_and_preview_never_expose_credentials_raw_comments_or_multiline_secrets() {
    let (root, path, host) = fixture();
    let text = "version = 1\n# COMMENT_CANARY\n[providers.summary]\nprovider = \"openai-compatible\"\napi_base = \"http://127.0.0.1:1/v1\"\nmodel = \"fixture-model\"\napi_key = \"\"\"\nMULTILINE_CANARY\n\"\"\"\n";
    std::fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    for operation in ["schema", "snapshot"] {
        let result = host
            .execute(operation, json!({}))
            .await
            .unwrap()
            .to_string();
        for forbidden in ["CANARY", "api_key", "raw", "candidate", "preview"] {
            assert!(
                !result.contains(forbidden),
                "{operation} exposed {forbidden}"
            );
        }
    }
    // The existing limited TOML editor rejects multiline strings. Refusal must
    // still redact the parser's source excerpt, without minting a plan.
    let rejected = host.execute("plan", edits()).await.unwrap();
    assert_eq!(rejected["ok"], false);
    assert!(!rejected.to_string().contains("CANARY"));
    assert!(host.plan.lock().unwrap().is_none());
    std::fs::write(
        &path,
        text.replace("\"\"\"\nMULTILINE_CANARY\n\"\"\"", "\"SINGLELINE_CANARY\""),
    )
    .unwrap();
    let plan = host.execute("plan", edits()).await.unwrap();
    assert_eq!(plan["ok"], true, "{plan}");
    assert!(!plan.to_string().contains("CANARY"));
    assert!(
        !host
            .approval_description(plan["plan_id"].as_str().unwrap())
            .unwrap()
            .contains("CANARY")
    );
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn config_symlinks_are_pinned_to_the_loaded_target() {
    let (root, path, _) = fixture();
    let link = root.join("symlink.toml");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let host = make_host(root.join("home"), link.clone());
    let plan = host.execute("plan", edits()).await.unwrap();
    assert_eq!(
        host.execute("apply", json!({"plan_id":plan["plan_id"]}))
            .await
            .unwrap()["ok"],
        true
    );
    assert!(link.is_symlink());
    assert_eq!(
        crate::config::LoadedConfig::load(Some(&link))
            .unwrap()
            .file
            .agent
            .compressor_context_window,
        Some(32768)
    );
    drop(host);
    std::fs::remove_dir_all(root).unwrap();
}
