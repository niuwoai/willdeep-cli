use super::*;
use willdeep_runtime_protocol::{AgentRun, AgentRunReason, AgentStatus};

fn seed_run(home: &Path, parent: uuid::Uuid, status: AgentStatus) -> (AgentRun, std::fs::File) {
    let id = uuid::Uuid::new_v4();
    let run = AgentRun {
        id,
        request_id: id,
        agent_id: uuid::Uuid::new_v4(),
        parent_session_id: parent,
        sequence: 1,
        reason: AgentRunReason::Spawn,
        status,
        label: "previous worker".into(),
        model: None,
        queued_at_ms: 1,
        started_at_ms: Some(1),
        finished_at_ms: None,
        max_turns: Some(1),
        token_budget: None,
        timeout_seconds: None,
        current_turn: 0,
        current_tool: None,
        usage: Default::default(),
        report: (status == AgentStatus::Completed).then(|| "unacknowledged report".into()),
        delivered: false,
    };
    let directory = home.join("workers/team-runs").join(parent.to_string());
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{id}.json")),
        serde_json::to_vec(&serde_json::json!({
            "version": 1, "record": run, "dispatch": null,
        }))
        .unwrap(),
    )
    .unwrap();
    let owner = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".owner"))
        .unwrap();
    owner.try_lock().unwrap();
    (run, owner)
}

#[tokio::test]
async fn tui_build_leaves_pending_team_and_live_owner_untouched() {
    for status in [
        AgentStatus::Queued,
        AgentStatus::Running,
        AgentStatus::Completed,
    ] {
        let root = std::env::temp_dir().join(format!("tui-team-owner-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        let config = home.join("config.toml");
        std::fs::write(&config, "version = 1\ndefault_provider = \"fixture\"\n[providers.fixture]\nprovider = \"openai-compatible\"\napi = \"chat-completions\"\napi_base = \"http://127.0.0.1:1/v1\"\napi_key = \"test-only\"\nmodel = \"fixture-model\"\n[agent]\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = false\nsafety_judge = false\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let store = willdeep_core::SessionStore::new(&home);
        let mut session = willdeep_core::Session::new(workspace.clone(), None, "existing session");
        store.save(&mut session).unwrap();
        let (run, owner) = seed_run(&home, session.id, status);
        let mut cli = <Cli as clap::Parser>::try_parse_from(["willdeep"]).unwrap();
        cli.config = Some(config.clone());
        cli.workspace = Some(workspace);
        let loaded = LoadedConfig::load(Some(&config)).unwrap();
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
        .expect("an observing TUI must open while Runtime owns a pending team");
        let runs = willdeep_core::subagent::AgentRunStore::new(home.join("workers"), session.id);
        assert_eq!(runs.get(run.id).unwrap().unwrap(), run);
        assert!(built.background_tasks.snapshots().is_empty());
        assert_eq!(store.load(session.id).unwrap().messages.len(), 0);
        drop((built, owner));
        std::fs::remove_dir_all(root).unwrap();
    }
}
