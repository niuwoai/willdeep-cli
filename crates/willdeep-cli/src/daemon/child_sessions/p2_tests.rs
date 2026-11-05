use super::*;

#[path = "p2_notification_tests.rs"]
mod notifications;
#[path = "p2_recovery_tests.rs"]
mod recovery;

fn apply(fixture: &Fixture, request: &ApiRequest) -> serde_json::Value {
    dispatch_for_test(&fixture.state, request)
        .map_err(|_| "child request failed")
        .unwrap()
}

fn control_request(
    fixture: &Fixture,
    task: &ChildSessionTask,
    operation: &str,
    revision: u64,
) -> ApiRequest {
    ApiRequest::new(
        operation,
        serde_json::json!({
            "parent_session_id": fixture.parent, "child_session_id": task.child_session_id,
            "expected_control_revision": revision,
        }),
    )
}

fn subscription(
    fixture: &Fixture,
    task: &ChildSessionTask,
    policy: &str,
    revision: u64,
) -> ApiRequest {
    ApiRequest::new(
        "session.child.task.subscription.set",
        serde_json::json!({
            "parent_session_id": fixture.parent, "assignment_id": task.assignment_id,
            "subscription": policy, "expected_subscription_revision": revision,
        }),
    )
}

fn dependent(fixture: &Fixture, dependencies: &[Uuid]) -> ChildSessionTask {
    let mut request = fixture.request();
    request.params["depends_on"] = serde_json::json!(dependencies);
    serde_json::from_value(apply(fixture, &request)).unwrap()
}

