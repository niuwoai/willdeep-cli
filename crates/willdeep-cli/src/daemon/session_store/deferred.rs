use super::*;

impl RuntimeSessionStore {
    pub(in crate::daemon) fn set_yield_suppressed(
        &self,
        id: uuid::Uuid,
        suppressed: bool,
    ) -> Result<()> {
        let mut sessions = self.lock()?;
        let session = sessions.get_mut(&id).context("Runtime Session not found")?;
        let original = session.human_wait_yield_suppressed;
        if original == suppressed {
            return Ok(());
        }
        session.human_wait_yield_suppressed = suppressed;
        if let Err(error) = persist_sessions(&self.path, &sessions) {
            sessions.get_mut(&id).unwrap().human_wait_yield_suppressed = original;
            return Err(error);
        }
        Ok(())
    }
    pub(in crate::daemon) fn independent_history(
        home: &Path,
        session_id: uuid::Uuid,
    ) -> Result<Vec<IndependentHistory>> {
        let path = super::super::DaemonPaths::new(home)
            .runtime_sessions
            .with_file_name("turns.json");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let turns = load_turns(&path)?;
        let store = willdeep_core::SessionStore::new(home.join("independent"));
        let mut selected = turns
            .values()
            .filter(|turn| {
                turn.metadata.session_id == session_id
                    && turn.metadata.execution_session_id.is_some()
            })
            .collect::<Vec<_>>();
        selected.sort_by_key(|turn| turn.metadata.queue_sequence);
        let mut result = Vec::new();
        for turn in selected {
            let core = store.load(turn.metadata.execution_session_id.unwrap())?;
            let start = turn.metadata.message_start.unwrap_or(0);
            let mut messages = core
                .messages
                .get(start..)
                .context("Independent history boundary changed")?
                .to_vec();
            if messages.is_empty() {
                messages.push(willdeep_core::Message::user_with_attachments(
                    turn_queue::editable_text(&turn.prompt).unwrap_or(&turn.prompt),
                    turn.attachments.clone(),
                ));
            }
            result.push(IndependentHistory {
                id: turn.metadata.id,
                status: turn.metadata.status,
                display_after: turn.metadata.display_after_message.unwrap_or(0),
                display_generation: turn.metadata.display_generation.unwrap_or(0),
                messages,
            });
        }
        Ok(result)
    }
    pub(in crate::daemon) fn has_running_lane(&self, id: uuid::Uuid) -> Result<bool> {
        Ok(self.turns_lock()?.values().any(|turn| {
            turn.metadata.session_id == id
                && (turn.metadata.status == RuntimeTurnStatus::Running
                    || (turn.metadata.execution_session_id.is_some()
                        && turn.metadata.status == RuntimeTurnStatus::Queued))
        }))
    }

    pub(in crate::daemon) fn claim_independent(
        &self,
        session_id: uuid::Uuid,
        id: uuid::Uuid,
        expected_text: &str,
    ) -> Result<Option<ClaimedRuntimeTurn>> {
        let mut sessions = self.lock()?;
        let session = sessions
            .get_mut(&session_id)
            .context("Runtime Session not found")?;
        if session.human_wait_yield_suppressed
            || session.active_turn_id.is_none()
            || !matches!(
                session.status,
                RuntimeSessionStatus::WaitingApproval | RuntimeSessionStatus::WaitingAnswer
            )
        {
            return Ok(None);
        }
        let mut turns = self.turns_lock()?;
        if turns.values().any(|turn| {
            turn.metadata.session_id == session_id
                && (turn.metadata.status == RuntimeTurnStatus::Running
                    || (turn.metadata.execution_session_id.is_some()
                        && turn.metadata.status == RuntimeTurnStatus::Queued))
        }) {
            return Ok(None);
        }
        let turn = turns.get_mut(&id).context("Queued message not found")?;
        if turn.metadata.session_id != session_id
            || turn.metadata.status != RuntimeTurnStatus::Queued
            || turn.metadata.execution_session_id.is_some()
            || turn_queue::editable_text(&turn.prompt) != Some(expected_text)
        {
            return Ok(None);
        }
        let original = turn.clone();
        let original_session = session.clone();
        let parent = self.core.load(session_id)?;
        let mut lane = parent.fork_snapshot();
        let display_after = parent.messages.len();
        lane.execution_checkpoint = None;
        lane.goal = None;
        lane.goal_state = None;
        lane.current_plan = None;
        lane.runtime_event_cursor = 0;
        willdeep_core::types::sanitize_tool_history(&mut lane.messages);
        lane.messages.push(willdeep_core::Message::host_instruction("The original task is suspended on unresolved human input. This execution lane handles ONLY the following independent user message. Do not continue the original task, infer an answer, approve its tool calls, or modify its pending targets. Tool permissions remain unchanged. If this request turns out to depend on the suspended work, explain the dependency and stop."));
        self.independent_core.save(&mut lane)?;
        turn.metadata.execution_session_id = Some(lane.id);
        turn.metadata.display_after_message = Some(display_after);
        turn.metadata.display_generation = Some(parent.compression_generation);
        turn.metadata.message_start = Some(lane.messages.len());
        turn.metadata.message_generation = lane.compression_generation;
        turn.metadata.attempts = turn.metadata.attempts.saturating_add(1);
        let claimed = ClaimedRuntimeTurn {
            metadata: turn.metadata.clone(),
            request: SubmitTask {
                execution_session_id: Some(lane.id),
                team_control: None,
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
                session_id: Some(session_id),
                turn_id: Some(id),
                origin_client: turn.origin_client.clone(),
                replay_existing_user_message: false,
            },
        };
        session.active_turn_id = Some(id);
        session.status = RuntimeSessionStatus::Queued;
        if let Err(error) = persist_turns(&self.turns_path, &turns) {
            turns.insert(id, original);
            sessions.insert(session_id, original_session);
            let _ = self.independent_core.delete(lane.id);
            return Err(error);
        }
        if let Err(error) = persist_sessions(&self.path, &sessions) {
            turns.insert(id, original);
            sessions.insert(session_id, original_session);
            persist_turns(&self.turns_path, &turns).context("rollback independent claim")?;
            let _ = self.independent_core.delete(lane.id);
            return Err(error);
        }
        Ok(Some(claimed))
    }
}
