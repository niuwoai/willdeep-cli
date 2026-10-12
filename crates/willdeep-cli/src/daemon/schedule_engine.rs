//! Runtime owns every schedule and every run; clients never write these files.
use super::*;
use willdeep_core::schedule::{ScheduleStore, ScheduledTask};
use willdeep_core::schedule_history::ScheduleHistoryStore;
use willdeep_runtime_protocol::schedule::{
    MissedRunPolicy, Schedule, ScheduleRun, ScheduleRunStatus, ScheduleRunUsage, ScheduleTrigger,
};

const MISSED_GRACE_SECONDS: u64 = 60;
const RESULT_PREVIEW_CHARS: usize = 2000;
const DEFAULT_MAX_ACTIVE: usize = 4;
static ADMISSION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(super) fn record_output(
    home: &Path,
    origin: Option<&str>,
    text: &str,
    agents: &AgentStore,
    runtime_task_id: uuid::Uuid,
    agent_id: Option<uuid::Uuid>,
) -> Result<()> {
    let Some(origin) = origin else {
        return Ok(());
    };
    let parts = origin.split(':').collect::<Vec<_>>();
    if parts.first() != Some(&"schedule") || parts.len() != 3 {
        return Ok(());
    }
    let task_id = parts[1]
        .parse()
        .context("invalid schedule task reference")?;
    let run_id = parts[2].parse().context("invalid schedule run reference")?;
    let history = ScheduleHistoryStore::new(home);
    let mut run = history
        .get(task_id, run_id)
        .map_err(anyhow::Error::msg)?
        .context("schedule run record missing")?;
    run.result = Some(
        willdeep_core::judge::redact_credentials(text)
            .chars()
            .take(RESULT_PREVIEW_CHARS)
            .collect(),
    );
    if let Some(agent_id) = agent_id
        && let Some(agent) = agents.get(agent_id)?
        && agent.task_id == runtime_task_id
    {
        if let (Some(input_tokens), Some(output_tokens), Some(total_tokens)) =
            (agent.input_tokens, agent.output_tokens, agent.total_tokens)
        {
            run.usage = Some(ScheduleRunUsage {
                input_tokens,
                output_tokens,
                total_tokens,
                model_calls: None,
            });
        }
        run.verification = agent
            .verifier_passed
            .map(|passed| if passed { "passed" } else { "failed" }.to_owned());
    }
    history.put(&run).map_err(anyhow::Error::msg)
}

pub(super) fn policy_for_origin(
    home: &Path,
    origin: Option<&str>,
) -> Result<Option<willdeep_runtime_protocol::schedule::ScheduleSettings>> {
    let Some(origin) = origin else {
        return Ok(None);
    };
    let parts = origin.split(':').collect::<Vec<_>>();
    if parts.first() != Some(&"schedule") || parts.len() != 3 {
        return Ok(None);
    }
    let task_id = parts[1]
        .parse()
        .context("invalid schedule task reference")?;
    let run_id = parts[2].parse().context("invalid schedule run reference")?;
    let run = ScheduleHistoryStore::new(home)
        .get(task_id, run_id)
        .map_err(anyhow::Error::msg)?
        .context("schedule run record missing")?;
    run.snapshot
        .settings
        .validate()
        .map_err(anyhow::Error::msg)?;
    Ok(Some(run.snapshot.settings))
}

pub(super) fn migrate(state: &ServerState, now: u64) -> Result<()> {
    let store = ScheduleStore::new(&state.home);
    if store
        .list()
        .map_err(anyhow::Error::msg)?
        .iter()
        .all(|task| task.settings.managed)
    {
        return Ok(());
    }
    let enabled = willdeep_core::plugin::registry::PluginRegistry::load(
        &willdeep_core::plugin::registry::PluginRegistry::default_path(&state.home),
    )?
    .is_enabled(super::scheduler::SCHEDULER_PLUGIN_ID);
    let history = ScheduleHistoryStore::new(&state.home);
    store
        .try_update(|tasks| {
            for task in tasks.iter_mut().filter(|task| !task.settings.managed) {
                task.settings.managed = true;
                task.settings.updated_at = now;
                if !enabled && task.enabled {
                    task.enabled = false;
                    task.settings.migration_reason = Some("legacy_plugin_disabled".to_owned());
                }
                if let Some(session_id) = task.last_session_id {
                    let key = format!("import:{}:{session_id}", task.id);
                    let mut run = new_run(
                        task.clone(),
                        key,
                        ScheduleTrigger::Imported,
                        task.last_run_at.unwrap_or(task.created_at),
                    );
                    run.session_id = Some(session_id);
                    run.status = ScheduleRunStatus::Unknown;
                    run.finished_at = Some(now);
                    run.reason = Some("legacy_history_unavailable".to_owned());
                    history.insert(&run)?;
                }
            }
            Ok(())
        })
        .map_err(anyhow::Error::msg)
}

