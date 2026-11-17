use super::*;

#[test]
fn restored_child_follows_same_parent_into_new_task_and_rejects_other_parent() {
    let root = std::env::temp_dir().join(format!("agent-store-recovery-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("agents.json");
    let parent = uuid::Uuid::new_v4();
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let child = uuid::Uuid::new_v4();
    let event = serde_json::json!({"type":"subagent_started","id":child,"profile":"scout","background":true,"workspace":root}).to_string();
    let store = AgentStore::open(path.clone()).unwrap();
    store
        .ensure_session_root(
            parent,
            first,
            root.clone(),
            None,
            None,
            RuntimeAgentStatus::Running,
        )
        .unwrap();
    store.apply_harness_event(first, &event).unwrap();
    drop(store);
    let store = AgentStore::open(path.clone()).unwrap();
    assert_eq!(
        store.get(child).unwrap().unwrap().status,
        RuntimeAgentStatus::Interrupted
    );
    store
        .ensure_session_root(
            parent,
            second,
            root.clone(),
            None,
            None,
            RuntimeAgentStatus::Running,
        )
        .unwrap();
    store.apply_harness_event(second, &event).unwrap();
    let restored = store.get(child).unwrap().unwrap();
    assert_eq!(restored.parent_id, Some(parent));
    assert_eq!(restored.task_id, second);
    assert_eq!(restored.status, RuntimeAgentStatus::Running);
    let unrelated = uuid::Uuid::new_v4();
    store
        .ensure_root(
            unrelated,
            root.clone(),
            None,
            None,
            RuntimeAgentStatus::Running,
        )
        .unwrap();
    assert!(store.apply_harness_event(unrelated, &event).is_err());
    assert_eq!(store.get(child).unwrap().unwrap(), restored);
    std::fs::remove_dir_all(root).unwrap();
}

fn assert_restored_child_rejects_old_task_events(sequenced: bool) {
    let root = std::env::temp_dir().join(format!("agent-task-provenance-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("agents.json");
    let store = AgentStore::open(path.clone()).unwrap();
    let parent = uuid::Uuid::new_v4();
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let child = uuid::Uuid::new_v4();
    let run = uuid::Uuid::new_v4();
    let event = |kind| {
        let mut value = serde_json::json!({
            "type":kind,"id":child,"profile":"scout","turn":7,"name":"shell",
            "status":"completed","report":"stale report","input_tokens":11,
            "output_tokens":13,"attempt":1,"delay_ms":100,"verifier_passed":true
        });
        if sequenced {
            value["run_id"] = serde_json::json!(run);
            value["run_sequence"] = serde_json::json!(1);
        }
        value.to_string()
    };
    for task in [first, second] {
        store
            .ensure_session_root(
                parent,
                task,
                root.clone(),
                None,
                None,
                RuntimeAgentStatus::Running,
            )
            .unwrap();
        store
            .apply_harness_event(task, &event("subagent_started"))
            .unwrap();
    }
    let restored = store.get(child).unwrap().unwrap();
    assert_eq!(restored.task_id, second);
    for kind in [
        "subagent_completed",
        "subagent_turn_started",
        "subagent_tool_requested",
        "subagent_tool_completed",
        "subagent_usage",
        "subagent_retry_wait",
        "subagent_retry_started",
        "subagent_verdict",
    ] {
        store.apply_harness_event(first, &event(kind)).unwrap();
        assert_eq!(
            store.get(child).unwrap().unwrap(),
            restored,
            "old task event {kind} must not affect its successor"
        );
        assert_eq!(load_agents(&path).unwrap()[&child], restored);
    }
    // The rebound owner can still report ordinary events for the same run.
    store
        .apply_harness_event(second, &event("subagent_turn_started"))
        .unwrap();
    assert_eq!(store.get(child).unwrap().unwrap().current_turn, 7);
    store
        .apply_harness_event(second, &event("subagent_completed"))
        .unwrap();
    assert_eq!(
        store.get(child).unwrap().unwrap().status,
        RuntimeAgentStatus::Completed
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn restored_child_rejects_old_task_same_run_events() {
    assert_restored_child_rejects_old_task_events(true);
}

#[test]
fn restored_child_rejects_old_task_legacy_events() {
    assert_restored_child_rejects_old_task_events(false);
}

#[test]
fn terminal_root_ignores_late_status_and_start_but_accepts_a_new_task() {
    let root = std::env::temp_dir().join(format!("agent-terminal-root-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("agents.json");
    let store = AgentStore::open(path.clone()).unwrap();
    let parent = uuid::Uuid::new_v4();
    for terminal in [
        RuntimeAgentStatus::Completed,
        RuntimeAgentStatus::Partial,
        RuntimeAgentStatus::Failed,
        RuntimeAgentStatus::Cancelled,
        RuntimeAgentStatus::Interrupted,
    ] {
        let task = uuid::Uuid::new_v4();
        store
            .ensure_session_root(
                parent,
                task,
                root.clone(),
                None,
                None,
                RuntimeAgentStatus::Queued,
            )
            .unwrap();
        store
            .set_status_for_task(task, RuntimeAgentStatus::Running, None)
            .unwrap();
        store
            .apply_harness_event(
                task,
                &serde_json::json!({"type":"turn_started","turn":1}).to_string(),
            )
            .unwrap();
        assert_eq!(store.get(parent).unwrap().unwrap().current_turn, 1);
        store
            .set_status_for_task(task, terminal, Some("finished execution".into()))
            .unwrap();
        let finished = store.get(parent).unwrap().unwrap();
        for status in [
            RuntimeAgentStatus::Queued,
            RuntimeAgentStatus::Running,
            RuntimeAgentStatus::WaitingApproval,
            RuntimeAgentStatus::WaitingAnswer,
        ] {
            store.set_status_for_task(task, status, None).unwrap();
            assert_eq!(store.get(parent).unwrap().unwrap(), finished);
        }
        store
            .apply_harness_event(
                task,
                &serde_json::json!({"type":"turn_started","turn":2}).to_string(),
            )
            .unwrap();
        assert_eq!(store.get(parent).unwrap().unwrap(), finished);
        assert_eq!(load_agents(&path).unwrap()[&parent], finished);

        let next = uuid::Uuid::new_v4();
        let rebound = store
            .ensure_session_root(
                parent,
                next,
                root.clone(),
                None,
                None,
                RuntimeAgentStatus::Queued,
            )
            .unwrap();
        assert_eq!(rebound.status, RuntimeAgentStatus::Queued);
        assert!(rebound.completed_at.is_none());
        assert!(rebound.error.is_none());
        store
            .set_status_for_task(next, RuntimeAgentStatus::Running, None)
            .unwrap();
        store
            .apply_harness_event(
                next,
                &serde_json::json!({"type":"turn_started","turn":3}).to_string(),
            )
            .unwrap();
        let active = store.get(parent).unwrap().unwrap();
        assert_eq!(active.status, RuntimeAgentStatus::Running);
        assert_eq!(active.current_turn, 3);
        store
            .apply_harness_event(
                task,
                &serde_json::json!({"type":"turn_started","turn":99}).to_string(),
            )
            .unwrap();
        assert_eq!(store.get(parent).unwrap().unwrap(), active);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn terminal_child_rejects_same_run_starts_and_accepts_a_new_run() {
    let root = std::env::temp_dir().join(format!("agent-terminal-child-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("agents.json");
    let store = AgentStore::open(path.clone()).unwrap();
    let task = uuid::Uuid::new_v4();
    store
        .ensure_root(task, root.clone(), None, None, RuntimeAgentStatus::Running)
        .unwrap();
    let child = uuid::Uuid::new_v4();
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let event = |kind, run, sequence| {
        serde_json::json!({
            "type":kind,"id":child,"run_id":run,"run_sequence":sequence,
            "profile":"scout","status":"partial","turn":2
        })
        .to_string()
    };
    store
        .apply_harness_event(task, &event("subagent_started", first, 1))
        .unwrap();
    store
        .apply_harness_event(task, &event("subagent_completed", first, 1))
        .unwrap();
    let finished = store.get(child).unwrap().unwrap();
    for kind in [
        "subagent_started",
        "subagent_queued",
        "subagent_turn_started",
    ] {
        store
            .apply_harness_event(task, &event(kind, first, 1))
            .unwrap();
        assert_eq!(store.get(child).unwrap().unwrap(), finished);
    }
    assert_eq!(load_agents(&path).unwrap()[&child], finished);
    // A retry owns a fresh run sequence and resets terminal execution state.
    store
        .apply_harness_event(task, &event("subagent_queued", second, 2))
        .unwrap();
    let queued = store.get(child).unwrap().unwrap();
    assert_eq!(queued.status, RuntimeAgentStatus::Queued);
    assert!(queued.completed_at.is_none());
    store
        .apply_harness_event(task, &event("subagent_started", second, 2))
        .unwrap();
    store
        .apply_harness_event(task, &event("subagent_turn_started", second, 2))
        .unwrap();
    let active = store.get(child).unwrap().unwrap();
    assert_eq!(active.status, RuntimeAgentStatus::Running);
    assert_eq!(active.current_turn, 2);
    assert_eq!(active.run_id, Some(second));
    assert_eq!(active.run_sequence, 2);
    assert!(active.completed_at.is_none());
    store
        .apply_harness_event(task, &event("subagent_started", first, 1))
        .unwrap();
    assert_eq!(store.get(child).unwrap().unwrap(), active);
    std::fs::remove_dir_all(root).unwrap();
}
