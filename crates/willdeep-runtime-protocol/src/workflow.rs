//! Versioned, host-independent declarative workflow protocol.
//!
//! Templates are JSON values; an object containing exactly `{"$ref":"input#/x"}`
//! reads a JSON Pointer. Other roots are `item`, `previous`, and `node:<id>`.
//! A scope sees only declared transitive dependencies and inherited upstreams.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{AgentRun, AgentUsageSummary};

pub const WORKFLOW_SPEC_VERSION: u32 = 1;
/// Absolute host-independent ceilings. Individual runs may only lower them.
pub const WORKFLOW_MAX_DEFINITION_BYTES: usize = 1024 * 1024;
pub const WORKFLOW_MAX_RUN_BYTES: usize = 32 * 1024 * 1024;
pub const WORKFLOW_MAX_NODES: usize = 256;
pub const WORKFLOW_MAX_DEPTH: usize = 16;
pub const WORKFLOW_MAX_INSTANCES: usize = 4096;
pub const WORKFLOW_MAX_ATTEMPTS: usize = 4096;
pub const WORKFLOW_MAX_FOREACH_ITEMS: usize = 256;
pub const WORKFLOW_MAX_ITERATIONS: u32 = 128;
pub const WORKFLOW_MAX_CONCURRENT: usize = 16;
pub const WORKFLOW_MAX_VALUE_BYTES: usize = 1024 * 1024;
pub const WORKFLOW_MAX_REPORT_BYTES: usize = 64 * 1024;
pub const WORKFLOW_MAX_NAME_BYTES: usize = 128;
pub const WORKFLOW_MAX_JSON_DEPTH: usize = 64;
pub const WORKFLOW_MAX_EXPRESSION_DEPTH: usize = 32;
pub const WORKFLOW_MAX_WRITE_TARGET_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSpecV1 {
    pub version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub input_schema: Option<Value>,
    #[serde(default)]
    pub phases: Vec<WorkflowPhase>,
    pub nodes: Vec<WorkflowNode>,
    #[serde(default)]
    pub output: Value,
}

/// Display grouping only. Phases never imply a scheduling barrier.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPhase {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowScope {
    pub nodes: Vec<WorkflowNode>,
    #[serde(default)]
    pub output: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(flatten)]
    pub kind: WorkflowNodeKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowNodeKind {
    Agent(WorkflowAgentSpec),
    Switch {
        condition: WorkflowCondition,
        then_scope: WorkflowScope,
        else_scope: WorkflowScope,
    },
    Foreach {
        items: Value,
        body: WorkflowScope,
    },
    /// `previous` starts as `initial`. After each body completes, evaluate
    /// `until` and `carry` against its outputs and the old `previous`. The new
    /// carry is the repeat output when until is true, or the next previous.
    Repeat {
        initial: Value,
        body: WorkflowScope,
        until: WorkflowCondition,
        carry: Value,
        max_iterations: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowAgentSpec {
    pub profile: String,
    pub prompt: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub output_schema: Option<Value>,
    #[serde(default)]
    pub worker_tier: Option<WorkflowWorkerTier>,
    #[serde(default)]
    pub max_turns: Option<u64>,
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default)]
    pub timeout_seconds: Option<u64>,
    #[serde(default)]
    pub write_targets: Vec<String>,
    /// Passed unchanged to the host's existing TaskPacket decoder and policy.
    #[serde(default)]
    pub task_packet: Option<Value>,
    /// The host validates expert escalation using its normal routing policy.
    #[serde(default)]
    pub escalation: Option<Value>,
    #[serde(default)]
    pub target_command: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowWorkerTier {
    Standard,
    Advanced,
    Expert,
}

/// Finite, typed operations only; no expressions or executable scripts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowCondition {
    Eq { left: Value, right: Value },
    Ne { left: Value, right: Value },
    Lt { left: Value, right: Value },
    Le { left: Value, right: Value },
    Gt { left: Value, right: Value },
    Ge { left: Value, right: Value },
    And { conditions: Vec<WorkflowCondition> },
    Or { conditions: Vec<WorkflowCondition> },
    Not { condition: Box<WorkflowCondition> },
}

/// Every bound is a hard error, never a truncation. A caller may lower defaults
/// but cannot raise the engine's absolute safety ceilings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkflowLimits {
    pub max_definition_bytes: usize,
    pub max_run_bytes: usize,
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_instances: usize,
    /// Total newly admitted leaf dispatches in one run, excluding reused output.
    pub max_attempts: usize,
    pub max_foreach_items: usize,
    pub max_iterations: u32,
    pub max_concurrent: usize,
    pub max_value_bytes: usize,
    pub max_report_bytes: usize,
    pub max_name_bytes: usize,
}

