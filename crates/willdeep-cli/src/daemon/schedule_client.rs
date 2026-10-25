use super::*;

pub(crate) async fn schedule_operation(
    home: &Path,
    operation: &str,
    params: serde_json::Value,
    request_id: Option<uuid::Uuid>,
) -> Result<serde_json::Value> {
    schedule_response(home, operation, params, request_id)
        .await?
        .into_result()
        .map_err(|error| anyhow::anyhow!("schedule operation failed: {error:?}"))
}

pub(crate) async fn schedule_response(
    home: &Path,
    operation: &str,
    params: serde_json::Value,
    request_id: Option<uuid::Uuid>,
) -> Result<willdeep_runtime_protocol::ApiResponse<serde_json::Value>> {
    if !operation.starts_with("schedule.")
        && !operation.starts_with("agent_library.")
        && !operation.starts_with("screenplay.")
    {
        bail!("not a library or schedule operation");
    }
    let state = state_for_operation(home, operation, &params).await?;
    Ok(runtime_client(&state)?
        .call::<_, serde_json::Value>(operation, &params, request_id)
        .await?)
}

pub(crate) fn schedule_policy(
    home: &Path,
    origin: Option<&str>,
) -> Result<Option<willdeep_runtime_protocol::schedule::ScheduleSettings>> {
    super::schedule_engine::policy_for_origin(home, origin)
}

pub(crate) async fn read_session_attachments(
    home: &Path,
    id: uuid::Uuid,
    reference: &str,
) -> Result<serde_json::Value> {
    let state = require_running(home).await?;
    runtime_client(&state)?
        .call::<_, serde_json::Value>(
            "session.attachments",
            &serde_json::json!({"id":id,"reference":reference}),
            None,
        )
        .await?
        .into_result()
        .map_err(|error| anyhow::anyhow!("attachment read failed: {error:?}"))
}
