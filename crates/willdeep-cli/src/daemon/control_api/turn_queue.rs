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
        let waiting = state
            .sessions
            .get_turn(id)
            .map_err(ApiFailure::internal)?
            .is_some_and(|turn| {
                matches!(
                    turn.status,
                    session_store::RuntimeTurnStatus::WaitingApproval
                        | session_store::RuntimeTurnStatus::WaitingAnswer
                )
            });
        if !waiting && params.action == willdeep_runtime_protocol::QueueAction::SendNow {
            let stop = ApiRequest::new("turn.stop", serde_json::json!({ "id": id }));
            super::turn_stop(state, &stop).await?;
        }
    }
    if matches!(
        params.action,
        willdeep_runtime_protocol::QueueAction::SendNow
            | willdeep_runtime_protocol::QueueAction::RunIndependently
    ) {
        state
            .sessions
            .set_yield_suppressed(params.session_id, false)
            .map_err(ApiFailure::internal)?;
    }
    state
        .tasks
        .schedule_session(params.session_id)
        .map_err(ApiFailure::internal)?;
    json_result(state.sessions.queued_messages(params.session_id))
}
