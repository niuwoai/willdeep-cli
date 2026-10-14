use super::*;
use willdeep_core::schedule::ScheduleStore;
use willdeep_core::schedule_history::ScheduleHistoryStore;
use willdeep_runtime_protocol::schedule::*;

pub(super) async fn dispatch(state: &ServerState, request: &ApiRequest) -> ApiResult {
    let now = now();
    super::super::schedule_engine::migrate(state, now).map_err(ApiFailure::internal)?;
    match request.operation.as_str() {
        "schedule.list" => list(state, now),
        "schedule.deleted" => {
            let active = ScheduleStore::new(&state.home)
                .list()
                .map_err(ApiFailure::internal)?;
            let retained = ScheduleHistoryStore::new(&state.home)
                .deleted_tasks()
                .map_err(ApiFailure::internal)?;
            json(
                retained
                    .into_iter()
                    .filter(|item| !active.iter().any(|task| task.id == item.task.id))
                    .collect::<Vec<_>>(),
            )
        }
        "schedule.get" => view(state, params::<IdParams>(request)?.id, now).and_then(json),
        "schedule.create" => create(state, params(request)?, now),
        "schedule.update" => update(state, params(request)?, now),
        "schedule.action" => action(state, params(request)?, now),
        "schedule.timezones" => json(willdeep_runtime_protocol::schedule::timezones()),
        "schedule.format_time" => format_local_time(&params::<ScheduleFormatTime>(request)?)
            .map_err(ApiFailure::invalid)
            .and_then(json),
        "schedule.preview" => preview(state, params(request)?),
        "schedule.resolve_time" => resolve_local_time(&params::<ScheduleLocalTime>(request)?)
            .map_err(ApiFailure::invalid)
            .and_then(json),
        "schedule.run" => run(state, params(request)?, request.request_id, now),
        "schedule.history" => {
            super::super::schedule_engine::refresh(state, now).map_err(ApiFailure::internal)?;
            ScheduleHistoryStore::new(&state.home)
                .list(&params::<ScheduleHistoryQuery>(request)?)
                .map_err(ApiFailure::internal)
                .and_then(json)
        }
        "schedule.run.get" => {
            let key: ScheduleRunRef = params(request)?;
            super::super::schedule_engine::refresh(state, now).map_err(ApiFailure::internal)?;
            history_run(state, &key).and_then(json)
        }
        "schedule.run.stop" => {
            let key: ScheduleRunRef = params(request)?;
            let mut run = history_run(state, &key)?;
            if run.status.is_terminal() {
                return Err(ApiFailure::conflict("scheduled run has already ended"));
            }
            if run.turn_id.is_none() {
                let history = ScheduleHistoryStore::new(&state.home);
                let _guard = history
                    .dispatch_lock(key.task_id, key.run_id)
                    .map_err(ApiFailure::internal)?;
                run = history_run(state, &key)?;
                if run.turn_id.is_none() {
                    run.status = ScheduleRunStatus::Cancelled;
                    run.finished_at = Some(now);
                    run.reason = Some("cancelled_before_enqueue".to_owned());
                    history.put(&run).map_err(ApiFailure::internal)?;
                    super::super::schedule_engine::refresh(state, now)
                        .map_err(ApiFailure::internal)?;
                    return json(run);
                }
            }
            let turn_id = run
                .turn_id
                .ok_or_else(|| ApiFailure::conflict("scheduled run has not been queued yet"))?;
            let stop = ApiRequest {
                params: serde_json::json!({ "id": turn_id }),
                ..request.clone()
            };
            turn_stop(state, &stop).await?;
            super::super::schedule_engine::refresh(state, now).map_err(ApiFailure::internal)?;
            history_run(state, &key).and_then(json)
        }
        "schedule.cleanup" => {
            let cleanup: ScheduleCleanup = params(request)?;
            let task = match task(state, cleanup.task_id) {
                Ok(task) => task,
                Err(_) => ScheduleHistoryStore::new(&state.home)
                    .deleted_tasks()
                    .map_err(ApiFailure::internal)?
                    .into_iter()
                    .find(|item| item.task.id == cleanup.task_id)
                    .map(|item| item.task)
                    .ok_or_else(|| ApiFailure::not_found("scheduled task metadata not found"))?,
            };
            check_revision(&task, cleanup.expected_revision)?;
            let ids = ScheduleHistoryStore::new(&state.home)
                .cleanup(task.id, now, task.settings.retention_days, cleanup.preview)
                .map_err(ApiFailure::internal)?;
            json(serde_json::json!({ "run_ids": ids, "preview": cleanup.preview }))
        }
        _ => Err(ApiFailure::invalid("unsupported scheduled task operation")),
    }
}