pub(super) fn tick(state: &ServerState, now: u64, offset: i64) -> Result<usize> {
    migrate(state, now)?;
    reconcile(state, now)?;
    let Some(gate) = state.work_gate.try_read().ok().filter(|gate| !**gate) else {
        return Ok(0);
    };
    let tasks = ScheduleStore::new(&state.home)
        .list()
        .map_err(anyhow::Error::msg)?;
    let mut fired = 0;
    for task in tasks {
        let Some(due) = task.next_fire(offset).filter(|due| *due <= now) else {
            continue;
        };
        let key = format!("scheduled:{}:{}:{due}", task.id, task.settings.revision);
        let run = trigger(
            state,
            task.id,
            key,
            ScheduleTrigger::Scheduled,
            due,
            now,
            None,
        )?;
        if run.status != ScheduleRunStatus::Skipped && run.status != ScheduleRunStatus::Failed {
            fired += 1;
        }
    }
    drop(gate);
    Ok(fired)
}

pub(super) fn trigger(
    state: &ServerState,
    task_id: uuid::Uuid,
    key: String,
    trigger: ScheduleTrigger,
    due: u64,
    now: u64,
    retry_of: Option<uuid::Uuid>,
) -> Result<ScheduleRun> {
    refresh(state, now)?;
    let store = ScheduleStore::new(&state.home);
    let history = ScheduleHistoryStore::new(&state.home);
    let run = store
        .try_update(|tasks| {
            let task = tasks
                .iter_mut()
                .find(|task| task.id == task_id)
                .ok_or("scheduled task not found")?;
            let run_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, key.as_bytes());
            if let Some(existing) = history.get(task_id, run_id)? {
                return Ok(existing);
            }
            if task.settings.archived_at.is_some() || task.settings.completed_at.is_some() {
                return Err("archived or completed tasks must be resumed before running".to_owned());
            }
            let (snapshot, configuration_error) = if let Some(original) = retry_of {
                (
                    history
                        .get(task_id, original)?
                        .ok_or("original scheduled run not found")?
                        .snapshot,
                    None,
                )
            } else {
                match freeze_configuration(state, task.clone()) {
                    Ok(snapshot) => (snapshot, None),
                    Err(error) => (
                        task.clone(),
                        Some(willdeep_core::judge::redact_credentials(&format!(
                            "configuration_unavailable: {error:#}"
                        ))),
                    ),
                }
            };
            let mut run = new_run(snapshot, key.clone(), trigger, due);
            run.retry_of = retry_of;
            let active = !history.active(task_id)?.is_empty();
            let missed = trigger == ScheduleTrigger::Scheduled
                && task.settings.missed_policy == MissedRunPolicy::Skip
                && now.saturating_sub(due) > MISSED_GRACE_SECONDS;
            if active || missed {
                run.status = ScheduleRunStatus::Skipped;
                run.reason = Some(
                    if active {
                        "previous_run_active"
                    } else {
                        "missed_run_policy"
                    }
                    .to_owned(),
                );
                run.finished_at = Some(now);
            } else if let Some(error) = configuration_error {
                run.status = ScheduleRunStatus::Failed;
                run.reason = Some(error);
                run.finished_at = Some(now);
            }
            let run = history.insert(&run)?;
            if trigger == ScheduleTrigger::Scheduled {
                task.last_run_at = Some(now);
            }
            task.settings.updated_at = now;
            Ok(run)
        })
        .map_err(anyhow::Error::msg)?;
    if run.status == ScheduleRunStatus::Skipped {
        state.events.append(
            "schedule.skipped",
            format!(
                "task_id={} run_id={} reason={}",
                run.task_id,
                run.id,
                run.reason.as_deref().unwrap_or("unknown")
            ),
        )?;
    }
    if run.status == ScheduleRunStatus::Failed {
        apply_outcome(state, &run, now)?;
    }
    if run.status == ScheduleRunStatus::Preparing {
        dispatch_run(state, run, now)
    } else {
        Ok(run)
    }
}

