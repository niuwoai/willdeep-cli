use super::super::dispatch_store::DispatchRecord;
use super::*;

impl AgentRunStore {
    /// Admission and durable outbox creation share the same lock as all spawns.
    /// A Runtime crash between this record and the session Turn is recovered by
    /// the team's queued-run scan, using exactly the same request identity.
    pub fn prepare_continuation(
        &self,
        agent_id: Uuid,
        message: &str,
        request_id: Uuid,
    ) -> Result<AgentRun, String> {
        let message = message.trim();
        if message.is_empty() || message.len() > 16 * 1024 {
            return Err("continuation message must contain 1 to 16384 bytes".into());
        }
        self.locked(|| {
            if let Some(existing) = self.load(request_id)? {
                let dispatch: DispatchRecord =
                    serde_json::from_value(existing.dispatch).map_err(storage_error)?;
                if existing.record.agent_id != agent_id
                    || existing.record.reason != AgentRunReason::Continue
                    || dispatch.args.prompt != message
                {
                    return Err("continuation request ID was reused for a different task".into());
                }
                return Ok(existing.record);
            }
            let latest = self.latest(agent_id)?;
            let mut dispatch: DispatchRecord = match &latest {
                Some(entry) => {
                    serde_json::from_value(entry.dispatch.clone()).map_err(storage_error)?
                }
                None => {
                    let home = self
                        .directory
                        .parent()
                        .and_then(Path::parent)
                        .ok_or("worker state home is unavailable")?;
                    super::super::dispatch_store::load(home, agent_id)
                        .map_err(storage_error)?
                        .ok_or("original Agent dispatch was not found")?
                }
            };
            if dispatch.id != agent_id
                || dispatch.parent_session != Some(self.parent)
                || dispatch.args.run_in_background != Some(true)
            {
                return Err("Agent is not a background child of this parent session".into());
            }
            dispatch.args.prompt = message.into();
            if let Some(task) = &mut dispatch.args.task {
                task.goal = message.into();
            }
            let previous = latest.map(|entry| entry.record);
            self.create_locked(RunRequest {
                request_id,
                agent_id,
                reason: AgentRunReason::Continue,
                label: previous.as_ref().map_or_else(
                    || dispatch.args.label.clone().unwrap_or_default(),
                    |run| run.label.clone(),
                ),
                model: dispatch.model.clone(),
                max_turns: previous.as_ref().and_then(|run| run.max_turns),
                token_budget: previous.as_ref().and_then(|run| run.token_budget),
                timeout_seconds: previous.as_ref().and_then(|run| run.timeout_seconds),
                dispatch: serde_json::to_value(dispatch).map_err(storage_error)?,
            })
            .map(|(run, _)| run)
        })
    }
}
