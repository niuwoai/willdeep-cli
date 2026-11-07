use super::*;

fn propose(fixture: &Fixture) -> (ApiRequest, Uuid) {
    propose_request(fixture, fixture.request())
}

fn propose_request(fixture: &Fixture, mut request: ApiRequest) -> (ApiRequest, Uuid) {
    let (turn, _) = fixture
        .state
        .sessions
        .enqueue_turn(
            fixture.parent,
            session_store::CreateRuntimeTurn {
                request_id: Uuid::new_v4(),
                team_control: None,
                prompt: "Create a child to investigate recovery".into(),
                attachments: Vec::new(),
                origin_client: Some("web:proposal-test".into()),
                defer_until_released: false,
            },
        )
        .unwrap();
    fixture
        .state
        .sessions
        .claim_next(fixture.parent)
        .unwrap()
        .unwrap();
    request.params["created_from_turn_id"] = serde_json::json!(turn.id);
    let result = dispatch_for_test(&fixture.state, &request).unwrap();
    assert_eq!(result["status"], "awaiting_user_start");
    assert_eq!(result["proposal_id"], request.request_id.to_string());
    (request, turn.id)
}

fn action(fixture: &Fixture, id: Uuid, operation: &str) -> ApiRequest {
    ApiRequest::new(
        operation,
        serde_json::json!({"parent_session_id":fixture.parent,"proposal_id":id}),
    )
}

#[test]
fn web_proposal_is_durable_without_child_execution_and_replays_once() {
    let fixture = Fixture::new();
    let (request, _) = propose(&fixture);
    dispatch_for_test(&fixture.state, &request).unwrap();
    let disk = ChildStore::new(&fixture.root);
    disk.transaction(|ledger| {
        assert_eq!(ledger.proposals.len(), 1);
        assert!(ledger.assignments.is_empty());
        assert!(ledger.links.is_empty());
        Ok(())
    })
    .unwrap();
    assert_eq!(fixture.state.sessions.list().unwrap().len(), 1);
    let list = ApiRequest::new(
        "session.child.proposal.list",
        serde_json::json!({"parent_session_id":fixture.parent}),
    );
    let result = dispatch_for_test(&fixture.state, &list).unwrap();
    assert_eq!(
        result["proposals"][0]["params"]["prompt"],
        request.params["prompt"]
    );
}

#[test]
fn confirmation_after_parent_finishes_survives_repeat_clicks_with_new_request_ids() {
    let fixture = Fixture::new();
    let (request, turn) = propose(&fixture);
    let task = Uuid::new_v4();
    fixture.state.sessions.bind_task(turn, task).unwrap();
    fixture
        .state
        .sessions
        .complete_task(task, RuntimeTaskStatus::Completed, None)
        .unwrap();
    let first = dispatch_for_test(
        &fixture.state,
        &action(&fixture, request.request_id, "session.child.proposal.start"),
    )
    .unwrap();
    let second = dispatch_for_test(
        &fixture.state,
        &action(&fixture, request.request_id, "session.child.proposal.start"),
    )
    .unwrap();
    assert_eq!(first["assignment_id"], second["assignment_id"]);
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            assert_eq!(ledger.assignments.len(), 1);
            assert_eq!(ledger.links.len(), 1);
            Ok(())
        })
        .unwrap();
    let list = ApiRequest::new(
        "session.child.proposal.list",
        serde_json::json!({"parent_session_id":fixture.parent}),
    );
    assert!(
        dispatch_for_test(&fixture.state, &list).unwrap()["proposals"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        dispatch_for_test(
            &fixture.state,
            &action(
                &fixture,
                request.request_id,
                "session.child.proposal.dismiss"
            )
        )
        .is_err()
    );
}

#[test]
fn dismissed_or_foreign_proposals_cannot_start() {
    let fixture = Fixture::new();
    let (request, _) = propose(&fixture);
    let mut foreign = action(&fixture, request.request_id, "session.child.proposal.start");
    foreign.params["parent_session_id"] = serde_json::json!(Uuid::new_v4());
    assert!(dispatch_for_test(&fixture.state, &foreign).is_err());
    dispatch_for_test(
        &fixture.state,
        &action(
            &fixture,
            request.request_id,
            "session.child.proposal.dismiss",
        ),
    )
    .unwrap();
    assert!(
        dispatch_for_test(
            &fixture.state,
            &action(&fixture, request.request_id, "session.child.proposal.start")
        )
        .is_err()
    );
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            assert!(ledger.assignments.is_empty());
            Ok(())
        })
        .unwrap();
}

