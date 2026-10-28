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
    /// 空闲时主动入队的消息：排着但不参与调度，要等用户明确要求立即发送。
    /// 老客户端不认识这个字段，缺省按普通待发消息处理。
    #[serde(default)]
    pub held: bool,
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
