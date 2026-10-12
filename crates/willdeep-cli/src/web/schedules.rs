//! Web is a workspace-scoped view of Runtime schedules, never a second scheduler.
use super::*;
use clap::Parser;
use willdeep_runtime_protocol::schedule::{
    ScheduleDraft, ScheduleHistoryQuery, SchedulePreview, ScheduleRunRef, ScheduleUpdate,
    ScheduledTaskView,
};
use willdeep_runtime_protocol::{ApiResponse, ErrorCode};

pub(super) fn routes() -> Router<Arc<WebState>> {
    Router::new()
        .route("/api/schedules", get(list).post(create))
        .route("/api/schedules/deleted", get(deleted))
        .route("/api/schedules/timezones", get(timezones))
        .route("/api/schedules/models", get(models))
        .route("/api/schedules/preview", post(preview))
        .route("/api/schedules/resolve-time", post(resolve_time))
        .route("/api/schedules/format-time", post(format_time))
        .route("/api/schedules/{id}", get(detail).put(update))
        .route("/api/schedules/{id}/action", post(action))
        .route("/api/schedules/{id}/run", post(run))
        .route("/api/schedules/{id}/history", get(history))
        .route("/api/schedules/{id}/history/{run_id}", get(run_detail))
        .route("/api/schedules/{id}/history/{run_id}/stop", post(stop))
        .route("/api/schedules/{id}/cleanup", post(cleanup))
        .route(
            "/api/sessions/{id}/attachments/{reference}",
            get(session_attachments),
        )
}

async fn session_attachments(
    State(state): State<Arc<WebState>>,
    Path((id, reference)): Path<(uuid::Uuid, String)>,
) -> Result<Json<serde_json::Value>, WebError> {
    let session = SessionStore::new(&state.home)
        .load(id)
        .map_err(|_| WebError::not_found("Session not found"))?;
    if !workspace_allowed(&state, &session.workspace).await? {
        return Err(WebError::not_found("Session not found in Web workspaces"));
    }
    crate::daemon::read_session_attachments(&state.home, id, &reference)
        .await
        .map(Json)
        .map_err(WebError::from_anyhow)
}

async fn call(
    state: &WebState,
    operation: &str,
    params: serde_json::Value,
    request_id: Option<uuid::Uuid>,
) -> Result<serde_json::Value, WebError> {
    let response = crate::daemon::schedule_response(&state.home, operation, params, request_id)
        .await
        .map_err(WebError::from_anyhow)?;
    match response {
        ApiResponse::Ok { data, .. } => Ok(data),
        ApiResponse::Error { error, .. } => Err(WebError {
            status: match error.code {
                ErrorCode::Conflict => StatusCode::CONFLICT,
                ErrorCode::NotFound => StatusCode::NOT_FOUND,
                ErrorCode::Forbidden => StatusCode::FORBIDDEN,
                ErrorCode::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::UnsupportedOperation | ErrorCode::UnsupportedProtocol => {
                    StatusCode::UPGRADE_REQUIRED
                }
                _ => StatusCode::BAD_REQUEST,
            },
            message: error.message,
        }),
    }
}

fn writable(state: &WebState) -> Result<(), WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "schedule maintenance requires a loopback Web server",
        ));
    }
    Ok(())
}

