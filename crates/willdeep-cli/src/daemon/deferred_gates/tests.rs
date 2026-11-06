use super::*;
use willdeep_core::{Message, SessionStore};
use willdeep_runtime_protocol::{QueueAction, UpdateTurnQueueParams};

fn event_count(state: &ServerState, kind: &str) -> usize {
    state
        .events
        .read_after(0, usize::MAX)
        .unwrap()
        .iter()
        .filter(|event| event.kind == kind)
        .count()
}

fn queue_action(
    state: &ServerState,
    session_id: uuid::Uuid,
    message_id: Option<uuid::Uuid>,
    action: QueueAction,
) {
    state
        .sessions
        .update_turn_queue(&UpdateTurnQueueParams {
            session_id,
            action,
            message_id,
            text: None,
            expected_text: None,
        })
        .unwrap();
}

async fn waiting_approval(
    state: &ServerState,
    session: uuid::Uuid,
    parked: bool,
) -> (
    RuntimeInteraction,
    tokio::sync::oneshot::Receiver<InteractionResolution>,
) {
    let original = enqueue(state, session, "original task");
    state.sessions.claim_next(session).unwrap().unwrap();
    let task = bind(state, session, original.id, false).await;
    let receiver = state
        .tasks
        .create_interaction(
            task,
            InteractionKind::Approval {
                description: "pending operation".into(),
                always_allow_available: false,
            },
        )
        .await
        .unwrap();
    let gate = state
        .tasks
        .pending_interactions()
        .await
        .into_iter()
        .find(|gate| gate.task_id == task)
        .unwrap();
    let gate_id = gate.id;
    let _guard = state.tasks.persistence.lock().await;
    state
        .tasks
        .interactions
        .write()
        .await
        .get_mut(&gate.id)
        .unwrap()
        .foreground_since = Some(now().saturating_sub(2));
    if parked {
        state.tasks.park_gates(&[gate]).await.unwrap();
    }
    (
        state
            .tasks
            .pending_interactions()
            .await
            .into_iter()
            .find(|gate| gate.id == gate_id)
            .unwrap(),
        receiver,
    )
}

async fn assert_gate_preserved(
    state: &ServerState,
    expected: &RuntimeInteraction,
    receiver: &mut tokio::sync::oneshot::Receiver<InteractionResolution>,
    parked: bool,
) -> RuntimeInteraction {
    let pending = state.tasks.pending_interactions().await;
    let gate = pending.iter().find(|gate| gate.id == expected.id).unwrap();
    assert_eq!(gate.id, expected.id);
    assert_eq!(gate.task_id, expected.task_id);
    assert_eq!(gate.kind, expected.kind);
    assert_eq!(gate.created_at, expected.created_at);
    assert_eq!(gate.status, InteractionStatus::Pending);
    assert!(gate.resolution.is_none());
    assert!(gate.resolved_at.is_none());
    assert_eq!(gate.parked, parked);
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    gate.clone()
}

