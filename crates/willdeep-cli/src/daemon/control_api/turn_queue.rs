use super::*;

pub(super) fn list(state: &ServerState, request: &ApiRequest) -> ApiResult {
    let params = params::<willdeep_runtime_protocol::ListTurnsParams>(request)?;
    json_result(state.sessions.queued_messages(params.session_id))
}

/// 对齐 Claude Code：`send_now` 默认不 `turn.stop`；纯文本项在活动轮执行中时走 steer。
async fn deliver_send_now_without_stopping_active_turn(
    state: &ServerState,
    params: &willdeep_runtime_protocol::UpdateTurnQueueParams,
    active_turn_id: uuid::Uuid,
) -> Result<(), ApiFailure> {
    let active = state
        .sessions
        .get_turn(active_turn_id)
        .map_err(ApiFailure::internal)?
        .context("active Runtime Turn missing after queue update")
        .map_err(ApiFailure::internal)?;
    if !matches!(active.status, session_store::RuntimeTurnStatus::Running) {
        return Ok(());
    }
    let Some(message_id) = params.message_id else {
        return Ok(());
    };
    let steer_text = state
        .sessions
        .steer_text_for_pending_queue_message(params.session_id, message_id)
        .map_err(ApiFailure::internal)?;
    let Some(steer_text) = steer_text else {
        return Ok(());
    };
    let delivered = super::deliver_session_steer(state, params.session_id, &steer_text)?.is_some();
    if !delivered {
        return Ok(());
    }
    state
        .sessions
        .update_turn_queue(&willdeep_runtime_protocol::UpdateTurnQueueParams {
            session_id: params.session_id,
            action: willdeep_runtime_protocol::QueueAction::Remove,
            message_id: Some(message_id),
            text: None,
            expected_text: None,
        })
        .map_err(|error| {
            ApiFailure::invalid(format!("cannot update queue after steer: {error}"))
        })?;
    state
        .events
        .append(
            "turn.queue_steered",
            format!(
                "session_id={} message_id={message_id} active_turn_id={active_turn_id}",
                params.session_id
            ),
        )
        .map_err(ApiFailure::internal)?;
    Ok(())
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
            deliver_send_now_without_stopping_active_turn(state, &params, id).await?;
        }
    }
    if matches!(
        params.action,
        willdeep_runtime_protocol::QueueAction::SendNow
            | willdeep_runtime_protocol::QueueAction::RunIndependently
    ) {
        state
            .sessions
            .children
            .transaction(|ledger| {
                ledger.operator_input(params.session_id);
                Ok(())
            })
            .map_err(ApiFailure::internal)?;
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