fn freeze_configuration(state: &ServerState, mut task: ScheduledTask) -> Result<ScheduledTask> {
    use clap::Parser;
    let requested_turns = task.settings.max_turns;
    let requested_tokens = task.settings.token_budget;
    let workspace = state.workspaces.ensure_registered(&task.workspace)?;
    task.settings.profile = task.settings.profile.or(workspace.provider_profile);
    if task.approval_mode.is_none() {
        task.approval_mode = Some(workspace.access.wire_name().replace('_', "-"));
    }
    let path = task
        .settings
        .config_path
        .clone()
        .unwrap_or_else(|| state.home.join("config.toml"));
    if path.exists() {
        let loaded = crate::config::LoadedConfig::load(Some(&path))?;
        let mut cli = crate::Cli::try_parse_from(["willdeep"])?;
        cli.profile = task
            .settings
            .profile
            .clone()
            .or(loaded.file.default_provider.clone());
        cli.model = task.settings.model.clone().or(cli.model);
        match crate::harness::resolve_parent_provider_config(&cli, &loaded, None) {
            Ok(provider) => {
                task.settings.profile = cli.profile;
                task.settings.model = Some(provider.model);
            }
            Err(error) => state
                .events
                .append(
                    "schedule.warning",
                    format!(
                        "task_id={} reason={}",
                        task.id,
                        willdeep_core::judge::redact_credentials(&error.to_string())
                    ),
                )
                .map(|_| ())?,
        }
        task.settings.max_turns = task.settings.max_turns.or(loaded
            .file
            .agent
            .max_turns
            .and_then(|value| u32::try_from(value).ok()));
        task.settings.token_budget = task
            .settings
            .token_budget
            .or(loaded.file.agent.token_budget);
    }
    if let Some(parameters) =
        willdeep_core::runtime_parameters::RuntimeParameters::load(&state.home)
            .map_err(anyhow::Error::msg)?
    {
        task.settings.max_turns = requested_turns.or(u32::try_from(parameters.max_turns).ok());
        task.settings.token_budget = requested_tokens.or(parameters.token_budget);
    }
    Ok(task)
}

fn new_run(task: ScheduledTask, key: String, trigger: ScheduleTrigger, due: u64) -> ScheduleRun {
    let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, key.as_bytes());
    ScheduleRun {
        sequence: 0,
        id,
        task_id: task.id,
        key,
        trigger,
        scheduled_at: due,
        queued_at: None,
        started_at: None,
        finished_at: None,
        session_id: None,
        turn_id: None,
        retry_of: None,
        status: ScheduleRunStatus::Preparing,
        reason: None,
        result: None,
        usage: None,
        verification: None,
        snapshot: task,
    }
}