fn state() -> (PathBuf, Arc<ServerState>, uuid::Uuid) {
    let home =
        std::env::temp_dir().join(format!("willdeep-deferred-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(home.join("runtime")).unwrap();
    std::fs::create_dir_all(home.join("project")).unwrap();
    let config = home.join("config.toml");
    std::fs::write(
        &config,
        "version = 1\n[agent]\nhuman_wait_yield_seconds = 1\n",
    )
    .unwrap();
    let sessions = Arc::new(
        session_store::RuntimeSessionStore::open(home.join("runtime/sessions.json"), &home)
            .unwrap(),
    );
    let session = sessions
        .create(session_store::CreateRuntimeSession {
            id: None,
            workspace: home.join("project"),
            profile: None,
            model: None,
            config: Some(config),
            title: None,
        })
        .unwrap();
    let state = Arc::new(super::super::tests::test_server_state(&home, sessions));
    (home, state, session.id)
}

fn enqueue(state: &ServerState, session: uuid::Uuid, prompt: &str) -> session_store::RuntimeTurn {
    state
        .sessions
        .enqueue_turn(
            session,
            session_store::CreateRuntimeTurn {
                team_control: None,
                request_id: uuid::Uuid::new_v4(),
                prompt: prompt.to_owned(),
                attachments: Vec::new(),
                origin_client: Some("tui:test".to_owned()),
                defer_until_released: false,
            },
        )
        .unwrap()
        .0
}

fn claim_queued(
    state: &ServerState,
    session_id: uuid::Uuid,
    message_id: uuid::Uuid,
) -> Option<session_store::ClaimedRuntimeTurn> {
    let session = state.sessions.get(session_id).unwrap().unwrap();
    let message = state
        .sessions
        .queued_messages(session_id)
        .unwrap()
        .into_iter()
        .find(|message| message.id == message_id)
        .unwrap();
    state
        .sessions
        .claim_independent(&session, &message)
        .unwrap()
}

async fn bind(
    state: &ServerState,
    session: uuid::Uuid,
    turn: uuid::Uuid,
    independent: bool,
) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    let source = state.sessions.get(session).unwrap().unwrap();
    if independent {
        state
            .agents
            .ensure_root(
                id,
                source.workspace.clone(),
                None,
                None,
                RuntimeAgentStatus::Running,
            )
            .unwrap();
    } else {
        state
            .agents
            .ensure_session_root(
                source.root_agent_id,
                id,
                source.workspace.clone(),
                None,
                None,
                RuntimeAgentStatus::Running,
            )
            .unwrap();
    }
    let task: RuntimeTask = serde_json::from_value(serde_json::json!({ "id":id,"session_id":session,"turn_id":turn,"status":"running","workspace":source.workspace,"created_at":now() })).unwrap();
    state.tasks.tasks.write().await.insert(id, task);
    assert!(state.sessions.bind_task(turn, id).unwrap());
    id
}

#[test]
fn timeout_is_inclusive_configurable_and_requires_queued_work() {
    assert!(!should_yield(1000, 1299, 300, true));
    assert!(should_yield(1000, 1300, 300, true));
    assert!(!should_yield(1000, 2000, 0, true));
    assert!(!should_yield(1000, 2000, 300, false));
    assert!(!should_yield(2000, 1000, 300, true));
}

#[tokio::test]
async fn stopped_session_cannot_claim_new_independent_work() {
    let (home, state, session) = state();
    let original = enqueue(&state, session, "original");
    state.sessions.claim_next(session).unwrap().unwrap();
    let task = bind(&state, session, original.id, false).await;
    let _receiver = state
        .tasks
        .create_interaction(
            task,
            InteractionKind::Approval {
                description: "pending operation".into(),
                always_allow_available: false,
            },
        )
        .await
        .unwrap();
    let next = enqueue(&state, session, "independent");
    state.sessions.set_yield_suppressed(session, true).unwrap();
    assert!(claim_queued(&state, session, next.id).is_none());
    assert_eq!(
        state.sessions.queued_messages(session).unwrap()[0].id,
        next.id
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn unknown_queue_keeps_the_gate_visible_without_answering_or_representing_it() {
    let (home, state, session) = state();
    let original = enqueue(&state, session, "original");
    state.sessions.claim_next(session).unwrap().unwrap();
    let task = bind(&state, session, original.id, false).await;
    let mut receiver = state
        .tasks
        .create_interaction(
            task,
            InteractionKind::Approval {
                description: "pending operation".into(),
                always_allow_available: false,
            },
        )
        .await
        .unwrap();
    let gate = state.tasks.pending_interactions().await[0].id;
    state
        .tasks
        .interactions
        .write()
        .await
        .get_mut(&gate)
        .unwrap()
        .foreground_since = Some(now().saturating_sub(2));
    let before = state.tasks.pending_interactions().await;
    // Context is intentionally too large to classify, so no provider is needed.
    for _ in 0..3 {
        enqueue(
            &state,
            session,
            &"x".repeat(MAX_CLASSIFIER_CONTEXT_BYTES + 1),
        );
    }
    let queued = state.sessions.queued_messages(session).unwrap();
    let mut checked = BTreeMap::new();
    for _ in 0..2 {
        tick(&state, &mut checked).await.unwrap();
        assert_eq!(state.tasks.pending_interactions().await, before);
    }
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(state.sessions.queued_messages(session).unwrap(), queued);
    assert_eq!(state.tasks.list().await.len(), 1);
    assert_eq!(event_count(&state, "task.interactions_parked"), 0);
    assert_eq!(event_count(&state, "task.interaction_presented"), 0);
    assert_eq!(checked.len(), 3);
    assert!(checked.values().all(|entry| *entry == (0, false)));
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn parked_gate_recovers_when_candidates_are_rejected_or_classification_fails() {
    #[derive(Clone, Copy)]
    enum Candidate {
        Unknown,
        Dependent,
        ClassificationError,
    }
    for candidate in [
        Candidate::Unknown,
        Candidate::Dependent,
        Candidate::ClassificationError,
    ] {
        let (home, state, session) = state();
        let (gate, mut receiver) = waiting_approval(&state, session, true).await;
        enqueue(
            &state,
            session,
            &"x".repeat(MAX_CLASSIFIER_CONTEXT_BYTES + 1),
        );
        let queued = state.sessions.queued_messages(session).unwrap();
        let fingerprint =
            serde_json::to_string(&(session, std::slice::from_ref(&gate), &queued[0])).unwrap();
        let mut checked = BTreeMap::new();
        match candidate {
            Candidate::Unknown => {}
            Candidate::Dependent => {
                checked.insert(fingerprint.clone(), (0, false));
            }
            Candidate::ClassificationError => {
                // Fail before provider/configuration access, using only fixture data.
                assert!(SessionStore::new(&home).delete(session).unwrap());
            }
        }
        tick(&state, &mut checked).await.unwrap();
        let restored = assert_gate_preserved(&state, &gate, &mut receiver, false).await;
        assert!(restored.foreground_since.unwrap() > gate.foreground_since.unwrap());
        let (at, allowed) = checked[&fingerprint];
        assert!(!allowed);
        assert_eq!(at > 0, matches!(candidate, Candidate::ClassificationError));
        tick(&state, &mut checked).await.unwrap();
        assert_eq!(state.tasks.pending_interactions().await, vec![restored]);
        assert_eq!(state.sessions.queued_messages(session).unwrap(), queued);
        assert_eq!(state.tasks.list().await.len(), 1);
        assert_eq!(event_count(&state, "task.interactions_parked"), 1);
        assert_eq!(event_count(&state, "task.interaction_presented"), 1);
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[tokio::test]
async fn parked_gate_recovers_with_empty_cleared_held_or_ineligible_queue() {
    #[derive(Clone, Copy)]
    enum Queue {
        Empty,
        Cleared,
        Held,
        BeforeTimeout,
        YieldDisabled,
    }
    for queue in [
        Queue::Empty,
        Queue::Cleared,
        Queue::Held,
        Queue::BeforeTimeout,
        Queue::YieldDisabled,
    ] {
        let (home, state, session) = state();
        let (gate, mut receiver) = waiting_approval(&state, session, true).await;
        if !matches!(queue, Queue::Empty) {
            state
                .sessions
                .enqueue_turn(
                    session,
                    session_store::CreateRuntimeTurn {
                        team_control: None,
                        request_id: uuid::Uuid::new_v4(),
                        prompt: "x".repeat(MAX_CLASSIFIER_CONTEXT_BYTES + 1),
                        attachments: Vec::new(),
                        origin_client: Some("tui:test".into()),
                        defer_until_released: matches!(queue, Queue::Held),
                    },
                )
                .unwrap();
        }
        match queue {
            Queue::Cleared => queue_action(&state, session, None, QueueAction::Clear),
            Queue::BeforeTimeout | Queue::YieldDisabled => {
                let seconds = if matches!(queue, Queue::YieldDisabled) {
                    0
                } else {
                    u64::from(u32::MAX)
                };
                std::fs::write(
                    home.join("config.toml"),
                    format!("version = 1\n[agent]\nhuman_wait_yield_seconds = {seconds}\n"),
                )
                .unwrap();
            }
            _ => {}
        }
        let queued = state.sessions.queued_messages(session).unwrap();
        let mut checked = BTreeMap::new();
        tick(&state, &mut checked).await.unwrap();
        let restored = assert_gate_preserved(&state, &gate, &mut receiver, false).await;
        tick(&state, &mut checked).await.unwrap();
        assert_eq!(state.tasks.pending_interactions().await, vec![restored]);
        assert_eq!(state.sessions.queued_messages(session).unwrap(), queued);
        assert!(checked.is_empty(), "ineligible work must not be classified");
        assert!(!state.sessions.has_running_lane(session).unwrap());
        assert_eq!(event_count(&state, "task.interactions_parked"), 1);
        assert_eq!(event_count(&state, "task.interaction_presented"), 1);
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[tokio::test]
async fn claim_none_never_hides_a_visible_gate_and_repairs_a_parked_gate_once() {
    for parked in [false, true] {
        let (home, state, session) = state();
        let (gate, mut receiver) = waiting_approval(&state, session, parked).await;
        // A legacy context-only message has no editable user text. An explicit
        // independent request passes scheduling but cannot claim this message.
        let legacy = format!(
            "{}\nlegacy context without a user boundary",
            willdeep_runtime_protocol::ATTACHED_CONTEXT_HEADER
        );
        let next = enqueue(&state, session, &legacy);
        queue_action(
            &state,
            session,
            Some(next.id),
            QueueAction::RunIndependently,
        );
        enqueue(
            &state,
            session,
            &"x".repeat(MAX_CLASSIFIER_CONTEXT_BYTES + 1),
        );
        let queued = state.sessions.queued_messages(session).unwrap();
        assert!(queued[0].independence_confirmed);
        assert!(!queued[0].can_edit);
        let mut checked = BTreeMap::new();
        tick(&state, &mut checked).await.unwrap();
        assert_eq!(
            checked.len(),
            1,
            "an ineligible message must not stop the scan"
        );
        let restored = assert_gate_preserved(&state, &gate, &mut receiver, false).await;
        tick(&state, &mut checked).await.unwrap();
        assert_eq!(state.tasks.pending_interactions().await, vec![restored]);
        assert_eq!(state.sessions.queued_messages(session).unwrap(), queued);
        assert!(!state.sessions.has_running_lane(session).unwrap());
        assert_eq!(state.tasks.list().await.len(), 1);
        assert_eq!(
            event_count(&state, "task.interactions_parked"),
            usize::from(parked)
        );
        assert_eq!(
            event_count(&state, "task.interaction_presented"),
            usize::from(parked)
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[tokio::test]
async fn claim_error_preserves_the_queue_and_restores_only_a_hidden_gate() {
    for parked in [false, true] {
        let (home, state, session) = state();
        let (gate, mut receiver) = waiting_approval(&state, session, parked).await;
        let next = enqueue(&state, session, "independent request");
        queue_action(
            &state,
            session,
            Some(next.id),
            QueueAction::RunIndependently,
        );
        let queued = state.sessions.queued_messages(session).unwrap();
        // Claim needs the parent snapshot; classification is explicitly bypassed.
        assert!(SessionStore::new(&home).delete(session).unwrap());
        let mut checked = BTreeMap::new();
        assert!(tick(&state, &mut checked).await.is_err());
        let restored = assert_gate_preserved(&state, &gate, &mut receiver, false).await;
        assert!(tick(&state, &mut checked).await.is_err());
        assert_eq!(state.tasks.pending_interactions().await, vec![restored]);
        assert_eq!(state.sessions.queued_messages(session).unwrap(), queued);
        assert!(!state.sessions.has_running_lane(session).unwrap());
        assert_eq!(state.tasks.list().await.len(), 1);
        assert_eq!(
            event_count(&state, "task.interactions_parked"),
            usize::from(parked)
        );
        assert_eq!(
            event_count(&state, "task.interaction_presented"),
            usize::from(parked)
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[tokio::test]
async fn claim_error_in_first_session_does_not_block_later_parked_gate_recovery() {
    let (home, state, session) = state();
    let other = state
        .sessions
        .create(session_store::CreateRuntimeSession {
            id: None,
            workspace: home.join("project"),
            profile: None,
            model: None,
            config: Some(home.join("config.toml")),
            title: None,
        })
        .unwrap();
    // tick uses UUID order, so force the failing session to be visited first.
    let (failing, healthy) = if session < other.id {
        (session, other.id)
    } else {
        (other.id, session)
    };
    let (failed_gate, mut failed_receiver) = waiting_approval(&state, failing, true).await;
    let (healthy_gate, mut healthy_receiver) = waiting_approval(&state, healthy, true).await;
    let next = enqueue(&state, failing, "independent request");
    queue_action(
        &state,
        failing,
        Some(next.id),
        QueueAction::RunIndependently,
    );
    let queued = state.sessions.queued_messages(failing).unwrap();
    assert!(SessionStore::new(&home).delete(failing).unwrap());
    let mut checked = BTreeMap::new();
    for _ in 0..2 {
        let error = tick(&state, &mut checked).await.unwrap_err();
        assert!(error.to_string().contains(&failing.to_string()));
        assert!(error.to_string().contains("start independent work"));
        assert_gate_preserved(&state, &failed_gate, &mut failed_receiver, false).await;
        assert_gate_preserved(&state, &healthy_gate, &mut healthy_receiver, false).await;
        assert_eq!(state.sessions.queued_messages(failing).unwrap(), queued);
        assert!(state.sessions.queued_messages(healthy).unwrap().is_empty());
        assert!(!state.sessions.has_running_lane(failing).unwrap());
        assert!(!state.sessions.has_running_lane(healthy).unwrap());
        assert_eq!(event_count(&state, "task.interaction_presented"), 2);
    }
    assert_eq!(state.tasks.pending_interactions().await.len(), 2);
    assert_eq!(state.tasks.list().await.len(), 2);
    assert!(checked.is_empty());
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn parking_write_failure_releases_the_claim_without_hiding_or_answering_the_gate() {
    let (home, state, session) = state();
    let (gate, mut receiver) = waiting_approval(&state, session, false).await;
    let original = state.sessions.get(session).unwrap().unwrap().active_turn_id;
    let next = enqueue(&state, session, "retain input on parking failure");
    queue_action(
        &state,
        session,
        Some(next.id),
        QueueAction::RunIndependently,
    );
    // Make the atomic interaction-file replacement fail in the isolated fixture.
    std::fs::remove_file(&state.tasks.interactions_path).unwrap();
    std::fs::create_dir(&state.tasks.interactions_path).unwrap();
    assert!(tick(&state, &mut BTreeMap::new()).await.is_err());
    assert_gate_preserved(&state, &gate, &mut receiver, false).await;
    assert!(!state.sessions.has_running_lane(session).unwrap());
    assert_eq!(
        state.sessions.get(session).unwrap().unwrap().active_turn_id,
        original
    );
    let histories =
        session_store::RuntimeSessionStore::independent_history(&home, session).unwrap();
    assert_eq!(
        histories[0].messages[0].content,
        "retain input on parking failure"
    );
    assert_eq!(
        histories[0].status,
        session_store::RuntimeTurnStatus::Failed
    );
    assert_eq!(event_count(&state, "task.interactions_parked"), 0);
    assert_eq!(event_count(&state, "task.interaction_presented"), 0);
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn parked_gate_recovery_obeys_stop_suppression_and_work_gate() {
    for draining in [false, true] {
        let (home, state, session) = state();
        let (gate, mut receiver) = waiting_approval(&state, session, true).await;
        enqueue(
            &state,
            session,
            &"x".repeat(MAX_CLASSIFIER_CONTEXT_BYTES + 1),
        );
        let queued = state.sessions.queued_messages(session).unwrap();
        if draining {
            *state.work_gate.write().await = true;
        } else {
            state.sessions.set_yield_suppressed(session, true).unwrap();
        }
        let mut checked = BTreeMap::new();
        tick(&state, &mut checked).await.unwrap();
        assert_gate_preserved(&state, &gate, &mut receiver, true).await;
        assert!(checked.is_empty());
        assert_eq!(event_count(&state, "task.interaction_presented"), 0);
        *state.work_gate.write().await = false;
        state.sessions.set_yield_suppressed(session, false).unwrap();
        tick(&state, &mut checked).await.unwrap();
        assert_gate_preserved(&state, &gate, &mut receiver, false).await;
        assert_eq!(state.sessions.queued_messages(session).unwrap(), queued);
        assert_eq!(event_count(&state, "task.interaction_presented"), 1);
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[tokio::test]
async fn stale_claim_cannot_hide_a_represented_gate_or_claim_changed_input() {
    for changed_gate in [false, true] {
        let (home, state, session) = state();
        let (gate, mut receiver) = waiting_approval(&state, session, changed_gate).await;
        let next = enqueue(&state, session, "independent request");
        let snapshot = state.sessions.get(session).unwrap().unwrap();
        let message = state.sessions.queued_messages(session).unwrap().remove(0);
        if changed_gate {
            state.tasks.present_gate(gate.id).await.unwrap();
        } else {
            state
                .sessions
                .update_turn_queue(&UpdateTurnQueueParams {
                    session_id: session,
                    action: QueueAction::Edit,
                    message_id: Some(next.id),
                    text: Some("changed request".into()),
                    expected_text: Some(message.text.clone()),
                })
                .unwrap();
        }
        let queued = state.sessions.queued_messages(session).unwrap();
        let attempt = claim_and_park(&state, &snapshot, std::slice::from_ref(&gate), &message)
            .await
            .unwrap();
        if changed_gate {
            assert!(matches!(attempt, ClaimAttempt::SnapshotChanged));
        } else {
            assert!(matches!(attempt, ClaimAttempt::MessageSkipped));
        }
        assert_gate_preserved(&state, &gate, &mut receiver, false).await;
        assert_eq!(state.sessions.queued_messages(session).unwrap(), queued);
        assert!(!state.sessions.has_running_lane(session).unwrap());
        assert_eq!(
            event_count(&state, "task.interactions_parked"),
            usize::from(changed_gate)
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[tokio::test]
async fn send_now_revokes_independent_confirmation_before_atomic_claim() {
    let (home, state, session) = state();
    let (gate, mut receiver) = waiting_approval(&state, session, false).await;
    let next = enqueue(&state, session, "independent request");
    queue_action(
        &state,
        session,
        Some(next.id),
        QueueAction::RunIndependently,
    );
    let snapshot = state.sessions.get(session).unwrap().unwrap();
    let message = state.sessions.queued_messages(session).unwrap().remove(0);
    assert!(message.independence_confirmed);
    queue_action(&state, session, Some(next.id), QueueAction::SendNow);
    let queued = state.sessions.queued_messages(session).unwrap();
    assert!(!queued[0].independence_confirmed);
    assert!(queued[0].priority_requested);
    assert_eq!(queued[0].text, message.text);
    // Exercise the claim itself: a separate preflight queue check cannot protect
    // against queue.update revoking the override before the claim locks are held.
    assert!(
        state
            .sessions
            .claim_independent(&snapshot, &message)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        claim_and_park(&state, &snapshot, std::slice::from_ref(&gate), &message)
            .await
            .unwrap(),
        ClaimAttempt::MessageSkipped
    ));
    assert_gate_preserved(&state, &gate, &mut receiver, false).await;
    assert_eq!(state.sessions.get(session).unwrap().unwrap(), snapshot);
    assert_eq!(state.sessions.queued_messages(session).unwrap(), queued);
    assert!(!state.sessions.has_running_lane(session).unwrap());
    assert_eq!(state.tasks.list().await.len(), 1);
    assert_eq!(event_count(&state, "task.interactions_parked"), 0);
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn stale_session_or_gate_snapshot_stops_scanning_remaining_messages() {
    for changed_session in [false, true] {
        let (home, state, session) = state();
        let (gate, mut receiver) = waiting_approval(&state, session, true).await;
        let next = enqueue(&state, session, "independent request");
        queue_action(
            &state,
            session,
            Some(next.id),
            QueueAction::RunIndependently,
        );
        enqueue(
            &state,
            session,
            &"x".repeat(MAX_CLASSIFIER_CONTEXT_BYTES + 1),
        );
        let queued = state.sessions.queued_messages(session).unwrap();
        let snapshot = state.sessions.get(session).unwrap().unwrap();
        if !changed_session {
            state.tasks.present_gate(gate.id).await.unwrap();
        }
        let guard = state.tasks.persistence.lock().await;
        let mut checked = BTreeMap::new();
        // First poll reaches claim_and_park and waits on persistence. Change the
        // session after try_start_independent captured it, before claim resumes.
        let (result, ()) = tokio::join!(
            biased;
            try_start_independent(&state, session, std::slice::from_ref(&gate), &mut checked),
            async {
                if changed_session {
                    state.sessions.set_yield_suppressed(session, true).unwrap();
                }
                drop(guard);
            }
        );
        result.unwrap();
        assert!(
            checked.is_empty(),
            "stale snapshots must not classify the remaining queue"
        );
        assert!(matches!(
            claim_and_park(&state, &snapshot, std::slice::from_ref(&gate), &queued[0])
                .await
                .unwrap(),
            ClaimAttempt::SnapshotChanged
        ));
        assert_gate_preserved(&state, &gate, &mut receiver, changed_session).await;
        assert_eq!(state.sessions.queued_messages(session).unwrap(), queued);
        assert!(!state.sessions.has_running_lane(session).unwrap());
        assert_eq!(state.tasks.list().await.len(), 1);
        assert_eq!(event_count(&state, "task.interactions_parked"), 1);
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[tokio::test]
async fn failed_independent_start_keeps_the_operator_input_visible_and_restores_the_parent() {
    let (home, state, session) = state();
    let (gate, mut receiver) = waiting_approval(&state, session, false).await;
    let original = state.sessions.get(session).unwrap().unwrap().active_turn_id;
    let queued = enqueue(&state, session, "retain this input after failed admission");
    queue_action(
        &state,
        session,
        Some(queued.id),
        QueueAction::RunIndependently,
    );
    // Admission fails before a worker/provider can start.
    std::fs::remove_dir(home.join("project")).unwrap();
    let mut checked = BTreeMap::new();
    tick(&state, &mut checked).await.unwrap();
    assert_gate_preserved(&state, &gate, &mut receiver, false).await;
    tick(&state, &mut checked).await.unwrap();
    assert_eq!(event_count(&state, "task.interactions_parked"), 1);
    assert_eq!(event_count(&state, "task.interaction_presented"), 1);
    assert_eq!(state.tasks.list().await.len(), 1);
    let histories =
        session_store::RuntimeSessionStore::independent_history(&home, session).unwrap();
    assert_eq!(
        histories[0].messages[0].content,
        "retain this input after failed admission"
    );
    assert_eq!(
        histories[0].status,
        session_store::RuntimeTurnStatus::Failed
    );
    assert_eq!(
        state.sessions.get(session).unwrap().unwrap().active_turn_id,
        original
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn old_decision_is_buffered_during_independent_work_then_resumes_exact_waiter() {
    let (home, state, session) = state();
    let original = enqueue(&state, session, "original task");
    state.sessions.claim_next(session).unwrap().unwrap();
    let original_task = bind(&state, session, original.id, false).await;
    let mut receiver = state
        .tasks
        .create_interaction(
            original_task,
            InteractionKind::Approval {
                description: "original command".to_owned(),
                always_allow_available: false,
            },
        )
        .await
        .unwrap();
    let gate = state.tasks.pending_interactions().await[0].clone();
    let _lease = willdeep_core::checkpoint::SessionCheckpointSink {
        store: SessionStore::new(&home),
        session_id: session,
    }
    .claim()
    .unwrap();
    let before =
        serde_json::to_vec(&SessionStore::new(&home).load(session).unwrap().messages).unwrap();
    let next = enqueue(&state, session, "independent question");
    state
        .tasks
        .park_gates(std::slice::from_ref(&gate))
        .await
        .unwrap();
    let claimed = claim_queued(&state, session, next.id).unwrap();
    let execution = claimed.request.execution_session_id.unwrap();
    assert_ne!(execution, session);
    let _side_lease = willdeep_core::checkpoint::SessionCheckpointSink {
        store: SessionStore::new(home.join("independent")),
        session_id: execution,
    }
    .claim()
    .unwrap();
    let side_task = bind(&state, session, next.id, true).await;
    state
        .tasks
        .resolve_interaction(gate.id, InteractionResolution::AllowOnce)
        .await
        .unwrap();
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(
        state
            .tasks
            .resolve_interaction(gate.id, InteractionResolution::Deny)
            .await
            .is_err()
    );
    let store = SessionStore::new(home.join("independent"));
    let mut lane = store.load_baselined(execution).unwrap();
    lane.messages.push(Message::user("independent question"));
    lane.messages
        .push(Message::assistant("independent answer", Vec::new()));
    // The side execution lease is still held; checkpoint writes own it.
    drop(_side_lease);
    store.save(&mut lane).unwrap();
    state
        .sessions
        .record_execution_end(side_task, lane.messages.len(), lane.compression_generation)
        .unwrap();
    state
        .sessions
        .complete_task(side_task, RuntimeTaskStatus::Completed, None)
        .unwrap();
    state.tasks.deliver_buffered_decisions().await.unwrap();
    assert_eq!(receiver.await.unwrap(), InteractionResolution::AllowOnce);
    assert_eq!(
        state.sessions.get(session).unwrap().unwrap().active_turn_id,
        Some(original.id)
    );
    assert_eq!(
        state
            .sessions
            .get_turn(original.id)
            .unwrap()
            .unwrap()
            .status,
        session_store::RuntimeTurnStatus::Running
    );
    assert_eq!(
        before,
        serde_json::to_vec(&SessionStore::new(&home).load(session).unwrap().messages).unwrap()
    );
    let display = super::super::turn_queue::display_session(
        &home,
        &SessionStore::new(&home).load(session).unwrap(),
    )
    .unwrap();
    assert!(
        display
            .messages
            .iter()
            .any(|message| message.content == "independent answer")
    );
    drop(_lease);
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn independent_completion_restores_pending_gate_without_resolving_or_dropping_it() {
    let (home, state, session) = state();
    let original = enqueue(&state, session, "task");
    state.sessions.claim_next(session).unwrap().unwrap();
    let original_task = bind(&state, session, original.id, false).await;
    let mut receiver = state
        .tasks
        .create_interaction(
            original_task,
            InteractionKind::Question {
                question: "choose a target".to_owned(),
                options: vec!["A".to_owned(), "B".to_owned()],
                multi_select: true,
            },
        )
        .await
        .unwrap();
    let gate = state.tasks.pending_interactions().await[0].clone();
    let next = enqueue(&state, session, "another task");
    let snapshot = state.sessions.get(session).unwrap().unwrap();
    let message = state.sessions.queued_messages(session).unwrap().remove(0);
    let ClaimAttempt::Claimed(claimed) =
        claim_and_park(&state, &snapshot, std::slice::from_ref(&gate), &message)
            .await
            .unwrap()
    else {
        panic!("independent message should be claimed");
    };
    assert_eq!(claimed.metadata.id, next.id);
    assert!(claimed.request.execution_session_id.is_some());
    let mut checked = BTreeMap::new();
    // Admission already owns a queued lane, even before task startup.
    tick(&state, &mut checked).await.unwrap();
    assert_gate_preserved(&state, &gate, &mut receiver, true).await;
    let side_task = bind(&state, session, next.id, true).await;
    tick(&state, &mut checked).await.unwrap();
    assert_gate_preserved(&state, &gate, &mut receiver, true).await;
    assert_eq!(event_count(&state, "task.interactions_parked"), 1);
    assert_eq!(event_count(&state, "task.interaction_presented"), 0);
    state
        .sessions
        .complete_task(side_task, RuntimeTaskStatus::Cancelled, None)
        .unwrap();
    tick(&state, &mut checked).await.unwrap();
    assert_eq!(event_count(&state, "task.interaction_presented"), 1);
    let restored = state.tasks.pending_interactions().await;
    assert_eq!(restored[0].id, gate.id);
    assert!(!restored[0].parked);
    assert!(restored[0].foreground_since.unwrap() >= gate.created_at);
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    state
        .tasks
        .resolve_interaction(
            gate.id,
            InteractionResolution::Answer(Some("A, B".to_owned())),
        )
        .await
        .unwrap();
    assert_eq!(
        receiver.await.unwrap(),
        InteractionResolution::Answer(Some("A, B".to_owned()))
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn repeated_yields_preserve_the_same_question_and_queued_followups() {
    let (home, state, session) = state();
    let original = enqueue(&state, session, "original");
    state.sessions.claim_next(session).unwrap().unwrap();
    let original_task = bind(&state, session, original.id, false).await;
    let mut receiver = state
        .tasks
        .create_interaction(
            original_task,
            InteractionKind::Question {
                question: "original question".into(),
                options: vec![],
                multi_select: false,
            },
        )
        .await
        .unwrap();
    let gate_id = state.tasks.pending_interactions().await[0].id;
    for prompt in ["first independent", "second independent"] {
        let queued = enqueue(&state, session, prompt);
        let gates = state.tasks.pending_interactions().await;
        state.tasks.park_gates(&gates).await.unwrap();
        claim_queued(&state, session, queued.id).unwrap();
        let task = bind(&state, session, queued.id, true).await;
        state
            .sessions
            .complete_task(task, RuntimeTaskStatus::Completed, None)
            .unwrap();
        state.tasks.present_oldest_gate(session).await.unwrap();
        assert_eq!(state.tasks.pending_interactions().await[0].id, gate_id);
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }
    state
        .tasks
        .resolve_interaction(
            gate_id,
            InteractionResolution::Answer(Some("exact answer".into())),
        )
        .await
        .unwrap();
    assert_eq!(
        receiver.await.unwrap(),
        InteractionResolution::Answer(Some("exact answer".into()))
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn stopped_group_does_not_resume_a_buffered_approval_until_explicitly_resumed() {
    let (home, state, session) = state();
    let original = enqueue(&state, session, "original");
    state.sessions.claim_next(session).unwrap().unwrap();
    let original_task = bind(&state, session, original.id, false).await;
    let mut receiver = state
        .tasks
        .create_interaction(
            original_task,
            InteractionKind::Approval {
                description: "original operation".into(),
                always_allow_available: false,
            },
        )
        .await
        .unwrap();
    let gates = state.tasks.pending_interactions().await;
    let queued = enqueue(&state, session, "independent");
    state.tasks.park_gates(&gates).await.unwrap();
    claim_queued(&state, session, queued.id).unwrap();
    let task = bind(&state, session, queued.id, true).await;
    state
        .tasks
        .resolve_interaction(gates[0].id, InteractionResolution::AllowOnce)
        .await
        .unwrap();
    state.sessions.set_yield_suppressed(session, true).unwrap();
    state
        .sessions
        .complete_task(task, RuntimeTaskStatus::Cancelled, None)
        .unwrap();
    state.tasks.deliver_buffered_decisions().await.unwrap();
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    state.sessions.set_yield_suppressed(session, false).unwrap();
    state.tasks.deliver_buffered_decisions().await.unwrap();
    assert_eq!(receiver.await.unwrap(), InteractionResolution::AllowOnce);
    std::fs::remove_dir_all(home).unwrap();
}
