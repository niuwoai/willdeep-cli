use super::*;
use crate::daemon::child_sessions::{Ledger, ensure_queue_capacity, is_host_origin};
use willdeep_runtime_protocol::child_session::{
    ChildSessionContext, ChildSessionLink, ChildSessionTask, ChildSessionWorkspaceMode,
};

impl RuntimeSessionStore {
    /// Origin comes from the persisted host submission, never tool arguments.
    pub(in crate::daemon) fn child_start_requires_confirmation(
        &self,
        session: uuid::Uuid,
        turn: uuid::Uuid,
    ) -> Result<bool> {
        let turns = self.turns_lock()?;
        let source = turns.get(&turn).context("parent Turn not found")?;
        anyhow::ensure!(
            source.metadata.session_id == session,
            "parent Turn belongs to another Session"
        );
        Ok(source
            .origin_client
            .as_deref()
            .is_some_and(|origin| origin == "web" || origin.starts_with("web:")))
    }

    /// Called only under the child control lock. Fresh starts an empty Core;
    /// fork copies only an immutable completed Turn prefix and clears execution
    /// state; worktree binds the child to its prepared dedicated workspace.
    pub(in crate::daemon) fn prepare_child(
        &self,
        source: &RuntimeSession,
        task: &ChildSessionTask,
        link: &ChildSessionLink,
        root: uuid::Uuid,
    ) -> Result<RuntimeSession> {
        let read_only = link.workspace_mode == ChildSessionWorkspaceMode::SharedReadOnly;
        let workspace = PathBuf::from(&link.workspace);
        if let Some(existing) = self.get(task.child_session_id)? {
            anyhow::ensure!(
                existing.parent_session_id == Some(source.id)
                    && existing.delegation_read_only == read_only,
                "prepared Session identity conflicts with another Session"
            );
            return Ok(existing);
        }
        let id = task.child_session_id;
        let mut core = match self.core.load(id) {
            Ok(core) => {
                anyhow::ensure!(
                    core.parent_session_id == Some(source.id)
                        && core.delegation_read_only == read_only,
                    "prepared Core identity conflicts with another Session"
                );
                core
            }
            Err(error) if !self.core_path_exists(id) => {
                let mut core = match task.context {
                    ChildSessionContext::Fresh => {
                        willdeep_core::Session::new(workspace.clone(), source.profile.clone(), "")
                    }
                    ChildSessionContext::Fork => {
                        self.fork_child_core(source, task, link, workspace.clone())?
                    }
                };
                core.id = id;
                core.title = normalized_title(task.title.clone())?;
                core.title_source = willdeep_core::TitleSource::User;
                core.parent_session_id = Some(source.id);
                core.delegation_read_only = read_only;
                core.model_context_window = link.model_context_window;
                core.runtime_managed = true;
                core.started_by = None;
                core.profile = source.profile.clone();
                core.magpie_integration = source.magpie_integration;
                core.model = task.model.clone().or_else(|| source.model.clone());
                core.config = source.config.clone();
                // Keep failure context without hiding a corrupt existing Core file.
                let _ = error;
                core
            }
            Err(error) => return Err(error).context("load prepared child Core"),
        };
        self.core.save(&mut core)?;
        let session = RuntimeSession {
            schema: RUNTIME_SESSION_SCHEMA,
            id,
            root_agent_id: root,
            started_by: None,
            parent_session_id: Some(source.id),
            delegation_read_only: read_only,
            delegation_detached: false,
            model_context_window: link.model_context_window,
            human_wait_yield_suppressed: false,
            workspace,
            profile: core.profile,
            model: core.model,
            reasoning_level: source.reasoning_level.clone(),
            config: core.config,
            magpie_integration: source.magpie_integration,
            approval_mode: source.approval_mode,
            mobile_skills: source.mobile_skills.clone(),
            mobile_plugins: source.mobile_plugins.clone(),
            status: RuntimeSessionStatus::Idle,
            active_turn_id: None,
            created_at: task.created_at,
            updated_at: now(),
            last_error: None,
        };
        let mut sessions = self.lock()?;
        let mut updated = sessions.clone();
        updated.insert(id, session.clone());
        persist_sessions(&self.path, &updated)?;
        *sessions = updated;
        Ok(session)
    }