fn dispatch_run(state: &ServerState, mut run: ScheduleRun, now: u64) -> Result<ScheduleRun> {
    let _admission = ADMISSION_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("schedule admission lock poisoned"))?;
    let history = ScheduleHistoryStore::new(&state.home);
    let _dispatch = history
        .dispatch_lock(run.task_id, run.id)
        .map_err(anyhow::Error::msg)?;
    run = history
        .get(run.task_id, run.id)
        .map_err(anyhow::Error::msg)?
        .context("schedule trigger disappeared")?;
    if run.status != ScheduleRunStatus::Preparing {
        return Ok(run);
    }
    let capacity = match has_capacity(state, &history) {
        Ok(capacity) => capacity,
        Err(error) => {
            run.status = ScheduleRunStatus::Failed;
            run.finished_at = Some(now);
            run.reason = Some(willdeep_core::judge::redact_credentials(&format!(
                "configuration_unavailable: {error:#}"
            )));
            history.put(&run).map_err(anyhow::Error::msg)?;
            apply_outcome(state, &run, now)?;
            return Ok(run);
        }
    };
    if !capacity {
        run.reason = Some("runtime_capacity_wait".to_owned());
        history.put(&run).map_err(anyhow::Error::msg)?;
        return Ok(run);
    }
    run.reason = None;
    let result = enqueue(state, &run).or_else(|error| {
        let session_id = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_OID,
            format!("schedule-session:{}", run.id).as_bytes(),
        );
        if let Ok(turns) = state.sessions.list_turns(session_id)
            && let Some(turn) = turns.into_iter().find(|turn| turn.request_id == run.id)
        {
            state.events.append(
                "schedule.warning",
                format!("run_id={} reason=enqueue_reply_recovered", run.id),
            )?;
            return Ok((session_id, turn.id));
        }
        Err(error)
    });
    match result {
        Ok((session_id, turn_id)) => {
            run.session_id = Some(session_id);
            run.turn_id = Some(turn_id);
            run.queued_at = Some(now);
            run.status = ScheduleRunStatus::Queued;
            history.put(&run).map_err(anyhow::Error::msg)?;
            ScheduleStore::new(&state.home)
                .update(|tasks| {
                    if let Some(task) = tasks.iter_mut().find(|task| task.id == run.task_id) {
                        task.last_session_id = Some(session_id);
                    }
                })
                .map_err(anyhow::Error::msg)?;
            state.events.append(
                "schedule.fired",
                format!(
                    "task_id={} run_id={} session_id={session_id} turn_id={turn_id}",
                    run.task_id, run.id
                ),
            )?;
            if let Err(error) = state.tasks.schedule_session(session_id) {
                state.events.append(
                    "schedule.warning",
                    format!(
                        "run_id={} reason=dispatch_channel_unavailable error={}",
                        run.id,
                        willdeep_core::judge::redact_credentials(&format!("{error:#}"))
                    ),
                )?;
            }
        }
        Err(error) => {
            // A Preparing record retains a deterministic session/request ID. A lost
            // reply cannot authorize a second independent turn on recovery.
            run.status = ScheduleRunStatus::Failed;
            run.finished_at = Some(now);
            run.reason = Some(willdeep_core::judge::redact_credentials(&format!(
                "{error:#}"
            )));
            history.put(&run).map_err(anyhow::Error::msg)?;
            apply_outcome(state, &run, now)?;
        }
    }
    Ok(run)
}

fn has_capacity(state: &ServerState, history: &ScheduleHistoryStore) -> Result<bool> {
    let path = state.home.join("config.toml");
    let limit = if path.exists() {
        crate::config::LoadedConfig::load(Some(&path))?
            .file
            .agent
            .schedule_max_active
            .unwrap_or(DEFAULT_MAX_ACTIVE)
    } else {
        DEFAULT_MAX_ACTIVE
    };
    if !(1..=64).contains(&limit) {
        bail!("agent.schedule_max_active must be between 1 and 64");
    }
    let mut reserved = std::collections::HashSet::new();
    for task in ScheduleStore::new(&state.home)
        .list()
        .map_err(anyhow::Error::msg)?
    {
        for run in history.active(task.id).map_err(anyhow::Error::msg)? {
            if run.status != ScheduleRunStatus::Preparing
                && let Some(session) = run.session_id
            {
                reserved.insert(session);
            }
        }
    }
    for session in state.sessions.list()? {
        if session.active_turn_id.is_some() {
            reserved.insert(session.id);
        }
    }
    Ok(reserved.len() < limit)
}

