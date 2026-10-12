use super::*;
use willdeep_runtime_protocol::agent_team::{
    AgentContinuationReceipt, AgentRunsParams, AgentTeamControl, ContinueAgentParams,
};

pub(super) fn parent_session(
    state: &ServerState,
    id: uuid::Uuid,
) -> Result<session_store::RuntimeSession, ApiFailure> {
    let agent = state
        .agents
        .get(id)
        .map_err(ApiFailure::internal)?
        .ok_or_else(|| ApiFailure::not_found("Runtime Agent not found"))?;
    let root = agent.parent_id.unwrap_or(agent.id);
    state
        .sessions
        .list()
        .map_err(ApiFailure::internal)?
        .into_iter()
        .find(|session| session.root_agent_id == root)
        .ok_or_else(|| ApiFailure::not_found("parent Runtime Session not found"))
}

pub(super) fn runs(state: &ServerState, request: &ApiRequest) -> ApiResult {
    let params = params::<AgentRunsParams>(request)?;
    let session = parent_session(state, params.id)?;
    let filter = (session.root_agent_id != params.id).then_some(params.id);
    let store = willdeep_core::subagent::AgentRunStore::new(state.home.join("workers"), session.id);
    let mut page = store
        .list(filter, params.after_sequence, params.limit.unwrap_or(32))
        .map_err(|error| ApiFailure::internal(anyhow::anyhow!(error)))?;
    for run in &mut page.runs {
        run.report = run
            .report
            .take()
            .map(|report| willdeep_core::judge::redact_credentials(&report));
    }
    json(page)
}

pub(super) fn team(state: &ServerState, request: &ApiRequest) -> ApiResult {
    let id = params::<IdParams>(request)?.id;
    let session = parent_session(state, id)?;
    let store = willdeep_core::subagent::AgentRunStore::new(state.home.join("workers"), session.id);
    let summary = store
        .team_summary(&willdeep_core::usage_ledger::ledger_dir(&state.home))
        .map_err(|error| ApiFailure::internal(anyhow::anyhow!(error)))?;
    json(summary)
}

pub(super) async fn continue_agent(state: &ServerState, request: &ApiRequest) -> ApiResult {
    let params = params::<ContinueAgentParams>(request)?;
    let message = params.message.trim();
    if message.is_empty() || message.len() > 16 * 1024 {
        return Err(ApiFailure::invalid(
            "continuation message must contain 1 to 16384 bytes",
        ));
    }
    let agent = state
        .agents
        .get(params.id)
        .map_err(ApiFailure::internal)?
        .ok_or_else(|| ApiFailure::not_found("Runtime Agent not found"))?;
    if agent.parent_id.is_none() || !agent.background {
        return Err(ApiFailure::invalid(
            "only a background child Agent can be continued",
        ));
    }
    let session = parent_session(state, params.id)?;
    let runs = willdeep_core::subagent::AgentRunStore::new(state.home.join("workers"), session.id);
    let existing = runs
        .get(params.request_id)
        .map_err(|error| ApiFailure::internal(anyhow::anyhow!(error)))?;
    if existing.is_none()
        && (agent.worktree_merged_at.is_some() || agent.worktree_quarantined_at.is_some())
    {
        return Err(ApiFailure::invalid(
            "Agent worktree was merged or archived; create a new Agent",
        ));
    }
    if existing.is_none()
        && matches!(
            agent.status,
            RuntimeAgentStatus::Queued | RuntimeAgentStatus::Running
        )
    {
        return Err(ApiFailure::invalid(
            "Agent is still running; use agent.prompt to add instructions",
        ));
    }
    let control = AgentTeamControl::Continue {
        id: params.id,
        message: message.into(),
        request_id: params.request_id,
    };
    let settings = crate::config::LoadedConfig::load(session.config.as_deref())
        .map_err(ApiFailure::internal)?
        .file
        .agent_team;
    runs.configure_team(settings).map_err(ApiFailure::invalid)?;
    runs.prepare_continuation(params.id, message, params.request_id)
        .map_err(ApiFailure::invalid)?;
    // This durable session Turn is also the fallback if the active-Harness signal
    // is interrupted or lost. Both paths use the same idempotent Run request ID.
    let (turn, created, _) = state
        .sessions
        .enqueue_turn_observed(
            session.id,
            session_store::CreateRuntimeTurn {
                team_control: Some(control),
                request_id: params.request_id,
                prompt: message.into(),
                attachments: Vec::new(),
                origin_client: Some("agent-control".into()),
            },
        )
        .map_err(|error| {
            ApiFailure::invalid(format!("cannot queue Agent continuation: {error}"))
        })?;
    if created {
        if let Some(parent) = state
            .agents
            .get(session.root_agent_id)
            .map_err(ApiFailure::internal)?
            && let Some(task) = state.tasks.get(parent.task_id).await
            && matches!(
                task.status,
                RuntimeTaskStatus::Running
                    | RuntimeTaskStatus::WaitingApproval
                    | RuntimeTaskStatus::WaitingAnswer
            )
        {
            state
                .agent_commands
                .enqueue_continuation(task.id, params.id, message.into(), params.request_id)
                .map_err(ApiFailure::internal)?;
        }
        state
            .tasks
            .schedule_session(session.id)
            .map_err(ApiFailure::internal)?;
    }
    json(AgentContinuationReceipt {
        agent_id: params.id,
        request_id: params.request_id,
        turn_id: turn.id,
        run_id: params.request_id,
    })
}

#[cfg(test)]
mod tests;