#[test]
fn rejected_admission_retains_the_original_proposal() {
    let fixture = Fixture::new();
    let (request, _) = propose(&fixture);
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            // Admission must still validate dependencies after a delayed confirmation.
            ledger
                .proposals
                .get_mut(&request.request_id)
                .unwrap()
                .params
                .depends_on = vec![Uuid::new_v4()];
            Ok(())
        })
        .unwrap();
    assert!(
        dispatch_for_test(
            &fixture.state,
            &action(&fixture, request.request_id, "session.child.proposal.start")
        )
        .is_err()
    );
    let list = ApiRequest::new(
        "session.child.proposal.list",
        serde_json::json!({"parent_session_id":fixture.parent}),
    );
    assert_eq!(
        dispatch_for_test(&fixture.state, &list).unwrap()["proposals"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

fn stale_fork_proposal(fixture: &Fixture) -> (ApiRequest, u64) {
    let through = complete_parent_turn(
        fixture,
        vec![
            willdeep_core::Message::user("question"),
            willdeep_core::Message::assistant("answer", Vec::new()),
        ],
    );
    let (request, _) = propose_request(fixture, fork_request(fixture, through));
    let store = willdeep_core::SessionStore::new(&fixture.root);
    let mut core = store.load_baselined(fixture.parent).unwrap();
    let original_generation = core.compression_generation;
    core.compression_generation += 1;
    store.save(&mut core).unwrap();
    (request, original_generation)
}

fn pending_proposals(fixture: &Fixture) -> serde_json::Value {
    dispatch_for_test(
        &fixture.state,
        &ApiRequest::new(
            "session.child.proposal.list",
            serde_json::json!({"parent_session_id": fixture.parent}),
        ),
    )
    .unwrap()
}

#[test]
fn failed_preparation_is_durable_visible_paused_and_dismissible() {
    let fixture = Fixture::new();
    let (request, _) = stale_fork_proposal(&fixture);
    for _ in 0..2 {
        let error = dispatch_for_test(
            &fixture.state,
            &action(&fixture, request.request_id, "session.child.proposal.start"),
        )
        .unwrap_err();
        assert!(format!("{error:?}").contains("compression"));
        let visible = pending_proposals(&fixture);
        assert_eq!(visible["proposals"].as_array().unwrap().len(), 1);
        assert!(
            visible["proposals"][0]["error"]
                .as_str()
                .unwrap()
                .contains("compression")
        );
        // A background tick must not endlessly retry the stale fork boundary.
        reconcile::tick(&fixture.state).unwrap();
    }
    let disk = ChildStore::new(&fixture.root);
    let id = disk
        .transaction(|ledger| {
            assert_eq!(ledger.assignments.len(), 1);
            let task = &ledger.assignments.values().next().unwrap().task;
            assert!(task.accepted_at.is_none());
            assert!(task.child_turn_id.is_none());
            assert!(task.error.is_some());
            Ok(task.assignment_id)
        })
        .unwrap();
    dispatch_for_test(
        &fixture.state,
        &action(
            &fixture,
            request.request_id,
            "session.child.proposal.dismiss",
        ),
    )
    .unwrap();
    assert!(
        pending_proposals(&fixture)["proposals"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    reconcile::tick(&fixture.state).unwrap();
    let task = fixture.get(id);
    assert_eq!(task.status, ChildSessionTaskStatus::Cancelled);
    assert_eq!(task.subscription, ChildSessionSubscription::None);
    assert!(task.child_turn_id.is_none());
    assert!(
        dispatch_for_test(
            &fixture.state,
            &action(&fixture, request.request_id, "session.child.proposal.start",)
        )
        .is_err()
    );
}

#[test]
fn corrected_preparation_retries_concurrently_with_one_assignment_and_turn() {
    let fixture = Fixture::new();
    let (request, original_generation) = stale_fork_proposal(&fixture);
    assert!(
        dispatch_for_test(
            &fixture.state,
            &action(&fixture, request.request_id, "session.child.proposal.start",)
        )
        .is_err()
    );
    let original_id = fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            Ok(ledger
                .assignments
                .values()
                .next()
                .unwrap()
                .task
                .assignment_id)
        })
        .unwrap();
    let store = willdeep_core::SessionStore::new(&fixture.root);
    let mut core = store.load_baselined(fixture.parent).unwrap();
    core.compression_generation = original_generation;
    store.save(&mut core).unwrap();
    let state = &fixture.state;
    let parent = fixture.parent;
    let proposal = request.request_id;
    let values = std::thread::scope(|scope| {
        (0..3)
            .map(|_| {
                scope.spawn(move || {
                    dispatch_for_test(state, &ApiRequest::new(
                "session.child.proposal.start",
                serde_json::json!({"parent_session_id": parent, "proposal_id": proposal}),
            )).unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>()
    });
    for value in &values {
        assert_eq!(value["assignment_id"], original_id.to_string());
        assert_eq!(value["child_turn_id"], values[0]["child_turn_id"]);
    }
    let task = fixture.get(original_id);
    assert!(task.child_turn_id.is_some());
    assert!(task.error.is_none());
    assert_eq!(
        fixture
            .state
            .sessions
            .list_turns(task.child_session_id)
            .unwrap()
            .len(),
        1
    );
    assert!(
        pending_proposals(&fixture)["proposals"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn dismiss_cannot_cancel_committed_turn_when_acceptance_receipt_is_missing() {
    let fixture = Fixture::new();
    let (request, _) = propose(&fixture);
    let value = dispatch_for_test(
        &fixture.state,
        &action(&fixture, request.request_id, "session.child.proposal.start"),
    )
    .unwrap();
    let task: ChildSessionTask = serde_json::from_value(value).unwrap();
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            let stored = &mut ledger
                .assignments
                .get_mut(&task.assignment_id)
                .unwrap()
                .task;
            stored.accepted_at = None;
            stored.child_turn_id = None;
            stored.status = ChildSessionTaskStatus::Preparing;
            Ok(())
        })
        .unwrap();
    assert!(
        dispatch_for_test(
            &fixture.state,
            &action(
                &fixture,
                request.request_id,
                "session.child.proposal.dismiss",
            )
        )
        .is_err()
    );
    let actual = fixture
        .state
        .sessions
        .get_turn(task.child_turn_id.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, RuntimeTurnStatus::Queued);
    let repeated = dispatch_for_test(
        &fixture.state,
        &action(&fixture, request.request_id, "session.child.proposal.start"),
    )
    .unwrap();
    assert_eq!(repeated["assignment_id"], task.assignment_id.to_string());
    assert_eq!(
        repeated["child_turn_id"],
        task.child_turn_id.unwrap().to_string()
    );
}

#[test]
fn non_web_starts_preserve_existing_execution_behavior() {
    let fixture = Fixture::new();
    let task = fixture.start();
    assert!(task.child_turn_id.is_some());
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            assert!(ledger.proposals.is_empty());
            Ok(())
        })
        .unwrap();
}