#[derive(Default, Deserialize)]
struct ModelQuery {
    profile: Option<String>,
}
async fn models(
    State(state): State<Arc<WebState>>,
    Query(query): Query<ModelQuery>,
) -> Result<Json<serde_json::Value>, WebError> {
    let loaded = crate::config::LoadedConfig::load(Some(&state.config_path))
        .map_err(WebError::from_anyhow)?;
    let mut cli = crate::Cli::try_parse_from(["willdeep"])
        .map_err(|error| WebError::bad_request(error.to_string()))?;
    cli.profile = query
        .profile
        .filter(|profile| !profile.is_empty())
        .or_else(|| state.profile.clone());
    let mut provider = crate::harness::resolve_parent_provider_config(&cli, &loaded, None)
        .map_err(WebError::from_anyhow)?;
    provider.request_timeout_secs = 15;
    let fallback = provider.model.clone();
    let (mut models, warning) = match willdeep_core::provider::list_models(&provider).await {
        Ok(models) => (models, None),
        Err(error) => (
            Vec::new(),
            Some(willdeep_core::judge::redact_credentials(&error.to_string())),
        ),
    };
    models.push(fallback);
    models.sort();
    models.dedup();
    Ok(Json(serde_json::json!({"models":models,"warning":warning})))
}

async fn allowed(state: &WebState, id: uuid::Uuid) -> Result<ScheduledTaskView, WebError> {
    let value = call(state, "schedule.get", serde_json::json!({"id":id}), None).await?;
    let view: ScheduledTaskView =
        serde_json::from_value(value).map_err(|error| WebError::internal(error.to_string()))?;
    if !workspace_allowed(state, &view.task.workspace).await? {
        return Err(WebError::not_found(
            "scheduled task not found in Web workspaces",
        ));
    }
    Ok(scope_history(view, &allowed_workspaces(state)))
}

fn scope_history(mut view: ScheduledTaskView, paths: &[PathBuf]) -> ScheduledTaskView {
    if view
        .last_run
        .as_ref()
        .is_some_and(|run| !paths.contains(&run.snapshot.workspace))
    {
        view.last_run = None;
    }
    if view
        .active_run
        .as_ref()
        .is_some_and(|run| !paths.contains(&run.snapshot.workspace))
    {
        view.active_run = None;
    }
    view
}

async fn list(State(state): State<Arc<WebState>>) -> Result<Json<serde_json::Value>, WebError> {
    let value = call(&state, "schedule.list", serde_json::json!({}), None).await?;
    let tasks: Vec<ScheduledTaskView> =
        serde_json::from_value(value).map_err(|error| WebError::internal(error.to_string()))?;
    let allowed = registered_web_workspaces(&state).await?;
    let paths = allowed_workspaces(&state);
    let tasks = tasks
        .into_iter()
        .filter(|view| workspace_in_allowlist(&allowed, &view.task.workspace))
        .map(|view| scope_history(view, &paths))
        .collect::<Vec<_>>();
    Ok(Json(
        serde_json::json!({"tasks":tasks,"writable":state.settings_writable}),
    ))
}

async fn deleted(State(state): State<Arc<WebState>>) -> Result<Json<serde_json::Value>, WebError> {
    let value = call(&state, "schedule.deleted", serde_json::json!({}), None).await?;
    let tasks: Vec<willdeep_runtime_protocol::schedule::DeletedScheduledTask> =
        serde_json::from_value(value).map_err(|error| WebError::internal(error.to_string()))?;
    let allowed = allowed_workspaces(&state);
    Ok(Json(serde_json::json!(tasks.into_iter().filter(|item| allowed.contains(&item.task.workspace))
        .map(|item| serde_json::json!({"task":item.task,"deleted":true,"deleted_at":item.deleted_at,"next_fire_at":null,"last_run":null})).collect::<Vec<_>>())))
}

async fn allowed_history(state: &WebState, id: uuid::Uuid) -> Result<(), WebError> {
    if let Ok(view) = allowed(state, id).await {
        return if allowed_workspaces(state).contains(&view.task.workspace) {
            Ok(())
        } else {
            Err(WebError::not_found("task not found"))
        };
    }
    let value = call(state, "schedule.deleted", serde_json::json!({}), None).await?;
    let retained: Vec<willdeep_runtime_protocol::schedule::DeletedScheduledTask> =
        serde_json::from_value(value).map_err(|error| WebError::internal(error.to_string()))?;
    if retained
        .iter()
        .any(|item| item.task.id == id && allowed_workspaces(state).contains(&item.task.workspace))
    {
        Ok(())
    } else {
        Err(WebError::not_found(
            "retained task not found in Web workspaces",
        ))
    }
}

