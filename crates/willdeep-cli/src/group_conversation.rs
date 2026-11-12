//! 群会话里能改的设置。工具只知道当前会话，宿主按会话找到它所属的那一个群。
//!
//! 改动落在群记录上，不写进配置，也不影响其他群。会话不是群会话时直接拒绝。

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};
use willdeep_core::tools::{GroupConversationHost, ToolError};

use crate::federation;

pub(crate) struct GroupConversation {
    home: PathBuf,
}

impl GroupConversation {
    pub(crate) fn new(home: PathBuf) -> Self {
        Self { home }
    }
}

#[async_trait]
impl GroupConversationHost for GroupConversation {
    async fn set_mention_limit(&self, session_id: &str, limit: u32) -> Result<Value, ToolError> {
        let session_id = uuid::Uuid::parse_str(session_id).map_err(|_| {
            ToolError::HookDenied("this turn has no federation group session".to_owned())
        })?;
        let Some(group) = federation::group_for_session(&self.home, session_id)
            .map_err(|error| ToolError::HookDenied(format!("{error:#}")))?
        else {
            return Err(ToolError::HookDenied(
                "this session is not a federation group, so there is no mention limit to change"
                    .to_owned(),
            ));
        };
        let updated = federation::set_group_mention_depth(&self.home, &group.id.to_string(), limit)
            .map_err(|error| ToolError::HookDenied(format!("{error:#}")))?;
        Ok(json!({
            "group": updated.name,
            "mention_limit": federation::mention_depth_limit(&updated),
            "note": "applies to the next message in this group only; a person's message starts the count over",
        }))
    }
}