    /// Copy only messages before a completed Turn boundary. The parent may be
    /// running a later Turn: a completed prefix is immutable and the generation
    /// check rejects a concurrent compression instead of splicing two histories.
    fn fork_child_core(
        &self,
        source: &RuntimeSession,
        task: &ChildSessionTask,
        link: &ChildSessionLink,
        workspace: PathBuf,
    ) -> Result<willdeep_core::Session> {
        let through = task
            .through_turn_id
            .context("fork child requires a completed through_turn_id")?;
        let boundary = self
            .get_turn(through)?
            .context("fork boundary Turn not found")?;
        anyhow::ensure!(
            boundary.session_id == source.id,
            "fork boundary Turn belongs to another Session"
        );
        anyhow::ensure!(
            boundary.status == RuntimeTurnStatus::Completed,
            "only a completed Turn can be a fork boundary"
        );
        let end = boundary
            .message_end
            .context("fork boundary Turn has no durable message boundary")?;
        let core = self
            .core
            .load(source.id)
            .context("load parent Core for fork")?;
        anyhow::ensure!(
            boundary.message_generation == core.compression_generation,
            "fork boundary predates the current compression checkpoint; retry with a newer completed Turn"
        );
        anyhow::ensure!(
            end <= core.messages.len(),
            "fork boundary exceeds the durable Core history"
        );
        anyhow::ensure!(
            link.context_source.as_ref().is_some_and(|source| {
                source.through_turn_id == through
                    && source.message_end == end
                    && source.compression_generation == boundary.message_generation
            }),
            "fork relation does not match the requested durable boundary"
        );
        ensure_closed_tool_history(&core.messages[..end])?;
        let mut forked = core.fork_snapshot();
        forked.messages = core.messages[..end].to_vec();
        forked.workspace = workspace;
        // Execution state belongs to the parent's Turns and must never travel
        // into the child, even when the message prefix itself is immutable.
        forked.current_plan = None;
        forked.goal = None;
        forked.goal_state = None;
        forked.compression_checkpoint = None;
        forked.execution_checkpoint = None;
        forked.attention_read.clear();
        forked.agent_reports_received.clear();
        forked.agent_control_requests.clear();
        forked.runtime_event_cursor = 0;
        forked.pinned_at = None;
        forked.manual_compression_usage = Default::default();
        if forked
            .context_summary
            .as_ref()
            .is_some_and(|summary| summary.through > end)
        {
            forked.context_summary = None;
        }
        Ok(forked)
    }

    fn core_path_exists(&self, id: uuid::Uuid) -> bool {
        // SessionStore::load returning a parse/permission error must not authorize overwrite.
        self.children
            .home
            .join("sessions")
            .join(format!("{id}.json"))
            .exists()
    }

    /// Called under ChildStore before sessions/turns locks. This changes only
    /// the tree projection; immutable parent and capability limits survive.
    pub(in crate::daemon) fn detach_child_projection(&self, id: uuid::Uuid) -> Result<()> {
        let Some(session) = self.get(id)? else {
            return Ok(());
        };
        if !session.delegation_detached {
            self.core
                .update(id, |core| core.delegation_detached = true)
                .context("detach child Core tree projection")?;
            let mut sessions = self.lock()?;
            let mut updated = sessions.clone();
            if let Some(child) = updated.get_mut(&id) {
                child.delegation_detached = true;
                child.updated_at = now();
            }
            persist_sessions(&self.path, &updated)?;
            *sessions = updated;
        }
        Ok(())
    }

    /// Bind a reclaimed child to the worktree path resolved at execution time.
    pub(in crate::daemon) fn set_child_workspace(
        &self,
        id: uuid::Uuid,
        workspace: PathBuf,
    ) -> Result<()> {
        self.core
            .update(id, |core| core.workspace = workspace.clone())
            .context("update child Core workspace")?;
        let mut sessions = self.lock()?;
        let mut updated = sessions.clone();
        let session = updated
            .get_mut(&id)
            .context("child Session disappeared while binding its worktree")?;
        session.workspace = workspace;
        session.updated_at = now();
        persist_sessions(&self.path, &updated)?;
        *sessions = updated;
        Ok(())
    }

