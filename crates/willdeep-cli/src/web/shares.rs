//! 会话分享的管理面（本机回环）。公开的读与写都在公网入口那个监听上，
//! 见 `daemon::intake`。

use super::*;
use crate::shares::{self, Share, ShareMode};

pub(super) fn routes() -> Router<Arc<WebState>> {
    Router::new().route(
        "/api/sessions/{id}/share",
        get(read).post(create).delete(remove),
    )
}

#[derive(Serialize)]
struct ShareView {
    token: String,
    mode: &'static str,
    created_at: u64,
    /// 入口监听上的路径；完整链接要配合入口的实际地址由前端拼出来。
    path: String,
}

#[derive(Serialize)]
struct ShareResponse {
    /// 没有分享时是 `null`，而不是省略字段：前端要区分「读不到」和「没分享」。
    share: Option<ShareView>,
}

fn view(token: String, share: &Share) -> ShareView {
    ShareView {
        mode: share.mode.wire_name(),
        created_at: share.created_at,
        path: shares::public_path(&token),
        token,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    mode: ShareMode,
}

/// 只有本机页面能管分享；会话还必须在这个 Web 的工作区白名单里。
async fn authorize(state: &WebState, id: uuid::Uuid) -> Result<(), WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "sharing a session is available only from a loopback Web server",
        ));
    }
    let session = SessionStore::new(&state.home)
        .load(id)
        .map_err(|_| WebError::bad_request("session was not found"))?;
    let allowed = registered_web_workspaces(state).await?;
    if !allowed.iter().any(|workspace| workspace.root == session.workspace) {
        return Err(WebError::bad_request(
            "session workspace is not in the server allowlist",
        ));
    }
    Ok(())
}

async fn read(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<ShareResponse>, WebError> {
    authorize(&state, id).await?;
    Ok(Json(ShareResponse {
        share: shares::token_for(&state.home, id).map(|(token, share)| view(token, &share)),
    }))
}

async fn create(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(request): Json<CreateRequest>,
) -> Result<Json<ShareResponse>, WebError> {
    authorize(&state, id).await?;
    shares::validate_mode(request.mode).map_err(WebError::from_anyhow)?;
    let (token, share) = shares::create(&state.home, id, request.mode).map_err(WebError::from_anyhow)?;
    Ok(Json(ShareResponse {
        share: Some(view(token, &share)),
    }))
}

async fn remove(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<ShareResponse>, WebError> {
    authorize(&state, id).await?;
    shares::remove(&state.home, id).map_err(WebError::from_anyhow)?;
    Ok(Json(ShareResponse { share: None }))
}
