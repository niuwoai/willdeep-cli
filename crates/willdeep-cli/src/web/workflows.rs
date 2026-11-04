//! Session-scoped workflow management through the authenticated Runtime API.
use super::*;

pub(super) fn routes() -> Router<Arc<WebState>> {
    Router::new().route("/api/workflows/{operation}", post(operation))
}

const OPERATIONS: &[&str] = &[
    "schema", "list", "get", "history", "validate", "save", "run", "stop", "retry", "resume",
];

async fn operation(
    State(state): State<Arc<WebState>>,
    Path(operation): Path<String>,
    Json(params): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, WebError> {
    if !OPERATIONS.contains(&operation.as_str()) {
        return Err(WebError::bad_request("workflow operation unavailable"));
    }
    // Resolve the parent in this Web server's workspace allowlist before forwarding.
    let parent = params
        .get("parent_session_id")
        .cloned()
        .ok_or_else(|| WebError::bad_request("parent_session_id is required"))?;
    let parent: uuid::Uuid = serde_json::from_value(parent)
        .map_err(|_| WebError::bad_request("invalid parent_session_id"))?;
    let session = SessionStore::new(&state.home)
        .load(parent)
        .map_err(|_| WebError::not_found("parent session was not found"))?;
    if !workspace_allowed(&state, &session.workspace).await? {
        return Err(WebError::forbidden(
            "session workspace is not in the server allowlist",
        ));
    }
    let request_id = params
        .get("request_id")
        .cloned()
        .map(serde_json::from_value::<uuid::Uuid>)
        .transpose()
        .map_err(|_| WebError::bad_request("invalid request_id"))?;
    let response = crate::daemon::schedule_response(
        &state.home,
        &format!("workflow.{operation}"),
        params,
        request_id,
    )
    .await
    .map_err(WebError::from_anyhow)?;
    match response {
        willdeep_runtime_protocol::ApiResponse::Ok { data, .. } => Ok(Json(data)),
        willdeep_runtime_protocol::ApiResponse::Error { error, .. } => {
            use willdeep_runtime_protocol::ErrorCode;
            let status = match error.code {
                ErrorCode::Forbidden | ErrorCode::Unauthorized => StatusCode::FORBIDDEN,
                ErrorCode::NotFound => StatusCode::NOT_FOUND,
                ErrorCode::Conflict | ErrorCode::StaleSnapshot => StatusCode::CONFLICT,
                ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::BAD_REQUEST,
            };
            Err(WebError {
                status,
                message: error.message,
            })
        }
    }
}