#[derive(Deserialize)]
struct Create {
    task: ScheduleDraft,
    request_id: uuid::Uuid,
}
async fn create(
    State(state): State<Arc<WebState>>,
    Json(mut input): Json<Create>,
) -> Result<Json<serde_json::Value>, WebError> {
    writable(&state)?;
    input.task.settings.config_path = Some(state.config_path.clone());
    input.task.settings.profile = input
        .task
        .settings
        .profile
        .or_else(|| state.profile.clone());
    if !workspace_allowed(&state, &input.task.workspace).await? {
        return Err(WebError::forbidden(
            "schedule workspace is outside the Web allowlist",
        ));
    }
    call(
        &state,
        "schedule.create",
        serde_json::to_value(input.task).map_err(|e| WebError::bad_request(e.to_string()))?,
        Some(input.request_id),
    )
    .await
    .map(Json)
}

async fn detail(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<ScheduledTaskView>, WebError> {
    allowed(&state, id).await.map(Json)
}

#[derive(Deserialize)]
struct Update {
    task: ScheduleDraft,
    expected_revision: u64,
    request_id: uuid::Uuid,
}
async fn update(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(mut input): Json<Update>,
) -> Result<Json<serde_json::Value>, WebError> {
    writable(&state)?;
    input.task.settings.config_path = Some(state.config_path.clone());
    input.task.settings.profile = input
        .task
        .settings
        .profile
        .or_else(|| state.profile.clone());
    allowed(&state, id).await?;
    if !workspace_allowed(&state, &input.task.workspace).await? {
        return Err(WebError::forbidden(
            "schedule workspace is outside the Web allowlist",
        ));
    }
    let params = ScheduleUpdate {
        id,
        expected_revision: input.expected_revision,
        task: input.task,
    };
    call(
        &state,
        "schedule.update",
        serde_json::to_value(params).map_err(|e| WebError::bad_request(e.to_string()))?,
        Some(input.request_id),
    )
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct Action {
    action: String,
    expected_revision: u64,
    request_id: uuid::Uuid,
}
async fn action(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(input): Json<Action>,
) -> Result<Json<serde_json::Value>, WebError> {
    writable(&state)?;
    allowed(&state, id).await?;
    call(&state, "schedule.action", serde_json::json!({"id":id,"expected_revision":input.expected_revision,"action":input.action}), Some(input.request_id)).await.map(Json)
}

#[derive(Deserialize)]
struct Run {
    request_id: uuid::Uuid,
    #[serde(default)]
    retry_of: Option<uuid::Uuid>,
}
async fn run(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(input): Json<Run>,
) -> Result<Json<serde_json::Value>, WebError> {
    writable(&state)?;
    allowed(&state, id).await?;
    if let Some(run_id) = input.retry_of {
        allowed_run(&state, id, run_id).await?;
    }
    call(
        &state,
        "schedule.run",
        serde_json::json!({"id":id,"retry_of":input.retry_of}),
        Some(input.request_id),
    )
    .await
    .map(Json)
}

async fn preview(
    State(state): State<Arc<WebState>>,
    Json(params): Json<SchedulePreview>,
) -> Result<Json<serde_json::Value>, WebError> {
    if let Some(id) = params.task_id {
        allowed(&state, id).await?;
    }
    call(
        &state,
        "schedule.preview",
        serde_json::to_value(params).map_err(|e| WebError::bad_request(e.to_string()))?,
        None,
    )
    .await
    .map(Json)
}
async fn timezones(
    State(state): State<Arc<WebState>>,
) -> Result<Json<serde_json::Value>, WebError> {
    call(&state, "schedule.timezones", serde_json::json!({}), None)
        .await
        .map(Json)
}

async fn resolve_time(
    State(state): State<Arc<WebState>>,
    Json(params): Json<willdeep_runtime_protocol::schedule::ScheduleLocalTime>,
) -> Result<Json<serde_json::Value>, WebError> {
    call(
        &state,
        "schedule.resolve_time",
        serde_json::to_value(params).map_err(|e| WebError::bad_request(e.to_string()))?,
        None,
    )
    .await
    .map(Json)
}

async fn format_time(
    State(state): State<Arc<WebState>>,
    Json(params): Json<willdeep_runtime_protocol::schedule::ScheduleFormatTime>,
) -> Result<Json<serde_json::Value>, WebError> {
    call(
        &state,
        "schedule.format_time",
        serde_json::to_value(params).map_err(|e| WebError::bad_request(e.to_string()))?,
        None,
    )
    .await
    .map(Json)
}

#[derive(Default, Deserialize)]
struct HistoryQuery {
    before: Option<String>,
    limit: Option<usize>,
    status: Option<willdeep_runtime_protocol::schedule::ScheduleRunStatus>,
}
async fn history(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, WebError> {
    allowed_history(&state, id).await?;
    let params = ScheduleHistoryQuery {
        task_id: id,
        before: query.before,
        limit: query.limit,
        status: query.status,
    };
    call(
        &state,
        "schedule.history",
        serde_json::to_value(params).map_err(|e| WebError::bad_request(e.to_string()))?,
        None,
    )
    .await
    .and_then(|value| {
        let mut page: willdeep_runtime_protocol::schedule::ScheduleHistoryPage =
            serde_json::from_value(value).map_err(|error| WebError::internal(error.to_string()))?;
        let paths = allowed_workspaces(&state);
        page.runs
            .retain(|run| paths.contains(&run.snapshot.workspace));
        serde_json::to_value(page)
            .map(Json)
            .map_err(|error| WebError::internal(error.to_string()))
    })
}

async fn allowed_run(
    state: &WebState,
    task_id: uuid::Uuid,
    run_id: uuid::Uuid,
) -> Result<serde_json::Value, WebError> {
    let params = serde_json::json!({"task_id":task_id,"run_id":run_id});
    let value = call(state, "schedule.run.get", params, None).await?;
    let run: willdeep_runtime_protocol::schedule::ScheduleRun =
        serde_json::from_value(value.clone()).map_err(|e| WebError::internal(e.to_string()))?;
    if !workspace_allowed(state, &run.snapshot.workspace).await? {
        return Err(WebError::not_found(
            "scheduled run not found in Web workspaces",
        ));
    }
    Ok(value)
}

async fn run_detail(
    State(state): State<Arc<WebState>>,
    Path((id, run_id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<Json<serde_json::Value>, WebError> {
    allowed_run(&state, id, run_id).await.map(Json)
}

#[derive(Deserialize)]
struct Stop {
    request_id: uuid::Uuid,
}
async fn stop(
    State(state): State<Arc<WebState>>,
    Path((id, run_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Json(input): Json<Stop>,
) -> Result<Json<serde_json::Value>, WebError> {
    writable(&state)?;
    allowed_run(&state, id, run_id).await?;
    call(
        &state,
        "schedule.run.stop",
        serde_json::to_value(ScheduleRunRef {
            task_id: id,
            run_id,
        })
        .map_err(|e| WebError::bad_request(e.to_string()))?,
        Some(input.request_id),
    )
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct Cleanup {
    expected_revision: u64,
    preview: bool,
    request_id: uuid::Uuid,
}
async fn cleanup(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(input): Json<Cleanup>,
) -> Result<Json<serde_json::Value>, WebError> {
    writable(&state)?;
    allowed_history(&state, id).await?;
    call(&state, "schedule.cleanup", serde_json::json!({"task_id":id,"expected_revision":input.expected_revision,"preview":input.preview}), Some(input.request_id)).await.map(Json)
}