fn task(state: &ServerState, id: uuid::Uuid) -> Result<ScheduledTask, ApiFailure> {
    ScheduleStore::new(&state.home)
        .list()
        .map_err(ApiFailure::internal)?
        .into_iter()
        .find(|task| task.id == id)
        .ok_or_else(|| ApiFailure::not_found("scheduled task not found"))
}

fn check_revision(task: &ScheduledTask, expected: u64) -> Result<(), ApiFailure> {
    if task.settings.revision != expected {
        return Err(ApiFailure::conflict(
            "scheduled task changed; refresh before saving",
        ));
    }
    Ok(())
}

fn view(state: &ServerState, id: uuid::Uuid, now: u64) -> Result<ScheduledTaskView, ApiFailure> {
    let task = task(state, id)?;
    let next_fire_at = task.next_fire(willdeep_core::schedule::local_utc_offset(now));
    let last_run = ScheduleHistoryStore::new(&state.home)
        .list(&ScheduleHistoryQuery {
            task_id: id,
            before: None,
            limit: Some(1),
            status: None,
        })
        .map_err(ApiFailure::internal)?
        .runs
        .into_iter()
        .next();
    Ok(ScheduledTaskView {
        task,
        next_fire_at,
        last_run,
        active_run: ScheduleHistoryStore::new(&state.home)
            .active(id)
            .map_err(ApiFailure::internal)?
            .into_iter()
            .next(),
    })
}

fn list(state: &ServerState, now: u64) -> ApiResult {
    super::super::schedule_engine::refresh(state, now).map_err(ApiFailure::internal)?;
    let tasks = ScheduleStore::new(&state.home)
        .list()
        .map_err(ApiFailure::internal)?;
    let views = tasks
        .into_iter()
        .map(|task| view(state, task.id, now))
        .collect::<Result<Vec<_>, _>>()?;
    json(views)
}

fn validate(draft: &ScheduleDraft, _now: u64) -> Result<(), ApiFailure> {
    if let Some(path) = &draft.settings.config_path {
        if !path.is_absolute() {
            return Err(ApiFailure::invalid("schedule config_path must be absolute"));
        }
        crate::config::LoadedConfig::load(Some(path)).map_err(|error| {
            ApiFailure::invalid(willdeep_core::judge::redact_credentials(&error.to_string()))
        })?;
    }
    draft.validate().map_err(ApiFailure::invalid)?;
    if !draft.workspace.is_dir() {
        return Err(ApiFailure::invalid("schedule workspace does not exist"));
    }
    if let Some(mode) = draft.approval_mode.as_deref()
        && willdeep_core::ApprovalMode::parse(mode).is_none()
    {
        return Err(ApiFailure::invalid("invalid schedule approval mode"));
    }
    if draft
        .settings
        .max_turns
        .is_some_and(|value| value as usize > crate::config::MAX_TURNS_CEILING)
        || draft.settings.token_budget.is_some_and(|value| {
            !(crate::config::TOKEN_BUDGET_MIN..=crate::config::TOKEN_BUDGET_MAX).contains(&value)
        })
        || draft
            .settings
            .timeout_seconds
            .is_some_and(|value| value > 86_400)
    {
        return Err(ApiFailure::invalid(
            "schedule run budget exceeds Runtime limits",
        ));
    }
    Ok(())
}

fn create(state: &ServerState, mut draft: ScheduleDraft, now: u64) -> ApiResult {
    validate(&draft, now)?;
    if matches!(draft.schedule, Schedule::OnceAt { at } if at <= now) {
        return Err(ApiFailure::invalid(
            "one-time schedule must be in the future",
        ));
    }
    draft.workspace = draft
        .workspace
        .canonicalize()
        .map_err(|error| ApiFailure::invalid(error.to_string()))?;
    validate_session_binding(state, &draft)?;
    let task = draft.into_task(uuid::Uuid::new_v4(), now);
    let id = task.id;
    ScheduleStore::new(&state.home)
        .update(|tasks| tasks.push(task))
        .map_err(ApiFailure::internal)?;
    state
        .events
        .append("schedule.created", format!("task_id={id}"))
        .map_err(ApiFailure::internal)?;
    view(state, id, now).and_then(json)
}

