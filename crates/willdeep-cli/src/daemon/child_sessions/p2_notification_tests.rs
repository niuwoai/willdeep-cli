use super::*;
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use willdeep_core::tools::SessionCollaboration;

async fn running_wake(fixture: &Fixture) -> (Uuid, Uuid) {
    let claimed = fixture
        .state
        .sessions
        .claim_next(fixture.parent)
        .unwrap()
        .unwrap();
    let id = Uuid::new_v4();
    assert!(
        fixture
            .state
            .sessions
            .bind_task(claimed.metadata.id, id)
            .unwrap()
    );
    fixture
        .state
        .tasks
        .insert_and_persist(crate::daemon::RuntimeTask {
            id,
            session_id: Some(fixture.parent),
            turn_id: Some(claimed.metadata.id),
            agent_id: None,
            event_start_sequence: 0,
            status: RuntimeTaskStatus::Running,
            workspace: fixture.root.join("workspace"),
            profile: None,
            model: None,
            prompt_excerpt: None,
            origin_client: fixture
                .state
                .sessions
                .turn_origin(claimed.metadata.id)
                .unwrap(),
            pid: None,
            created_at: now(),
            started_at: Some(now()),
            completed_at: None,
            exit_code: None,
            failure_domain: None,
            error: None,
        })
        .await
        .unwrap();
    (claimed.metadata.id, id)
}

fn assert_idle(fixture: &Fixture) {
    let parent = fixture.state.sessions.get(fixture.parent).unwrap().unwrap();
    assert_eq!(parent.status, RuntimeSessionStatus::Idle);
    assert!(parent.active_turn_id.is_none() && parent.last_error.is_none());
}

#[tokio::test]
async fn revoked_claimed_wake_finishes_normally_and_another_result_can_wake() {
    for operation in ["subscription", "takeover"] {
        let fixture = Fixture::new();
        let first = fixture.start();
        let second = fixture.start();
        fixture.finish(&first, RuntimeTaskStatus::Completed);
        let (turn, execution) = running_wake(&fixture).await;
        if operation == "subscription" {
            apply(&fixture, &subscription(&fixture, &first, "none", 1));
        } else {
            apply(
                &fixture,
                &control_request(&fixture, &first, "session.child.takeover", 1),
            );
        }
        let core = willdeep_core::SessionStore::new(&fixture.root)
            .load(fixture.parent)
            .unwrap();
        let error = fixture
            .state
            .sessions
            .prepare_execution(execution, &core)
            .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<willdeep_core::tools::ToolError>(),
            Some(willdeep_core::tools::ToolError::HostContextRevoked)
        ));
        // The production error classifier and TaskManager finish path must both
        // run: mapping this to ordinary Cancelled alone used to leave Failed.
        fixture
            .state
            .tasks
            .finish_runtime_error(execution, error.context("prepare Runtime execution"))
            .await
            .unwrap();
        assert_idle(&fixture);
        let task = fixture.state.tasks.get(execution).await.unwrap();
        assert_eq!(task.status, RuntimeTaskStatus::Cancelled);
        assert!(task.error.is_none() && task.failure_domain.is_none());
        assert_eq!(
            fixture
                .state
                .sessions
                .get_turn(turn)
                .unwrap()
                .unwrap()
                .status,
            RuntimeTurnStatus::Cancelled
        );
        assert!(
            fixture
                .get(first.assignment_id)
                .result
                .unwrap()
                .consumed_at
                .is_none()
        );
        fixture
            .state
            .sessions
            .children
            .transaction(|ledger| {
                assert!(!ledger.control(fixture.parent).suppressed);
                Ok(())
            })
            .unwrap();
        // Check the persisted Session too, not just the in-memory status.
        let reopened =
            RuntimeSessionStore::open(fixture.root.join("runtime-sessions.json"), &fixture.root)
                .unwrap();
        assert_eq!(
            reopened.get(fixture.parent).unwrap().unwrap().status,
            RuntimeSessionStatus::Idle
        );
        fixture.finish(&second, RuntimeTaskStatus::Completed);
        let next = fixture
            .state
            .sessions
            .claim_next(fixture.parent)
            .unwrap()
            .unwrap();
        assert_ne!(next.metadata.id, turn);
        let notices = fixture
            .state
            .sessions
            .children
            .host_context_for_turn(fixture.parent, Some(next.metadata.id))
            .unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].0,
            fixture
                .get(second.assignment_id)
                .result
                .unwrap()
                .notification_id
        );
    }
}