#[test]
fn control_is_concurrent_durable_and_preserves_accepted_turn_and_capabilities() {
    let fixture = Fixture::new();
    let task = fixture.start();
    let requests = (0..4)
        .map(|_| control_request(&fixture, &task, "session.child.takeover", 1))
        .collect::<Vec<_>>();
    let state = &fixture.state;
    let results = std::thread::scope(|scope| {
        requests
            .iter()
            .map(|request| scope.spawn(move || dispatch_for_test(state, request)))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let (index, response) = results
        .iter()
        .enumerate()
        .find_map(|(index, result)| result.as_ref().ok().map(|response| (index, response)))
        .unwrap();
    assert_eq!(*response, apply(&fixture, &requests[index]));
    assert_eq!(response["control_revision"], 2);
    assert_eq!(
        fixture.get(task.assignment_id).control_state,
        ChildSessionControlState::UserOwned
    );
    let child = fixture
        .state
        .sessions
        .get(task.child_session_id)
        .unwrap()
        .unwrap();
    assert!(child.delegation_read_only);
    assert_eq!(child.parent_session_id, Some(fixture.parent));
    assert!(!child.delegation_detached);
    // Takeover does not cancel or silently replace the already accepted Turn.
    let turn = fixture
        .state
        .sessions
        .claim_next(task.child_session_id)
        .unwrap()
        .unwrap();
    assert_eq!(Some(turn.metadata.id), task.child_turn_id);
    let detached = apply(
        &fixture,
        &control_request(&fixture, &task, "session.child.detach", 2),
    );
    assert_eq!(detached["control_revision"], 3);
    assert_eq!(*response, apply(&fixture, &requests[index]));
    let child = fixture
        .state
        .sessions
        .get(task.child_session_id)
        .unwrap()
        .unwrap();
    assert!(child.delegation_detached && child.delegation_read_only);
    assert_eq!(child.parent_session_id, Some(fixture.parent));
    let core = willdeep_core::SessionStore::new(&fixture.root)
        .load(child.id)
        .unwrap();
    assert!(core.delegation_detached && core.delegation_read_only);
    assert_eq!(core.parent_session_id, Some(fixture.parent));
}

#[tokio::test]
async fn revoked_relation_rejects_parent_stop_continue_retry_and_new_dependency() {
    let fixture = Fixture::new();
    let task = fixture.start();
    apply(
        &fixture,
        &control_request(&fixture, &task, "session.child.takeover", 1),
    );
    let stop_request = ApiRequest::new(
        "session.child.task.stop",
        serde_json::json!({"parent_session_id":fixture.parent,"assignment_id":task.assignment_id}),
    );
    assert!(stop(&fixture.state, &stop_request).await.is_err());
    let continuation = ApiRequest::new(
        "session.child.continue",
        serde_json::json!({"parent_session_id":fixture.parent,"child_session_id":task.child_session_id,"prompt":"continue"}),
    );
    assert!(dispatch_for_test(&fixture.state, &continuation).is_err());
    let mut dependency = fixture.request();
    dependency.params["depends_on"] = serde_json::json!([task.assignment_id]);
    assert!(dispatch_for_test(&fixture.state, &dependency).is_err());
    fixture.finish(&task, RuntimeTaskStatus::Failed);
    let retry = ApiRequest::new(
        "session.child.retry",
        serde_json::json!({"parent_session_id":fixture.parent,"assignment_id":task.assignment_id,"expected_result_revision":1}),
    );
    assert!(dispatch_for_test(&fixture.state, &retry).is_err());
    let mut wrong = control_request(&fixture, &task, "session.child.detach", 2);
    wrong.params["parent_session_id"] = serde_json::json!(Uuid::new_v4());
    assert!(dispatch_for_test(&fixture.state, &wrong).is_err());
    assert!(
        fixture
            .state
            .sessions
            .children
            .host_context(fixture.parent)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn policy_downgrade_keeps_result_identity_and_cancels_empty_queued_wake() {
    let fixture = Fixture::new();
    let task = fixture.start();
    fixture.finish(&task, RuntimeTaskStatus::Completed);
    let before = fixture.get(task.assignment_id).result.unwrap();
    let request = subscription(&fixture, &task, "inbox", 1);
    let changed: ChildSessionTask = serde_json::from_value(apply(&fixture, &request)).unwrap();
    assert_eq!(changed.subscription, ChildSessionSubscription::Inbox);
    assert_eq!(changed.notify, ChildSessionNotify::None);
    assert_eq!(changed.result.as_ref().unwrap(), &before);
    assert!(
        fixture
            .state
            .sessions
            .children
            .host_context(fixture.parent)
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .state
            .sessions
            .claim_next(fixture.parent)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        apply(&fixture, &request),
        serde_json::to_value(&changed).unwrap()
    );
    let mut conflict = request;
    conflict.params["subscription"] = "none".into();
    assert!(dispatch_for_test(&fixture.state, &conflict).is_err());
    assert_eq!(fixture.get(task.assignment_id).result.unwrap(), before);
}

#[test]
fn downgrade_after_claim_is_rechecked_before_execution() {
    let fixture = Fixture::new();
    let task = fixture.start();
    fixture.finish(&task, RuntimeTaskStatus::Completed);
    let claimed = fixture
        .state
        .sessions
        .claim_next(fixture.parent)
        .unwrap()
        .unwrap();
    let execution = Uuid::new_v4();
    fixture
        .state
        .sessions
        .bind_task(claimed.metadata.id, execution)
        .unwrap();
    apply(&fixture, &subscription(&fixture, &task, "none", 1));
    assert!(
        fixture
            .state
            .sessions
            .children
            .host_context_for_turn(fixture.parent, Some(claimed.metadata.id))
            .is_err()
    );
    let core = willdeep_core::SessionStore::new(&fixture.root)
        .load(fixture.parent)
        .unwrap();
    assert!(
        fixture
            .state
            .sessions
            .prepare_execution(execution, &core)
            .is_err()
    );
    assert!(
        fixture
            .get(task.assignment_id)
            .result
            .unwrap()
            .consumed_at
            .is_none()
    );
}

#[test]
fn mixed_wake_batch_keeps_other_relation_results_after_takeover() {
    let fixture = Fixture::new();
    let first = fixture.start();
    let second = fixture.start();
    fixture.finish(&first, RuntimeTaskStatus::Completed);
    fixture.finish(&second, RuntimeTaskStatus::Completed);
    apply(
        &fixture,
        &control_request(&fixture, &first, "session.child.takeover", 1),
    );
    let contexts = fixture
        .state
        .sessions
        .children
        .host_context(fixture.parent)
        .unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(
        contexts[0].0,
        fixture
            .get(second.assignment_id)
            .result
            .unwrap()
            .notification_id
    );
    assert!(
        fixture
            .state
            .sessions
            .claim_next(fixture.parent)
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .get(first.assignment_id)
            .result
            .unwrap()
            .consumed_at
            .is_none()
    );
}

#[test]
fn subscription_changes_preserve_receipts_and_parent_stop_suppression() {
    let fixture = Fixture::new();
    let task = fixture.start();
    fixture.finish(&task, RuntimeTaskStatus::Completed);
    let result = fixture.get(task.assignment_id).result.unwrap();
    let store = willdeep_core::SessionStore::new(&fixture.root);
    let mut core = store.load_baselined(fixture.parent).unwrap();
    core.messages
        .push(willdeep_core::Message::host_instruction(format!(
            "<host-context id=\"{}\">\nreceipt",
            result.notification_id
        )));
    store.save(&mut core).unwrap();
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            ledger.stop(fixture.parent, Uuid::new_v4());
            Ok(())
        })
        .unwrap();
    fixture
        .state
        .sessions
        .children
        .host_context(fixture.parent)
        .unwrap();
    let consumed = fixture.get(task.assignment_id).result.unwrap().consumed_at;
    assert!(consumed.is_some());
    apply(&fixture, &subscription(&fixture, &task, "none", 1));
    apply(&fixture, &subscription(&fixture, &task, "auto_wake", 2));
    assert_eq!(
        fixture.get(task.assignment_id).result.unwrap().consumed_at,
        consumed
    );
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            assert!(ledger.control(fixture.parent).suppressed);
            Ok(())
        })
        .unwrap();
}