fn update(state: &ServerState, mut update: ScheduleUpdate, now: u64) -> ApiResult {
    validate(&update.task, now)?;
    update.task.workspace = update
        .task
        .workspace
        .canonicalize()
        .map_err(|error| ApiFailure::invalid(error.to_string()))?;
    let previous = task(state, update.id)?;
    update.task.origin_session_id = previous.origin_session_id;
    validate_session_binding(state, &update.task)?;
    let id = update.id;
    ScheduleStore::new(&state.home)
        .try_update(|tasks| {
            let old = tasks
                .iter_mut()
                .find(|task| task.id == id)
                .ok_or("scheduled task not found")?;
            if old.settings.revision != update.expected_revision {
                return Err("scheduled task changed; refresh before saving".to_owned());
            }
            if matches!(update.task.schedule, Schedule::OnceAt { at } if at <= now)
                && (old.enabled || update.task.schedule != old.schedule)
            {
                return Err("one-time schedule must be in the future".to_owned());
            }
            let mut replacement = update.task.into_task(id, old.created_at);
            replacement.enabled = old.enabled;
            replacement.last_run_at = old.last_run_at;
            replacement.last_session_id = old.last_session_id;
            replacement.origin_session_id = old.origin_session_id;
            replacement.settings.revision = old.settings.revision + 1;
            replacement.settings.updated_at = now;
            replacement.settings.archived_at = old.settings.archived_at;
            replacement.settings.completed_at = old.settings.completed_at;
            replacement.settings.consecutive_failures = 0;
            replacement.settings.migration_reason = old.settings.migration_reason.clone();
            *old = replacement;
            Ok(())
        })
        .map_err(ApiFailure::conflict)?;
    state
        .events
        .append("schedule.updated", format!("task_id={id}"))
        .map_err(ApiFailure::internal)?;
    view(state, id, now).and_then(json)
}

fn validate_session_binding(state: &ServerState, draft: &ScheduleDraft) -> Result<(), ApiFailure> {
    let Some(id) = draft.origin_session_id else {
        return Ok(());
    };
    let session = state
        .sessions
        .get(id)
        .map_err(ApiFailure::internal)?
        .ok_or_else(|| ApiFailure::invalid("source session not found"))?;
    if session.workspace != draft.workspace {
        return Err(ApiFailure::invalid(
            "schedule workspace must match its source session",
        ));
    }
    Ok(())
}

