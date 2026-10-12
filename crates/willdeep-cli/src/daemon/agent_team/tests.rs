use super::*;

#[test]
fn startup_recovers_an_undelivered_team_report_once_without_a_new_parent_prompt() {
    use willdeep_runtime_protocol::{AgentRun, AgentRunReason, AgentStatus};
    let root = std::env::temp_dir().join(format!("team-startup-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let sessions = crate::daemon::tests::test_runtime_session_store(&root);
    let session = sessions
        .create(session_store::CreateRuntimeSession {
            id: None,
            workspace,
            profile: None,
            model: None,
            config: None,
            title: None,
        })
        .unwrap();
    let id = uuid::Uuid::new_v4();
    let record = AgentRun {
        id,
        request_id: id,
        agent_id: uuid::Uuid::new_v4(),
        parent_session_id: session.id,
        sequence: 1,
        reason: AgentRunReason::Spawn,
        status: AgentStatus::Completed,
        label: "fixture".into(),
        model: None,
        queued_at_ms: 1,
        started_at_ms: Some(2),
        finished_at_ms: Some(3),
        max_turns: Some(8),
        token_budget: None,
        timeout_seconds: Some(30),
        current_turn: 1,
        current_tool: None,
        usage: Default::default(),
        report: Some("undelivered report".into()),
        delivered: false,
    };
    let directory = root.join("workers/team-runs").join(session.id.to_string());
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{id}.json")),
        serde_json::to_vec(&serde_json::json!({"version":1,"record":record,"dispatch":{}}))
            .unwrap(),
    )
    .unwrap();
    let (sender, mut scheduled) = tokio::sync::mpsc::unbounded_channel();
    let state =
        crate::daemon::tests::test_server_state_with_scheduler(&root, sessions.clone(), sender);
    assert_eq!(recover(&state).unwrap(), 1);
    assert_eq!(scheduled.try_recv().unwrap(), session.id);
    assert_eq!(recover(&state).unwrap(), 0);
    let turns = sessions.list_turns(session.id).unwrap();
    assert_eq!(turns.len(), 1);
    let claimed = sessions.claim_next(session.id).unwrap().unwrap();
    assert_eq!(
        claimed.request.team_control,
        Some(AgentTeamControl::RecoverQueue)
    );
    assert!(claimed.request.prompt.is_empty());
    assert!(!claimed.request.replay_existing_user_message);
    std::fs::remove_dir_all(root).unwrap();
}
