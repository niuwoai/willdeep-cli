use super::*;
use willdeep_runtime_protocol::{AgentRun, AgentStatus};

fn run_error(error: impl ToString) -> AgentError {
    AgentError::Checkpoint(error.to_string())
}

#[cfg(test)]
mod tests;

impl SubagentCatalog {
    /// Append a new task to the same durable worker conversation and permissions.
    pub async fn continue_background_agent(
        &self,
        agent_id: uuid::Uuid,
        message: &str,
        request_id: uuid::Uuid,
    ) -> Result<AgentRun, AgentError> {
        let message = message.trim();
        if message.is_empty() || message.len() > 16 * 1024 {
            return Err(AgentError::Subagent(
                "continuation message must contain 1 to 16384 bytes".into(),
            ));
        }
        let store = self
            .background
            .agent_run_store()
            .ok_or_else(|| run_error("Agent run store is unavailable"))?;
        if let Some(existing) = store.load(request_id).map_err(run_error)? {
            let dispatch: super::super::dispatch_store::DispatchRecord =
                serde_json::from_value(existing.dispatch).map_err(run_error)?;
            if existing.record.agent_id != agent_id
                || existing.record.reason != AgentRunReason::Continue
                || dispatch.args.prompt != message
            {
                return Err(AgentError::Subagent(
                    "continuation request ID was reused for a different task".into(),
                ));
            }
            if existing.record.status == AgentStatus::Queued
                && self
                    .background
                    .task_for_agent(agent_id)
                    .is_none_or(|task| task.run_id != Some(request_id))
            {
                self.validate_run_owner(&dispatch, agent_id)?;
                if let Some(model) = &dispatch.model {
                    self.model_overrides
                        .lock()
                        .expect("subagent model overrides")
                        .insert(agent_id, model.clone());
                }
                if let Err(error) = self
                    .run_with_id(
                        agent_id,
                        dispatch.args,
                        dispatch.approved_targets,
                        dispatch.approved_command,
                        Some(dispatch.prepared),
                        Some((request_id, AgentRunReason::Continue)),
                    )
                    .await
                {
                    store
                        .finish(request_id, AgentStatus::Blocked, &error.to_string())
                        .map_err(run_error)?;
                }
            }
            return Ok(existing.record);
        }
        if self
            .background
            .agent_status(agent_id)
            .is_some_and(|status| status.is_pending())
        {
            return Err(AgentError::Subagent(
                "this Agent already has an unfinished run".into(),
            ));
        }
        let latest = store.latest(agent_id).map_err(run_error)?;
        let mut dispatch = if let Some(latest) = &latest {
            if latest.record.is_pending() {
                return Err(AgentError::Subagent(
                    "this Agent already has an unfinished durable run".into(),
                ));
            }
            serde_json::from_value(latest.dispatch.clone()).map_err(run_error)?
        } else {
            let home = self
                .state_home
                .as_ref()
                .ok_or_else(|| run_error("worker state home is unavailable"))?;
            super::super::dispatch_store::load(home, agent_id)?
                .ok_or_else(|| run_error("original Agent dispatch was not found"))?
        };
        self.validate_run_owner(&dispatch, agent_id)?;
        // A never-started worker may still materialize its planned checkout. A used
        // checkout must remain present; continuing cannot recreate lost/merged code.
        let never_started = latest
            .as_ref()
            .is_some_and(|entry| entry.record.started_at_ms.is_none());
        if dispatch.prepared.workspace.exists() || !never_started {
            super::super::dispatch_store::validate_workspace(&dispatch.prepared, &self.workspace)
                .await?;
        }
        dispatch.args.prompt = message.to_owned();
        if let Some(task) = &mut dispatch.args.task {
            task.goal = message.to_owned();
        }
        if let Some(model) = &dispatch.model {
            self.model_overrides
                .lock()
                .expect("subagent model overrides")
                .insert(agent_id, model.clone());
        }
        self.run_with_id(
            agent_id,
            dispatch.args,
            dispatch.approved_targets,
            dispatch.approved_command,
            Some(dispatch.prepared),
            Some((request_id, AgentRunReason::Continue)),
        )
        .await?;
        store
            .get(request_id)
            .map_err(run_error)?
            .ok_or_else(|| run_error("accepted Agent run was not persisted"))
    }

    /// Called once after the Harness owns its parent session. Read-only clients
    /// never invoke this: running tools become Interrupted, only Queued resumes.
    pub async fn restore_queued_runs(&self) -> Result<usize, AgentError> {
        let Some(store) = self.background.agent_run_store() else {
            return Ok(0);
        };
        // 空团队没有恢复工作。界面启动也会构建 Catalog，不能提前占住
        // Runtime 的执行锁；实际派工仍由 BackgroundTaskRegistry 原子取锁。
        if !store.needs_recovery().map_err(run_error)? {
            return Ok(0);
        }
        self.background.claim_agent_team().map_err(run_error)?;
        if self.background.running_subagents() > 0 {
            return Err(run_error("cannot recover a team with live workers"));
        }
        store.interrupt_running().map_err(run_error)?;
        let mut restored = 0;
        for entry in store.pending().map_err(run_error)? {
            let result = async {
                let dispatch: super::super::dispatch_store::DispatchRecord =
                    serde_json::from_value(entry.dispatch).map_err(run_error)?;
                self.validate_run_owner(&dispatch, entry.record.agent_id)?;
                if let Some(model) = dispatch.model {
                    self.model_overrides
                        .lock()
                        .expect("subagent model overrides")
                        .insert(entry.record.agent_id, model);
                }
                self.run_with_id(
                    entry.record.agent_id,
                    dispatch.args,
                    dispatch.approved_targets,
                    dispatch.approved_command,
                    Some(dispatch.prepared),
                    Some((entry.record.id, entry.record.reason)),
                )
                .await
            }
            .await;
            match result {
                Ok(_) => restored += 1,
                Err(error) => {
                    store
                        .finish(entry.record.id, AgentStatus::Blocked, &error.to_string())
                        .map_err(run_error)?;
                    self.sink
                        .emit(AgentEvent::RunScoped {
                            run_id: entry.record.id,
                            sequence: entry.record.sequence,
                            event: Box::new(AgentEvent::SubagentCompleted {
                                id: entry.record.agent_id,
                                status: SubagentLifecycleStatus::Blocked,
                                report: Some(error.to_string()),
                            }),
                        })
                        .await;
                }
            }
        }
        Ok(restored)
    }

    fn validate_run_owner(
        &self,
        dispatch: &super::super::dispatch_store::DispatchRecord,
        agent_id: uuid::Uuid,
    ) -> Result<(), AgentError> {
        if dispatch.id != agent_id
            || self.parent_session.is_none()
            || dispatch.parent_session != self.parent_session
            || dispatch.args.run_in_background != Some(true)
        {
            return Err(AgentError::Subagent(
                "Agent run is not a background child of this parent session".into(),
            ));
        }
        if dispatch
            .args
            .profile
            .as_ref()
            .is_none_or(|profile| !self.profiles.contains_key(profile))
        {
            return Err(AgentError::Subagent(
                "original Agent profile is no longer configured".into(),
            ));
        }
        if dispatch
            .prepared
            .root_workspace
            .canonicalize()
            .map_err(run_error)?
            != self.workspace.canonicalize().map_err(run_error)?
        {
            return Err(AgentError::Subagent(
                "parent workspace changed since the Agent was dispatched".into(),
            ));
        }
        Ok(())
    }
}
