use super::*;

fn propose(fixture: &Fixture) -> (ApiRequest, Uuid) {
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
    let mut request = fixture.request();
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
