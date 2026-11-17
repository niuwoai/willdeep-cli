use super::*;
use session_store::{
    CreateRuntimeSession, CreateRuntimeTurn, RuntimeSessionStatus, RuntimeTurnStatus,
};

struct Fixture {
    root: PathBuf,
    state: ServerState,
    session: session_store::RuntimeSession,
    turn: session_store::RuntimeTurn,
    task: uuid::Uuid,
    _scheduler: tokio::sync::mpsc::UnboundedReceiver<uuid::Uuid>,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("runtime-terminal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let sessions = crate::daemon::tests::test_runtime_session_store(&root);
        let session = sessions
            .create(CreateRuntimeSession {
                id: None,
                workspace: root.clone(),
                profile: None,
                model: None,
                config: None,
                title: None,
            })
            .unwrap();
        let (scheduler, receiver) = tokio::sync::mpsc::unbounded_channel();
        let state =
            crate::daemon::tests::test_server_state_with_scheduler(&root, sessions, scheduler);
        let turn = enqueue(&state, session.id);
        state.sessions.claim_next(session.id).unwrap().unwrap();
        let task = uuid::Uuid::new_v4();
        state
            .agents
            .ensure_session_root(
                session.root_agent_id,
                task,
                root.clone(),
                None,
                None,
                RuntimeAgentStatus::Running,
            )
            .unwrap();
        state
            .tasks
            .insert_and_persist(
                serde_json::from_value(serde_json::json!({
                    "id":task, "session_id":session.id, "turn_id":turn.id,
                    "agent_id":session.root_agent_id, "status":"running",
                    "workspace":root, "created_at":now(), "started_at":now()
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(state.sessions.bind_task(turn.id, task).unwrap());
        Self {
            root,
            state,
            session,
            turn,
            task,
            _scheduler: receiver,
        }
    }

    async fn gate(
        &self,
        question: bool,
    ) -> (
        RuntimeInteraction,
        tokio::sync::oneshot::Receiver<InteractionResolution>,
    ) {
        let kind = if question {
            InteractionKind::Question {
                question: "choose a target".into(),
                options: Vec::new(),
                multi_select: false,
            }
        } else {
            InteractionKind::Approval {
                description: "open the browser".into(),
                always_allow_available: true,
            }
        };
        let receiver = self
            .state
            .tasks
            .create_interaction(self.task, kind)
            .await
            .unwrap();
        let gate = self.state.tasks.pending_interactions().await.remove(0);
        (gate, receiver)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn enqueue(state: &ServerState, session_id: uuid::Uuid) -> session_store::RuntimeTurn {
    state
        .sessions
        .enqueue_turn(
            session_id,
            CreateRuntimeTurn {
                team_control: None,
                request_id: uuid::Uuid::new_v4(),
                prompt: "inspect the page".into(),
                attachments: Vec::new(),
                origin_client: Some("cli:test".into()),
                defer_until_released: false,
            },
        )
        .unwrap()
        .0
}

async fn assert_replay_repairs_failed_resume(blocked_store: &str) {
    for question in [false, true] {
        let fixture = Fixture::new().await;
        let state = &fixture.state;
        let (gate, mut receiver) = fixture.gate(question).await;
        let resolution = if question {
            InteractionResolution::Answer(Some("the active tab".into()))
        } else {
            InteractionResolution::AllowOnce
        };
        let waiting_turn = state.sessions.get_turn(fixture.turn.id).unwrap().unwrap();
        let waiting_session = state.sessions.get(fixture.session.id).unwrap().unwrap();
        let waiting_turns = std::fs::read(fixture.root.join("turns.json")).unwrap();
        let waiting_sessions = std::fs::read(fixture.root.join("runtime-sessions.json")).unwrap();
        let path = fixture.root.join(blocked_store);
        let backup = path.with_extension("backup");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        let result = state
            .tasks
            .resolve_interaction(gate.id, resolution.clone())
            .await;
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&backup, &path).unwrap();
        assert!(result.is_err(), "the selected persistence write must fail");
        assert_eq!(
            state.tasks.get(fixture.task).await.unwrap().status,
            RuntimeTaskStatus::Running
        );
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        let failed_turn = state.sessions.get_turn(fixture.turn.id).unwrap().unwrap();
        let failed_session = state.sessions.get(fixture.session.id).unwrap().unwrap();
        let failed_turns = std::fs::read(fixture.root.join("turns.json")).unwrap();
        let failed_sessions = std::fs::read(fixture.root.join("runtime-sessions.json")).unwrap();
        // Use the scheduler's entry point, including its running-lane precheck.
        state.tasks.deliver_buffered_decisions().await.unwrap();
        assert_eq!(receiver.try_recv(), Ok(resolution));
        assert_eq!(failed_turn, waiting_turn);
        assert_eq!(failed_session, waiting_session);
        assert_eq!(failed_turns, waiting_turns);
        assert_eq!(failed_sessions, waiting_sessions);
        assert_eq!(
            state
                .sessions
                .get_turn(fixture.turn.id)
                .unwrap()
                .unwrap()
                .status,
            RuntimeTurnStatus::Running
        );
        assert_eq!(
            state
                .sessions
                .get(fixture.session.id)
                .unwrap()
                .unwrap()
                .status,
            RuntimeSessionStatus::Running
        );
        assert_eq!(
            state
                .agents
                .get(fixture.session.root_agent_id)
                .unwrap()
                .unwrap()
                .status,
            RuntimeAgentStatus::Running
        );
        assert_eq!(
            load_agents(&fixture.root.join("agents.json")).unwrap()[&fixture.session.root_agent_id]
                .status,
            RuntimeAgentStatus::Running
        );
        assert_eq!(
            load_tasks(&state.tasks.path).unwrap()[&fixture.task].status,
            RuntimeTaskStatus::Running
        );
    }
}

#[tokio::test]
async fn replay_repairs_resume_after_task_persistence_failure() {
    assert_replay_repairs_failed_resume("tasks.json").await;
}

#[tokio::test]
async fn replay_repairs_resume_after_agent_persistence_failure() {
    assert_replay_repairs_failed_resume("agents.json").await;
}

#[tokio::test]
async fn replay_repairs_resume_after_turn_persistence_failure() {
    assert_replay_repairs_failed_resume("turns.json").await;
}

#[tokio::test]
async fn replay_repairs_resume_after_session_persistence_failure() {
    assert_replay_repairs_failed_resume("runtime-sessions.json").await;
}

#[tokio::test]
async fn buffered_repair_waits_for_another_claimed_and_running_lane() {
    for blocked_store in [
        "tasks.json",
        "agents.json",
        "turns.json",
        "runtime-sessions.json",
    ] {
        let fixture = Fixture::new().await;
        let state = &fixture.state;
        let (gate, mut receiver) = fixture.gate(false).await;
        let path = fixture.root.join(blocked_store);
        let backup = path.with_extension("backup");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        let result = state
            .tasks
            .resolve_interaction(gate.id, InteractionResolution::AllowOnce)
            .await;
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&backup, &path).unwrap();
        assert!(result.is_err());
        let side = enqueue(state, fixture.session.id);
        let snapshot = state.sessions.get(fixture.session.id).unwrap().unwrap();
        let message = state
            .sessions
            .queued_messages(fixture.session.id)
            .unwrap()
            .remove(0);
        let claimed = state
            .sessions
            .claim_independent(&snapshot, &message)
            .unwrap()
            .unwrap();
        assert_eq!(claimed.metadata.id, side.id);
        assert!(claimed.metadata.execution_session_id.is_some());
        // Both the published reservation and the bound execution must block replay.
        state.tasks.deliver_buffered_decisions().await.unwrap();
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        let side_task = uuid::Uuid::new_v4();
        assert!(state.sessions.bind_task(side.id, side_task).unwrap());
        state.tasks.deliver_buffered_decisions().await.unwrap();
        assert!(matches!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(
            state
                .sessions
                .get(fixture.session.id)
                .unwrap()
                .unwrap()
                .active_turn_id,
            Some(side.id)
        );
        state
            .sessions
            .complete_task(side_task, RuntimeTaskStatus::Completed, None)
            .unwrap();
        state.tasks.deliver_buffered_decisions().await.unwrap();
        assert_eq!(receiver.try_recv(), Ok(InteractionResolution::AllowOnce));
        assert_eq!(
            state
                .sessions
                .get(fixture.session.id)
                .unwrap()
                .unwrap()
                .active_turn_id,
            Some(fixture.turn.id)
        );
    }
}

#[tokio::test]
async fn replay_after_failed_resume_cannot_reopen_finished_task_or_touch_successor() {
    for blocked_store in [
        "tasks.json",
        "agents.json",
        "turns.json",
        "runtime-sessions.json",
    ] {
        let fixture = Fixture::new().await;
        let state = &fixture.state;
        let (gate, receiver) = fixture.gate(false).await;
        let path = fixture.root.join(blocked_store);
        let backup = path.with_extension("backup");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        let result = state
            .tasks
            .resolve_interaction(gate.id, InteractionResolution::AllowOnce)
            .await;
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(&backup, &path).unwrap();
        assert!(result.is_err());
        state
            .tasks
            .finish(fixture.task, RuntimeTaskStatus::Completed, None, None, None)
            .await
            .unwrap();
        assert_eq!(receiver.await.unwrap(), InteractionResolution::Deny);
        let terminal = state.sessions.get_turn(fixture.turn.id).unwrap().unwrap();
        let next = enqueue(state, fixture.session.id);
        state
            .sessions
            .claim_next(fixture.session.id)
            .unwrap()
            .unwrap();
        let next_task = uuid::Uuid::new_v4();
        assert!(state.sessions.bind_task(next.id, next_task).unwrap());
        let agent = state
            .agents
            .ensure_session_root(
                fixture.session.root_agent_id,
                next_task,
                fixture.root.clone(),
                None,
                None,
                RuntimeAgentStatus::Running,
            )
            .unwrap();
        let session = state.sessions.get(fixture.session.id).unwrap().unwrap();
        state
            .tasks
            .resolve_interaction_as(gate.id, InteractionResolution::Deny, false, true)
            .await
            .unwrap();
        assert_eq!(
            state.tasks.get(fixture.task).await.unwrap().status,
            RuntimeTaskStatus::Completed
        );
        assert_eq!(
            state.sessions.get_turn(fixture.turn.id).unwrap().unwrap(),
            terminal
        );
        assert_eq!(
            state.sessions.get(fixture.session.id).unwrap().unwrap(),
            session
        );
        assert_eq!(state.agents.get(agent.id).unwrap().unwrap(), agent);
        assert_eq!(
            load_agents(&fixture.root.join("agents.json")).unwrap()[&agent.id],
            agent
        );
    }
}

#[tokio::test]
async fn cancel_rechecks_terminal_state_after_waiting_for_persistence() {
    use std::future::Future;
    for (status, _) in TERMINAL_CASES {
        let fixture = Fixture::new().await;
        let manager = &fixture.state.tasks;
        let cancellation = Arc::new(Notify::new());
        manager
            .cancellations
            .lock()
            .unwrap()
            .insert(fixture.task, cancellation.clone());
        let persistence = manager.persistence.lock().await;
        let mut finish = std::pin::pin!(manager.finish(fixture.task, status, None, None, None));
        let mut cancel = std::pin::pin!(manager.cancel(fixture.task));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        // Tokio's FIFO mutex puts finish before cancel; cancel has already
        // copied its handle when both futures are suspended at this barrier.
        assert!(finish.as_mut().poll(&mut context).is_pending());
        assert!(cancel.as_mut().poll(&mut context).is_pending());
        drop(persistence);
        // finish commits the terminal records, then waits behind cancel for its
        // gate-presentation cleanup. No timer or scheduler race selects the order.
        assert!(finish.as_mut().poll(&mut context).is_pending());
        assert_eq!(manager.get(fixture.task).await.unwrap().status, status);
        manager.cancellations.lock().unwrap().remove(&fixture.task);
        let persisted = std::fs::read(&manager.path).unwrap();
        let cancelled = cancel.await.unwrap().unwrap();
        finish.await.unwrap();
        assert_eq!(cancelled.status, status);
        assert_eq!(manager.get(fixture.task).await.unwrap().status, status);
        assert_eq!(std::fs::read(&manager.path).unwrap(), persisted);
        let mut notified = std::pin::pin!(cancellation.notified());
        assert!(notified.as_mut().poll(&mut context).is_pending());
    }
}

const TERMINAL_CASES: [(RuntimeTaskStatus, RuntimeTurnStatus); 5] = [
    (RuntimeTaskStatus::Completed, RuntimeTurnStatus::Completed),
    (RuntimeTaskStatus::Partial, RuntimeTurnStatus::Partial),
    (RuntimeTaskStatus::Failed, RuntimeTurnStatus::Failed),
    (RuntimeTaskStatus::Cancelled, RuntimeTurnStatus::Cancelled),
    (
        RuntimeTaskStatus::Interrupted,
        RuntimeTurnStatus::Interrupted,
    ),
];

#[tokio::test]
async fn finish_resolves_gates_without_reopening_terminal_execution() {
    for (task_status, turn_status) in TERMINAL_CASES {
        for question in [false, true] {
            let fixture = Fixture::new().await;
            let state = &fixture.state;
            let (gate, receiver) = fixture.gate(question).await;
            state
                .tasks
                .finish(fixture.task, task_status, None, None, None)
                .await
                .unwrap();
            let resolution = if question {
                InteractionResolution::Answer(None)
            } else {
                InteractionResolution::Deny
            };
            assert_eq!(receiver.await.unwrap(), resolution);
            assert!(state.tasks.pending_interactions().await.is_empty());
            assert!(state.tasks.interaction_waiters.lock().unwrap().is_empty());
            assert_eq!(
                state.tasks.get(fixture.task).await.unwrap().status,
                task_status
            );
            let terminal = state.sessions.get_turn(fixture.turn.id).unwrap().unwrap();
            assert_eq!(terminal.status, turn_status);
            assert!(terminal.completed_at.is_some());
            let agent = state
                .agents
                .get(fixture.session.root_agent_id)
                .unwrap()
                .unwrap();
            assert_eq!(agent.status, agent_status(task_status));
            assert!(agent.completed_at.is_some());
            let session = state.sessions.get(fixture.session.id).unwrap().unwrap();
            assert!(session.active_turn_id.is_none());
            assert_ne!(session.status, RuntimeSessionStatus::Running);

            // A buffered decision may arrive again after finish has cleaned up
            // its waiter. Replaying it must leave the durable terminal records intact.
            state
                .tasks
                .resolve_interaction_as(gate.id, resolution, false, true)
                .await
                .unwrap();
            assert_eq!(
                state.sessions.get_turn(fixture.turn.id).unwrap().unwrap(),
                terminal
            );
            assert_eq!(
                state.sessions.get(fixture.session.id).unwrap().unwrap(),
                session
            );
            assert_eq!(state.agents.get(agent.id).unwrap().unwrap(), agent);
            assert_eq!(
                load_agents(&fixture.root.join("agents.json")).unwrap()[&agent.id],
                agent
            );
            let reopened = session_store::RuntimeSessionStore::open(
                fixture.root.join("runtime-sessions.json"),
                &fixture.root,
            )
            .unwrap();
            assert_eq!(
                reopened.get_turn(fixture.turn.id).unwrap().unwrap(),
                terminal
            );
        }
    }
}

#[tokio::test]
async fn late_gate_updates_cannot_reopen_terminal_turn_or_steal_next_claim() {
    for (task_status, _) in TERMINAL_CASES {
        let fixture = Fixture::new().await;
        let state = &fixture.state;
        state
            .tasks
            .finish(fixture.task, task_status, None, None, None)
            .await
            .unwrap();
        let terminal = state.sessions.get_turn(fixture.turn.id).unwrap().unwrap();
        let session = state.sessions.get(fixture.session.id).unwrap().unwrap();
        let updates = [
            RuntimeTurnStatus::WaitingApproval,
            RuntimeTurnStatus::WaitingAnswer,
            RuntimeTurnStatus::Running,
        ];
        for status in updates {
            state
                .sessions
                .set_task_waiting(fixture.task, status)
                .unwrap();
            assert_eq!(
                state.sessions.get_turn(fixture.turn.id).unwrap().unwrap(),
                terminal
            );
            assert_eq!(
                state.sessions.get(fixture.session.id).unwrap().unwrap(),
                session
            );
        }

        let next = enqueue(state, fixture.session.id);
        assert_eq!(
            state
                .sessions
                .claim_next(fixture.session.id)
                .unwrap()
                .unwrap()
                .metadata
                .id,
            next.id
        );
        let next_task = uuid::Uuid::new_v4();
        assert!(state.sessions.bind_task(next.id, next_task).unwrap());
        for status in updates {
            state.sessions.set_task_waiting(next_task, status).unwrap();
            let next_session = state.sessions.get(fixture.session.id).unwrap().unwrap();
            assert_eq!(next_session.active_turn_id, Some(next.id));
            let next_turn = state.sessions.get_turn(next.id).unwrap().unwrap();
            assert_eq!(next_turn.status, status);
            assert!(next_turn.completed_at.is_none());
            assert_eq!(next_turn.attempts, 1);
            state
                .sessions
                .set_task_waiting(fixture.task, RuntimeTurnStatus::Running)
                .unwrap();
            assert_eq!(
                state.sessions.get(fixture.session.id).unwrap().unwrap(),
                next_session
            );
            assert_eq!(
                state.sessions.get_turn(fixture.turn.id).unwrap().unwrap(),
                terminal
            );
        }
        state
            .sessions
            .complete_task(next_task, RuntimeTaskStatus::Completed, None)
            .unwrap();
    }
}

#[tokio::test]
async fn normal_resolutions_resume_waiting_execution() {
    for question in [false, true] {
        let fixture = Fixture::new().await;
        let state = &fixture.state;
        let (gate, receiver) = fixture.gate(question).await;
        let resolution = if question {
            InteractionResolution::Answer(Some("the active tab".into()))
        } else {
            InteractionResolution::AllowOnce
        };
        state
            .tasks
            .resolve_interaction(gate.id, resolution.clone())
            .await
            .unwrap();
        assert_eq!(receiver.await.unwrap(), resolution);
        assert_eq!(
            state.tasks.get(fixture.task).await.unwrap().status,
            RuntimeTaskStatus::Running
        );
        let turn = state.sessions.get_turn(fixture.turn.id).unwrap().unwrap();
        assert_eq!(turn.status, RuntimeTurnStatus::Running);
        assert!(turn.completed_at.is_none());
        assert_eq!(
            state
                .sessions
                .get(fixture.session.id)
                .unwrap()
                .unwrap()
                .status,
            RuntimeSessionStatus::Running
        );
        let agent = state
            .agents
            .get(fixture.session.root_agent_id)
            .unwrap()
            .unwrap();
        assert_eq!(agent.status, RuntimeAgentStatus::Running);
        assert!(agent.completed_at.is_none());
    }
}

#[tokio::test]
async fn resolving_a_gate_while_cancelling_does_not_resume_execution() {
    let fixture = Fixture::new().await;
    let state = &fixture.state;
    let (gate, receiver) = fixture.gate(false).await;
    let cancellation = Arc::new(Notify::new());
    state
        .tasks
        .cancellations
        .lock()
        .unwrap()
        .insert(fixture.task, cancellation.clone());
    let stopped = state.sessions.request_cancel(fixture.turn.id).unwrap();
    assert_eq!(stopped.task_id, Some(fixture.task));
    assert_eq!(
        state
            .tasks
            .cancel(fixture.task)
            .await
            .unwrap()
            .unwrap()
            .status,
        RuntimeTaskStatus::Cancelling
    );
    tokio::time::timeout(Duration::from_secs(1), cancellation.notified())
        .await
        .unwrap();
    let waiting_turn = state.sessions.get_turn(fixture.turn.id).unwrap().unwrap();
    let waiting_agent = state
        .agents
        .get(fixture.session.root_agent_id)
        .unwrap()
        .unwrap();
    state
        .tasks
        .resolve_interaction(gate.id, InteractionResolution::AllowOnce)
        .await
        .unwrap();
    assert_eq!(receiver.await.unwrap(), InteractionResolution::AllowOnce);
    assert_eq!(
        state.tasks.get(fixture.task).await.unwrap().status,
        RuntimeTaskStatus::Cancelling
    );
    assert_eq!(
        state.sessions.get_turn(fixture.turn.id).unwrap().unwrap(),
        waiting_turn
    );
    assert_eq!(
        state
            .agents
            .get(fixture.session.root_agent_id)
            .unwrap()
            .unwrap(),
        waiting_agent
    );
    state
        .tasks
        .cancellations
        .lock()
        .unwrap()
        .remove(&fixture.task);
    state
        .tasks
        .finish(fixture.task, RuntimeTaskStatus::Cancelled, None, None, None)
        .await
        .unwrap();
    assert_eq!(
        state
            .sessions
            .get_turn(fixture.turn.id)
            .unwrap()
            .unwrap()
            .status,
        RuntimeTurnStatus::Cancelled
    );
}