fn action(state: &ServerState, action: ScheduleAction, now: u64) -> ApiResult {
    let current = task(state, action.id)?;
    check_revision(&current, action.expected_revision)?;
    if action.action == "complete"
        && ScheduleHistoryStore::new(&state.home)
            .active(action.id)
            .map_err(ApiFailure::internal)?
            .iter()
            .any(|run| run.snapshot.settings.revision != action.expected_revision)
    {
        return Err(ApiFailure::conflict(
            "an older configuration is still running; archive explicitly or stop that run before completing the edited task",
        ));
    }
    if action.action == "delete"
        && !ScheduleHistoryStore::new(&state.home)
            .active(action.id)
            .map_err(ApiFailure::internal)?
            .is_empty()
    {
        return Err(ApiFailure::conflict(
            "stop active runs before permanently deleting a task",
        ));
    }
    ScheduleStore::new(&state.home)
        .try_update(|tasks| {
            let position = tasks
                .iter()
                .position(|task| task.id == action.id)
                .ok_or("scheduled task not found")?;
            let task = &mut tasks[position];
            if task.settings.revision != action.expected_revision {
                return Err("scheduled task changed; refresh before saving".to_owned());
            }
            match action.action.as_str() {
                "pause" => task.enabled = false,
                "resume" => {
                    if matches!(task.schedule, Schedule::OnceAt { at } if at <= now) {
                        return Err("set a future time before resuming a one-time task".to_owned());
                    }
                    task.enabled = true;
                    task.settings.archived_at = None;
                    task.settings.completed_at = None;
                    task.settings.consecutive_failures = 0;
                    task.settings.migration_reason = None;
                    task.last_run_at = if matches!(task.schedule, Schedule::OnceAt { .. }) { None } else { Some(now) };
                }
                "archive" => {
                    task.enabled = false;
                    task.settings.archived_at = Some(now);
                }
                "complete" => {
                    if ScheduleHistoryStore::new(&state.home).active(action.id)?.iter().any(|run| run.snapshot.settings.revision != action.expected_revision) {
                        return Err("an older configuration is still running; archive or stop it before completing the edited task".to_owned());
                    }
                    task.enabled = false;
                    task.settings.completed_at = Some(now);
                }
                "delete" => {
                    if !ScheduleHistoryStore::new(&state.home)
                        .active(action.id)?
                        .is_empty()
                    {
                        return Err(
                            "stop active runs before permanently deleting a task".to_owned()
                        );
                    }
                    ScheduleHistoryStore::new(&state.home).retain_deleted_task(task, now)?;
                    tasks.remove(position);
                    return Ok(());
                }
                _ => return Err("unsupported schedule action".to_owned()),
            }
            task.settings.revision += 1;
            task.settings.updated_at = now;
            Ok(())
        })
        .map_err(ApiFailure::conflict)?;
    state
        .events
        .append(
            "schedule.changed",
            format!("task_id={} action={}", action.id, action.action),
        )
        .map_err(ApiFailure::internal)?;
    if action.action == "delete" {
        json(serde_json::json!({ "deleted": action.id, "history_retained": true }))
    } else {
        view(state, action.id, now).and_then(json)
    }
}

fn run(
    state: &ServerState,
    params: ScheduleRunRequest,
    request_id: uuid::Uuid,
    now: u64,
) -> ApiResult {
    if let Some(retry_of) = params.retry_of {
        let original = history_run(
            state,
            &ScheduleRunRef {
                task_id: params.id,
                run_id: retry_of,
            },
        )?;
        if !matches!(
            original.status,
            ScheduleRunStatus::Failed
                | ScheduleRunStatus::Partial
                | ScheduleRunStatus::Cancelled
                | ScheduleRunStatus::Unknown
        ) {
            return Err(ApiFailure::conflict(
                "only an ended unsuccessful run can be retried",
            ));
        }
    }
    let trigger = if params.retry_of.is_some() {
        ScheduleTrigger::Retry
    } else {
        ScheduleTrigger::Manual
    };
    let key = format!("manual:{}:{request_id}", params.id);
    super::super::schedule_engine::trigger(
        state,
        params.id,
        key,
        trigger,
        now,
        now,
        params.retry_of,
    )
    .map_err(ApiFailure::internal)
    .and_then(json)
}

fn history_run(state: &ServerState, key: &ScheduleRunRef) -> Result<ScheduleRun, ApiFailure> {
    ScheduleHistoryStore::new(&state.home)
        .get(key.task_id, key.run_id)
        .map_err(ApiFailure::internal)?
        .ok_or_else(|| ApiFailure::not_found("scheduled run not found"))
}