impl Default for WorkflowLimits {
    fn default() -> Self {
        Self {
            max_definition_bytes: WORKFLOW_MAX_DEFINITION_BYTES,
            max_run_bytes: WORKFLOW_MAX_RUN_BYTES,
            max_nodes: WORKFLOW_MAX_NODES,
            max_depth: WORKFLOW_MAX_DEPTH,
            max_instances: WORKFLOW_MAX_INSTANCES,
            max_attempts: WORKFLOW_MAX_ATTEMPTS,
            max_foreach_items: WORKFLOW_MAX_FOREACH_ITEMS,
            max_iterations: WORKFLOW_MAX_ITERATIONS,
            max_concurrent: WORKFLOW_MAX_CONCURRENT,
            max_value_bytes: WORKFLOW_MAX_VALUE_BYTES,
            max_report_bytes: WORKFLOW_MAX_REPORT_BYTES,
            max_name_bytes: WORKFLOW_MAX_NAME_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    Running,
    Stopping,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl WorkflowRunStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running | Self::Stopping)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeStatus {
    Pending,
    DispatchPending,
    Dispatched,
    Running,
    Stopping,
    Completed,
    Failed,
    Blocked,
    Interrupted,
    Cancelled,
}

impl WorkflowNodeStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Blocked | Self::Interrupted | Self::Cancelled
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReuseSource {
    pub run_id: Uuid,
    pub node_instance_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowNodeInstance {
    pub id: Uuid,
    pub node_id: String,
    pub path: String,
    pub attempt_id: Uuid,
    pub status: WorkflowNodeStatus,
    pub request_id: Option<Uuid>,
    pub agent_run: Option<AgentRun>,
    pub output: Option<Value>,
    pub report: Option<String>,
    pub error: Option<String>,
    pub usage: AgentUsageSummary,
    pub created_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub reused_from: Option<WorkflowReuseSource>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRun {
    pub id: Uuid,
    /// Monotonic committed snapshot generation for query/event reconciliation.
    #[serde(default)]
    pub revision: u64,
    pub parent_session_id: Uuid,
    pub definition_id: String,
    /// Immutable definition snapshot; editing a saved definition cannot alter a run.
    pub spec: WorkflowSpecV1,
    pub input: Value,
    pub limits: WorkflowLimits,
    pub status: WorkflowRunStatus,
    pub nodes: Vec<WorkflowNodeInstance>,
    pub output: Option<Value>,
    pub report: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub source_run_id: Option<Uuid>,
    pub acknowledged: bool,
}

/// Durable intent: poll persists this BEFORE returning it. Repeated polls and
/// recovery return the same request UUID until record_dispatch binds AgentRun.
/// No field here grants permissions; the driver must use its safe dispatch API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReadyTask {
    pub run_id: Uuid,
    pub parent_session_id: Uuid,
    pub node_instance_id: Uuid,
    pub attempt_id: Uuid,
    pub request_id: Uuid,
    pub node_path: String,
    pub profile: String,
    pub prompt: String,
    pub input: Value,
    pub output_schema: Option<Value>,
    pub worker_tier: Option<WorkflowWorkerTier>,
    pub max_turns: Option<u64>,
    pub token_budget: Option<u64>,
    pub timeout_seconds: Option<u64>,
    pub write_targets: Vec<String>,
    pub task_packet: Option<Value>,
    #[serde(default)]
    pub escalation: Option<Value>,
    #[serde(default)]
    pub target_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowTaskResult {
    pub agent_run: AgentRun,
    /// Structured output is independent of the human-readable AgentRun report.
    pub output: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowPoll {
    pub run: WorkflowRun,
    /// Includes previously persisted unbound intents: reconcile request IDs
    /// with the host ledger before dispatching after a restart.
    pub ready: Vec<WorkflowReadyTask>,
    /// Stop asks the host to cancel these AgentRuns; state stays Stopping
    /// until the host records their actual terminal statuses.
    pub cancel_agent_run_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReport {
    pub run_id: Uuid,
    pub status: WorkflowRunStatus,
    pub output: Option<Value>,
    pub report: String,
    pub usage: AgentUsageSummary,
    pub acknowledged: bool,
}

#[cfg(test)]
mod tests;
