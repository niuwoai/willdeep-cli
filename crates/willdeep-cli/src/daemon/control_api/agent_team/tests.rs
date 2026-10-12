use super::*;

#[test]
fn team_query_is_read_only_and_does_not_invent_an_unconfigured_policy() {
    let (state, session, child, root, _) = fixture();
    let directory = root.join("workers/team-runs").join(session.to_string());
    assert!(!directory.exists());
    let request = ApiRequest::new("agent.team", serde_json::json!({"id":child}));
    let value = team(&state, &request)
        .map_err(|error| error.message)
        .unwrap();
    let summary: willdeep_runtime_protocol::agent_team::AgentTeamSummary =
        serde_json::from_value(value).unwrap();
    assert_eq!(summary.parent_session_id, session);
    assert!(summary.policy.is_none());
    assert_eq!(summary.running, 0);
    assert_eq!(summary.queued, 0);
    assert!(!directory.exists());
    std::fs::remove_dir_all(root).unwrap();
}

fn fixture() -> (
    ServerState,
    uuid::Uuid,
    uuid::Uuid,
    PathBuf,
    tokio::sync::mpsc::UnboundedReceiver<uuid::Uuid>,
) {
    let root = std::env::temp_dir().join(format!("continue-api-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let config = root.join("config.toml");
    std::fs::write(&config, "version = 1\n").unwrap();
    let sessions = crate::daemon::tests::test_runtime_session_store(&root);
    let session = sessions
        .create(session_store::CreateRuntimeSession {
            id: None,
            workspace: workspace.clone(),
            profile: None,
            model: None,
            config: Some(config),
            title: None,
        })
        .unwrap();
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let state = crate::daemon::tests::test_server_state_with_scheduler(&root, sessions, sender);
    let task = uuid::Uuid::new_v4();
    state
        .agents
        .ensure_session_root(
            session.root_agent_id,
            task,
            workspace,
            None,
            None,
            RuntimeAgentStatus::Completed,
        )
        .unwrap();
    let child = uuid::Uuid::new_v4();
    let dispatches = root.join("workers/dispatches");
    std::fs::create_dir_all(&dispatches).unwrap();
    std::fs::write(dispatches.join(format!("{child}.json")), serde_json::to_vec(&serde_json::json!({
        "version":1, "id":child, "parent_session":session.id,
        "args":{"profile":"scout", "prompt":"fixture", "run_in_background":true},
        "approved_targets":null, "approved_command":null, "model":null,
        "prepared":{"workspace":session.workspace,"root_workspace":session.workspace,"branch":null,"dedicated":false}
    })).unwrap()).unwrap();
    for value in [
        serde_json::json!({"type":"subagent_started","id":child,"profile":"scout","label":"fixture","background":true}),
        serde_json::json!({"type":"subagent_completed","id":child,"status":"completed"}),
    ] {
        state
            .agents
            .apply_harness_event(task, &value.to_string())
            .unwrap();
    }
    (state, session.id, child, root, receiver)
}

#[tokio::test]
async fn idle_continuation_is_durable_idempotent_and_uses_the_parent_session_scheduler() {
    let (state, session, child, root, mut scheduled) = fixture();
    let request_id = uuid::Uuid::new_v4();
    let make = |message: &str| {
        ApiRequest::new(
            "agent.continue",
            serde_json::json!({"id":child,"message":message,"request_id":request_id}),
        )
    };
    let first = continue_agent(&state, &make("followup"))
        .await
        .map_err(|error| error.message)
        .unwrap();
    let second = continue_agent(&state, &make("followup"))
        .await
        .map_err(|error| error.message)
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(scheduled.try_recv().unwrap(), session);
    assert!(scheduled.try_recv().is_err());
    assert!(
        continue_agent(&state, &make("different task"))
            .await
            .is_err()
    );
    let turns = state.sessions.list_turns(session).unwrap();
    assert_eq!(turns.len(), 1);
    let reopened =
        session_store::RuntimeSessionStore::open(root.join("runtime-sessions.json"), &root)
            .unwrap();
    let claimed = reopened.claim_next(session).unwrap().unwrap();
    assert!(
        matches!(claimed.request.team_control, Some(AgentTeamControl::Continue { id, request_id: saved, .. }) if id == child && saved == request_id)
    );
    assert_eq!(claimed.request.prompt, "followup");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn continuation_rejects_root_unknown_agents_and_second_pending_request() {
    let (state, session, child, root, _scheduled) = fixture();
    let parent = state.sessions.get(session).unwrap().unwrap().root_agent_id;
    for id in [parent, uuid::Uuid::new_v4()] {
        let request = ApiRequest::new(
            "agent.continue",
            serde_json::json!({"id":id,"message":"inspect","request_id":uuid::Uuid::new_v4()}),
        );
        assert!(continue_agent(&state, &request).await.is_err());
    }
    let request = || {
        ApiRequest::new(
            "agent.continue",
            serde_json::json!({"id":child,"message":"inspect","request_id":uuid::Uuid::new_v4()}),
        )
    };
    continue_agent(&state, &request())
        .await
        .map_err(|error| error.message)
        .unwrap();
    assert!(continue_agent(&state, &request()).await.is_err());
    std::fs::remove_dir_all(root).unwrap();
}
