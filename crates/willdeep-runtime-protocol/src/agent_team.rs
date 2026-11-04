use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AgentStatus;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunReason {
    Spawn,
    Continue,
    Retry,
    Recover,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentUsageSummary {
    pub uncertain_execution: bool,
    pub model_calls: u64,
    pub unknown_usage_calls: u64,
    pub input_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub accounted_tokens: u64,
}

/// A worker remains in its parent's team and scheduler, with optional workflow attribution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRunOwner {
    pub workflow_run_id: Uuid,
    pub node_instance_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: Uuid,
    pub request_id: Uuid,
    pub agent_id: Uuid,
    pub parent_session_id: Uuid,
    /// Absent for ordinary parent-owned workers and records written before workflows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_owner: Option<WorkflowRunOwner>,
    pub sequence: u64,
    pub reason: AgentRunReason,
    pub status: AgentStatus,
    pub label: String,
    pub model: Option<String>,
    pub queued_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub max_turns: Option<u64>,
    pub token_budget: Option<u64>,
    pub timeout_seconds: Option<u64>,
    pub current_turn: u64,
    pub current_tool: Option<String>,
    #[serde(default)]
    pub usage: AgentUsageSummary,
    pub report: Option<String>,
    #[serde(default)]
    pub delivered: bool,
}

impl AgentRun {
    pub fn is_pending(&self) -> bool {
        matches!(self.status, AgentStatus::Queued | AgentStatus::Running)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunsPage {
    pub runs: Vec<AgentRun>,
    pub next_after_sequence: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContinueAgentParams {
    pub id: Uuid,
    pub message: String,
    pub request_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentTeamControl {
    Continue {
        id: Uuid,
        message: String,
        request_id: Uuid,
    },
    RecoverQueue,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentContinuationReceipt {
    pub agent_id: Uuid,
    pub request_id: Uuid,
    pub turn_id: Uuid,
    pub run_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunsParams {
    pub id: Uuid,
    pub after_sequence: Option<u64>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTeamPolicy {
    pub max_concurrent: usize,
    pub max_queued: usize,
    pub token_budget: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTeamSummary {
    pub parent_session_id: Uuid,
    /// Root Session whose family-wide budget and concurrency cover this team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_session_id: Option<Uuid>,
    pub policy: Option<AgentTeamPolicy>,
    pub running: usize,
    pub queued: usize,
    pub completed_runs: usize,
    pub workers: Vec<AgentWorkerSummary>,
    /// Canonical ledger totals, including parent, child, compression and auxiliary calls.
    pub usage: AgentUsageSummary,
    pub parent_usage: AgentUsageSummary,
    pub child_usage: AgentUsageSummary,
    pub auxiliary_usage: AgentUsageSummary,
    pub compression_usage: AgentUsageSummary,
    /// Deduplicated totals for the root Session and all its child conversations.
    #[serde(default)]
    pub family_usage: AgentUsageSummary,
    pub ledger_incomplete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentWorkerSummary {
    pub agent_id: Uuid,
    pub latest_run: AgentRun,
    pub run_count: usize,
    pub queue_position: Option<usize>,
    pub cumulative_usage: AgentUsageSummary,
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    #[test]
    fn old_runs_default_to_parent_and_workflow_owner_round_trips() {
        let mut json = serde_json::json!({
            "id": Uuid::new_v4(), "request_id": Uuid::new_v4(), "agent_id": Uuid::new_v4(),
            "parent_session_id": Uuid::new_v4(), "sequence": 1, "reason": "spawn",
            "status": "queued", "label": "inspect", "model": null, "queued_at_ms": 1,
            "started_at_ms": null, "finished_at_ms": null, "max_turns": null,
            "token_budget": null, "timeout_seconds": null, "current_turn": 0,
            "current_tool": null, "report": null
        });
        let ordinary: AgentRun = serde_json::from_value(json.clone()).unwrap();
        assert!(ordinary.workflow_owner.is_none());
        assert!(
            serde_json::to_value(ordinary)
                .unwrap()
                .get("workflow_owner")
                .is_none()
        );
        let owner = WorkflowRunOwner {
            workflow_run_id: Uuid::new_v4(),
            node_instance_id: "node:0".into(),
        };
        json["workflow_owner"] = serde_json::to_value(&owner).unwrap();
        let workflow: AgentRun = serde_json::from_value(json).unwrap();
        assert_eq!(workflow.workflow_owner.as_ref(), Some(&owner));
        let reread: AgentRun =
            serde_json::from_value(serde_json::to_value(&workflow).unwrap()).unwrap();
        assert_eq!(reread, workflow);
    }
}
