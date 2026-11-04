use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use willdeep_core::{Session, SessionStore};

use super::{FileStamp, WebError, WebState, workspace_allowed};

/// 文件指纹没变时直接返回缓存的工作区。
pub(super) fn cached_session_workspace(
    cache: &std::sync::Mutex<HashMap<uuid::Uuid, (FileStamp, PathBuf)>>,
    session_id: uuid::Uuid,
    stamp: FileStamp,
) -> Option<PathBuf> {
    cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&session_id)
        .filter(|(cached, _)| *cached == stamp)
        .map(|(_, workspace)| workspace.clone())
}

/// 指纹对不上（或还没有）时读会话、记下新的归属。`load` 失败返回 `None`，
/// 不写缓存——下一次请求再试，而不是把一次瞬时的读失败记成永久拒绝。
pub(super) fn remember_session_workspace(
    cache: &std::sync::Mutex<HashMap<uuid::Uuid, (FileStamp, PathBuf)>>,
    session_id: uuid::Uuid,
    stamp: FileStamp,
    load: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    let workspace = load()?;
    cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(session_id, (stamp, workspace.clone()));
    Some(workspace)
}

/// 会话必须属于白名单里的工作区，否则一律拒绝。
///
/// 与会话详情同一条门：浏览器端没有应用层鉴权，能看到什么完全由这份白名单
/// 决定，事件不该是例外。
///
/// 这条路径被前端每两秒轮询一次。归属只取决于会话的工作区，而工作区写进
/// 会话文件后基本不变，所以按文件指纹缓存：文件没变就不再整读会话。
pub(super) async fn authorized_event_session(
    state: &Arc<WebState>,
    session_id: uuid::Uuid,
) -> Result<(), WebError> {
    let not_found = || WebError::bad_request("session was not found");
    let path = SessionStore::new(&state.home).path(session_id);
    let metadata = std::fs::metadata(&path).map_err(|_| not_found())?;
    let stamp = FileStamp::of(&metadata);
    let cached = cached_session_workspace(&state.session_workspaces, session_id, stamp);
    let workspace = match cached {
        Some(workspace) => workspace,
        None => {
            // 整读会话是贵的（长会话几 MB），别堵在 tokio worker 上。
            let home = state.home.clone();
            let cache = Arc::clone(&state.session_workspaces);
            tokio::task::spawn_blocking(move || {
                remember_session_workspace(&cache, session_id, stamp, || {
                    SessionStore::new(&home)
                        .load(session_id)
                        .map(|session| session.workspace)
                        .ok()
                })
            })
            .await
            .map_err(|error| WebError::internal(error.to_string()))?
            .ok_or_else(not_found)?
        }
    };
    if workspace_allowed(state, &workspace).await? {
        return Ok(());
    }
    // A delegated writable child lives in a runtime-owned worktree outside the
    // project allowlist. It is reachable only when its parent Session's
    // workspace is allowed; the child still cannot widen that policy.
    let home = state.home.clone();
    let parent = tokio::task::spawn_blocking(move || {
        SessionStore::new(&home)
            .load(session_id)
            .ok()
            .and_then(|session| session.parent_session_id)
    })
    .await
    .map_err(|error| WebError::internal(error.to_string()))?;
    if let Some(parent) = parent
        && let Ok(parent_session) = SessionStore::new(&state.home).load(parent)
        && workspace_allowed(state, &parent_session.workspace).await?
    {
        return Ok(());
    }
    Err(WebError::bad_request(
        "session workspace is not in the server allowlist",
    ))
}

/// A delegated child may live in a Runtime-owned worktree outside the project
/// allowlist. It is reachable only when its parent Session's workspace is in
/// the allowlist; the child never widens that policy by itself.
pub(super) async fn session_workspace_allowed(
    state: &WebState,
    session: &Session,
) -> Result<bool, WebError> {
    if workspace_allowed(state, &session.workspace).await? {
        return Ok(true);
    }
    let Some(parent) = session.parent_session_id else {
        return Ok(false);
    };
    let Ok(parent) = SessionStore::new(&state.home).load(parent) else {
        return Ok(false);
    };
    workspace_allowed(state, &parent.workspace).await
}