#[tokio::test]
async fn skipped_wake_preserves_a_real_parent_stop_generation_and_suppression() {
    let fixture = Fixture::new();
    let first = fixture.start();
    let second = fixture.start();
    fixture.finish(&first, RuntimeTaskStatus::Completed);
    let (turn, execution) = running_wake(&fixture).await;
    let generation = fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            ledger.stop(fixture.parent, turn);
            Ok(ledger.control(fixture.parent).generation)
        })
        .unwrap();
    let core = willdeep_core::SessionStore::new(&fixture.root)
        .load(fixture.parent)
        .unwrap();
    let error = fixture
        .state
        .sessions
        .prepare_execution(execution, &core)
        .unwrap_err();
    fixture
        .state
        .tasks
        .finish_runtime_error(execution, error)
        .await
        .unwrap();
    assert_idle(&fixture);
    fixture.finish(&second, RuntimeTaskStatus::Completed);
    fixture
        .state
        .sessions
        .children
        .transaction(|ledger| {
            let control = ledger.control(fixture.parent);
            assert!(control.suppressed);
            assert_eq!(control.generation, generation);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        fixture
            .state
            .sessions
            .list_turns(fixture.parent)
            .unwrap()
            .len(),
        1
    );
    assert!(
        fixture
            .state
            .sessions
            .claim_next(fixture.parent)
            .unwrap()
            .is_none()
    );
}

struct PausedNotifications {
    inner: crate::daemon::peer_client::PeerSessionClient,
    snapshot_read: Arc<Barrier>,
    release_snapshot: Arc<Barrier>,
    paused: AtomicBool,
}

