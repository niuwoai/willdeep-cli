use super::*;
use crate::daemon::child_sessions::Ledger;
use willdeep_runtime_protocol::child_session::CHILD_SESSION_RUNNING_LIMIT;

impl RuntimeSessionStore {
    pub fn claim_next(&self, session_id: uuid::Uuid) -> Result<Option<ClaimedRuntimeTurn>> {
        self.children
            .transaction(|ledger| self.claim_next_under_control(session_id, ledger))
    }

    fn claim_next_under_control(
        &self,
        session_id: uuid::Uuid,
        ledger: &mut Ledger,
    ) -> Result<Option<ClaimedRuntimeTurn>> {
        let mut sessions = self.lock()?;
        let session = sessions
            .get(&session_id)
            .context("Runtime Session not found")?;
        if !session_can_claim(session) {
            return Ok(None);
        }
        let mut turns = self.turns_lock()?;
        let Some(turn) = turns
            .values()
            .filter(|turn| {
                turn.metadata.session_id == session_id
                    && turn.metadata.status == RuntimeTurnStatus::Queued
                    && !turn.metadata.held
            })
            .min_by_key(|turn| (turn.metadata.queue_sequence, turn.metadata.id))
        else {
            return Ok(None);
        };
        // Signals can arrive out of order after a full-capacity attempt. Decide
        // fairness at the reservation boundary under control -> sessions -> turns.
        // Invalid wakes still reach their cancellation path below; roots never
        // participate in child slot admission or wait behind delegated work.
        if session.parent_session_id.is_some()
            && turn.team_control.is_none()
            && Self::child_wake_is_eligible(session, turn, ledger)
            && oldest_claimable_child(&sessions, &turns, ledger) != Some(turn.metadata.id)
        {
            return Ok(None);
        }
        let turn_id = turn.metadata.id;
        let session = sessions.get_mut(&session_id).unwrap();
        let turn = turns.get_mut(&turn_id).unwrap();
        if !self.check_child_claim(session, turn, ledger)? {
            let stale_wake = turn.origin_client.as_deref().is_some_and(|origin| {
                origin.starts_with(crate::daemon::child_sessions::WAKE_ORIGIN)
            });
            if stale_wake {
                turn.metadata.status = RuntimeTurnStatus::Cancelled;
                turn.metadata.completed_at = Some(now());
                persist_turns(&self.turns_path, &turns)?;
            }
            return Ok(None);
        }
        self.children.persist(ledger)?;
        turn.metadata.attempts = turn.metadata.attempts.saturating_add(1);
        let preparation = (|| -> Result<_> {
            let mut core = self
                .core
                .load_baselined(session_id)
                .with_context(|| format!("load Core Session {session_id}"))?;
            if core.magpie_integration != session.magpie_integration {
                core.magpie_integration = session.magpie_integration;
                self.core
                    .save(&mut core)
                    .context("save Core integration settings before claim")?;
            }
            let message_start = if turn.replay_existing_user_message {
                core.messages
                    .len()
                    .checked_sub(1)
                    .context("recovered Turn is missing its persisted user message")?
            } else {
                core.messages.len()
            };
            Ok((core.compression_generation, message_start))
        })();
        let (message_generation, message_start) = match preparation {
            Ok(boundary) => boundary,
            Err(error) => {
                // A failed queue head must not retain global child priority or
                // its reservation. Publish a durable failure before releasing
                // capacity; reconciliation records the assignment's real result.
                let error =
                    format!("prepare Runtime Turn {turn_id} for Session {session_id}: {error:#}");
                turn.metadata.status = RuntimeTurnStatus::Failed;
                turn.metadata.completed_at = Some(now());
                turn.metadata.error = Some(error.clone());
                session.status = RuntimeSessionStatus::Failed;
                session.updated_at = now();
                session.last_error = Some(error);
                persist_turns(&self.turns_path, &turns)?;
                persist_sessions(&self.path, &sessions)?;
                ledger.slots.remove(&turn_id);
                return Ok(None);
            }
        };
        turn.metadata.message_generation = message_generation;
        turn.metadata.message_start = Some(message_start);
        turn.metadata.message_end = None;
        session.status = RuntimeSessionStatus::Queued;
        session.active_turn_id = Some(turn.metadata.id);
        session.updated_at = now();
        session.last_error = None;
        let claimed = ClaimedRuntimeTurn {
            metadata: turn.metadata.clone(),
            request: SubmitTask {
                execution_session_id: None,
                team_control: turn.team_control.clone(),
                prompt: turn.prompt.clone(),
                attachments: turn.attachments.clone(),
                workspace: session.workspace.clone(),
                workspace_access: None,
                approval_handle: None,
                instruction_inbox: None,
                workspace_skills: None,
                workspace_mcp_servers: None,
                profile: session.profile.clone(),
                model: session.model.clone(),
                config: session.config.clone(),
                session_id: Some(session.id),
                turn_id: Some(turn.metadata.id),
                origin_client: turn.origin_client.clone(),
                replay_existing_user_message: turn.replay_existing_user_message,
            },
        };
        persist_turns(&self.turns_path, &turns)?;
        persist_sessions(&self.path, &sessions)?;
        Ok(Some(claimed))
    }

