//! Private browser projection of the Runtime child-session coordinator.
use super::*;
use willdeep_runtime_protocol::child_session::{ChildSessionTask, ChildSessionTaskPage};

type Id = uuid::Uuid;

pub(super) fn router() -> axum::Router<Arc<WebState>> {
    axum::Router::new()
        .route(
            "/api/sessions/{id}/children",
            axum::routing::get(list).post(start),
        )
        .route(
            "/api/sessions/{id}/children/capabilities",
            axum::routing::get(capabilities),
        )
        .route(
            "/api/sessions/{id}/children/{assignment}",
            axum::routing::get(get),
        )
        .route(
            "/api/sessions/{id}/children/{assignment}/retry",
            axum::routing::post(retry),
        )
        .route(
            "/api/sessions/{id}/children/{assignment}/stop",
            axum::routing::post(stop),
        )
        .route(
            "/api/sessions/{id}/children/{child}/continue",
            axum::routing::post(continue_task),
        )
        .route(
            "/api/sessions/{id}/children/links/{child}",
            axum::routing::get(link),
        )
        .route(
            "/api/sessions/{id}/children/links/{child}/takeover",
            axum::routing::post(takeover),
        )
        .route(
            "/api/sessions/{id}/children/links/{child}/detach",
            axum::routing::post(detach),
        )
        .route(
            "/api/sessions/{id}/children/{assignment}/subscription",
            axum::routing::post(subscription),
        )
}