#[async_trait::async_trait]
impl SessionCollaboration for PausedNotifications {
    fn host_context(
        &self,
    ) -> std::result::Result<Vec<(Uuid, String)>, willdeep_core::tools::ToolError> {
        let snapshot = self.inner.host_context()?;
        if !self.paused.swap(true, Ordering::SeqCst) {
            self.snapshot_read.wait();
            self.release_snapshot.wait();
        }
        Ok(snapshot)
    }
    fn commit_host_context(
        &self,
        candidates: &[(Uuid, String)],
        commit: &mut willdeep_core::tools::HostContextCommit<'_>,
    ) -> std::result::Result<(), willdeep_core::AgentError> {
        self.inner.commit_host_context(candidates, commit)
    }
    async fn execute(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> std::result::Result<String, willdeep_core::tools::ToolError> {
        unreachable!("the fixture never dispatches tools")
    }
}

struct NoRevokedResult {
    marker: String,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl willdeep_core::provider::Provider for NoRevokedResult {
    async fn complete(
        &self,
        messages: &[willdeep_core::Message],
        _: &[willdeep_core::types::ToolDefinition],
    ) -> std::result::Result<willdeep_core::types::Completion, willdeep_core::provider::ProviderError>
    {
        assert!(
            !messages
                .iter()
                .any(|message| message.content.contains(&self.marker)),
            "a revoked notification reached the provider"
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(willdeep_core::types::Completion {
            reasoning: None,
            content: "Continued the existing task.".into(),
            tool_calls: Vec::new(),
            usage: None,
            finish_reason: Some("stop".into()),
        })
    }
}

#[tokio::test]
async fn revocation_after_snapshot_before_checkpoint_never_reaches_history_or_model() {
    for wake in [false, true] {
        for operation in ["none", "inbox", "takeover", "detach"] {
            let fixture = Fixture::new();
            let child = fixture.start();
            fixture.finish(&child, RuntimeTaskStatus::Completed);
            let runtime_task = if wake {
                Some(running_wake(&fixture).await)
            } else {
                None
            };
            let snapshot_read = Arc::new(Barrier::new(2));
            let release_snapshot = Arc::new(Barrier::new(2));
            let calls = Arc::new(AtomicUsize::new(0));
            let notification = fixture
                .get(child.assignment_id)
                .result
                .unwrap()
                .notification_id;
            let marker = format!("<host-context id=\"{notification}\">\n");
            let root = fixture.root.clone();
            let parent = fixture.parent;
            let client = PausedNotifications {
                inner: crate::daemon::peer_client::PeerSessionClient {
                    home: root.clone(),
                    session_id: parent,
                    turn_id: runtime_task.map(|(turn, _)| turn),
                },
                snapshot_read: snapshot_read.clone(),
                release_snapshot: release_snapshot.clone(),
                paused: AtomicBool::new(false),
            };
            let provider = NoRevokedResult {
                marker: marker.clone(),
                calls: calls.clone(),
            };
            let result = std::thread::scope(|scope| {
                let worker = scope.spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    let store = willdeep_core::SessionStore::new(&root);
                    let tools = willdeep_core::tools::ToolRegistry::new(
                        &root,
                        willdeep_core::tools::ApprovalMode::ReadOnly,
                    )
                    .unwrap()
                    .with_session_collaboration(Arc::new(client));
                    let agent = willdeep_core::Agent::new(
                        Arc::new(provider),
                        tools,
                        willdeep_core::AgentConfig {
                            max_turns: 1,
                            system_prompt: String::new(),
                            context_window: 128_000,
                            token_budget: None,
                        },
                    );
                    let sink = willdeep_core::checkpoint::SessionCheckpointSink {
                        store: store.clone(),
                        session_id: parent,
                    };
                    runtime.block_on(agent.run_checkpointed(
                        store.load(parent).unwrap().messages,
                        willdeep_core::Message::host_instruction("Continue the existing task"),
                        Some(&sink),
                    ))
                });
                snapshot_read.wait();
                // The snapshot is outside ChildStore now. Revoke successfully
                // before releasing the synchronous checkpoint commit boundary.
                if matches!(operation, "none" | "inbox") {
                    apply(&fixture, &subscription(&fixture, &child, operation, 1));
                } else {
                    apply(
                        &fixture,
                        &control_request(
                            &fixture,
                            &child,
                            &format!("session.child.{operation}"),
                            1,
                        ),
                    );
                }
                release_snapshot.wait();
                worker.join().unwrap()
            });
            if let Some((_, execution)) = runtime_task {
                assert!(matches!(
                    result,
                    Err(willdeep_core::AgentError::Tool(
                        willdeep_core::tools::ToolError::HostContextRevoked
                    ))
                ));
                fixture
                    .state
                    .tasks
                    .finish_runtime_error(execution, result.unwrap_err().into())
                    .await
                    .unwrap();
                assert_idle(&fixture);
                assert_eq!(calls.load(Ordering::SeqCst), 0);
            } else {
                let outcome = result.unwrap();
                assert!(
                    !outcome
                        .messages
                        .iter()
                        .any(|message| message.content.starts_with(&marker))
                );
                assert_eq!(calls.load(Ordering::SeqCst), 1);
            }
            let saved = willdeep_core::SessionStore::new(&fixture.root)
                .load(parent)
                .unwrap();
            assert!(
                !saved
                    .messages
                    .iter()
                    .any(|message| message.content.starts_with(&marker))
            );
            fixture
                .state
                .sessions
                .children
                .host_context(fixture.parent)
                .unwrap();
            assert!(
                fixture
                    .get(child.assignment_id)
                    .result
                    .unwrap()
                    .consumed_at
                    .is_none()
            );
        }
    }
}

#[test]
fn notification_commit_serializes_control_through_the_durable_history_write() {
    use fs2::FileExt;
    let fixture = Fixture::new();
    let child = fixture.start();
    fixture.finish(&child, RuntimeTaskStatus::Completed);
    let client = crate::daemon::peer_client::PeerSessionClient {
        home: fixture.root.clone(),
        session_id: fixture.parent,
        turn_id: None,
    };
    let candidates = client.host_context().unwrap();
    let store = willdeep_core::SessionStore::new(&fixture.root);
    client
        .commit_host_context(&candidates, &mut |eligible| {
            let contender = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(fixture.root.join("runtime/session-children.lock"))
                .unwrap();
            assert_eq!(
                FileExt::try_lock_exclusive(&contender)
                    .unwrap_err()
                    .raw_os_error(),
                fs2::lock_contended_error().raw_os_error()
            );
            assert_eq!(eligible.len(), 1);
            store
                .update(fixture.parent, |session| {
                    session
                        .messages
                        .push(willdeep_core::Message::host_instruction(format!(
                            "<host-context id=\"{}\">\n{}\n</host-context>",
                            eligible[0].0, eligible[0].1
                        )));
                })
                .map_err(|error| willdeep_core::AgentError::Checkpoint(error.to_string()))?;
            Ok(())
        })
        .unwrap();
    // Successful save does not forge a receipt: only the usual reconciliation
    // may inspect that saved marker and acknowledge it afterward.
    assert!(
        fixture
            .get(child.assignment_id)
            .result
            .unwrap()
            .consumed_at
            .is_none()
    );
    let contender = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.root.join("runtime/session-children.lock"))
        .unwrap();
    FileExt::try_lock_exclusive(&contender).unwrap();
    FileExt::unlock(&contender).unwrap();
    apply(&fixture, &subscription(&fixture, &child, "none", 1));
    fixture
        .state
        .sessions
        .children
        .host_context(fixture.parent)
        .unwrap();
    assert!(
        fixture
            .get(child.assignment_id)
            .result
            .unwrap()
            .consumed_at
            .is_some()
    );
}

#[test]
fn failed_checkpoint_does_not_consume_a_still_eligible_result() {
    let fixture = Fixture::new();
    let child = fixture.start();
    fixture.finish(&child, RuntimeTaskStatus::Completed);
    let client = crate::daemon::peer_client::PeerSessionClient {
        home: fixture.root.clone(),
        session_id: fixture.parent,
        turn_id: None,
    };
    let candidates = client.host_context().unwrap();
    let error = client
        .commit_host_context(&candidates, &mut |eligible| {
            assert_eq!(eligible.len(), 1);
            assert_eq!(eligible[0].0, candidates[0].0);
            Err(willdeep_core::AgentError::Checkpoint(
                "isolated checkpoint failure".into(),
            ))
        })
        .unwrap_err();
    assert!(matches!(error, willdeep_core::AgentError::Checkpoint(_)));
    fixture
        .state
        .sessions
        .children
        .host_context(fixture.parent)
        .unwrap();
    assert!(
        fixture
            .get(child.assignment_id)
            .result
            .unwrap()
            .consumed_at
            .is_none()
    );
    assert!(
        willdeep_core::SessionStore::new(&fixture.root)
            .load(fixture.parent)
            .unwrap()
            .messages
            .is_empty()
    );
}