    pub(in crate::daemon) fn check_child_enqueue(
        &self,
        session: &RuntimeSession,
        request: &CreateRuntimeTurn,
        ledger: &Ledger,
    ) -> Result<()> {
        if let Some(parent) = session.parent_session_id {
            anyhow::ensure!(
                request.team_control.is_none(),
                "delegated child Session cannot dispatch independent execution lanes"
            );
            let duplicate = self
                .list_turns(session.id)?
                .iter()
                .any(|turn| turn.request_id == request.request_id);
            if !duplicate && !is_host_origin(request.origin_client.as_deref()) {
                ensure_queue_capacity(self, ledger, parent)?;
            }
        }
        Ok(())
    }

    /// Read-only eligibility shared by fair admission and the actual claim.
    /// A stale wake must not hold up unrelated child queue heads.
    pub(super) fn child_wake_is_eligible(
        session: &RuntimeSession,
        turn: &StoredRuntimeTurn,
        ledger: &Ledger,
    ) -> bool {
        let Some(batch) = ledger.wakes.values().find(|batch| {
            batch.turn_id == Some(turn.metadata.id)
                || (batch.parent == session.id && batch.request_id == turn.metadata.request_id)
        }) else {
            return !turn.origin_client.as_deref().is_some_and(|origin| {
                origin.starts_with(super::super::child_sessions::WAKE_ORIGIN)
            });
        };
        let control = ledger.control(session.id);
        if batch.invalidated || control.suppressed || batch.generation != control.generation {
            return false;
        }
        let pending = ledger
            .pending(session.id)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<HashSet<_>>();
        batch.members.iter().any(|id| pending.contains(id))
    }

    pub(super) fn check_child_claim(
        &self,
        session: &RuntimeSession,
        turn: &StoredRuntimeTurn,
        ledger: &mut Ledger,
    ) -> Result<bool> {
        let eligible = Self::child_wake_is_eligible(session, turn, ledger);
        if let Some(id) = ledger
            .wakes
            .values()
            .find(|batch| {
                batch.turn_id == Some(turn.metadata.id)
                    || (batch.parent == session.id && batch.request_id == turn.metadata.request_id)
            })
            .map(|batch| batch.id)
        {
            let batch = ledger.wakes.get_mut(&id).unwrap();
            batch.turn_id = Some(turn.metadata.id);
            if !eligible {
                batch.invalidated = true;
                return Ok(false);
            }
            batch.started = true;
        } else if !eligible {
            return Ok(false);
        }
        if let Some(parent) = session.parent_session_id {
            anyhow::ensure!(
                turn.team_control.is_none(),
                "child Session capability boundary is invalid"
            );
            return Ok(ledger.reserve_slot(turn.metadata.id, parent));
        }
        Ok(true)
    }

    pub(in crate::daemon) fn turn_origin(&self, turn_id: uuid::Uuid) -> Result<Option<String>> {
        Ok(self
            .turns_lock()?
            .get(&turn_id)
            .and_then(|turn| turn.origin_client.clone()))
    }
}

/// A fork boundary may only split a history whose tool calls are all closed.
/// An unfinished call would ask the child's model to continue another Harness's
/// in-flight work; a dangling result cannot be replayed safely either.
fn ensure_closed_tool_history(messages: &[willdeep_core::Message]) -> Result<()> {
    let mut pending = HashSet::new();
    for message in messages {
        if message.role == willdeep_core::Role::Assistant {
            for call in &message.tool_calls {
                anyhow::ensure!(
                    pending.insert(call.id.clone()),
                    "fork boundary repeats a tool call identity"
                );
            }
        } else if message.role == willdeep_core::Role::Tool {
            let call = message
                .tool_call_id
                .as_ref()
                .context("fork boundary contains a tool result without a call identity")?;
            anyhow::ensure!(
                pending.remove(call),
                "fork boundary contains a tool result without an open call"
            );
        }
    }
    anyhow::ensure!(
        pending.is_empty(),
        "fork boundary contains unfinished tool calls"
    );
    Ok(())
}