    /// Called after the Core execution lease is acquired, before any new message.
    pub fn prepare_execution(
        &self,
        task_id: uuid::Uuid,
        core: &willdeep_core::Session,
    ) -> Result<()> {
        self.children
            .transaction(|ledger| self.prepare_execution_under_control(task_id, core, ledger))
    }

    fn prepare_execution_under_control(
        &self,
        task_id: uuid::Uuid,
        core: &willdeep_core::Session,
        ledger: &super::super::child_sessions::Ledger,
    ) -> Result<()> {
        let mut turns = self.turns_lock()?;
        let Some(turn) = turns
            .values_mut()
            .find(|turn| turn.metadata.active_task_id == Some(task_id))
        else {
            return Ok(()); // Legacy tasks need not belong to a Runtime Turn.
        };
        anyhow::ensure!(
            turn.metadata
                .execution_session_id
                .unwrap_or(turn.metadata.session_id)
                == core.id
                && turn.metadata.status == RuntimeTurnStatus::Running,
            "Runtime Turn is no longer eligible for execution"
        );
        if turn
            .origin_client
            .as_deref()
            .is_some_and(|origin| origin.starts_with(super::super::child_sessions::WAKE_ORIGIN))
        {
            let control = ledger.control(turn.metadata.session_id);
            let pending = ledger
                .pending(turn.metadata.session_id)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<HashSet<_>>();
            if control.suppressed
                || !ledger.wakes.values().any(|batch| {
                    batch.turn_id == Some(turn.metadata.id)
                        && !batch.invalidated
                        && batch.generation == control.generation
                        && (batch.members.iter().any(|id| pending.contains(id))
                            || ledger.wake_has_consumed_result(batch))
                })
            {
                return Err(willdeep_core::tools::ToolError::HostContextRevoked.into());
            }
        }
        if turn.replay_existing_user_message {
            anyhow::ensure!(
                turn.metadata.message_generation == core.compression_generation
                    && turn
                        .metadata
                        .message_start
                        .and_then(|start| start.checked_add(1))
                        == Some(core.messages.len()),
                "recovered Runtime Turn history changed before execution"
            );
            let message = core
                .messages
                .last()
                .context("recovered user message is missing")?;
            anyhow::ensure!(
                message.role == willdeep_core::Role::User
                    && message.content == turn.prompt
                    && serde_json::to_vec(&message.attachments)?
                        == serde_json::to_vec(&turn.attachments)?,
                "recovered Runtime Turn user message changed before execution"
            );
        } else {
            turn.metadata.message_start = Some(core.messages.len());
            turn.metadata.message_generation = core.compression_generation;
        }
        turn.metadata.message_end = None;
        persist_turns(&self.turns_path, &turns)
    }
}

fn session_can_claim(session: &RuntimeSession) -> bool {
    session.active_turn_id.is_none()
        && matches!(
            session.status,
            RuntimeSessionStatus::Idle
                | RuntimeSessionStatus::Failed
                | RuntimeSessionStatus::Interrupted
        )
}