fn enqueue(state: &ServerState, run: &ScheduleRun) -> Result<(uuid::Uuid, uuid::Uuid)> {
    let task = &run.snapshot;
    let workspace = state.workspaces.ensure_registered(&task.workspace)?;
    let session_id = uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        format!("schedule-session:{}", run.id).as_bytes(),
    );
    if state.sessions.get(session_id)?.is_none() {
        let core = willdeep_core::SessionStore::new(&state.home);
        match core.load(session_id) {
            Ok(_) => {}
            Err(willdeep_core::session::SessionError::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                let mut session = willdeep_core::Session::new(
                    workspace.root.clone(),
                    task.settings
                        .profile
                        .clone()
                        .or(workspace.provider_profile.clone()),
                    &format!("⏰ {}", task.name),
                );
                session.id = session_id;
                session.model = task.settings.model.clone();
                session.config = task.settings.config_path.clone();
                session.title_source = willdeep_core::TitleSource::User;
                core.save(&mut session)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let (session, created) = state.sessions.ensure(session_store::CreateRuntimeSession {
        id: Some(session_id),
        title: Some(format!("⏰ {}", task.name)),
        workspace: workspace.root,
        profile: task.settings.profile.clone().or(workspace.provider_profile),
        model: task.settings.model.clone(),
        config: task.settings.config_path.clone(),
    })?;
    if created {
        state
            .sessions
            .rename(session.id, format!("⏰ {}", task.name))?;
    }
    if let Some(mode) = task.approval_mode.as_deref() {
        let access =
            willdeep_core::ApprovalMode::parse(mode).context("invalid scheduled approval mode")?;
        let access = match access {
            willdeep_core::ApprovalMode::ReadOnly => workspace_store::WorkspaceAccess::ReadOnly,
            willdeep_core::ApprovalMode::Strict => workspace_store::WorkspaceAccess::Strict,
            willdeep_core::ApprovalMode::Smart => workspace_store::WorkspaceAccess::Smart,
            willdeep_core::ApprovalMode::WorkspaceAccess => {
                workspace_store::WorkspaceAccess::WorkspaceWrite
            }
            willdeep_core::ApprovalMode::FullAccess => workspace_store::WorkspaceAccess::FullAccess,
        };
        state.sessions.update_approval_mode(session.id, access)?;
    }
    let (turn, _, _) = state.sessions.enqueue_turn_observed(
        session.id,
        session_store::CreateRuntimeTurn {
            request_id: run.id,
            prompt: task.run_prompt(),
            attachments: Vec::new(),
            origin_client: Some(format!("schedule:{}:{}", task.id, run.id)),
        },
    )?;
    Ok((session.id, turn.id))
}

pub(super) fn reconcile(state: &ServerState, now: u64) -> Result<()> {
    reconcile_inner(state, now, true)
}

pub(super) fn refresh(state: &ServerState, now: u64) -> Result<()> {
    reconcile_inner(state, now, false)
}

fn reconcile_inner(state: &ServerState, now: u64, resume_preparing: bool) -> Result<()> {
    let store = ScheduleStore::new(&state.home);
    let history = ScheduleHistoryStore::new(&state.home);
    for task in store.list().map_err(anyhow::Error::msg)? {
        for mut run in history.active(task.id).map_err(anyhow::Error::msg)? {
            if run.status == ScheduleRunStatus::Preparing {
                if !resume_preparing {
                    continue;
                }
                let Some(_gate) = state.work_gate.try_read().ok().filter(|gate| !**gate) else {
                    continue;
                };
                let _ = dispatch_run(state, run, now)?;
                continue;
            }
            let Some(turn_id) = run.turn_id else {
                continue;
            };
            let turn = state.sessions.get_turn(turn_id)?;
            let previous = run.status;
            if let Some(turn) = turn {
                run.status = match turn.status {
                    session_store::RuntimeTurnStatus::Queued => ScheduleRunStatus::Queued,
                    session_store::RuntimeTurnStatus::Running => ScheduleRunStatus::Running,
                    session_store::RuntimeTurnStatus::WaitingApproval => {
                        ScheduleRunStatus::WaitingApproval
                    }
                    session_store::RuntimeTurnStatus::WaitingAnswer => {
                        ScheduleRunStatus::WaitingAnswer
                    }
                    session_store::RuntimeTurnStatus::Completed => ScheduleRunStatus::Completed,
                    session_store::RuntimeTurnStatus::Partial => ScheduleRunStatus::Partial,
                    session_store::RuntimeTurnStatus::Failed
                    | session_store::RuntimeTurnStatus::Interrupted => ScheduleRunStatus::Failed,
                    session_store::RuntimeTurnStatus::Cancelled => ScheduleRunStatus::Cancelled,
                };
                run.started_at = turn.started_at;
                run.finished_at = turn.completed_at;
                run.reason = turn
                    .error
                    .as_deref()
                    .map(willdeep_core::judge::redact_credentials);
                if run.status.is_terminal() {
                    fill_result(state, &mut run)?;
                }
            } else {
                run.status = ScheduleRunStatus::Unknown;
                run.reason = Some("runtime_turn_unavailable".to_owned());
                run.finished_at = Some(now);
            }
            history.put(&run).map_err(anyhow::Error::msg)?;
            if previous != run.status && run.status.is_terminal() {
                apply_outcome(state, &run, now)?;
            }
        }
        let (latest, _) = history
            .outcome_summary(task.id, task.settings.revision, task.settings.failure_limit)
            .map_err(anyhow::Error::msg)?;
        if let Some(run) = latest {
            apply_outcome(state, &run, now)?;
        }
    }
    Ok(())
}

fn fill_result(state: &ServerState, run: &mut ScheduleRun) -> Result<()> {
    let Some(session_id) = run.session_id else {
        return Ok(());
    };
    if let Ok(core) = willdeep_core::SessionStore::new(&state.home).load(session_id) {
        let turn = run
            .turn_id
            .map(|id| state.sessions.get_turn(id))
            .transpose()?
            .flatten();
        if run.result.is_none()
            && let Some(turn) = &turn
        {
            let bounds = turn
                .message_start
                .zip(turn.message_end)
                .filter(|(start, end)| {
                    *start <= *end
                        && *end <= core.messages.len()
                        && turn.message_generation == core.compression_generation
                });
            let messages = bounds
                .map(|(start, end)| &core.messages[start..end])
                .or_else(|| {
                    state
                        .sessions
                        .list_turns(session_id)
                        .ok()
                        .filter(|turns| turns.len() == 1)
                        .map(|_| core.messages.as_slice())
                });
            run.result = messages
                .and_then(|messages| {
                    messages
                        .iter()
                        .rev()
                        .find(|message| message.role == willdeep_core::Role::Assistant)
                })
                .map(|message| {
                    willdeep_core::judge::redact_credentials(&message.content)
                        .chars()
                        .take(RESULT_PREVIEW_CHARS)
                        .collect()
                });
        }
        if let Some(session) = state.sessions.get(session_id)?
            && let Some(agent) = state.agents.get(session.root_agent_id)?
            && turn
                .as_ref()
                .is_some_and(|turn| turn.active_task_id == Some(agent.task_id))
        {
            if let (Some(input_tokens), Some(output_tokens), Some(total_tokens)) =
                (agent.input_tokens, agent.output_tokens, agent.total_tokens)
            {
                run.usage = Some(ScheduleRunUsage {
                    input_tokens,
                    output_tokens,
                    total_tokens,
                    model_calls: None,
                });
            }
            run.verification = agent
                .verifier_passed
                .map(|passed| if passed { "passed" } else { "failed" }.to_owned());
        }
    } else {
        state.events.append(
            "schedule.warning",
            format!("run_id={} reason=run_output_unavailable", run.id),
        )?;
    }
    Ok(())
}

fn apply_outcome(state: &ServerState, run: &ScheduleRun, now: u64) -> Result<()> {
    let (_, failures) = ScheduleHistoryStore::new(&state.home)
        .outcome_summary(
            run.task_id,
            run.snapshot.settings.revision,
            run.snapshot.settings.failure_limit,
        )
        .map_err(anyhow::Error::msg)?;
    ScheduleStore::new(&state.home)
        .update(|tasks| {
            if let Some(task) = tasks.iter_mut().find(|task| task.id == run.task_id) {
                if task.settings.revision != run.snapshot.settings.revision {
                    return;
                }
                let before = task.clone();
                task.settings.consecutive_failures = failures;
                if task.settings.consecutive_failures >= task.settings.failure_limit {
                    task.enabled = false;
                    task.settings.migration_reason = Some("consecutive_failures".to_owned());
                }
                if matches!(task.schedule, Schedule::OnceAt { .. }) && run.status.is_terminal() {
                    task.enabled = false;
                    if task.goal.is_none() && run.status == ScheduleRunStatus::Completed {
                        task.settings
                            .completed_at
                            .get_or_insert(run.finished_at.unwrap_or(now));
                    } else if task.settings.migration_reason.is_none() {
                        task.settings.migration_reason = Some("one_time_ended".to_owned());
                    }
                }
                if *task != before {
                    task.settings.updated_at = now;
                }
            }
        })
        .map_err(anyhow::Error::msg)
}
