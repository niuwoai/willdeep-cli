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
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "agent library requires a loopback Web server",
        ));
    }
    const ALLOWED: &[&str] = &[
        "list", "save", "import", "export", "fork", "history", "pin", "enable", "rollback",
        "catalog", "preview", "install", "share", "sync", "delete", "logout", "bind",
    ];
    if !ALLOWED.contains(&operation.as_str()) {
        return Err(WebError::forbidden("agent library operation unavailable"));
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
