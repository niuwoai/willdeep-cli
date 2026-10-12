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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRun {
    pub id: Uuid,
    pub request_id: Uuid,
    pub agent_id: Uuid,
    pub parent_session_id: Uuid,
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
