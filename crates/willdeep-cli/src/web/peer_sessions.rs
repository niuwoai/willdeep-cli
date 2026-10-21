use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SendNote {
    session_id: uuid::Uuid,
    message: String,
    reply_to: Option<uuid::Uuid>,
    request_id: uuid::Uuid,
}

pub(super) async fn peers(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, WebError> {
    ensure_web_runtime_session(&state, id).await?;
    let peers = crate::daemon::peer_operation(
        &state.home,
        "session.peers",
        serde_json::json!({"id":id}),
        None,
    )
    .await
    .map_err(WebError::from_anyhow)?;
    let mut allowed = Vec::new();
    for peer in peers
        .as_array()
        .ok_or_else(|| WebError::bad_request("invalid peer response"))?
    {
        let Some(target) = peer
            .get("id")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.parse().ok())
        else {
            continue;
        };
        // Project membership does not enlarge this browser's workspace allowlist.
        if authorized_event_session(&state, target).await.is_ok() {
            allowed.push(peer.clone());
        }
    }
    Ok(Json(serde_json::Value::Array(allowed)))
}

pub(super) async fn inbox(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, WebError> {
    authorized_event_session(&state, id).await?;
    crate::daemon::peer_operation(
        &state.home,
        "session.peer.inbox",
        serde_json::json!({"session_id":id}),
        None,
    )
    .await
    .map(Json)
    .map_err(WebError::from_anyhow)
}

pub(super) async fn send(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<SendNote>,
) -> Result<Json<serde_json::Value>, WebError> {
    ensure_web_runtime_session(&state, id).await?;
    ensure_web_runtime_session(&state, body.session_id).await?;
    crate::daemon::peer_operation(&state.home, "session.peer.send", serde_json::json!({
        "source_session_id":id, "session_id":body.session_id, "message":body.message, "reply_to":body.reply_to
    }), Some(body.request_id)).await.map(Json).map_err(WebError::from_anyhow)
}

pub(super) async fn read(
    State(state): State<Arc<WebState>>,
    Path((id, note)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<Json<serde_json::Value>, WebError> {
    authorized_event_session(&state, id).await?;
    crate::daemon::peer_operation(
        &state.home,
        "session.peer.read",
        serde_json::json!({
            "session_id":id, "message_id":note, "client_id":"web"
        }),
        None,
    )
    .await
    .map(Json)
    .map_err(WebError::from_anyhow)
}