fn add_coordination_params(
    params: &mut serde_json::Value,
    subscription: Option<String>,
    depends_on: Vec<Id>,
) {
    // Do not send unknown P2 fields to older Runtimes for a plain P0/P1 request.
    if let Some(subscription) = subscription {
        params["subscription"] = subscription.into();
    }
    if !depends_on.is_empty() {
        params["depends_on"] = serde_json::json!(depends_on);
    }
}

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
    #[serde(default)]
    subscription: Option<String>,
    #[serde(default)]
    depends_on: Vec<Id>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Continue {
    prompt: String,
    request_id: Id,
    #[serde(default)]
    subscription: Option<String>,
    #[serde(default)]
    depends_on: Vec<Id>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Control {
    request_id: Id,
    expected_control_revision: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Subscription {
    request_id: Id,
    expected_subscription_revision: u64,
    subscription: String,
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
    if !SessionStore::new(&state.home).path(child).exists() {
        // Waiting dependencies may not have prepared a Core conversation yet.
        // Their durable Link still authorizes status/control through this parent.
        let link: willdeep_runtime_protocol::child_session::ChildSessionLink =
            serde_json::from_value(
                call(
                    state,
                    "session.child.link.get",
                    serde_json::json!({
                        "parent_session_id": parent.id, "child_session_id": child
                    }),
                    None,
                )
                .await?,
            )
            .map_err(|error| WebError::internal(error.to_string()))?;
        if link.parent_session_id != parent.id || link.child_session_id != child {
            return Err(WebError::forbidden(
                "child conversation relation did not match",
            ));
        }
        return Ok(());
    }
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
    if !SessionStore::new(&state.home)
        .path(task.child_session_id)
        .exists()
    {
        // The Runtime already validated this durable assignment's parent. A
        // waiting, cancelled or blocked task may never have prepared Core state;
        // its history must stay queryable, including on P0/P1 without link.get.
        return Ok(());
    }
    authorize_child(state, parent, task.child_session_id).await
}

fn legacy_subscription(value: &mut serde_json::Value) {
    if let Some(task) = value.as_object_mut()
        && !task.contains_key("subscription")
    {
        let subscription = if task.get("notify").and_then(serde_json::Value::as_str) == Some("none")
        {
            "none"
        } else {
            "auto_wake"
        };
        task.insert("subscription".into(), subscription.into());
    }
}

fn decode_task(mut value: serde_json::Value) -> Result<ChildSessionTask, WebError> {
    legacy_subscription(&mut value);
    serde_json::from_value(value).map_err(|error| WebError::internal(error.to_string()))
}

fn decode_task_page(mut value: serde_json::Value) -> Result<ChildSessionTaskPage, WebError> {
    if let Some(tasks) = value
        .get_mut("tasks")
        .and_then(serde_json::Value::as_array_mut)
    {
        for task in tasks {
            legacy_subscription(task);
        }
    }
    serde_json::from_value(value).map_err(|error| WebError::internal(error.to_string()))
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
    let task = decode_task(value)?;
    authorize_task(state, parent, &task).await?;
    Ok(task)
}

#[derive(Serialize)]
pub(super) struct TaskPage {
    #[serde(flatten)]
    page: ChildSessionTaskPage,
    capabilities: Vec<String>,
}

async fn runtime_capabilities(state: &WebState) -> Result<serde_json::Value, WebError> {
    crate::daemon::runtime_operation(
        &state.home,
        "runtime.capabilities",
        serde_json::json!({}),
        None,
    )
    .await
    .map_err(WebError::from_anyhow)
}

fn child_capabilities(value: &serde_json::Value) -> Vec<String> {
    value
        .get("capabilities")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|capability| capability.starts_with("session_children"))
        .map(str::to_owned)
        .collect()
}

pub(super) async fn list(
    State(state): State<Arc<WebState>>,
    Path(id): Path<Id>,
    Query(page): Query<Page>,
) -> Result<Json<TaskPage>, WebError> {
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
    let page = decode_task_page(value)?;
    for task in &page.tasks {
        authorize_task(&state, &parent, task).await?;
    }
    let capabilities = child_capabilities(&runtime_capabilities(&state).await?);
    Ok(Json(TaskPage { page, capabilities }))
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
    let mut params = serde_json::json!({
        "parent_session_id":id, "title":body.title, "prompt":body.prompt,
        "context":context,
        "through_turn_id":through_turn_id,
        "workspace_mode":body.workspace_mode.unwrap_or_else(|| "shared_read_only".into()),
        "worktree_base":body.worktree_base.unwrap_or_else(|| "head".into()),
        "model":body.model
    });
    add_coordination_params(&mut params, body.subscription, body.depends_on);
    call(&state, "session.child.start", params, Some(body.request_id))
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
    let mut params = serde_json::json!({
        "parent_session_id":id, "child_session_id":child, "prompt":body.prompt
    });
    add_coordination_params(&mut params, body.subscription, body.depends_on);
    call(
        &state,
        "session.child.continue",
        params,
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

async fn capabilities(
    State(state): State<Arc<WebState>>,
    Path(id): Path<Id>,
) -> Result<Json<serde_json::Value>, WebError> {
    private_parent(&state, id).await?;
    runtime_capabilities(&state).await.map(Json)
}

async fn link(
    State(state): State<Arc<WebState>>,
    Path((id, child)): Path<(Id, Id)>,
) -> Result<Json<serde_json::Value>, WebError> {
    let parent = private_parent(&state, id).await?;
    authorize_child(&state, &parent, child).await?;
    let mut value = call(
        &state,
        "session.child.link.get",
        serde_json::json!({
            "parent_session_id": id, "child_session_id": child
        }),
        None,
    )
    .await?;
    value["capabilities"] =
        serde_json::json!(child_capabilities(&runtime_capabilities(&state).await?));
    Ok(Json(value))
}

async fn change_control(
    state: Arc<WebState>,
    parent_id: Id,
    child_id: Id,
    body: Control,
    operation: &str,
) -> Result<Json<serde_json::Value>, WebError> {
    let parent = private_parent(&state, parent_id).await?;
    authorize_child(&state, &parent, child_id).await?;
    call(
        &state,
        operation,
        serde_json::json!({
            "parent_session_id": parent_id, "child_session_id": child_id,
            "expected_control_revision": body.expected_control_revision
        }),
        Some(body.request_id),
    )
    .await
    .map(Json)
}

async fn takeover(
    State(state): State<Arc<WebState>>,
    Path((id, child)): Path<(Id, Id)>,
    Json(body): Json<Control>,
) -> Result<Json<serde_json::Value>, WebError> {
    change_control(state, id, child, body, "session.child.takeover").await
}

async fn detach(
    State(state): State<Arc<WebState>>,
    Path((id, child)): Path<(Id, Id)>,
    Json(body): Json<Control>,
) -> Result<Json<serde_json::Value>, WebError> {
    change_control(state, id, child, body, "session.child.detach").await
}

async fn subscription(
    State(state): State<Arc<WebState>>,
    Path((id, assignment)): Path<(Id, Id)>,
    Json(body): Json<Subscription>,
) -> Result<Json<serde_json::Value>, WebError> {
    let parent = private_parent(&state, id).await?;
    get_task(&state, &parent, assignment).await?;
    call(
        &state,
        "session.child.task.subscription.set",
        serde_json::json!({
            "parent_session_id": id, "assignment_id": assignment,
            "expected_subscription_revision": body.expected_subscription_revision,
            "subscription": body.subscription
        }),
        Some(body.request_id),
    )
    .await
    .map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordination_fields_are_omitted_for_legacy_requests() {
        let mut legacy = serde_json::json!({"prompt": "Read the code"});
        add_coordination_params(&mut legacy, None, Vec::new());
        assert_eq!(legacy, serde_json::json!({"prompt": "Read the code"}));
        let dependency = Id::new_v4();
        add_coordination_params(&mut legacy, Some("inbox".into()), vec![dependency]);
        assert_eq!(legacy["subscription"], "inbox");
        assert_eq!(legacy["depends_on"], serde_json::json!([dependency]));
    }

    #[test]
    fn legacy_runtime_notifications_survive_list_and_detail_projection() {
        use willdeep_runtime_protocol::child_session::ChildSessionSubscription;
        for (notify, subscription, expected) in [
            ("none", None, ChildSessionSubscription::None),
            ("on_task_terminal", None, ChildSessionSubscription::AutoWake),
            ("none", Some("inbox"), ChildSessionSubscription::Inbox),
        ] {
            let mut value = serde_json::json!({
                "assignment_id": Id::new_v4(), "link_id": Id::new_v4(),
                "parent_session_id": Id::new_v4(), "child_session_id": Id::new_v4(),
                "request_id": Id::new_v4(), "title": "Legacy task", "prompt": "Read",
                "notify": notify, "status": "completed", "created_at": 1, "updated_at": 2
            });
            if let Some(subscription) = subscription {
                value["subscription"] = subscription.into();
            }
            let task = decode_task(value.clone()).unwrap();
            let page = decode_task_page(serde_json::json!({
                "tasks": [value], "next_cursor": null
            }))
            .unwrap();
            assert_eq!(task.subscription, expected);
            assert_eq!(page.tasks[0].subscription, expected);
        }
    }

    #[test]
    fn user_control_requires_a_revision_and_rejects_forged_identity() {
        let request = Id::new_v4();
        assert!(
            serde_json::from_value::<Control>(serde_json::json!({
                "request_id": request
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<Control>(serde_json::json!({
                "request_id": request, "expected_control_revision": 1, "operator": true
            }))
            .is_err()
        );
        let control: Control = serde_json::from_value(serde_json::json!({
            "request_id": request, "expected_control_revision": 3
        }))
        .unwrap();
        assert_eq!(control.expected_control_revision, 3);
    }
}
