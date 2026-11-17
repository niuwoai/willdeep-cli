use super::*;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

thread_local! {
    // One-shot barrier on this worker only; unrelated stores and tests do not pause.
    static SESSION_LOCK_BARRIER: std::cell::RefCell<Option<(SyncSender<()>, Receiver<()>)>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn before_session_lock() {
    let barrier = SESSION_LOCK_BARRIER.with(|barrier| barrier.borrow_mut().take());
    if let Some((entered, resume)) = barrier {
        entered.send(()).unwrap();
        resume.recv().unwrap();
    }
}

struct Fixture {
    root: PathBuf,
    store: RuntimeSessionStore,
    session: RuntimeSession,
    original: RuntimeTurn,
    original_task: uuid::Uuid,
    side: RuntimeTurn,
    side_task: uuid::Uuid,
    ordinary: Option<RuntimeTurn>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_ordinary(false)
    }

    fn with_ordinary(include: bool) -> Self {
        let root =
            std::env::temp_dir().join(format!("session-completion-race-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let store = RuntimeSessionStore::open(root.join("runtime-sessions.json"), &root).unwrap();
        let session = store
            .create(CreateRuntimeSession {
                id: None,
                workspace: root.clone(),
                profile: None,
                model: None,
                config: None,
                title: None,
            })
            .unwrap();
        let original = enqueue(&store, session.id);
        store.claim_next(session.id).unwrap().unwrap();
        let original_task = uuid::Uuid::new_v4();
        assert!(store.bind_task(original.id, original_task).unwrap());
        store
            .set_task_waiting(original_task, RuntimeTurnStatus::WaitingApproval)
            .unwrap();
        // This older message stays unclaimed while the independent message runs.
        let ordinary = include.then(|| enqueue(&store, session.id));
        let side = enqueue(&store, session.id);
        let snapshot = store.get(session.id).unwrap().unwrap();
        let queued = store
            .queued_messages(session.id)
            .unwrap()
            .into_iter()
            .find(|message| message.id == side.id)
            .unwrap();
        store
            .claim_independent(&snapshot, &queued)
            .unwrap()
            .unwrap();
        let side_task = uuid::Uuid::new_v4();
        Self {
            root,
            store,
            session,
            original,
            original_task,
            side,
            side_task,
            ordinary,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn enqueue(store: &RuntimeSessionStore, session: uuid::Uuid) -> RuntimeTurn {
    store
        .enqueue_turn(
            session,
            CreateRuntimeTurn {
                team_control: None,
                request_id: uuid::Uuid::new_v4(),
                prompt: "inspect".into(),
                attachments: Vec::new(),
                origin_client: Some("cli:test".into()),
                defer_until_released: false,
            },
        )
        .unwrap()
        .0
}

fn assert_completion_uses_current_foreground(claim_failure: bool, original_finishes: bool) {
    let fixture = Fixture::new();
    let store = &fixture.store;
    if !claim_failure {
        assert!(store.bind_task(fixture.side.id, fixture.side_task).unwrap());
    }
    std::thread::scope(|scope| {
        let (entered_tx, entered_rx) = sync_channel(0);
        let (resume_tx, resume_rx) = sync_channel(0);
        let side_id = fixture.side.id;
        let side_task = fixture.side_task;
        let worker = scope.spawn(move || {
            SESSION_LOCK_BARRIER
                .with(|barrier| *barrier.borrow_mut() = Some((entered_tx, resume_rx)));
            if claim_failure {
                store
                    .complete_claim_failure(side_id, "submission failed".into())
                    .unwrap();
            } else {
                store
                    .complete_task(side_task, RuntimeTaskStatus::Completed, None)
                    .unwrap();
            }
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // The old implementation has already captured WaitingApproval here.
        // The corrected lock order observes these changes before taking a snapshot.
        store
            .set_task_waiting(fixture.original_task, RuntimeTurnStatus::Running)
            .unwrap();
        let successor = if original_finishes {
            // A concurrent completion can finish the side lane too, before the
            // delayed completion obtains the Session lock.
            if claim_failure {
                store
                    .complete_claim_failure(fixture.side.id, "submission failed".into())
                    .unwrap();
            } else {
                store
                    .complete_task(fixture.side_task, RuntimeTaskStatus::Completed, None)
                    .unwrap();
            }
            store
                .complete_task(fixture.original_task, RuntimeTaskStatus::Completed, None)
                .unwrap();
            let next = enqueue(store, fixture.session.id);
            store.claim_next(fixture.session.id).unwrap().unwrap();
            let next_task = uuid::Uuid::new_v4();
            assert!(store.bind_task(next.id, next_task).unwrap());
            Some(next)
        } else {
            None
        };
        resume_tx.send(()).unwrap();
        worker.join().unwrap();
        let session = store.get(fixture.session.id).unwrap().unwrap();
        assert_eq!(session.status, RuntimeSessionStatus::Running);
        assert_eq!(
            session.active_turn_id,
            Some(
                successor
                    .as_ref()
                    .map_or(fixture.original.id, |turn| turn.id)
            )
        );
        assert_eq!(
            store.get_turn(fixture.original.id).unwrap().unwrap().status,
            if original_finishes {
                RuntimeTurnStatus::Completed
            } else {
                RuntimeTurnStatus::Running
            }
        );
        let (sessions, _) = load_sessions(&store.path).unwrap();
        assert_eq!(sessions[&session.id], session);
    });
}

fn assert_original_completion_preserves_independent_reservation(claim_failure: bool) {
    let fixture = Fixture::with_ordinary(true);
    let ordinary = fixture.ordinary.as_ref().unwrap();
    let store = &fixture.store;
    let claimed = store.get_turn(fixture.side.id).unwrap().unwrap();
    assert_eq!(claimed.status, RuntimeTurnStatus::Queued);
    assert!(claimed.execution_session_id.is_some());
    assert!(claimed.active_task_id.is_none());
    assert_eq!(
        store
            .get(fixture.session.id)
            .unwrap()
            .unwrap()
            .active_turn_id,
        Some(fixture.side.id)
    );
    // Deterministic order: claim has published the reservation, but binding has
    // not started when the old completion acquires sessions -> turns.
    if claim_failure {
        store
            .complete_claim_failure(fixture.original.id, "original startup failed".into())
            .unwrap();
    } else {
        assert_eq!(
            store.request_cancel(fixture.original.id).unwrap().task_id,
            Some(fixture.original_task)
        );
        store
            .complete_task(fixture.original_task, RuntimeTaskStatus::Cancelled, None)
            .unwrap();
    }
    let session = store.get(fixture.session.id).unwrap().unwrap();
    assert_eq!(session.active_turn_id, Some(fixture.side.id));
    assert_eq!(session.status, RuntimeSessionStatus::Queued);
    assert_eq!(store.get_turn(fixture.side.id).unwrap().unwrap(), claimed);
    let (persisted, _) = load_sessions(&store.path).unwrap();
    assert_eq!(persisted[&session.id], session);
    assert!(!store.bind_task(ordinary.id, uuid::Uuid::new_v4()).unwrap());
    assert!(store.bind_task(fixture.side.id, fixture.side_task).unwrap());
    let bound = store.get(fixture.session.id).unwrap().unwrap();
    assert_eq!(bound.active_turn_id, Some(fixture.side.id));
    assert_eq!(bound.status, RuntimeSessionStatus::Running);
    store
        .complete_task(fixture.side_task, RuntimeTaskStatus::Completed, None)
        .unwrap();
    let completed = store.get(fixture.session.id).unwrap().unwrap();
    assert!(completed.active_turn_id.is_none());
    assert_eq!(completed.status, RuntimeSessionStatus::Idle);
    // The older unclaimed queue head is eligible only after the reservation ends.
    assert_eq!(
        store
            .claim_next(fixture.session.id)
            .unwrap()
            .unwrap()
            .metadata
            .id,
        ordinary.id
    );
    assert!(store.bind_task(ordinary.id, uuid::Uuid::new_v4()).unwrap());
}

#[test]
fn task_completion_preserves_claimed_independent_queued_reservation() {
    assert_original_completion_preserves_independent_reservation(false);
}

#[test]
fn claim_failure_preserves_claimed_independent_queued_reservation() {
    assert_original_completion_preserves_independent_reservation(true);
}

#[test]
fn old_completion_preserves_queued_foreground_over_another_waiting_lane() {
    for claim_failure in [false, true] {
        let fixture = Fixture::new();
        let store = &fixture.store;
        assert!(store.bind_task(fixture.side.id, fixture.side_task).unwrap());
        store
            .set_task_waiting(fixture.side_task, RuntimeTurnStatus::WaitingAnswer)
            .unwrap();
        let newest = enqueue(store, fixture.session.id);
        let snapshot = store.get(fixture.session.id).unwrap().unwrap();
        let message = store
            .queued_messages(fixture.session.id)
            .unwrap()
            .into_iter()
            .find(|message| message.id == newest.id)
            .unwrap();
        store
            .claim_independent(&snapshot, &message)
            .unwrap()
            .unwrap();
        if claim_failure {
            store
                .complete_claim_failure(fixture.original.id, "startup failed".into())
                .unwrap();
        } else {
            store
                .complete_task(fixture.original_task, RuntimeTaskStatus::Cancelled, None)
                .unwrap();
        }
        let session = store.get(fixture.session.id).unwrap().unwrap();
        assert_eq!(session.active_turn_id, Some(newest.id));
        assert_eq!(session.status, RuntimeSessionStatus::Queued);
        assert!(store.bind_task(newest.id, uuid::Uuid::new_v4()).unwrap());
    }
}

fn assert_late_completion_preserves_claimed_regular_successor(claim_failure: bool) {
    let fixture = Fixture::new();
    let store = &fixture.store;
    store
        .complete_claim_failure(fixture.side.id, "startup failed".into())
        .unwrap();
    store
        .complete_task(fixture.original_task, RuntimeTaskStatus::Completed, None)
        .unwrap();
    let next = enqueue(store, fixture.session.id);
    store.claim_next(fixture.session.id).unwrap().unwrap();
    let claimed = store.get_turn(next.id).unwrap().unwrap();
    assert_eq!(claimed.status, RuntimeTurnStatus::Queued);
    assert!(claimed.execution_session_id.is_none());
    // A delayed completion arrives after the successor's claim, before its bind.
    if claim_failure {
        store
            .complete_claim_failure(fixture.side.id, "startup failed".into())
            .unwrap();
    } else {
        store
            .complete_task(fixture.original_task, RuntimeTaskStatus::Completed, None)
            .unwrap();
    }
    let session = store.get(fixture.session.id).unwrap().unwrap();
    assert_eq!(session.active_turn_id, Some(next.id));
    assert_eq!(session.status, RuntimeSessionStatus::Queued);
    assert_eq!(store.get_turn(next.id).unwrap().unwrap(), claimed);
    assert!(store.bind_task(next.id, uuid::Uuid::new_v4()).unwrap());
    assert_eq!(
        store.get(fixture.session.id).unwrap().unwrap().status,
        RuntimeSessionStatus::Running
    );
}

#[test]
fn task_completion_preserves_claimed_regular_queued_successor() {
    assert_late_completion_preserves_claimed_regular_successor(false);
}

#[test]
fn claim_failure_preserves_claimed_regular_queued_successor() {
    assert_late_completion_preserves_claimed_regular_successor(true);
}

#[test]
fn claim_failure_does_not_restore_stale_waiting_snapshot() {
    assert_completion_uses_current_foreground(true, false);
}

#[test]
fn claim_failure_does_not_reclaim_finished_turn_over_successor() {
    assert_completion_uses_current_foreground(true, true);
}

#[test]
fn task_completion_does_not_restore_stale_waiting_snapshot() {
    assert_completion_uses_current_foreground(false, false);
}

#[test]
fn task_completion_does_not_reclaim_finished_turn_over_successor() {
    assert_completion_uses_current_foreground(false, true);
}

#[test]
fn late_claim_failure_preserves_cancelled_turn_and_current_session() {
    for with_successor in [false, true] {
        let fixture = Fixture::new();
        let store = &fixture.store;
        std::thread::scope(|scope| {
            let (entered_tx, entered_rx) = sync_channel(0);
            let (resume_tx, resume_rx) = sync_channel(0);
            let side_id = fixture.side.id;
            let worker = scope.spawn(move || {
                SESSION_LOCK_BARRIER
                    .with(|barrier| *barrier.borrow_mut() = Some((entered_tx, resume_rx)));
                store
                    .complete_claim_failure(side_id, "late submission failure".into())
                    .unwrap();
            });
            entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(store.request_cancel(side_id).unwrap().cancelled_queued);
            let cancelled = store.get_turn(side_id).unwrap().unwrap();
            assert_eq!(cancelled.status, RuntimeTurnStatus::Cancelled);
            assert!(cancelled.completed_at.is_some());
            if with_successor {
                store
                    .complete_task(fixture.original_task, RuntimeTaskStatus::Completed, None)
                    .unwrap();
                let successor = enqueue(store, fixture.session.id);
                store.claim_next(fixture.session.id).unwrap().unwrap();
                assert_eq!(
                    store
                        .get(fixture.session.id)
                        .unwrap()
                        .unwrap()
                        .active_turn_id,
                    Some(successor.id)
                );
            }
            let session = store.get(fixture.session.id).unwrap().unwrap();
            resume_tx.send(()).unwrap();
            worker.join().unwrap();
            assert_eq!(store.get_turn(side_id).unwrap().unwrap(), cancelled);
            assert_eq!(
                load_turns(&store.turns_path).unwrap()[&side_id].metadata,
                cancelled
            );
            assert_eq!(store.get(fixture.session.id).unwrap().unwrap(), session);
            assert_eq!(load_sessions(&store.path).unwrap().0[&session.id], session);
        });
    }
}

#[test]
fn late_claim_failure_preserves_other_terminal_turns_and_current_session() {
    for status in [
        RuntimeTaskStatus::Completed,
        RuntimeTaskStatus::Partial,
        RuntimeTaskStatus::Interrupted,
    ] {
        let fixture = Fixture::new();
        let store = &fixture.store;
        assert!(store.bind_task(fixture.side.id, fixture.side_task).unwrap());
        store
            .complete_task(fixture.side_task, status, Some("first completion".into()))
            .unwrap();
        let completed = store.get_turn(fixture.side.id).unwrap().unwrap();
        assert!(turn_status_is_terminal(completed.status));
        let session = store.get(fixture.session.id).unwrap().unwrap();
        store
            .complete_claim_failure(fixture.side.id, "late submission failure".into())
            .unwrap();
        assert_eq!(store.get_turn(fixture.side.id).unwrap().unwrap(), completed);
        assert_eq!(
            load_turns(&store.turns_path).unwrap()[&fixture.side.id].metadata,
            completed
        );
        assert_eq!(store.get(fixture.session.id).unwrap().unwrap(), session);
        assert_eq!(load_sessions(&store.path).unwrap().0[&session.id], session);
    }
}

#[test]
fn claim_failure_replay_repairs_session_write_without_overwriting_first_failure() {
    let mut fixture = Fixture::new();
    let store = &mut fixture.store;
    let before = store.get(fixture.session.id).unwrap().unwrap();
    let blocked_path = fixture.root.join("blocked-session-write");
    std::fs::create_dir(&blocked_path).unwrap();
    let session_path = std::mem::replace(&mut store.path, blocked_path);
    assert!(
        store
            .complete_claim_failure(fixture.side.id, "first submission failure".into())
            .is_err()
    );
    store.path = session_path;
    let mut failed = store.get_turn(fixture.side.id).unwrap().unwrap();
    assert_eq!(failed.status, RuntimeTurnStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("first submission failure"));
    assert!(failed.completed_at.is_some());
    assert_eq!(
        load_turns(&store.turns_path).unwrap()[&fixture.side.id].metadata,
        failed
    );
    assert_eq!(load_sessions(&store.path).unwrap().0[&before.id], before);
    let completed_session = store.get(fixture.session.id).unwrap().unwrap();
    assert_eq!(completed_session.active_turn_id, Some(fixture.original.id));
    assert_eq!(
        completed_session.status,
        RuntimeSessionStatus::WaitingApproval
    );
    assert_ne!(completed_session, before);
    // A distinct durable timestamp catches overwrites even within the same second.
    failed.completed_at = Some(failed.completed_at.unwrap().saturating_sub(1));
    {
        let mut turns = store.turns_lock().unwrap();
        turns.get_mut(&fixture.side.id).unwrap().metadata = failed.clone();
        persist_turns(&store.turns_path, &turns).unwrap();
    }
    store
        .complete_claim_failure(fixture.side.id, "replayed submission failure".into())
        .unwrap();
    assert_eq!(store.get_turn(fixture.side.id).unwrap().unwrap(), failed);
    assert_eq!(
        load_turns(&store.turns_path).unwrap()[&fixture.side.id].metadata,
        failed
    );
    let session = store.get(fixture.session.id).unwrap().unwrap();
    assert_eq!(session.active_turn_id, Some(fixture.original.id));
    assert_eq!(session.status, RuntimeSessionStatus::WaitingApproval);
    assert_eq!(session.last_error, failed.error);
    assert_eq!(load_sessions(&store.path).unwrap().0[&session.id], session);
}

#[test]
fn claim_failure_restores_waiting_foreground_without_resolving_it() {
    let fixture = Fixture::new();
    fixture
        .store
        .complete_claim_failure(fixture.side.id, "submission failed".into())
        .unwrap();
    let session = fixture.store.get(fixture.session.id).unwrap().unwrap();
    assert_eq!(session.active_turn_id, Some(fixture.original.id));
    assert_eq!(session.status, RuntimeSessionStatus::WaitingApproval);
    assert_eq!(
        fixture
            .store
            .get_turn(fixture.original.id)
            .unwrap()
            .unwrap()
            .status,
        RuntimeTurnStatus::WaitingApproval
    );
    assert_eq!(
        fixture
            .store
            .get_turn(fixture.side.id)
            .unwrap()
            .unwrap()
            .status,
        RuntimeTurnStatus::Failed
    );
}
