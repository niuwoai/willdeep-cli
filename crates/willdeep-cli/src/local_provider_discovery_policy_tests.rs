use super::*;
use clap::Parser;

const ORIGINAL_CONFIG: &str = "# discovery must preserve this file\nversion=1\n";
const ZERO_REQUEST_WINDOW: Duration = Duration::from_millis(30);

async fn assert_skipped(workspace: &WorkspaceFixture, context: &ModelPolicyContext) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let path = workspace.0.join("config.toml");
    std::fs::write(&path, ORIGINAL_CONFIG).unwrap();
    assert_eq!(
        discover_with_bases(Some(&path), context, Some(base.clone()), Some(base.clone()),)
            .await
            .unwrap(),
        0
    );
    // Also cover the Magpie models route independently of its denied hello.
    let mut config = ProviderConfig::new(
        ProviderKind::OpenAiCompatible,
        ApiDialect::ChatCompletions,
        format!("{base}/v1"),
        "",
        "",
    );
    config.policy_context = Some(context.clone());
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(PROBE_TIMEOUT)
        .build()
        .unwrap();
    assert!(
        json(&client, &config, format!("{base}/v1/models"))
            .await
            .is_none()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), ORIGINAL_CONFIG);
    assert!(!workspace.0.join(".local-provider-discovery.lock").exists());
    assert!(
        tokio::time::timeout(ZERO_REQUEST_WINDOW, listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn empty_or_nonempty_policy_skips_unknown_model_discovery_without_requests_or_writes() {
    for allow in [
        serde_json::json!([]),
        serde_json::json!([{
            "provider": "openai-compatible", "model": "qwen:local", "origin": "http://127.0.0.1:11434",
        }]),
    ] {
        let workspace = WorkspaceFixture::new();
        workspace.policy(allow);
        assert_skipped(&workspace, &workspace.context()).await;
    }
}

#[tokio::test]
async fn even_an_allowlisted_local_origin_requires_a_known_model_before_discovery() {
    let workspace = WorkspaceFixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    workspace.policy(serde_json::json!([{
        "provider": "openai-compatible", "model": "qwen:local", "origin": base,
    }]));
    let context = workspace.context();
    let missing_config = workspace.0.join("never-created/config.toml");
    assert_eq!(
        discover_with_bases(
            Some(&missing_config),
            &context,
            Some(base.clone()),
            Some(base.clone()),
        )
        .await
        .unwrap(),
        0
    );
    let config = probe_config(&base, &context);
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(PROBE_TIMEOUT)
        .build()
        .unwrap();
    assert!(
        json(&client, &config, format!("{base}/v1/models"))
            .await
            .is_none()
    );
    assert!(!missing_config.parent().unwrap().exists());
    assert!(
        tokio::time::timeout(ZERO_REQUEST_WINDOW, listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn existing_context_observes_new_policy_and_keeps_boundary_after_removal() {
    let workspace = WorkspaceFixture::new();
    let context = workspace.context();
    workspace.policy(serde_json::json!([]));
    assert_skipped(&workspace, &context).await;
    std::fs::remove_file(workspace.0.join(".willdeep/model-policy.json")).unwrap();
    assert_skipped(&workspace, &context).await;
}

#[tokio::test]
async fn no_policy_keeps_both_services_and_configuration_persistence() {
    let workspace = WorkspaceFixture::new();
    let path = workspace.0.join("config.toml");
    std::fs::write(&path, ORIGINAL_CONFIG).unwrap();
    let (ollama, first) = fixture("ollama", true);
    let (magpie, second) = fixture("magpie", true);
    assert_eq!(
        discover_with_bases(
            Some(&path),
            &workspace.context(),
            Some(ollama),
            Some(magpie),
        )
        .await
        .unwrap(),
        2
    );
    first.join().unwrap();
    second.join().unwrap();
    let loaded = crate::config::LoadedConfig::load(Some(&path)).unwrap();
    assert_eq!(loaded.file.providers.len(), 2);
    assert_eq!(
        loaded.file.providers["ollama"].model.as_deref(),
        Some("qwen:local")
    );
    assert_eq!(
        loaded.file.providers["magpie"].model.as_deref(),
        Some("group/coder")
    );
    assert!(
        std::fs::read_to_string(path)
            .unwrap()
            .contains(ORIGINAL_CONFIG)
    );
}

fn cli_for_session(workspace: &Path, id: uuid::Uuid) -> crate::Cli {
    let mut cli = crate::Cli::try_parse_from(["willdeep"]).unwrap();
    cli.workspace = Some(workspace.to_path_buf());
    cli.resume = Some(id.to_string());
    cli
}

#[tokio::test]
async fn startup_discovery_uses_persisted_owner_and_legacy_ancestry_instead_of_execution_root() {
    let owner = WorkspaceFixture::new();
    owner.policy(serde_json::json!([]));
    let execution = WorkspaceFixture::new();
    let store = willdeep_core::SessionStore::new(&execution.0);
    let mut parent = willdeep_core::Session::new(owner.0.clone(), None, "parent");
    store.save(&mut parent).unwrap();
    let mut child = willdeep_core::Session::new(execution.0.clone(), None, "child");
    child.model_policy_workspace = Some(owner.0.clone());
    child.parent_session_id = Some(parent.id);
    store.save(&mut child).unwrap();
    let cli = cli_for_session(&execution.0, child.id);
    let canonical_owner = owner.0.canonicalize().unwrap();
    for legacy_child in [false, true] {
        if legacy_child {
            child.model_policy_workspace = None;
            store.save(&mut child).unwrap();
        }
        let context = policy_context_for_cli(&cli, &store).unwrap();
        assert_eq!(context.workspace(), Some(canonical_owner.as_path()));
        assert_skipped(&execution, &context).await;
    }
}

#[test]
fn missing_legacy_parent_or_persisted_owner_does_not_fall_back_to_execution_root() {
    let execution = WorkspaceFixture::new();
    let store = willdeep_core::SessionStore::new(&execution.0);
    let mut child = willdeep_core::Session::new(execution.0.clone(), None, "legacy child");
    child.model_policy_workspace = None;
    child.parent_session_id = Some(uuid::Uuid::new_v4());
    store.save(&mut child).unwrap();
    let cli = cli_for_session(&execution.0, child.id);
    assert!(policy_context_for_cli(&cli, &store).is_err());
    child.model_policy_workspace = Some(execution.0.join("missing-owner"));
    store.save(&mut child).unwrap();
    assert!(policy_context_for_cli(&cli, &store).is_err());
    let mut missing_session = cli;
    missing_session.resume = Some(uuid::Uuid::new_v4().to_string());
    assert!(policy_context_for_cli(&missing_session, &store).is_err());
    assert!(!execution.0.join("config.toml").exists());
}

#[tokio::test]
async fn fresh_workspace_and_legacy_root_without_policy_remain_compatible() {
    let workspace = WorkspaceFixture::new();
    let store = willdeep_core::SessionStore::new(&workspace.0);
    let mut cli = crate::Cli::try_parse_from(["willdeep"]).unwrap();
    cli.workspace = Some(workspace.0.clone());
    let mut session = willdeep_core::Session::new(workspace.0.clone(), None, "legacy root");
    session.model_policy_workspace = None;
    store.save(&mut session).unwrap();
    for resume in [None, Some(session.id.to_string()), Some("latest".into())] {
        cli.resume = resume;
        let context = policy_context_for_cli(&cli, &store).unwrap();
        let (base, worker) = fixture("ollama", true);
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(PROBE_TIMEOUT)
            .build()
            .unwrap();
        assert!(
            probe(&client, "ollama", Some(base), &context)
                .await
                .is_some()
        );
        worker.join().unwrap();
    }
}

#[test]
fn multi_workspace_web_discovery_cannot_choose_the_unrestricted_project() {
    let unrestricted = WorkspaceFixture::new();
    let restricted = WorkspaceFixture::new();
    restricted.policy(serde_json::json!([]));
    let store = willdeep_core::SessionStore::new(&unrestricted.0);
    let mut cli = crate::Cli::try_parse_from(["willdeep", "--web"]).unwrap();
    cli.workspace = Some(unrestricted.0.clone());
    cli.web_workspaces = vec![restricted.0.clone()];
    assert!(policy_context_for_cli(&cli, &store).is_err());
    cli.web_workspaces.clear();
    assert!(policy_context_for_cli(&cli, &store).is_ok());
}

#[test]
fn startup_chooser_candidates_keep_their_durable_owner_before_selection() {
    let execution = WorkspaceFixture::new();
    let owner = WorkspaceFixture::new();
    owner.policy(serde_json::json!([]));
    let store = willdeep_core::SessionStore::new(&execution.0);
    let mut session = willdeep_core::Session::new(execution.0.clone(), None, "saved child");
    session
        .messages
        .push(willdeep_core::Message::user("previous question"));
    session.model_policy_workspace = Some(owner.0.clone());
    store.save(&mut session).unwrap();
    let workspace = execution.0.canonicalize().unwrap();
    assert!(policy_context_before_startup_selection(&store, &workspace).is_err());
    session.model_policy_workspace = None;
    session.parent_session_id = Some(uuid::Uuid::new_v4());
    store.save(&mut session).unwrap();
    assert!(policy_context_before_startup_selection(&store, &workspace).is_err());
    let no_policy = WorkspaceFixture::new();
    let no_policy_store = willdeep_core::SessionStore::new(&no_policy.0);
    let mut legacy_root = willdeep_core::Session::new(no_policy.0.clone(), None, "saved root");
    legacy_root.model_policy_workspace = None;
    legacy_root
        .messages
        .push(willdeep_core::Message::user("previous question"));
    no_policy_store.save(&mut legacy_root).unwrap();
    assert!(
        policy_context_before_startup_selection(
            &no_policy_store,
            &no_policy.0.canonicalize().unwrap(),
        )
        .is_ok()
    );
}
