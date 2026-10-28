use super::*;
use willdeep_runtime_protocol::{QueueAction, QueuedTurnMessage, UpdateTurnQueueParams};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Mutation {
    action: QueueAction,
    message_id: Option<uuid::Uuid>,
    text: Option<String>,
    expected_text: Option<String>,
}

pub(super) async fn list(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Vec<QueuedTurnMessage>>, WebError> {
    authorized_event_session(&state, id).await?;
    crate::daemon::turn_queue::list(&state.home, id)
        .await
        .map(Json)
        .map_err(WebError::from_anyhow)
}

#[derive(Serialize)]
pub(super) struct IndependentTask {
    id: uuid::Uuid,
    status: String,
    messages: Vec<willdeep_core::conversation::ConversationItem>,
}

pub(super) async fn independent(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Vec<IndependentTask>>, WebError> {
    authorized_event_session(&state, id).await?;
    let histories = crate::daemon::turn_queue::independent_history(&state.home, id)
        .map_err(WebError::from_anyhow)?;
    Ok(Json(
        histories
            .into_iter()
            .map(|history| IndependentTask {
                id: history.id,
                status: serde_json::to_value(history.status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "interrupted".to_owned()),
                messages: willdeep_core::conversation::project(&history.messages, None),
            })
            .collect(),
    ))
}

pub(super) async fn update(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<Mutation>,
) -> Result<Json<Vec<QueuedTurnMessage>>, WebError> {
    authorized_event_session(&state, id).await?;
    crate::daemon::turn_queue::update(
        &state.home,
        &UpdateTurnQueueParams {
            session_id: id,
            action: body.action,
            message_id: body.message_id,
            text: body.text,
            expected_text: body.expected_text,
        },
    )
    .await
    .map(Json)
    .map_err(WebError::from_anyhow)
}

/// Submission acknowledges durable Runtime ownership, without waiting for a
/// foreground harness slot or tying execution to a browser's SSE connection.
pub(super) async fn enqueue(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(input): Json<ChatRequest>,
) -> Result<Json<willdeep_runtime_protocol::RuntimeTurn>, WebError> {
    if input.session_id.as_deref() != Some(id.to_string().as_str()) {
        return Err(WebError::bad_request(
            "Queue Session does not match request",
        ));
    }
    authorized_event_session(&state, id).await?;
    validate_chat(&state, &input).await?;
    ensure_web_runtime_session(&state, id).await?;
    let request_id = input
        .request_id
        .ok_or_else(|| WebError::bad_request("Queue needs request_id"))?;
    crate::daemon::submit_runtime_turn_with_options(
        &state.home,
        id,
        input.prompt,
        input.attachments,
        crate::Surface::Web,
        request_id,
        input.defer_until_released,
    )
    .await
    .map(Json)
    .map_err(WebError::from_anyhow)
}