#[test]
fn dependency_waits_without_turn_or_slot_then_queues_once_after_success() {
    let fixture = Fixture::new();
    let first = fixture.start();
    let waiting = dependent(&fixture, &[first.assignment_id]);
    assert_eq!(waiting.status, ChildSessionTaskStatus::Preparing);
    assert!(waiting.waiting_on_dependencies && waiting.child_turn_id.is_none());
    assert!(waiting.accepted_at.is_some());
    assert!(
        fixture
            .state
            .sessions
            .list_turns(waiting.child_session_id)
            .unwrap()
            .is_empty()
    );
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            assert!(ledger.slots.is_empty());
            Ok(())
        })
        .unwrap();
    fixture.finish(&first, RuntimeTaskStatus::Completed);
    reconcile::tick(&fixture.state).unwrap();
    let ready = fixture.get(waiting.assignment_id);
    assert_eq!(ready.status, ChildSessionTaskStatus::Queued);
    assert!(!ready.waiting_on_dependencies);
    assert_eq!(ready.depends_on, vec![first.assignment_id]);
    reconcile::tick(&fixture.state).unwrap();
    assert_eq!(
        fixture
            .state
            .sessions
            .list_turns(waiting.child_session_id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn failed_dependency_blocks_and_successful_retry_cannot_rewrite_it() {
    let fixture = Fixture::new();
    let first = fixture.start();
    let waiting = dependent(&fixture, &[first.assignment_id]);
    fixture.finish(&first, RuntimeTaskStatus::Failed);
    reconcile::tick(&fixture.state).unwrap();
    let blocked = fixture.get(waiting.assignment_id);
    assert_eq!(blocked.status, ChildSessionTaskStatus::Blocked);
    assert!(blocked.child_turn_id.is_none());
    assert!(
        blocked
            .error
            .as_ref()
            .unwrap()
            .contains(&first.assignment_id.to_string())
    );
    let retry = ApiRequest::new(
        "session.child.retry",
        serde_json::json!({"parent_session_id":fixture.parent,"assignment_id":first.assignment_id,"expected_result_revision":1}),
    );
    let retry: ChildSessionTask = serde_json::from_value(apply(&fixture, &retry)).unwrap();
    fixture.finish(&retry, RuntimeTaskStatus::Completed);
    reconcile::tick(&fixture.state).unwrap();
    assert_eq!(fixture.get(waiting.assignment_id), blocked);
    assert_eq!(
        fixture.get(first.assignment_id).status,
        ChildSessionTaskStatus::Failed
    );
}

#[tokio::test]
async fn waiting_task_can_stop_without_creating_turn_and_takeover_blocks_other_waiter() {
    let fixture = Fixture::new();
    let first = fixture.start();
    let waiting = dependent(&fixture, &[first.assignment_id]);
    let request = ApiRequest::new(
        "session.child.task.stop",
        serde_json::json!({"parent_session_id":fixture.parent,"assignment_id":waiting.assignment_id}),
    );
    let stopped = stop(&fixture.state, &request)
        .await
        .map_err(|_| "stop failed")
        .unwrap();
    assert_eq!(stopped["status"], "cancelled");
    assert!(
        fixture
            .state
            .sessions
            .list_turns(waiting.child_session_id)
            .unwrap()
            .is_empty()
    );
    let other = dependent(&fixture, &[first.assignment_id]);
    apply(
        &fixture,
        &control_request(&fixture, &other, "session.child.detach", 1),
    );
    assert_eq!(
        fixture.get(other.assignment_id).status,
        ChildSessionTaskStatus::Blocked
    );
    assert!(
        fixture
            .state
            .sessions
            .list_turns(other.child_session_id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn dependencies_enforce_ownership_uniqueness_limit_and_historical_cycles() {
    let fixture = Fixture::new();
    let first = fixture.start();
    for dependencies in [
        vec![Uuid::new_v4()],
        vec![first.assignment_id, first.assignment_id],
        vec![Uuid::new_v4(); CHILD_SESSION_DEPENDENCY_LIMIT + 1],
    ] {
        let mut request = fixture.request();
        request.params["depends_on"] = serde_json::json!(dependencies);
        assert!(dispatch_for_test(&fixture.state, &request).is_err());
    }
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            let task = &mut ledger
                .assignments
                .get_mut(&first.assignment_id)
                .unwrap()
                .task;
            task.depends_on = vec![first.assignment_id];
            assert!(
                dependencies::validate(ledger, fixture.parent, &[first.assignment_id]).is_err()
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn waiting_capacity_is_bounded_and_released_without_execution_slot() {
    let fixture = Fixture::new();
    let first = fixture.start();
    for _ in 1..CHILD_SESSION_QUEUE_LIMIT {
        dependent(&fixture, &[first.assignment_id]);
    }
    let mut request = fixture.request();
    request.params["depends_on"] = serde_json::json!([first.assignment_id]);
    assert!(dispatch_for_test(&fixture.state, &request).is_err());
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            assert!(ledger.slots.is_empty());
            Ok(())
        })
        .unwrap();
    fixture.finish(&first, RuntimeTaskStatus::Failed);
    reconcile::tick(&fixture.state).unwrap();
    assert_eq!(
        dependent(&fixture, &[first.assignment_id]).status,
        ChildSessionTaskStatus::Blocked
    );
}

#[test]
fn schema_one_migration_keeps_private_original_backup_and_legacy_notify() {
    let fixture = Fixture::new();
    let task = fixture.start();
    let path = fixture.root.join("runtime/session-children.json");
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    old["schema"] = 1.into();
    old.as_object_mut().unwrap().remove("mutations");
    for link in old["links"].as_object_mut().unwrap().values_mut() {
        for key in ["control_state", "control_revision", "control_updated_at"] {
            link.as_object_mut().unwrap().remove(key);
        }
    }
    for assignment in old["assignments"].as_object_mut().unwrap().values_mut() {
        let task = assignment["task"].as_object_mut().unwrap();
        task.insert("notify".into(), "none".into());
        for key in [
            "subscription",
            "subscription_revision",
            "control_state",
            "control_revision",
            "depends_on",
            "waiting_on_dependencies",
        ] {
            task.remove(key);
        }
    }
    let original = serde_json::to_vec(&old).unwrap();
    std::fs::write(&path, &original).unwrap();
    let migrated = fixture.get(task.assignment_id);
    assert_eq!(migrated.subscription, ChildSessionSubscription::None);
    assert_eq!(migrated.subscription_revision, 1);
    assert_eq!(migrated.control_state, ChildSessionControlState::Delegated);
    assert_eq!(migrated.control_revision, 1);
    assert_eq!(
        std::fs::read(path.with_extension("schema1.bak")).unwrap(),
        original
    );
    let current: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(current["schema"], 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path.with_extension("schema1.bak"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn restart_waits_for_dependencies_and_repairs_committed_turn_before_revocation() {
    let fixture = Fixture::new();
    let first = fixture.start();
    let waiting = dependent(&fixture, &[first.assignment_id]);
    let reopened =
        RuntimeSessionStore::open(fixture.root.join("runtime-sessions.json"), &fixture.root)
            .unwrap();
    assert!(
        reopened
            .list_turns(waiting.child_session_id)
            .unwrap()
            .is_empty()
    );
    fixture.finish(&first, RuntimeTaskStatus::Completed);
    reconcile::tick(&fixture.state).unwrap();
    let ready = fixture.get(waiting.assignment_id);
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            let task = &mut ledger
                .assignments
                .get_mut(&waiting.assignment_id)
                .unwrap()
                .task;
            task.child_turn_id = None;
            task.status = ChildSessionTaskStatus::Preparing;
            Ok(())
        })
        .unwrap();
    apply(
        &fixture,
        &control_request(&fixture, &ready, "session.child.takeover", 1),
    );
    let recovered = fixture.get(waiting.assignment_id);
    assert_eq!(recovered.child_turn_id, ready.child_turn_id);
    assert_eq!(recovered.status, ChildSessionTaskStatus::Queued);
    assert!(
        fixture
            .state
            .sessions
            .claim_next(waiting.child_session_id)
            .unwrap()
            .is_some()
    );
}