/// Only compare each available child's actual queue head. A later valid Turn
/// cannot lend its priority to an earlier stale wake in the same Session.
fn oldest_claimable_child(
    sessions: &HashMap<uuid::Uuid, RuntimeSession>,
    turns: &HashMap<uuid::Uuid, StoredRuntimeTurn>,
    ledger: &Ledger,
) -> Option<uuid::Uuid> {
    let mut heads = HashMap::<uuid::Uuid, &StoredRuntimeTurn>::new();
    for turn in turns
        .values()
        .filter(|turn| turn.metadata.status == RuntimeTurnStatus::Queued && !turn.metadata.held)
    {
        let Some(session) = sessions
            .get(&turn.metadata.session_id)
            .filter(|session| session.parent_session_id.is_some() && session_can_claim(session))
        else {
            continue;
        };
        heads
            .entry(session.id)
            .and_modify(|head| {
                if (turn.metadata.queue_sequence, turn.metadata.id)
                    < (head.metadata.queue_sequence, head.metadata.id)
                {
                    *head = turn;
                }
            })
            .or_insert(turn);
    }
    heads
        .values()
        .filter_map(|turn| {
            let session = sessions.get(&turn.metadata.session_id)?;
            let parent = session.parent_session_id?;
            let reserved = ledger.slots.get(&turn.metadata.id) == Some(&parent);
            let has_capacity = ledger.slots.len() < CHILD_SESSION_RUNNING_LIMIT
                && ledger.slots.values().filter(|id| **id == parent).count()
                    < CHILD_SESSION_RUNNING_LIMIT;
            if turn.team_control.is_some()
                || !RuntimeSessionStore::child_wake_is_eligible(session, turn, ledger)
                || !(reserved || has_capacity)
            {
                return None;
            }
            Some((turn.metadata.queue_sequence, session.id, turn.metadata.id))
        })
        .min()
        .map(|(_, _, turn)| turn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::child_sessions as delegation;
    use willdeep_runtime_protocol::{ApiRequest, child_session::ChildSessionTask};

    struct Fixture {
        root: PathBuf,
        state: ServerState,
        parent: uuid::Uuid,
        _scheduled: tokio::sync::mpsc::UnboundedReceiver<uuid::Uuid>,
    }

    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("child-claim-fairness-{}", uuid::Uuid::new_v4()));
            let workspace = root.join("workspace");
            std::fs::create_dir_all(&workspace).unwrap();
            let sessions = crate::daemon::tests::test_runtime_session_store(&root);
            let parent = sessions
                .create(CreateRuntimeSession {
                    id: None,
                    workspace,
                    profile: None,
                    model: None,
                    config: None,
                    title: Some("Parent".into()),
                })
                .unwrap()
                .id;
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let state =
                crate::daemon::tests::test_server_state_with_scheduler(&root, sessions, sender);
            Self {
                root,
                state,
                parent,
                _scheduled: receiver,
            }
        }

        fn start(&self) -> ChildSessionTask {
            let request = ApiRequest::new(
                "session.child.start",
                serde_json::json!({
                    "parent_session_id": self.parent,
                    "title": "Inspect source",
                    "prompt": "Inspect source and report evidence",
                    "notify": "none",
                }),
            );
            serde_json::from_value(
                delegation::dispatch_for_test(&self.state, &request)
                    .map_err(|_| "child fixture request failed")
                    .unwrap(),
            )
            .unwrap()
        }

        fn enqueue(&self, session: uuid::Uuid, held: bool) -> RuntimeTurn {
            self.state
                .sessions
                .enqueue_turn(
                    session,
                    CreateRuntimeTurn {
                        team_control: None,
                        request_id: uuid::Uuid::new_v4(),
                        prompt: "Interactive follow-up".into(),
                        attachments: Vec::new(),
                        origin_client: None,
                        defer_until_released: held,
                    },
                )
                .unwrap()
                .0
        }

        fn claim(&self, session: uuid::Uuid) -> Option<ClaimedRuntimeTurn> {
            self.state.sessions.claim_next(session).unwrap()
        }

        fn turn(&self, turn: uuid::Uuid) -> RuntimeTurn {
            self.state.sessions.get_turn(turn).unwrap().unwrap()
        }

        fn release_terminal_slot(&self, turn: uuid::Uuid) {
            assert!(turn_status_is_terminal(self.turn(turn).status));
            // Match reconciliation's release after the real Runtime terminal write.
            self.state
                .sessions
                .children
                .transaction(|ledger| {
                    assert!(ledger.slots.remove(&turn).is_some());
                    Ok(())
                })
                .unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn missing_replay_message_fails_preparation_and_releases_child_priority() {
        let fixture = Fixture::new();
        let broken = fixture.start();
        let healthy = fixture.start();
        let store = &fixture.state.sessions;
        let turn_id = broken.child_turn_id.unwrap();
        let core_store = willdeep_core::SessionStore::new(&fixture.root);
        let mut core = core_store.load(broken.child_session_id).unwrap();
        core.messages.clear();
        core_store.save(&mut core).unwrap();
        {
            let mut turns = store.turns_lock().unwrap();
            turns
                .get_mut(&turn_id)
                .unwrap()
                .replay_existing_user_message = true;
            persist_turns(&store.turns_path, &turns).unwrap();
        }
        assert!(fixture.claim(broken.child_session_id).is_none());
        let failed = fixture.turn(turn_id);
        assert_eq!(failed.status, RuntimeTurnStatus::Failed);
        assert_eq!(failed.attempts, 1);
        assert!(failed.completed_at.is_some() && failed.active_task_id.is_none());
        assert!(
            failed
                .error
                .unwrap()
                .contains("recovered Turn is missing its persisted user message")
        );
        store
            .children
            .transaction(|ledger| {
                assert!(ledger.slots.is_empty());
                Ok(())
            })
            .unwrap();
        assert_eq!(
            fixture.claim(healthy.child_session_id).unwrap().metadata.id,
            healthy.child_turn_id.unwrap()
        );
        assert!(fixture.claim(broken.child_session_id).is_none());
        assert_eq!(fixture.turn(turn_id).attempts, 1);
    }

    #[test]
    fn an_older_child_denied_at_capacity_keeps_priority_when_a_newer_signal_arrives_first() {
        let fixture = Fixture::new();
        let children = (0..CHILD_SESSION_RUNNING_LIMIT + 2)
            .map(|_| fixture.start())
            .collect::<Vec<_>>();
        let store = &fixture.state.sessions;
        let occupied = children[..CHILD_SESSION_RUNNING_LIMIT]
            .iter()
            .map(|child| fixture.claim(child.child_session_id).unwrap())
            .collect::<Vec<_>>();
        let older = &children[CHILD_SESSION_RUNNING_LIMIT];
        let newer = &children[CHILD_SESSION_RUNNING_LIMIT + 1];
        let older_turn = older.child_turn_id.unwrap();
        let newer_turn = newer.child_turn_id.unwrap();
        assert!(fixture.turn(older_turn).queue_sequence < fixture.turn(newer_turn).queue_sequence);
        assert!(fixture.claim(older.child_session_id).is_none());

        // Full child capacity and an earlier child queue must not block interaction.
        let interactive = fixture.enqueue(fixture.parent, false);
        assert_eq!(
            fixture.claim(fixture.parent).unwrap().metadata.id,
            interactive.id
        );
        store
            .children
            .transaction(|ledger| {
                assert_eq!(ledger.slots.len(), CHILD_SESSION_RUNNING_LIMIT);
                assert!(!ledger.slots.contains_key(&interactive.id));
                Ok(())
            })
            .unwrap();

        assert!(
            store
                .request_cancel(occupied[0].metadata.id)
                .unwrap()
                .cancelled_queued
        );
        fixture.release_terminal_slot(occupied[0].metadata.id);
        assert!(fixture.claim(newer.child_session_id).is_none());
        let waiting = fixture.turn(newer_turn);
        assert_eq!(waiting.status, RuntimeTurnStatus::Queued);
        assert_eq!(waiting.attempts, 0);
        store
            .children
            .transaction(|ledger| {
                assert_eq!(ledger.slots.len(), CHILD_SESSION_RUNNING_LIMIT - 1);
                assert!(!ledger.slots.contains_key(&newer_turn));
                Ok(())
            })
            .unwrap();

        let claimed = fixture.claim(older.child_session_id).unwrap();
        assert_eq!(claimed.metadata.id, older_turn);
        let task = uuid::Uuid::new_v4();
        assert!(store.bind_task(older_turn, task).unwrap());
        assert_eq!(fixture.turn(older_turn).status, RuntimeTurnStatus::Running);
        assert!(fixture.claim(newer.child_session_id).is_none());
        store
            .complete_task(task, RuntimeTaskStatus::Completed, None)
            .unwrap();
        fixture.release_terminal_slot(older_turn);
        assert_eq!(
            fixture.claim(newer.child_session_id).unwrap().metadata.id,
            newer_turn
        );
    }

    #[test]
    fn unavailable_older_child_sessions_do_not_block_a_newer_claim() {
        for (status, active) in [
            (RuntimeSessionStatus::Idle, true),
            (RuntimeSessionStatus::Queued, false),
            (RuntimeSessionStatus::Running, false),
            (RuntimeSessionStatus::WaitingApproval, false),
            (RuntimeSessionStatus::WaitingAnswer, false),
            (RuntimeSessionStatus::Archived, false),
        ] {
            let fixture = Fixture::new();
            let older = fixture.start();
            let newer = fixture.start();
            let store = &fixture.state.sessions;
            {
                let mut sessions = store.lock().unwrap();
                let session = sessions.get_mut(&older.child_session_id).unwrap();
                session.status = status;
                session.active_turn_id = active.then_some(older.child_turn_id.unwrap());
                persist_sessions(&store.path, &sessions).unwrap();
            }
            assert!(
                fixture.claim(older.child_session_id).is_none(),
                "{status:?}"
            );
            assert_eq!(
                fixture.claim(newer.child_session_id).unwrap().metadata.id,
                newer.child_turn_id.unwrap(),
                "{status:?}"
            );
        }
    }

    #[test]
    fn a_held_head_does_not_lend_priority_to_a_later_unheld_turn() {
        let fixture = Fixture::new();
        let older = fixture.start();
        let held = older.child_turn_id.unwrap();
        let store = &fixture.state.sessions;
        {
            let mut turns = store.turns_lock().unwrap();
            turns.get_mut(&held).unwrap().metadata.held = true;
            persist_turns(&store.turns_path, &turns).unwrap();
        }
        let next = fixture.start();
        let later = fixture.enqueue(older.child_session_id, false);
        assert!(fixture.claim(older.child_session_id).is_none());
        assert_eq!(
            fixture.claim(next.child_session_id).unwrap().metadata.id,
            next.child_turn_id.unwrap()
        );
        assert_eq!(
            fixture.claim(older.child_session_id).unwrap().metadata.id,
            later.id
        );
        assert!(fixture.turn(held).held);
    }

    #[test]
    fn invalid_older_wakes_do_not_block_children_and_are_still_cancelled() {
        for with_batch in [false, true] {
            let fixture = Fixture::new();
            let older = fixture.start();
            let newer = fixture.start();
            let store = &fixture.state.sessions;
            let stale = older.child_turn_id.unwrap();
            let batch = uuid::Uuid::new_v4();
            {
                let mut turns = store.turns_lock().unwrap();
                turns.get_mut(&stale).unwrap().origin_client =
                    Some(format!("{}{batch}", delegation::WAKE_ORIGIN));
                persist_turns(&store.turns_path, &turns).unwrap();
            }
            if with_batch {
                let metadata = fixture.turn(stale);
                store
                    .children
                    .transaction(|ledger| {
                        ledger.wakes.insert(
                            batch,
                            serde_json::from_value(serde_json::json!({
                                "id": batch, "parent": older.child_session_id,
                                "generation": 0, "request_id": metadata.request_id,
                                "members": [], "turn_id": stale,
                                "started": false, "invalidated": true,
                            }))?,
                        );
                        Ok(())
                    })
                    .unwrap();
            }
            assert!(fixture.claim(newer.child_session_id).is_some());
            assert!(fixture.claim(older.child_session_id).is_none());
            let cancelled = fixture.turn(stale);
            assert_eq!(cancelled.status, RuntimeTurnStatus::Cancelled);
            assert_eq!(cancelled.attempts, 0);
            let next = fixture.enqueue(older.child_session_id, false);
            assert_eq!(
                fixture.claim(older.child_session_id).unwrap().metadata.id,
                next.id
            );
        }
    }

    #[test]
    fn a_recoverable_existing_slot_is_claimable_when_older_work_cannot_reserve() {
        let fixture = Fixture::new();
        let children = (0..CHILD_SESSION_RUNNING_LIMIT + 1)
            .map(|_| fixture.start())
            .collect::<Vec<_>>();
        let store = &fixture.state.sessions;
        store
            .children
            .transaction(|ledger| {
                // Model the crash boundary after slot persistence but before claim publication.
                for child in &children[1..] {
                    assert!(ledger.reserve_slot(child.child_turn_id.unwrap(), fixture.parent));
                }
                Ok(())
            })
            .unwrap();
        assert!(fixture.claim(children[0].child_session_id).is_none());
        assert_eq!(
            fixture
                .claim(children[1].child_session_id)
                .unwrap()
                .metadata
                .id,
            children[1].child_turn_id.unwrap()
        );
        store
            .children
            .transaction(|ledger| {
                assert_eq!(ledger.slots.len(), CHILD_SESSION_RUNNING_LIMIT);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_valid_root_result_wake_runs_while_older_children_wait_at_capacity() {
        use willdeep_runtime_protocol::child_session::{
            ChildSessionNotify, ChildSessionResult, ChildSessionSubscription,
            ChildSessionTaskStatus,
        };
        let fixture = Fixture::new();
        let source = fixture.start();
        let store = &fixture.state.sessions;
        let completed = fixture.claim(source.child_session_id).unwrap();
        let task_id = uuid::Uuid::new_v4();
        assert!(store.bind_task(completed.metadata.id, task_id).unwrap());
        store
            .complete_task(task_id, RuntimeTaskStatus::Completed, None)
            .unwrap();
        fixture.release_terminal_slot(completed.metadata.id);
        for _ in 0..CHILD_SESSION_RUNNING_LIMIT {
            let child = fixture.start();
            assert!(fixture.claim(child.child_session_id).is_some());
        }
        let waiting = fixture.start();
        assert!(fixture.claim(waiting.child_session_id).is_none());
        let batch = uuid::Uuid::new_v4();
        let (wake, _) = store
            .enqueue_turn(
                fixture.parent,
                CreateRuntimeTurn {
                    team_control: None,
                    request_id: uuid::Uuid::new_v4(),
                    prompt: "Host result is available".into(),
                    attachments: Vec::new(),
                    origin_client: Some(format!("{}{batch}", delegation::WAKE_ORIGIN)),
                    defer_until_released: false,
                },
            )
            .unwrap();
        let notice = uuid::Uuid::new_v4();
        store
            .children
            .transaction(|ledger| {
                let generation = ledger.control(fixture.parent).generation;
                let source = &mut ledger
                    .assignments
                    .get_mut(&source.assignment_id)
                    .unwrap()
                    .task;
                source.status = ChildSessionTaskStatus::Completed;
                source.subscription = ChildSessionSubscription::AutoWake;
                source.notify = ChildSessionNotify::OnTaskTerminal;
                source.result = Some(ChildSessionResult {
                    revision: 1,
                    final_task_id: Some(task_id),
                    completed_at: now(),
                    model: None,
                    summary: "Completed fixture result".into(),
                    report: None,
                    report_complete: false,
                    notification_id: notice,
                    consumed_at: None,
                    superseded_by: None,
                    workspace: None,
                    worktree: None,
                });
                ledger.wakes.insert(
                    batch,
                    serde_json::from_value(serde_json::json!({
                        "id": batch, "parent": fixture.parent,
                        "generation": generation,
                        "request_id": wake.request_id, "members": [notice],
                        "turn_id": wake.id, "started": false, "invalidated": false,
                    }))?,
                );
                Ok(())
            })
            .unwrap();
        assert_eq!(fixture.claim(fixture.parent).unwrap().metadata.id, wake.id);
        assert_eq!(fixture.turn(wake.id).status, RuntimeTurnStatus::Queued);
        store
            .children
            .transaction(|ledger| {
                let batch = ledger.wakes.get(&batch).unwrap();
                assert!(batch.started && !batch.invalidated);
                assert_eq!(ledger.slots.len(), CHILD_SESSION_RUNNING_LIMIT);
                assert!(!ledger.slots.contains_key(&wake.id));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn an_invalid_child_capability_does_not_hold_other_queue_heads() {
        let fixture = Fixture::new();
        let older = fixture.start();
        let newer = fixture.start();
        let store = &fixture.state.sessions;
        {
            let mut turns = store.turns_lock().unwrap();
            turns
                .get_mut(&older.child_turn_id.unwrap())
                .unwrap()
                .team_control = Some(
                willdeep_runtime_protocol::agent_team::AgentTeamControl::Continue {
                    id: uuid::Uuid::new_v4(),
                    message: "Invalid child lane".into(),
                    request_id: uuid::Uuid::new_v4(),
                },
            );
            persist_turns(&store.turns_path, &turns).unwrap();
        }
        assert!(fixture.claim(newer.child_session_id).is_some());
        assert!(store.claim_next(older.child_session_id).is_err());
        assert_eq!(fixture.turn(older.child_turn_id.unwrap()).attempts, 0);
    }
}
