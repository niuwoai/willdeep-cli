//! Startup outbox recovery shares the normal per-session Turn scheduler.
use super::*;
use willdeep_runtime_protocol::agent_team::AgentTeamControl;

pub(super) fn recover(state: &ServerState) -> Result<usize> {
    for agent in state.agents.list()? {
        if (agent.worktree_merged_at.is_some() || agent.worktree_quarantined_at.is_some())
            && let Some(runs) = willdeep_core::subagent::AgentRunStore::from_dispatch(
                &state.home.join("workers"),
                agent.id,
            )
            .map_err(anyhow::Error::msg)?
        {
            runs.retire_agent(agent.id).map_err(anyhow::Error::msg)?;
        }
    }
    let mut queued = 0;
    for session in state.sessions.list()? {
        if session.status == session_store::RuntimeSessionStatus::Archived {
            continue;
        }
        let runs =
            willdeep_core::subagent::AgentRunStore::new(state.home.join("workers"), session.id);
        if !runs.needs_recovery().map_err(anyhow::Error::msg)? {
            continue;
        }
        let turns = state.sessions.list_turns(session.id)?;
        if turns
            .iter()
            .any(|turn| turn.status == session_store::RuntimeTurnStatus::Queued)
        {
            state.tasks.schedule_session(session.id)?;
            continue;
        }
        if session.active_turn_id.is_some() {
            continue;
        }
        let latest_turn = turns
            .iter()
            .max_by_key(|turn| turn.queue_sequence)
            .map(|turn| turn.id);
        let mut after = None;
        let mut identities = String::new();
        loop {
            let page = runs.list(None, after, 64).map_err(anyhow::Error::msg)?;
            for run in page.runs {
                if run.is_pending() || (!run.delivered && run.report.is_some()) {
                    identities.push_str(&format!("{}:{:?}:{};", run.id, run.status, run.delivered));
                }
            }
            after = page.next_after_sequence;
            if after.is_none() {
                break;
            }
        }
        let request_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_OID,
            format!("agent-recovery:{}:{latest_turn:?}:{identities}", session.id).as_bytes(),
        );
        let (turn, created, _) = state.sessions.enqueue_turn_observed(
            session.id,
            session_store::CreateRuntimeTurn {
                team_control: Some(AgentTeamControl::RecoverQueue),
                request_id,
                prompt: String::new(),
                attachments: Vec::new(),
                origin_client: Some("agent-recovery".into()),
            },
        )?;
        if created {
            state.events.append(
                "agent.team_recovery_queued",
                format!("session_id={} turn_id={}", session.id, turn.id),
            )?;
            state.tasks.schedule_session(session.id)?;
            queued += 1;
        }
    }
    Ok(queued)
}

#[cfg(test)]
mod tests;
