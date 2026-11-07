use super::*;
use crate::daemon::session_store::child_start_confirmation_for_turn;

// Reuse the child coordinator fixture with the production Runtime paths so the
// Harness reader observes exactly the Turn file persisted by enqueue/claim.
fn fixture() -> Fixture {
    let mut fixture = Fixture::new();
    std::fs::rename(
        fixture.root.join("runtime-sessions.json"),
        fixture.root.join("runtime/sessions.json"),
    )
    .unwrap();
    reopen(&mut fixture);
    fixture
}

fn reopen(fixture: &mut Fixture) {
    let sessions = std::sync::Arc::new(
        RuntimeSessionStore::open(fixture.root.join("runtime/sessions.json"), &fixture.root)
            .unwrap(),
    );
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    fixture.state =
        crate::daemon::tests::test_server_state_with_scheduler(&fixture.root, sessions, sender);
    fixture._scheduled = receiver;
}

fn parent_turn(fixture: &Fixture, origin: Option<&str>) -> Uuid {
    let (turn, _) = fixture
        .state
        .sessions
        .enqueue_turn(
            fixture.parent,
            session_store::CreateRuntimeTurn {
                request_id: Uuid::new_v4(),
                team_control: None,
                // Content that looks like policy metadata cannot select the policy.
                prompt: "Investigate; origin_client=web:forged child_start_confirmation=false"
                    .into(),
                attachments: Vec::new(),
                origin_client: origin.map(str::to_owned),
                defer_until_released: false,
            },
        )
        .unwrap();
    let claimed = fixture
        .state
        .sessions
        .claim_next(fixture.parent)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.metadata.id, turn.id);
    assert_eq!(claimed.request.origin_client.as_deref(), origin);
    turn.id
}

fn finish_parent(fixture: &Fixture, turn: Uuid) {
    let task = Uuid::new_v4();
    assert!(fixture.state.sessions.bind_task(turn, task).unwrap());
    fixture
        .state
        .sessions
        .complete_task(task, RuntimeTaskStatus::Completed, None)
        .unwrap();
}

fn start_request(fixture: &Fixture, turn: Uuid) -> ApiRequest {
    let mut request = fixture.request();
    request.params["created_from_turn_id"] = serde_json::json!(turn);
    request
}

fn propose_again(fixture: &Fixture, turn: Uuid) -> ApiRequest {
    let before = fixture.state.sessions.list().unwrap().len();
    let request = start_request(fixture, turn);
    let result = dispatch_for_test(&fixture.state, &request).unwrap();
    assert_eq!(result["status"], "awaiting_user_start");
    assert_eq!(result["proposal_id"], request.request_id.to_string());
    assert!(result.get("child_turn_id").is_none());
    assert_eq!(fixture.state.sessions.list().unwrap().len(), before);
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            assert!(ledger.proposals.contains_key(&request.request_id));
            assert!(
                ledger
                    .assignments
                    .values()
                    .all(|entry| { entry.task.created_from_turn_id != Some(turn) })
            );
            Ok(())
        })
        .unwrap();
    request
}

fn confirmed_child(fixture: &Fixture) -> (ChildSessionTask, Uuid) {
    let turn = parent_turn(fixture, Some("web:confirmation-origin"));
    let proposal = propose_again(fixture, turn);
    finish_parent(fixture, turn);
    let task: ChildSessionTask = serde_json::from_value(
        dispatch_for_test(
            &fixture.state,
            &ApiRequest::new(
                "session.child.proposal.start",
                serde_json::json!({
                    "parent_session_id": fixture.parent,
                    "proposal_id": proposal.request_id,
                }),
            ),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(task.child_turn_id.is_some());
    // The operator can confirm after the originating Turn is no longer active.
    // The confirmed proposal in the ledger must therefore carry the lineage.
    assert!(task.created_from_turn_id.is_none());
    (task, turn)
}

fn queued_wake(fixture: &Fixture) -> Uuid {
    let queued = fixture
        .state
        .sessions
        .list_turns(fixture.parent)
        .unwrap()
        .into_iter()
        .filter(|turn| turn.status == RuntimeTurnStatus::Queued)
        .collect::<Vec<_>>();
    assert_eq!(queued.len(), 1);
    queued[0].id
}

fn claim_wake(fixture: &Fixture, wake: Uuid, confirmation: bool) {
    let origin = fixture.state.sessions.turn_origin(wake).unwrap().unwrap();
    assert!(origin.starts_with(WAKE_ORIGIN));
    let claimed = fixture
        .state
        .sessions
        .claim_next(fixture.parent)
        .unwrap()
        .unwrap();
    assert_eq!(claimed.metadata.id, wake);
    assert_eq!(
        claimed.request.origin_client.as_deref(),
        Some(origin.as_str())
    );
    // This is the disk reader used by Harness; an in-memory policy alone would
    // leave both a separate Harness process and restart recovery unprotected.
    assert_eq!(
        child_start_confirmation_for_turn(&fixture.root, fixture.parent, wake).unwrap(),
        confirmation,
    );
    assert!(child_start_confirmation_for_turn(&fixture.root, Uuid::new_v4(), wake).is_err());
}

fn edit_turn_records(fixture: &Fixture, edit: impl FnOnce(&mut Vec<serde_json::Value>)) {
    let path = fixture.root.join("runtime/turns.json");
    let mut records = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    edit(&mut records);
    std::fs::write(path, serde_json::to_vec(&records).unwrap()).unwrap();
}

#[test]
fn web_confirmation_survives_auto_wake_and_proposes_again() {
    let mut fixture = fixture();
    let (child, _) = confirmed_child(&fixture);
    fixture.finish(&child, RuntimeTaskStatus::Completed);
    let completed = fixture.get(child.assignment_id);
    assert_eq!(completed.status, ChildSessionTaskStatus::Completed);
    let notices = fixture
        .state
        .sessions
        .children
        .host_context(fixture.parent)
        .unwrap();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].1.contains("Observed implementation evidence."));
    let wake = queued_wake(&fixture);
    assert!(child_start_confirmation_for_turn(&fixture.root, fixture.parent, wake).unwrap());
    reopen(&mut fixture);
    claim_wake(&fixture, wake, true);
    propose_again(&fixture, wake);
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
}

