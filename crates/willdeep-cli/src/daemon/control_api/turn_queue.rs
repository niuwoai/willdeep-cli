use super::*;

pub(super) fn list(state: &ServerState, request: &ApiRequest) -> ApiResult {
    let params = params::<willdeep_runtime_protocol::ListTurnsParams>(request)?;
    json_result(state.sessions.queued_messages(params.session_id))
}

pub(super) async fn update(state: &ServerState, request: &ApiRequest) -> ApiResult {
    let params = params::<willdeep_runtime_protocol::UpdateTurnQueueParams>(request)?;
    let active = state
        .sessions
        .update_turn_queue(&params)
        .map_err(|error| ApiFailure::invalid(format!("cannot update queue: {error}")))?;
    state
        .events
        .append(
            "turn.queue_updated",
            format!("session_id={}", params.session_id),
        )
        .map_err(ApiFailure::internal)?;
    if let Some(id) = active {
        // Cancel only the previous active turn. The selected message remains first
        // until its cancellation completes, then the ordinary scheduler claims it.
        let stop = ApiRequest::new("turn.stop", serde_json::json!({ "id": id }));
        super::turn_stop(state, &stop).await?;
    }
    state
        .tasks
        .schedule_session(params.session_id)
        .map_err(ApiFailure::internal)?;
    json_result(state.sessions.queued_messages(params.session_id))
}
