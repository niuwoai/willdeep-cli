//! Private browser projection of the Runtime child-session coordinator.
use super::*;
use willdeep_runtime_protocol::child_session::{ChildSessionTask, ChildSessionTaskPage};

type Id = uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Start {
    title: String,
    prompt: String,
    request_id: Id,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    through_turn_id: Option<Id>,
    #[serde(default)]
    workspace_mode: Option<String>,
    #[serde(default)]
    worktree_base: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Continue {
    prompt: String,
    request_id: Id,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Retry {
    expected_result_revision: u64,
    request_id: Id,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stop {
    request_id: Id,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Page {
    after: Option<Id>,
    limit: Option<usize>,
}

async fn private_parent(state: &Arc<WebState>, parent: Id) -> Result<Session, WebError> {
    // Assignment briefs, results and paths are never exposed on public listeners
    // or by the separate public-share router.
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "child conversations are only available on a loopback Web server",
        ));
    }
    ensure_web_runtime_session(state, parent).await?;
    SessionStore::new(&state.home)
        .load(parent)
        .map_err(|_| WebError::not_found("parent conversation was not found"))
}

async fn authorize_child(
    state: &Arc<WebState>,
    parent: &Session,
    child: Id,
) -> Result<(), WebError> {
    authorized_event_session(state, child).await?;
    let session = SessionStore::new(&state.home)
        .load(child)
        .map_err(|_| WebError::not_found("child conversation was not found"))?;
    if session.parent_session_id != Some(parent.id) {
        return Err(WebError::forbidden(
            "child conversation is outside this parent's relation",
        ));
    }
    // A shared-read-only child stays in the parent workspace; a worktree child
    // must be exactly the path the coordinator derives from its own identity.
    let worktree = state
        .home
        .join("worktrees")
        .join("children")
        .join(child.to_string());
    if session.workspace != parent.workspace && session.workspace != worktree {
        return Err(WebError::forbidden(
            "child conversation workspace is not the prepared shared path or worktree",
        ));
    }
    Ok(())
}

async fn authorize_task(
    state: &Arc<WebState>,
    parent: &Session,
    task: &ChildSessionTask,
) -> Result<(), WebError> {
    if task.parent_session_id != parent.id {
        return Err(WebError::forbidden(
            "assignment belongs to another parent conversation",
        ));
    }
    if task.status == willdeep_runtime_protocol::child_session::ChildSessionTaskStatus::Preparing
        && !SessionStore::new(&state.home)
            .path(task.child_session_id)
            .exists()
    {
        return Ok(());
    }
    authorize_child(state, parent, task.child_session_id).await
}

async fn call(
    state: &WebState,
    operation: &str,
    params: serde_json::Value,
    request: Option<Id>,
) -> Result<serde_json::Value, WebError> {
    crate::daemon::child_operation(&state.home, operation, params, request)
        .await
        .map_err(|error| {
            let message = willdeep_core::judge::redact_credentials(&error.to_string());
            if message.contains("unsupported child sessions") {
                WebError {
                    status: StatusCode::NOT_IMPLEMENTED,
                    message,
                }
            } else {
                WebError::conflict(message)
            }
        })
}

async fn get_task(
    state: &Arc<WebState>,
    parent: &Session,
    assignment: Id,
) -> Result<ChildSessionTask, WebError> {
    let value = call(
        state,
        "session.child.task.get",
        serde_json::json!({
            "parent_session_id":parent.id, "assignment_id":assignment
        }),
        None,
    )
    .await?;
    let task: ChildSessionTask =
        serde_json::from_value(value).map_err(|error| WebError::internal(error.to_string()))?;
    authorize_task(state, parent, &task).await?;
    Ok(task)
}

pub(super) async fn list(
    State(state): State<Arc<WebState>>,
    Path(id): Path<Id>,
    Query(page): Query<Page>,
) -> Result<Json<ChildSessionTaskPage>, WebError> {
    let parent = private_parent(&state, id).await?;
    let limit = page.limit.unwrap_or(50);
    if !(1..=100).contains(&limit) {
        return Err(WebError::bad_request(
            "page limit must be between 1 and 100",
        ));
    }
    let value = call(
        &state,
        "session.child.task.list",
        serde_json::json!({
            "parent_session_id":id, "after":page.after, "limit":limit
        }),
        None,
    )
    .await?;
    let page: ChildSessionTaskPage =
        serde_json::from_value(value).map_err(|error| WebError::internal(error.to_string()))?;
    for task in &page.tasks {
        authorize_task(&state, &parent, task).await?;
    }
    Ok(Json(page))
}

pub(super) async fn start(
    State(state): State<Arc<WebState>>,
    Path(id): Path<Id>,
    Json(body): Json<Start>,
) -> Result<Json<serde_json::Value>, WebError> {
    private_parent(&state, id).await?;
    let context = body.context.unwrap_or_else(|| "fresh".into());
    let mut through_turn_id = body.through_turn_id;
    if context == "fork" && through_turn_id.is_none() {
        // The UI offers "inherit the latest completed history" without exposing
        // Runtime Turn IDs. The coordinator still persists the exact boundary.
        let turns: Vec<willdeep_runtime_protocol::RuntimeTurn> = serde_json::from_value(
            crate::daemon::runtime_operation(
                &state.home,
                "turn.list",
                serde_json::json!({"session_id": id}),
                None,
            )
            .await
            .map_err(|error| WebError::conflict(error.to_string()))?,
        )
        .map_err(|error| WebError::internal(error.to_string()))?;
        through_turn_id = turns
            .into_iter()
            .rev()
            .find(|turn| {
                turn.status == willdeep_runtime_protocol::TurnStatus::Completed
                    && turn.message_end.is_some()
            })
            .map(|turn| turn.id);
        if through_turn_id.is_none() {
            return Err(WebError::bad_request(
                "no completed Turn is available to fork",
            ));
        }
    }
    call(
        &state,
        "session.child.start",
        serde_json::json!({
            "parent_session_id":id, "title":body.title, "prompt":body.prompt,
            "context":context,
            "through_turn_id":through_turn_id,
            "workspace_mode":body.workspace_mode.unwrap_or_else(|| "shared_read_only".into()),
            "worktree_base":body.worktree_base.unwrap_or_else(|| "head".into()),
            "model":body.model
        }),
        Some(body.request_id),
    )
    .await
    .map(Json)
}

pub(super) async fn get(
    State(state): State<Arc<WebState>>,
    Path((id, assignment)): Path<(Id, Id)>,
) -> Result<Json<ChildSessionTask>, WebError> {
    let parent = private_parent(&state, id).await?;
    get_task(&state, &parent, assignment).await.map(Json)
}

pub(super) async fn continue_task(
    State(state): State<Arc<WebState>>,
    Path((id, child)): Path<(Id, Id)>,
    Json(body): Json<Continue>,
) -> Result<Json<serde_json::Value>, WebError> {
    let parent = private_parent(&state, id).await?;
    authorize_child(&state, &parent, child).await?;
    call(
        &state,
        "session.child.continue",
        serde_json::json!({
            "parent_session_id":id, "child_session_id":child, "prompt":body.prompt
        }),
        Some(body.request_id),
    )
    .await
    .map(Json)
}

pub(super) async fn retry(
    State(state): State<Arc<WebState>>,
    Path((id, assignment)): Path<(Id, Id)>,
    Json(body): Json<Retry>,
) -> Result<Json<serde_json::Value>, WebError> {
    let parent = private_parent(&state, id).await?;
    get_task(&state, &parent, assignment).await?;
    call(&state, "session.child.retry", serde_json::json!({
        "parent_session_id":id, "assignment_id":assignment, "expected_result_revision":body.expected_result_revision
    }), Some(body.request_id)).await.map(Json)
}

pub(super) async fn stop(
    State(state): State<Arc<WebState>>,
    Path((id, assignment)): Path<(Id, Id)>,
    Json(body): Json<Stop>,
) -> Result<Json<serde_json::Value>, WebError> {
    let parent = private_parent(&state, id).await?;
    get_task(&state, &parent, assignment).await?;
    call(
        &state,
        "session.child.task.stop",
        serde_json::json!({
            "parent_session_id":id, "assignment_id":assignment
        }),
        Some(body.request_id),
    )
    .await
    .map(Json)
}