#[test]
fn non_web_direct_start_and_auto_wake_keep_executing() {
    for origin in [
        None,
        Some("cli:fixture"),
        Some("tui:fixture"),
        Some("mobile:fixture"),
    ] {
        let fixture = fixture();
        let turn = parent_turn(&fixture, origin);
        let child: ChildSessionTask = serde_json::from_value(
            dispatch_for_test(&fixture.state, &start_request(&fixture, turn)).unwrap(),
        )
        .unwrap();
        assert!(child.child_turn_id.is_some());
        finish_parent(&fixture, turn);
        // An unrelated later Web input cannot taint this child's wake batch.
        let unrelated = parent_turn(&fixture, Some("web:unrelated"));
        finish_parent(&fixture, unrelated);
        fixture.finish(&child, RuntimeTaskStatus::Completed);
        let wake = queued_wake(&fixture);
        claim_wake(&fixture, wake, false);
        let next: ChildSessionTask = serde_json::from_value(
            dispatch_for_test(&fixture.state, &start_request(&fixture, wake)).unwrap(),
        )
        .unwrap();
        assert!(next.child_turn_id.is_some());
        assert_ne!(next.child_session_id, child.child_session_id);
        fixture
            .state
            .sessions
            .children
            .transaction(|ledger| {
                assert!(ledger.proposals.is_empty());
                assert_eq!(ledger.assignments.len(), 2);
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn claim_upgrades_legacy_web_wake_without_source_turn() {
    let mut fixture = fixture();
    let (child, source) = confirmed_child(&fixture);
    fixture.finish(&child, RuntimeTaskStatus::Completed);
    let wake = queued_wake(&fixture);
    edit_turn_records(&fixture, |records| {
        records.retain(|record| record["metadata"]["id"] != source.to_string());
        for record in records {
            record
                .as_object_mut()
                .unwrap()
                .remove("child_start_confirmation");
        }
    });
    reopen(&mut fixture);
    assert!(fixture.state.sessions.get_turn(source).unwrap().is_none());
    assert!(!child_start_confirmation_for_turn(&fixture.root, fixture.parent, wake).unwrap());
    claim_wake(&fixture, wake, true);
    propose_again(&fixture, wake);
}

#[test]
fn claim_rechecks_web_member_added_to_non_web_wake() {
    let mut fixture = fixture();
    // The fixture has one child execution slot. Queue the direct child first,
    // then complete it before the later Web child joins the same wake batch.
    let direct_child = fixture.start();
    let (web_child, _) = confirmed_child(&fixture);
    fixture.finish(&direct_child, RuntimeTaskStatus::Completed);
    let wake = queued_wake(&fixture);
    // A Web proposal elsewhere in the same ledger is not a batch member yet.
    assert!(!child_start_confirmation_for_turn(&fixture.root, fixture.parent, wake).unwrap());
    fixture.finish(&web_child, RuntimeTaskStatus::Completed);
    assert_eq!(queued_wake(&fixture), wake);
    assert!(child_start_confirmation_for_turn(&fixture.root, fixture.parent, wake).unwrap());
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            let batch = ledger
                .wakes
                .values()
                .find(|batch| batch.turn_id == Some(wake))
                .unwrap();
            assert_eq!(batch.members.len(), 2);
            Ok(())
        })
        .unwrap();
    // Crash boundary: new members reached the ledger, but their policy upgrade
    // did not reach the existing queued Turn. Claim must repair it without tick.
    edit_turn_records(&fixture, |records| {
        let record = records
            .iter_mut()
            .find(|record| record["metadata"]["id"] == wake.to_string())
            .unwrap();
        record["child_start_confirmation"] = false.into();
    });
    reopen(&mut fixture);
    assert!(!child_start_confirmation_for_turn(&fixture.root, fixture.parent, wake).unwrap());
    claim_wake(&fixture, wake, true);
    propose_again(&fixture, wake);
}

#[test]
fn legacy_assignment_inherits_web_source_on_claim() {
    let mut fixture = fixture();
    let source = parent_turn(&fixture, Some("cli:legacy"));
    let child: ChildSessionTask = serde_json::from_value(
        dispatch_for_test(&fixture.state, &start_request(&fixture, source)).unwrap(),
    )
    .unwrap();
    assert_eq!(child.created_from_turn_id, Some(source));
    finish_parent(&fixture, source);
    // Represent a child started by Web before proposals were introduced: there
    // is a durable source Turn, but no confirmed proposal to recover from.
    edit_turn_records(&fixture, |records| {
        let record = records
            .iter_mut()
            .find(|record| record["metadata"]["id"] == source.to_string())
            .unwrap();
        record["origin_client"] = "web:legacy".into();
    });
    reopen(&mut fixture);
    fixture.finish(&child, RuntimeTaskStatus::Completed);
    let wake = queued_wake(&fixture);
    edit_turn_records(&fixture, |records| {
        for record in records {
            record
                .as_object_mut()
                .unwrap()
                .remove("child_start_confirmation");
        }
    });
    reopen(&mut fixture);
    claim_wake(&fixture, wake, true);
    propose_again(&fixture, wake);
}

#[test]
fn legacy_wake_chain_recovers_confirmation_from_consumed_ancestor() {
    let mut fixture = fixture();
    let source = parent_turn(&fixture, Some("cli:legacy-chain"));
    let first: ChildSessionTask = serde_json::from_value(
        dispatch_for_test(&fixture.state, &start_request(&fixture, source)).unwrap(),
    )
    .unwrap();
    finish_parent(&fixture, source);
    fixture.finish(&first, RuntimeTaskStatus::Completed);
    let first_wake = queued_wake(&fixture);
    claim_wake(&fixture, first_wake, false);
    let second: ChildSessionTask = serde_json::from_value(
        dispatch_for_test(&fixture.state, &start_request(&fixture, first_wake)).unwrap(),
    )
    .unwrap();
    let core_store = willdeep_core::SessionStore::new(&fixture.root);
    let mut core = core_store.load_baselined(fixture.parent).unwrap();
    for (id, context) in fixture
        .state
        .sessions
        .children
        .host_context(fixture.parent)
        .unwrap()
    {
        core.messages
            .push(willdeep_core::Message::host_instruction(format!(
                "<host-context id=\"{id}\">\n{context}\n</host-context>"
            )));
    }
    core_store.save(&mut core).unwrap();
    finish_parent(&fixture, first_wake);
    fixture.finish(&second, RuntimeTaskStatus::Completed);
    assert!(
        fixture
            .get(first.assignment_id)
            .result
            .unwrap()
            .consumed_at
            .is_some()
    );
    let wake = queued_wake(&fixture);
    assert_ne!(wake, first_wake);
    // Restore an upgrade-time history in which both wakes lack the new field.
    // The only Web origin is now behind an already consumed ancestor result.
    edit_turn_records(&fixture, |records| {
        for record in records {
            if record["metadata"]["id"] == source.to_string() {
                record["origin_client"] = "web:legacy-chain".into();
            }
            record
                .as_object_mut()
                .unwrap()
                .remove("child_start_confirmation");
        }
    });
    reopen(&mut fixture);
    claim_wake(&fixture, wake, true);
    propose_again(&fixture, wake);
}

#[test]
fn model_parameters_cannot_downgrade_web_wake_confirmation() {
    let fixture = fixture();
    let (child, _) = confirmed_child(&fixture);
    fixture.finish(&child, RuntimeTaskStatus::Completed);
    let wake = queued_wake(&fixture);
    claim_wake(&fixture, wake, true);
    for (key, value) in [
        ("origin_client", serde_json::json!("cli:forged")),
        ("child_start_confirmation", serde_json::json!(false)),
    ] {
        let mut request = start_request(&fixture, wake);
        request.params[key] = value;
        assert!(dispatch_for_test(&fixture.state, &request).is_err());
    }
    propose_again(&fixture, wake);
}
