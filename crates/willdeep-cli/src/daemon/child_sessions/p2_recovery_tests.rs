use super::*;

#[test]
fn corrupt_core_fails_the_queue_head_without_blocking_another_parent() {
    let fixture = Fixture::new();
    let broken = fixture.start();
    let waiting = dependent(&fixture, &[broken.assignment_id]);
    let other_parent = fixture
        .state
        .sessions
        .create(session_store::CreateRuntimeSession {
            id: None,
            workspace: fixture.root.join("workspace"),
            profile: Some("fixture".into()),
            model: None,
            config: None,
            title: Some("Other parent".into()),
        })
        .unwrap();
    let mut request = fixture.request();
    request.params["parent_session_id"] = serde_json::json!(other_parent.id);
    let healthy: ChildSessionTask = serde_json::from_value(apply(&fixture, &request)).unwrap();
    let sessions = &fixture.state.sessions;
    let broken_turn = broken.child_turn_id.unwrap();
    assert!(
        sessions
            .get_turn(broken_turn)
            .unwrap()
            .unwrap()
            .queue_sequence
            < sessions
                .get_turn(healthy.child_turn_id.unwrap())
                .unwrap()
                .unwrap()
                .queue_sequence
    );
    let core_path = willdeep_core::SessionStore::new(&fixture.root).path(broken.child_session_id);
    let corrupt = b"{invalid synthetic Core JSON";
    std::fs::write(&core_path, corrupt).unwrap();
    assert!(
        sessions
            .claim_next(healthy.child_session_id)
            .unwrap()
            .is_none()
    );
    assert!(
        sessions
            .claim_next(broken.child_session_id)
            .unwrap()
            .is_none()
    );
    let failed = sessions.get_turn(broken_turn).unwrap().unwrap();
    assert_eq!(failed.status, RuntimeTurnStatus::Failed);
    assert_eq!(failed.attempts, 1);
    assert!(failed.completed_at.is_some() && failed.active_task_id.is_none());
    let error = failed.error.unwrap();
    assert!(error.contains(&broken_turn.to_string()));
    assert!(error.contains(&broken.child_session_id.to_string()));
    assert!(error.contains("load Core Session"));
    let child = sessions.get(broken.child_session_id).unwrap().unwrap();
    assert_eq!(child.status, RuntimeSessionStatus::Failed);
    assert!(child.active_turn_id.is_none());
    assert_eq!(child.last_error.as_deref(), Some(error.as_str()));
    sessions
        .children
        .transaction(|ledger| {
            assert!(ledger.slots.is_empty());
            Ok(())
        })
        .unwrap();
    // No restart, reconciliation tick, manual stop or synthetic Task is needed
    // to admit the other family after the failed head releases its reservation.
    let claimed = sessions
        .claim_next(healthy.child_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(Some(claimed.metadata.id), healthy.child_turn_id);
    reconcile::tick(&fixture.state).unwrap();
    let task = fixture.get(broken.assignment_id);
    assert_eq!(task.status, ChildSessionTaskStatus::Failed);
    assert!(task.error.unwrap().contains(&broken_turn.to_string()));
    let result = task.result.unwrap();
    assert!(result.final_task_id.is_none() && result.report.is_none());
    assert!(!result.report_complete);
    // A tick advances dependencies before projecting Runtime terminal states.
    // The next tick observes the newly durable failure and blocks its waiter.
    let still_waiting = fixture.get(waiting.assignment_id);
    assert_eq!(still_waiting.status, ChildSessionTaskStatus::Preparing);
    assert!(still_waiting.waiting_on_dependencies && still_waiting.child_turn_id.is_none());
    reconcile::tick(&fixture.state).unwrap();
    assert_eq!(
        fixture.get(waiting.assignment_id).status,
        ChildSessionTaskStatus::Blocked
    );
    assert!(fixture.get(waiting.assignment_id).child_turn_id.is_none());
    reconcile::tick(&fixture.state).unwrap();
    assert_eq!(fixture.get(broken.assignment_id).result.unwrap(), result);
    assert_eq!(std::fs::read(core_path).unwrap(), corrupt);
}

#[test]
fn fan_out_and_fan_in_wait_for_every_successful_dependency() {
    let fixture = Fixture::new();
    let root = fixture.start();
    let left = dependent(&fixture, &[root.assignment_id]);
    let right = dependent(&fixture, &[root.assignment_id]);
    let join = dependent(&fixture, &[left.assignment_id, right.assignment_id]);
    fixture.finish(&root, RuntimeTaskStatus::Completed);
    reconcile::tick(&fixture.state).unwrap();
    assert!(fixture.get(left.assignment_id).child_turn_id.is_some());
    assert!(fixture.get(right.assignment_id).child_turn_id.is_some());
    assert!(fixture.get(join.assignment_id).child_turn_id.is_none());
    // Reconciliation may enqueue either sibling first. Exercise both in the
    // durable admission order rather than assuming a particular HashMap order.
    let mut branches = [
        fixture.get(left.assignment_id),
        fixture.get(right.assignment_id),
    ];
    branches.sort_by_key(|task| {
        fixture
            .state
            .sessions
            .get_turn(task.child_turn_id.unwrap())
            .unwrap()
            .unwrap()
            .queue_sequence
    });
    fixture.finish(&branches[0], RuntimeTaskStatus::Completed);
    reconcile::tick(&fixture.state).unwrap();
    assert!(fixture.get(join.assignment_id).waiting_on_dependencies);
    fixture.finish(&branches[1], RuntimeTaskStatus::Completed);
    reconcile::tick(&fixture.state).unwrap();
    assert_eq!(
        fixture.get(join.assignment_id).status,
        ChildSessionTaskStatus::Queued
    );
    assert_eq!(
        fixture
            .state
            .sessions
            .list_turns(join.child_session_id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn every_unsuccessful_terminal_dependency_blocks_without_a_turn() {
    for status in [
        RuntimeTaskStatus::Partial,
        RuntimeTaskStatus::Failed,
        RuntimeTaskStatus::Cancelled,
        RuntimeTaskStatus::Interrupted,
    ] {
        let fixture = Fixture::new();
        let root = fixture.start();
        let waiting = dependent(&fixture, &[root.assignment_id]);
        fixture.finish(&root, status);
        reconcile::tick(&fixture.state).unwrap();
        let blocked = fixture.get(waiting.assignment_id);
        assert_eq!(
            blocked.status,
            ChildSessionTaskStatus::Blocked,
            "{status:?}"
        );
        assert!(!blocked.waiting_on_dependencies);
        assert!(blocked.child_turn_id.is_none() && blocked.result.is_some());
        let nested = dependent(&fixture, &[waiting.assignment_id]);
        assert_eq!(nested.status, ChildSessionTaskStatus::Blocked);
        assert!(
            nested
                .error
                .unwrap()
                .contains(&waiting.assignment_id.to_string())
        );
    }
}

#[test]
fn dependency_from_other_parent_is_rejected() {
    let fixture = Fixture::new();
    let root = fixture.start();
    let parent = fixture
        .state
        .sessions
        .create(session_store::CreateRuntimeSession {
            id: None,
            workspace: fixture.root.join("workspace"),
            profile: Some("fixture".into()),
            model: None,
            config: None,
            title: Some("Other parent".into()),
        })
        .unwrap();
    let mut request = fixture.request();
    request.params["parent_session_id"] = serde_json::json!(parent.id);
    request.params["depends_on"] = serde_json::json!([root.assignment_id]);
    assert!(dispatch_for_test(&fixture.state, &request).is_err());
}

#[test]
fn takeover_of_dependency_blocks_waiter_without_restoring_control() {
    let fixture = Fixture::new();
    let root = fixture.start();
    let waiting = dependent(&fixture, &[root.assignment_id]);
    apply(
        &fixture,
        &control_request(&fixture, &root, "session.child.takeover", 1),
    );
    reconcile::tick(&fixture.state).unwrap();
    let blocked = fixture.get(waiting.assignment_id);
    assert_eq!(blocked.status, ChildSessionTaskStatus::Blocked);
    assert!(blocked.error.unwrap().contains("no longer delegated"));
    assert!(blocked.child_turn_id.is_none());
    fixture.finish(&root, RuntimeTaskStatus::Completed);
    reconcile::tick(&fixture.state).unwrap();
    assert_eq!(
        fixture.get(waiting.assignment_id).status,
        ChildSessionTaskStatus::Blocked
    );
}

#[test]
fn committed_wake_context_can_continue_after_its_consumption_receipt() {
    let fixture = Fixture::new();
    let task = fixture.start();
    fixture.finish(&task, RuntimeTaskStatus::Completed);
    let wake = fixture
        .state
        .sessions
        .claim_next(fixture.parent)
        .unwrap()
        .unwrap();
    let context = fixture
        .state
        .sessions
        .children
        .host_context_for_turn(fixture.parent, Some(wake.metadata.id))
        .unwrap();
    assert_eq!(context.len(), 1);
    let store = willdeep_core::SessionStore::new(&fixture.root);
    let mut core = store.load_baselined(fixture.parent).unwrap();
    core.messages
        .push(willdeep_core::Message::host_instruction(format!(
            "<host-context id=\"{}\">\n{}",
            context[0].0, context[0].1
        )));
    store.save(&mut core).unwrap();
    assert!(
        fixture
            .state
            .sessions
            .children
            .host_context_for_turn(fixture.parent, Some(wake.metadata.id))
            .unwrap()
            .is_empty()
    );
    apply(&fixture, &subscription(&fixture, &task, "none", 1));
    apply(
        &fixture,
        &control_request(&fixture, &task, "session.child.takeover", 1),
    );
    // An existing receipt remains authoritative; subsequent model iterations
    // may continue with the result already present in their durable history.
    assert!(
        fixture
            .state
            .sessions
            .children
            .host_context_for_turn(fixture.parent, Some(wake.metadata.id))
            .is_ok()
    );
    assert!(
        fixture
            .get(task.assignment_id)
            .result
            .unwrap()
            .consumed_at
            .is_some()
    );
}

#[test]
fn waiting_worktree_task_creates_neither_checkout_nor_turn() {
    let fixture = Fixture::new();
    let status = std::process::Command::new("git")
        .arg("init")
        .arg("--quiet")
        .arg(fixture.root.join("workspace"))
        .status()
        .unwrap();
    assert!(status.success());
    let root = fixture.start();
    let mut request = fixture.request();
    request.params["workspace_mode"] = "worktree".into();
    request.params["depends_on"] = serde_json::json!([root.assignment_id]);
    let waiting: ChildSessionTask = serde_json::from_value(apply(&fixture, &request)).unwrap();
    let child = fixture
        .state
        .sessions
        .get(waiting.child_session_id)
        .unwrap()
        .unwrap();
    assert!(!child.workspace.exists());
    assert!(!child.delegation_read_only);
    assert!(waiting.waiting_on_dependencies && waiting.child_turn_id.is_none());
    assert!(
        fixture
            .state
            .sessions
            .list_turns(child.id)
            .unwrap()
            .is_empty()
    );
    apply(
        &fixture,
        &control_request(&fixture, &waiting, "session.child.detach", 1),
    );
    let detached = fixture.state.sessions.get(child.id).unwrap().unwrap();
    assert!(detached.delegation_detached);
    assert_eq!(detached.parent_session_id, Some(fixture.parent));
    assert!(!detached.workspace.exists());
}

#[test]
fn continue_and_takeover_have_a_single_control_lock_linearization() {
    let fixture = Fixture::new();
    let first = fixture.start();
    fixture.finish(&first, RuntimeTaskStatus::Completed);
    let continuation = ApiRequest::new(
        "session.child.continue",
        serde_json::json!({
            "parent_session_id": fixture.parent, "child_session_id": first.child_session_id,
            "prompt": "Another task", "notify": "none"
        }),
    );
    let takeover = control_request(&fixture, &first, "session.child.takeover", 1);
    let (continued, controlled) = std::thread::scope(|scope| {
        let first = scope.spawn(|| dispatch_for_test(&fixture.state, &continuation));
        let second = scope.spawn(|| dispatch_for_test(&fixture.state, &takeover));
        (first.join().unwrap(), second.join().unwrap())
    });
    assert!(controlled.is_ok());
    if let Ok(value) = continued {
        let accepted: ChildSessionTask = serde_json::from_value(value).unwrap();
        let current = fixture.get(accepted.assignment_id);
        assert_eq!(current.control_state, ChildSessionControlState::UserOwned);
        assert_eq!(current.status, ChildSessionTaskStatus::Queued);
        assert!(current.child_turn_id.is_some());
    } else {
        assert_eq!(
            fixture
                .state
                .sessions
                .list_turns(first.child_session_id)
                .unwrap()
                .len(),
            1
        );
    }
    assert!(dispatch_for_test(&fixture.state, &continuation).is_err());
}

#[test]
fn retry_preserves_failed_dependency_identity_instead_of_rebinding_to_success() {
    let fixture = Fixture::new();
    let root = fixture.start();
    fixture.finish(&root, RuntimeTaskStatus::Failed);
    let blocked = dependent(&fixture, &[root.assignment_id]);
    assert_eq!(blocked.status, ChildSessionTaskStatus::Blocked);
    let request = ApiRequest::new(
        "session.child.retry",
        serde_json::json!({
            "parent_session_id": fixture.parent, "assignment_id": blocked.assignment_id,
            "expected_result_revision": 1,
        }),
    );
    let retry: ChildSessionTask = serde_json::from_value(apply(&fixture, &request)).unwrap();
    assert_eq!(retry.depends_on, vec![root.assignment_id]);
    assert_eq!(retry.status, ChildSessionTaskStatus::Blocked);
    assert!(retry.child_turn_id.is_none());
    assert_eq!(retry.retry_of_assignment_id, Some(blocked.assignment_id));
}
