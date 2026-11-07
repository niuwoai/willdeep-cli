use super::*;

pub(super) async fn list(
    State(state): State<Arc<WebState>>,
    Path(parent): Path<Id>,
) -> Result<Json<serde_json::Value>, WebError> {
    private_parent(&state, parent).await?;
    call(
        &state,
        "session.child.proposal.list",
        serde_json::json!({"parent_session_id":parent}),
        None,
    )
    .await
    .map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Confirm {
    request_id: Id,
}

pub(super) async fn start(
    State(state): State<Arc<WebState>>,
    Path((parent, proposal)): Path<(Id, Id)>,
    Json(body): Json<Confirm>,
) -> Result<Json<serde_json::Value>, WebError> {
    private_parent(&state, parent).await?;
    call(
        &state,
        "session.child.proposal.start",
        serde_json::json!({
            "parent_session_id":parent, "proposal_id":proposal
        }),
        Some(body.request_id),
    )
    .await
    .map(Json)
}

pub(super) async fn dismiss(
    State(state): State<Arc<WebState>>,
    Path((parent, proposal)): Path<(Id, Id)>,
    Json(body): Json<Confirm>,
) -> Result<Json<serde_json::Value>, WebError> {
    private_parent(&state, parent).await?;
    call(
        &state,
        "session.child.proposal.dismiss",
        serde_json::json!({
            "parent_session_id":parent, "proposal_id":proposal
        }),
        Some(body.request_id),
    )
    .await
    .map(Json)
}
