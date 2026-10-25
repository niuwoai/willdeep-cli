use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueuedTurnMessage {
    #[serde(default)]
    pub priority_requested: bool,
    #[serde(default)]
    pub independence_confirmed: bool,
    pub id: uuid::Uuid,
    pub session_id: uuid::Uuid,
    pub text: String,
    pub can_edit: bool,
    pub text_preview: String,
    pub image_count: usize,
    pub text_attachment_count: usize,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueueAction {
    Edit,
    Remove,
    Clear,
    SendNow,
    RunIndependently,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateTurnQueueParams {
    pub session_id: uuid::Uuid,
    pub action: QueueAction,
    #[serde(default)]
    pub message_id: Option<uuid::Uuid>,
    #[serde(default)]
    pub text: Option<String>,
    /// Compare-and-set: reject an edit based on an outdated snapshot.
    #[serde(default)]
    pub expected_text: Option<String>,
}
