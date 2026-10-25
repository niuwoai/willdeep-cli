//! Three-episode autonomous screenplay pilot. Runtime owns execution; plugin owns content.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenplayStart {
    pub brief: String,
    pub workspace: PathBuf,
    pub max_calls: u32,
    pub max_tokens: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenplayTask {
    pub id: Uuid,
    pub grant: Uuid,
    pub arguments: serde_json::Value,
    pub state: String,
    pub result: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenplayCall {
    pub id: Uuid,
    pub task_id: Uuid,
    pub state: String,
    pub reservation: u64,
    pub tokens: Option<u64>,
    pub model: Option<String>,
    #[serde(default)]
    pub request_digest: String,
    #[serde(default)]
    pub response_digest: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenplayWorkflow {
    pub id: Uuid,
    pub request_id: Uuid,
    pub input: ScreenplayStart,
    pub status: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub tasks: Vec<ScreenplayTask>,
    pub calls: Vec<ScreenplayCall>,
    pub domain: Option<serde_json::Value>,
    pub error: Option<String>,
    pub cancel_requested: bool,
    #[serde(default)]
    pub budget_changes: Vec<ScreenplayBudgetChange>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenplayBudgetUpdate {
    pub id: Uuid,
    pub expected_calls: u32,
    pub expected_tokens: u64,
    pub max_calls: u32,
    pub max_tokens: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScreenplayBudgetChange {
    pub request_id: Uuid,
    pub at: u64,
    pub previous_calls: u32,
    pub previous_tokens: u64,
    pub max_calls: u32,
    pub max_tokens: u64,
}
