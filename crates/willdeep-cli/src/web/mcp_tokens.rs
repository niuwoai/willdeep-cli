//! MCP token 的管理接口，给 Web 设置页用。
//!
//! 明文只在创建那一次的响应里出现，之后连这个接口也只返回哈希过的记录。

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;

use super::{WebError, WebState};
use crate::daemon::mcp_tokens::{self, McpTokenRecord};

#[derive(Deserialize)]
pub(super) struct CreateToken {
    label: String,
}

#[derive(serde::Serialize)]
pub(super) struct IssuedToken {
    id: uuid::Uuid,
    label: String,
    created_at: u64,
    /// 唯一一次返回明文。前端展示完就不再留着。
    token: String,
}

pub(super) async fn list(State(state): State<Arc<WebState>>) -> Json<Vec<McpTokenRecord>> {
    Json(mcp_tokens::list(&state.home))
}

pub(super) async fn create(
    State(state): State<Arc<WebState>>,
    Json(body): Json<CreateToken>,
) -> Result<Json<IssuedToken>, WebError> {
    let label = body.label.trim();
    if label.is_empty() {
        return Err(WebError::bad_request("Token needs a label"));
    }
    let (record, token) = mcp_tokens::issue(&state.home, label)
        .map_err(|error| WebError::internal(error.to_string()))?;
    Ok(Json(IssuedToken {
        id: record.id,
        label: record.label,
        created_at: record.created_at,
        token,
    }))
}

pub(super) async fn revoke(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Status>, WebError> {
    let found = mcp_tokens::revoke(&state.home, id)
        .map_err(|error| WebError::internal(error.to_string()))?;
    if !found {
        return Err(WebError::not_found("Token not found"));
    }
    Ok(Json(Status { revoked: true }))
}

#[derive(serde::Serialize)]
pub(super) struct Status {
    revoked: bool,
}
