use super::*;
use willdeep_core::{Message, SessionStore};

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
            },
        )
        .unwrap()
        .0
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
    assert!(
        state
            .sessions
            .claim_independent(session, next.id, "independent")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        state.sessions.queued_messages(session).unwrap()[0].id,
        next.id
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn unknown_queue_still_parks_the_gate_and_never_reopens_or_answers_it_automatically() {
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
    // Context is intentionally too large to classify, so no provider is needed.
    let queued = enqueue(&state, session, &"x".repeat(64 * 1024));
    let mut checked = BTreeMap::new();
    for _ in 0..2 {
        tick(&state, &mut checked).await.unwrap();
    }
    let pending = state.tasks.pending_interactions().await;
    assert_eq!(pending[0].id, gate);
    assert!(pending[0].parked);
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(
        state.sessions.queued_messages(session).unwrap()[0].id,
        queued.id
    );
    assert_eq!(state.tasks.list().await.len(), 1);
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn failed_independent_start_keeps_the_operator_input_visible_and_restores_the_parent() {
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
    let queued = enqueue(&state, session, "retain this input after failed admission");
    state
        .sessions
        .claim_independent(
            session,
            queued.id,
            "retain this input after failed admission",
        )
        .unwrap()
        .unwrap();
    state
        .sessions
        .complete_claim_failure(queued.id, "fixture admission failure".into())
        .unwrap();
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
        Some(original.id)
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
    state.tasks.park_gates(&[gate.clone()]).await.unwrap();
    let claimed = state
        .sessions
        .claim_independent(session, next.id, "independent question")
        .unwrap()
        .unwrap();
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
    let mut lane = store.load(execution).unwrap();
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
    state.tasks.park_gates(&[gate.clone()]).await.unwrap();
    state
        .sessions
        .claim_independent(session, next.id, "another task")
        .unwrap()
        .unwrap();
    let side_task = bind(&state, session, next.id, true).await;
    state
        .sessions
        .complete_task(side_task, RuntimeTaskStatus::Cancelled, None)
        .unwrap();
    state.tasks.present_oldest_gate(session).await.unwrap();
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
        state
            .sessions
            .claim_independent(session, queued.id, prompt)
            .unwrap()
            .unwrap();
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
    state
        .sessions
        .claim_independent(session, queued.id, "independent")
        .unwrap()
        .unwrap();
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