fn preview(state: &ServerState, params: SchedulePreview) -> ApiResult {
    params.schedule.validate().map_err(ApiFailure::invalid)?;
    ScheduleSettings {
        timezone: params.timezone.clone(),
        ..Default::default()
    }
    .validate()
    .map_err(ApiFailure::invalid)?;
    let mut times = Vec::new();
    let mut after = params.after;
    if let Some(id) = params.task_id
        && matches!(params.schedule, Schedule::Interval { .. })
    {
        let stored = task(state, id)?;
        if let Some(next) = params
            .schedule
            .next_fire(stored.last_run_at.unwrap_or(stored.created_at), 0)
        {
            after = next.max(after);
            times.push(after);
        }
    }
    for _ in times.len()..params.count.unwrap_or(5).clamp(1, 20) {
        let Some(next) = params
            .schedule
            .next_fire_in_timezone(after, &params.timezone)
        else {
            break;
        };
        times.push(next);
        after = next;
    }
    json(times)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        state: ServerState,
        draft: serde_json::Value,
        _scheduler: tokio::sync::mpsc::UnboundedReceiver<uuid::Uuid>,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("runtime-schedule-api-{}", uuid::Uuid::new_v4()));
            let workspace = root.join("workspace");
            std::fs::create_dir_all(&workspace).unwrap();
            let sessions = crate::daemon::tests::test_runtime_session_store(&root);
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let state =
                crate::daemon::tests::test_server_state_with_scheduler(&root, sessions, sender);
            let draft = serde_json::json!({ "name":"watch", "prompt":"Check the project", "workspace":workspace,
                "schedule":{ "kind":"interval", "minutes":5 }, "timezone":"Asia/Shanghai" });
            Self {
                root,
                state,
                draft,
                _scheduler: receiver,
            }
        }
        async fn call(&self, operation: &str, value: serde_json::Value) -> serde_json::Value {
            super::super::execute(&self.state, ApiRequest::new(operation, value))
                .await
                .body
                .into_result()
                .unwrap()
        }
        async fn create(&self) -> uuid::Uuid {
            let value = self.call("schedule.create", self.draft.clone()).await;
            serde_json::from_value(value["task"]["id"].clone()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn conversation_task_continues_its_owner_without_changing_permissions_or_title() {
        let fixture = Fixture::new();
        let workspace: PathBuf =
            serde_json::from_value(fixture.draft["workspace"].clone()).unwrap();
        let (session, _) = fixture
            .state
            .sessions
            .ensure(session_store::CreateRuntimeSession {
                id: None,
                workspace,
                title: Some("Original chat".into()),
                profile: None,
                model: None,
                config: None,
            })
            .unwrap();
        fixture
            .state
            .sessions
            .update_approval_mode(session.id, workspace_store::WorkspaceAccess::Strict)
            .unwrap();
        let mut draft = fixture.draft.clone();
        draft["origin_session_id"] = serde_json::json!(session.id);
        draft["continue_in_session"] = true.into();
        draft["approval_mode"] = "full-access".into();
        let created = fixture.call("schedule.create", draft.clone()).await;
        let id = &created["task"]["id"];
        let first = fixture
            .call("schedule.run", serde_json::json!({"id":id}))
            .await;
        assert_eq!(first["session_id"], serde_json::json!(session.id));
        assert_eq!(first["snapshot"]["approval_mode"], "strict");
        assert_eq!(fixture.state.sessions.list().unwrap().len(), 1);
        assert_eq!(
            fixture
                .state
                .sessions
                .get(session.id)
                .unwrap()
                .unwrap()
                .approval_mode,
            Some(workspace_store::WorkspaceAccess::Strict)
        );
        assert_eq!(
            willdeep_core::SessionStore::new(&fixture.root)
                .load(session.id)
                .unwrap()
                .title,
            "Original chat"
        );
        let other = fixture.call("schedule.create", draft).await;
        let skipped = fixture
            .call(
                "schedule.run",
                serde_json::json!({"id":other["task"]["id"]}),
            )
            .await;
        assert_eq!(skipped["status"], "skipped");
        assert_eq!(skipped["reason"], "source_session_busy");
        assert_eq!(
            fixture.state.sessions.list_turns(session.id).unwrap().len(),
            1
        );
        fixture
            .call(
                "schedule.action",
                serde_json::json!({"id":id,"action":"pause","expected_revision":1}),
            )
            .await;
        let paused = fixture
            .call("schedule.get", serde_json::json!({"id":id}))
            .await;
        assert_eq!(paused["task"]["enabled"], false);
        assert!(!paused["active_run"].is_null());
        assert_eq!(
            fixture
                .call("schedule.history", serde_json::json!({"task_id":id}))
                .await["runs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn rejects_missing_or_cross_workspace_conversation_binding() {
        let fixture = Fixture::new();
        let mut draft = fixture.draft.clone();
        draft["continue_in_session"] = true.into();
        let invalid = super::super::execute(
            &fixture.state,
            ApiRequest::new("schedule.create", draft.clone()),
        )
        .await;
        assert!(invalid.body.into_result().is_err());
        draft["origin_session_id"] = serde_json::json!(uuid::Uuid::new_v4());
        let invalid =
            super::super::execute(&fixture.state, ApiRequest::new("schedule.create", draft)).await;
        assert!(invalid.body.into_result().is_err());
    }

    #[tokio::test]
    async fn native_task_runs_without_enabling_the_plugin_and_replay_does_not_enqueue_twice() {
        let fixture = Fixture::new();
        let id = fixture.create().await;
        let request = ApiRequest::new("schedule.run", serde_json::json!({ "id": id }));
        let first = super::super::execute(&fixture.state, request.clone())
            .await
            .body
            .into_result()
            .unwrap();
        let again = super::super::execute(&fixture.state, request)
            .await
            .body
            .into_result()
            .unwrap();
        assert_eq!(first["id"], again["id"]);
        let session: uuid::Uuid = serde_json::from_value(first["session_id"].clone()).unwrap();
        assert_eq!(fixture.state.sessions.list_turns(session).unwrap().len(), 1);
        assert_eq!(first["status"], "queued");
        let history = fixture
            .call("schedule.history", serde_json::json!({"task_id":id}))
            .await;
        assert_eq!(history["runs"].as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn editing_does_not_rewrite_history_and_a_retry_uses_the_original_snapshot() {
        let fixture = Fixture::new();
        let id = fixture.create().await;
        let first = fixture
            .call("schedule.run", serde_json::json!({"id":id}))
            .await;
        let turn_id = serde_json::from_value(first["turn_id"].clone()).unwrap();
        fixture
            .state
            .sessions
            .complete_claim_failure(turn_id, "fixture failure".to_owned())
            .unwrap();
        fixture
            .call("schedule.history", serde_json::json!({"task_id":id}))
            .await;
        let mut updated = fixture.draft.clone();
        updated["prompt"] = "Different project instruction".into();
        fixture
            .call(
                "schedule.update",
                serde_json::json!({"id":id, "expected_revision":1, "task":updated}),
            )
            .await;
        let retry = fixture
            .call(
                "schedule.run",
                serde_json::json!({"id":id,"retry_of":first["id"]}),
            )
            .await;
        assert_ne!(retry["id"], first["id"]);
        assert_eq!(retry["snapshot"]["prompt"], "Check the project");
        let page = fixture
            .call("schedule.history", serde_json::json!({"task_id":id}))
            .await;
        assert_eq!(page["runs"].as_array().unwrap().len(), 2);
        assert!(
            page["runs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|run| run["status"] == "failed")
        );
        let stale = super::super::execute(
            &fixture.state,
            ApiRequest::new(
                "schedule.update",
                serde_json::json!({"id":id,"expected_revision":1,"task":fixture.draft}),
            ),
        )
        .await;
        assert_eq!(stale.status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn archive_does_not_stop_the_current_run_and_stop_is_explicit() {
        let fixture = Fixture::new();
        let id = fixture.create().await;
        let run = fixture
            .call("schedule.run", serde_json::json!({"id":id}))
            .await;
        fixture
            .call(
                "schedule.action",
                serde_json::json!({"id":id,"expected_revision":1,"action":"archive"}),
            )
            .await;
        let page = fixture
            .call("schedule.history", serde_json::json!({"task_id":id}))
            .await;
        assert_eq!(page["runs"][0]["status"], "queued");
        let stopped = fixture
            .call(
                "schedule.run.stop",
                serde_json::json!({"task_id":id,"run_id":run["id"]}),
            )
            .await;
        assert_eq!(stopped["status"], "cancelled");
        assert!(
            fixture
                .call("schedule.get", serde_json::json!({"id":id}))
                .await["task"]["archived_at"]
                .is_number()
        );
    }

    #[tokio::test]
    async fn old_disabled_plugin_imports_paused_and_preserves_unknown_history() {
        let fixture = Fixture::new();
        let draft: ScheduleDraft = serde_json::from_value(fixture.draft.clone()).unwrap();
        let mut old = draft.into_task(uuid::Uuid::new_v4(), 100);
        old.settings.managed = false;
        old.last_session_id = Some(uuid::Uuid::new_v4());
        let id = old.id;
        ScheduleStore::new(&fixture.root)
            .update(|tasks| tasks.push(old))
            .unwrap();
        let view = fixture
            .call("schedule.get", serde_json::json!({"id":id}))
            .await;
        assert_eq!(view["task"]["enabled"], false);
        assert_eq!(view["task"]["migration_reason"], "legacy_plugin_disabled");
        assert_eq!(view["last_run"]["status"], "unknown");
        fixture
            .call(
                "schedule.action",
                serde_json::json!({"id":id,"expected_revision":1,"action":"resume"}),
            )
            .await;
        let resumed = fixture
            .call("schedule.get", serde_json::json!({"id":id}))
            .await;
        assert_eq!(resumed["task"]["enabled"], true);
        assert!(resumed["task"]["migration_reason"].is_null());
    }

    #[tokio::test]
    async fn drain_blocks_new_runs_but_keeps_configuration_and_history_available() {
        let fixture = Fixture::new();
        let id = fixture.create().await;
        *fixture.state.work_gate.write().await = true;
        let blocked = super::super::execute(
            &fixture.state,
            ApiRequest::new("schedule.run", serde_json::json!({"id":id})),
        )
        .await;
        assert_eq!(blocked.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            fixture
                .call("schedule.list", serde_json::json!({}))
                .await
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn global_capacity_defers_a_durable_run_and_stop_cancels_it_before_enqueue() {
        let fixture = Fixture::new();
        let mut last = serde_json::Value::Null;
        for _ in 0..5 {
            let id = fixture.create().await;
            last = fixture
                .call("schedule.run", serde_json::json!({"id":id}))
                .await;
        }
        assert_eq!(last["status"], "preparing");
        assert_eq!(last["reason"], "runtime_capacity_wait");
        assert!(last["turn_id"].is_null());
        let stopped = fixture
            .call(
                "schedule.run.stop",
                serde_json::json!({"task_id":last["task_id"],"run_id":last["id"]}),
            )
            .await;
        assert_eq!(stopped["status"], "cancelled");
        super::super::super::schedule_engine::reconcile(&fixture.state, now()).unwrap();
        let run = fixture
            .call(
                "schedule.run.get",
                serde_json::json!({"task_id":last["task_id"],"run_id":last["id"]}),
            )
            .await;
        assert_eq!(run["status"], "cancelled");
        assert!(run["turn_id"].is_null());
    }

    #[tokio::test]
    async fn failures_pause_schedule_and_refresh_does_not_double_count() {
        let fixture = Fixture::new();
        let id = fixture.create().await;
        for _ in 0..3 {
            let run = fixture
                .call("schedule.run", serde_json::json!({"id":id}))
                .await;
            let turn = serde_json::from_value(run["turn_id"].clone()).unwrap();
            fixture
                .state
                .sessions
                .complete_claim_failure(turn, "provider failure".to_owned())
                .unwrap();
            fixture
                .call("schedule.history", serde_json::json!({"task_id":id}))
                .await;
        }
        for _ in 0..2 {
            let view = fixture
                .call("schedule.get", serde_json::json!({"id":id}))
                .await;
            assert_eq!(view["task"]["consecutive_failures"], 3);
            assert_eq!(view["task"]["enabled"], false);
            assert_eq!(view["task"]["migration_reason"], "consecutive_failures");
        }
    }

    #[tokio::test]
    async fn ended_once_task_can_be_renamed_but_requires_future_time_to_resume() {
        let mut fixture = Fixture::new();
        fixture.draft["schedule"] = serde_json::json!({"kind":"onceAt","at":now()+3600});
        let id = fixture.create().await;
        ScheduleStore::new(&fixture.state.home)
            .update(|tasks| {
                let task = tasks.iter_mut().find(|task| task.id == id).unwrap();
                task.enabled = false;
                task.schedule = Schedule::OnceAt { at: now() - 60 };
            })
            .unwrap();
        fixture.draft["schedule"] = serde_json::json!({"kind":"onceAt","at":now()-60});
        fixture.draft["name"] = serde_json::json!("renamed ended task");
        let updated = fixture
            .call(
                "schedule.update",
                serde_json::json!({"id":id,"expected_revision":1,"task":fixture.draft}),
            )
            .await;
        assert_eq!(updated["task"]["name"], "renamed ended task");
        let result = super::super::execute(
            &fixture.state,
            ApiRequest::new(
                "schedule.action",
                serde_json::json!({"id":id,"expected_revision":2,"action":"resume"}),
            ),
        )
        .await
        .body
        .into_result();
        assert!(result.is_err());
        assert!(!ScheduleStore::new(&fixture.state.home).list().unwrap()[0].enabled);
    }

    #[tokio::test]
    async fn failed_one_time_run_stops_future_triggers_without_claiming_success() {
        let mut fixture = Fixture::new();
        fixture.draft["schedule"] = serde_json::json!({"kind":"onceAt","at":now()+3600});
        let id = fixture.create().await;
        let run = fixture
            .call("schedule.run", serde_json::json!({"id":id}))
            .await;
        fixture
            .state
            .sessions
            .complete_claim_failure(
                serde_json::from_value(run["turn_id"].clone()).unwrap(),
                "fixture failure".to_owned(),
            )
            .unwrap();
        fixture
            .call("schedule.history", serde_json::json!({"task_id":id}))
            .await;
        let view = fixture
            .call("schedule.get", serde_json::json!({"id":id}))
            .await;
        assert_eq!(view["task"]["enabled"], false);
        assert!(view["task"]["completed_at"].is_null());
        assert!(view["next_fire_at"].is_null());
    }

    #[tokio::test]
    async fn lost_enqueue_reply_recovers_original_turn_without_replay() {
        let fixture = Fixture::new();
        let id = fixture.create().await;
        let value = fixture
            .call("schedule.run", serde_json::json!({"id":id}))
            .await;
        let mut run: ScheduleRun = serde_json::from_value(value).unwrap();
        let session = run.session_id.unwrap();
        let turn = run.turn_id;
        run.status = ScheduleRunStatus::Preparing;
        run.session_id = None;
        run.turn_id = None;
        ScheduleHistoryStore::new(&fixture.root).put(&run).unwrap();
        super::super::super::schedule_engine::reconcile(&fixture.state, now()).unwrap();
        let recovered = fixture
            .call(
                "schedule.run.get",
                serde_json::json!({"task_id":id,"run_id":run.id}),
            )
            .await;
        assert_eq!(recovered["turn_id"], serde_json::json!(turn));
        assert_eq!(fixture.state.sessions.list_turns(session).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn deleted_task_remains_discoverable_and_its_history_can_be_maintained() {
        let fixture = Fixture::new();
        let id = fixture.create().await;
        let run = fixture
            .call("schedule.run", serde_json::json!({"id":id}))
            .await;
        fixture
            .call(
                "schedule.run.stop",
                serde_json::json!({"task_id":id,"run_id":run["id"]}),
            )
            .await;
        fixture
            .call(
                "schedule.action",
                serde_json::json!({"id":id,"expected_revision":1,"action":"delete"}),
            )
            .await;
        let deleted = fixture
            .call("schedule.deleted", serde_json::json!({}))
            .await;
        assert_eq!(deleted[0]["task"]["id"], serde_json::json!(id));
        assert_eq!(
            fixture
                .call("schedule.history", serde_json::json!({"task_id":id}))
                .await["runs"][0]["status"],
            "cancelled"
        );
        let cleanup = fixture
            .call(
                "schedule.cleanup",
                serde_json::json!({"task_id":id,"expected_revision":1,"preview":true}),
            )
            .await;
        assert!(cleanup["run_ids"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn attachment_reference_survives_reordering_and_rejects_a_removed_source() {
        let fixture = Fixture::new();
        let mut session =
            willdeep_core::Session::new(fixture.root.join("workspace"), None, "attachments");
        for content in ["first", "second"] {
            session
                .messages
                .push(willdeep_core::Message::user_with_attachments(
                    content,
                    vec![willdeep_core::MessageAttachment::Text {
                        name: "same.txt".to_owned(),
                        content: content.to_owned(),
                    }],
                ));
        }
        let reference =
            willdeep_core::conversation::attachment_reference(&session.messages[0].attachments)
                .unwrap();
        let store = willdeep_core::SessionStore::new(&fixture.root);
        store.save(&mut session).unwrap();
        session.messages.swap(0, 1);
        store.save(&mut session).unwrap();
        let read = fixture
            .call(
                "session.attachments",
                serde_json::json!({"id":session.id,"reference":reference}),
            )
            .await;
        assert_eq!(read["attachments"][0]["content"], "first");
        session.messages.pop();
        store.save(&mut session).unwrap();
        let response = super::super::execute(
            &fixture.state,
            ApiRequest::new(
                "session.attachments",
                serde_json::json!({"id":session.id,"reference":reference}),
            ),
        )
        .await;
        assert_eq!(response.status, StatusCode::NOT_FOUND);
    }
}
