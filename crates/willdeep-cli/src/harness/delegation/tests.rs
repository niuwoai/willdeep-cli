use super::super::*;
use willdeep_core::provider::{Provider, ProviderError};
use willdeep_core::types::{Completion, MessageSource, ToolDefinition};
use willdeep_core::{Message, Role};

#[test]
fn delegation_assignment_and_result_wakes_are_host_input_but_real_chat_is_not() {
    for prefix in [
        crate::daemon::CHILD_ASSIGNMENT_ORIGIN,
        crate::daemon::CHILD_RESULT_WAKE_ORIGIN,
        crate::daemon::PEER_WAKE_ORIGIN,
    ] {
        assert!(super::is_host_input(Some(&format!(
            "{prefix}{}",
            uuid::Uuid::new_v4()
        ))));
    }
    for origin in [
        None,
        Some("web:fixture"),
        Some("cli:fixture"),
        Some("child-assignment"),
        Some("some-other-client"),
    ] {
        assert!(!super::is_host_input(origin));
    }
}

#[derive(Default)]
struct ReadOnlyModel(std::sync::Mutex<Vec<Vec<Message>>>);
#[async_trait::async_trait]
impl Provider for ReadOnlyModel {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        let system = messages.iter().find(|m| m.role == Role::System).unwrap();
        assert!(
            system
                .content
                .contains("persisted read-only capability ceiling")
        );
        assert!(system.content.contains("project-rule-fixture"));
        assert!(!system.content.contains("must-run-verifier-fixture"));
        for name in ["read_file", "grep_files", "search_files", "list_directory"] {
            assert!(tools.iter().any(|t| t.name == name), "missing {name}");
        }
        for name in [
            "spawn_agent",
            "start_session",
            "continue_session",
            "retry_session_task",
            "run_command",
            "git_status",
            "git_diff",
            "git_log",
            "git_blame",
            "list_worktrees",
            "edit_file",
            "call_mcp_tool",
            "workflow_control",
            "agent_library_manage",
            "send_session_message",
        ] {
            assert!(!tools.iter().any(|t| t.name == name), "offered {name}");
        }
        self.0.lock().unwrap().push(messages.to_vec());
        Ok(Completion {
            content: "Read-only investigation complete.".into(),
            finish_reason: Some("stop".into()),
            ..Completion::default()
        })
    }
}

#[tokio::test]
async fn delegation_harness_rebuilds_main_rules_without_executing_extensions() {
    let root = std::env::temp_dir().join(format!("delegation-harness-{}", uuid::Uuid::new_v4()));
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("AGENTS.md"),
        "project-rule-fixture: cite file evidence",
    )
    .unwrap();
    let config = home.join("config.toml");
    std::fs::write(&config, "version = 1\ndefault_provider = \"fixture\"\n[providers.fixture]\nprovider = \"openai-compatible\"\napi = \"chat-completions\"\napi_base = \"http://127.0.0.1:1/v1\"\napi_key = \"test-only\"\nmodel = \"fixture-model\"\n[agent]\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = true\nsafety_judge = false\nverification_commands = [\"cargo test --lib --package must-run-verifier-fixture\"]\n[rules]\nfiles = []\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut loaded = LoadedConfig::load(Some(&config)).unwrap();
    loaded.file.mcp_servers.insert(
        "must-not-start".into(),
        willdeep_core::McpServerConfig {
            command: Some("willdeep-fixture-nonexistent-mcp-program".into()),
            ..Default::default()
        },
    );
    let mut session = willdeep_core::Session::new(workspace.clone(), None, "child investigation");
    session.parent_session_id = Some(uuid::Uuid::new_v4());
    session.delegation_read_only = true;
    session.goal = Some("must-not-restore-goal-fixture".into());
    let mut cli = <Cli as clap::Parser>::try_parse_from(["willdeep"]).unwrap();
    cli.config = Some(config);
    cli.workspace = Some(workspace);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let built = build(
        &cli,
        &loaded,
        &home,
        Language::ZhCn,
        Some(&session),
        HarnessFrontend::Tui { tx },
    )
    .await
    .unwrap();
    assert!(built._workflow_driver.is_none());
    assert!(built._command_watcher.is_none());
    assert!(built.agent.goal_continuation().is_none());
    assert!(!built.agent.workflows_pending());
    assert!(
        built
            .agent
            .workflow_operation("run", serde_json::json!({}))
            .await
            .is_err()
    );
    built.agent.prepare_background_execution().await.unwrap();
    assert!(built.background_tasks.snapshots().is_empty());
    assert!(
        !home
            .join("workers/team-runs")
            .join(session.id.to_string())
            .exists()
    );
    let provider = Arc::new(ReadOnlyModel::default());
    built
        .agent
        .main_model()
        .set_provider(provider.clone())
        .unwrap();
    let host = Message::host_instruction("Read the requested files");
    let outcome = built
        .agent
        .run_with_history_message(Vec::new(), host)
        .await
        .unwrap();
    assert_eq!(outcome.final_text, "Read-only investigation complete.");
    built
        .agent
        .run("actual child operator input")
        .await
        .unwrap();
    let captured = provider.0.lock().unwrap();
    assert!(
        captured[0]
            .iter()
            .any(|m| m.content == "Read the requested files"
                && m.source == Some(MessageSource::HostInstruction))
    );
    assert!(
        !captured[0]
            .iter()
            .any(|m| m.source == Some(MessageSource::OperatorInput) && m.role == Role::User)
    );
    assert!(
        captured[1]
            .iter()
            .any(|m| m.content == "actual child operator input"
                && m.source == Some(MessageSource::OperatorInput)
                && m.role == Role::User)
    );
    drop(captured);
    drop(built);
    // 账本写入器按目录共享，drop 后仍可能正在把队列里的行落盘；先等它排空，
    // 再对偶发的“删到一半又有新文件”重试，否则 macOS 会报 DirectoryNotEmpty。
    willdeep_core::usage_ledger::flush_all(std::time::Duration::from_secs(2));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match std::fs::remove_dir_all(&root) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_error) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(error) => panic!("remove {}: {error}", root.display()),
        }
    }
}
