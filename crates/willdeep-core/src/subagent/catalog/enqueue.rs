//! Stable, owned dispatch using the parent's existing queue and Scheduler.
use super::super::types::WorkflowDispatchIdentity;
use super::*;
use willdeep_runtime_protocol::agent_team::WorkflowRunOwner;
use willdeep_runtime_protocol::{AgentRun, AgentStatus};

pub(super) fn dispatch_intent(
    args: &SpawnAgentArgs,
    targets: &Option<BTreeSet<PathBuf>>,
    command: &Option<String>,
) -> Result<serde_json::Value, AgentError> {
    serde_json::to_value((args, targets, command))
        .map_err(|error| AgentError::Checkpoint(error.to_string()))
}

impl SubagentCatalog {
    /// Instantaneous shared-team headroom. Final admission is still atomic and can return full.
    pub fn available_dispatch_capacity(&self) -> Result<usize, AgentError> {
        let live = self
            .background
            .available_agent_capacity()
            .map_err(AgentError::Subagent)?;
        let durable = self
            .background
            .agent_run_store()
            .map(|store| store.available_dispatch_capacity())
            .transpose()
            .map_err(AgentError::Checkpoint)?;
        Ok(durable.map_or(live, |durable| durable.min(live)))
    }

    /// Trusted host entry after the parent's normal safety/admission path.
    /// request_id is also run_id, as for all existing AgentRunStore records.
    pub async fn run_authorized_with_identity(
        &self,
        mut args: SpawnAgentArgs,
        approved_targets: Option<BTreeSet<PathBuf>>,
        approved_command: Option<String>,
        identity: WorkflowDispatchIdentity,
    ) -> Result<AgentRun, AgentError> {
        if identity.node_instance_id.trim().is_empty() || identity.node_instance_id.len() > 256 {
            return Err(AgentError::Subagent(
                "workflow node instance ID must contain 1 to 256 bytes".into(),
            ));
        }
        args.workflow_owner = Some(WorkflowRunOwner {
            workflow_run_id: identity.workflow_run_id,
            node_instance_id: identity.node_instance_id,
        });
        self.enqueue_owned(
            identity.request_id,
            identity.agent_id,
            args,
            approved_targets,
            approved_command,
        )
        .await
    }

    /// Enqueue on this catalog, preserving the shared registry, run store and scheduler.
    /// The caller must already have completed the parent Agent's permission/tier admission.
    pub async fn enqueue_owned(
        &self,
        request_id: uuid::Uuid,
        agent_id: uuid::Uuid,
        mut args: SpawnAgentArgs,
        approved_targets: Option<BTreeSet<PathBuf>>,
        approved_command: Option<String>,
    ) -> Result<AgentRun, AgentError> {
        args.run_in_background = Some(true);
        let store = self.background.agent_run_store().ok_or_else(|| {
            AgentError::Checkpoint("stable dispatch requires the parent's Agent run store".into())
        })?;
        let intent = dispatch_intent(&args, &approved_targets, &approved_command)?;
        // Serializes only dispatch acceptance, never a worker's execution. Clones share it.
        let _admission = self.dispatch_admission.lock().await;
        if let Some(saved) = store.load(request_id).map_err(AgentError::Checkpoint)? {
            let dispatch: super::super::dispatch_store::DispatchRecord =
                serde_json::from_value(saved.dispatch)
                    .map_err(|e| AgentError::Checkpoint(e.to_string()))?;
            if saved.record.agent_id != agent_id
                || saved.record.reason != AgentRunReason::Spawn
                || dispatch.parent_session != self.parent_session
                || dispatch.workflow_owner != args.workflow_owner
                || dispatch.intent.as_ref() != Some(&intent)
            {
                return Err(AgentError::Subagent(
                    "Agent request ID was already used for a different dispatch intent".into(),
                ));
            }
            if saved.record.status == AgentStatus::Queued
                && self
                    .background
                    .task_for_agent(agent_id)
                    .is_none_or(|task| task.run_id != Some(request_id))
            {
                self.run_with_id(
                    agent_id,
                    dispatch.args.clone(),
                    dispatch.approved_targets.clone(),
                    dispatch.approved_command.clone(),
                    Some(dispatch),
                    Some((request_id, AgentRunReason::Spawn)),
                )
                .await?;
            }
            return store
                .get(request_id)
                .map_err(AgentError::Checkpoint)?
                .ok_or_else(|| AgentError::Checkpoint("accepted Agent run disappeared".into()));
        }
        if self.available_dispatch_capacity()? == 0 {
            return Err(AgentError::Subagent(
                "the Agent queue is full; wait for a worker to finish".into(),
            ));
        }
        // A failed final reservation may already have saved a write-once dispatch.
        // Retry that same intent from its snapshot, even if the catalog was refreshed.
        let prepared_dispatch = self
            .state_home
            .as_ref()
            .map(|home| super::super::dispatch_store::load(home, agent_id))
            .transpose()?
            .flatten();
        if let Some(dispatch) = &prepared_dispatch
            && (dispatch.parent_session != self.parent_session
                || dispatch.workflow_owner != args.workflow_owner
                || dispatch.intent.as_ref() != Some(&intent))
        {
            return Err(AgentError::Subagent(
                "Agent identity was already used for a different dispatch intent".into(),
            ));
        }
        self.run_with_id(
            agent_id,
            args,
            approved_targets,
            approved_command,
            prepared_dispatch,
            Some((request_id, AgentRunReason::Spawn)),
        )
        .await?;
        store
            .get(request_id)
            .map_err(AgentError::Checkpoint)?
            .ok_or_else(|| AgentError::Checkpoint("accepted Agent run was not persisted".into()))
    }
}
