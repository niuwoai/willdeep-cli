//! Web forwards all management to the authenticated local Runtime.
use super::*;

pub(super) fn routes() -> Router<Arc<WebState>> {
    Router::new().route("/api/agent-library/{operation}", post(operation))
}

async fn operation(
    State(state): State<Arc<WebState>>,
    Path(operation): Path<String>,
    Json(params): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, WebError> {
    if let Some(message) = agent_library_forbidden(state.settings_writable, &operation) {
        return Err(WebError::forbidden(message));
    }
    let response = crate::daemon::schedule_response(
        &state.home,
        &format!("agent_library.{operation}"),
        params,
        None,
    )
    .await
    .map_err(WebError::from_anyhow)?;
    match response {
        willdeep_runtime_protocol::ApiResponse::Ok { data, .. } => Ok(Json(data)),
        willdeep_runtime_protocol::ApiResponse::Error { error, .. } => Err(WebError {
            status: StatusCode::BAD_REQUEST,
            message: error.message,
        }),
    }
}

/// 精选目录和官方包预览来自 some.im 公开接口，托管站点也要能看。
/// 安装、同步、保存仍只允许本机 loopback，避免写到远端进程的家目录。
fn agent_library_forbidden(settings_writable: bool, operation: &str) -> Option<&'static str> {
    const PUBLIC_READ: &[&str] = &["catalog", "preview"];
    const ALLOWED: &[&str] = &[
        "list",
        "save",
        "import",
        "export",
        "fork",
        "history",
        "pin",
        "enable",
        "rollback",
        "catalog",
        "preview",
        "install",
        "share",
        "sync",
        "resolve",
        "delete",
        "logout",
        "bind",
        "rsi.list",
        "rsi.candidate",
        "rsi.prepare",
        "rsi.run",
        "rsi.report",
        "rsi.upload",
    ];
    if !ALLOWED.contains(&operation) {
        return Some("agent library operation unavailable");
    }
    if !settings_writable && !PUBLIC_READ.contains(&operation) {
        return Some("agent library requires a loopback Web server");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::agent_library_forbidden;

    #[test]
    fn hosted_web_can_read_the_public_catalog_but_not_the_local_library() {
        assert_eq!(agent_library_forbidden(false, "catalog"), None);
        assert_eq!(agent_library_forbidden(false, "preview"), None);
        assert_eq!(
            agent_library_forbidden(false, "list"),
            Some("agent library requires a loopback Web server")
        );
        assert_eq!(
            agent_library_forbidden(false, "install"),
            Some("agent library requires a loopback Web server")
        );
        assert_eq!(
            agent_library_forbidden(false, "sync"),
            Some("agent library requires a loopback Web server")
        );
        assert_eq!(
            agent_library_forbidden(false, "save"),
            Some("agent library requires a loopback Web server")
        );
        assert_eq!(
            agent_library_forbidden(false, "drop-tables"),
            Some("agent library operation unavailable")
        );
        assert_eq!(agent_library_forbidden(true, "list"), None);
        assert_eq!(agent_library_forbidden(true, "install"), None);
    }
}
