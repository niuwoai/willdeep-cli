//! `willdeep-scheduler` 内置插件：`schedule_task` / `complete_scheduled_task` /
//! `list_scheduled_tasks`。
//!
//! 语义对齐 Xedit（`AgentToolsExpertsAndSchedules.swift`、`BundledSkills/scheduler`）：
//! 建任务不会立刻跑任何东西；到点由 daemon 的调度器开一个全新会话、把 prompt
//! 当第一句发出去。任务写进 `$WILLDEEP_HOME/schedules.json`，调度器读同一份。

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};
use willdeep_core::schedule::{Schedule, parse_hhmm};
use willdeep_runtime_protocol::schedule::{ScheduleDraft, ScheduleSettings, ScheduledTaskView};

use super::stdio_server::{BuiltinPlugin, HostClient, string_arg};

pub(crate) struct Scheduler {
    home: PathBuf,
    /// 没给 `workspace` 时的默认工作区：插件进程的当前目录。
    default_workspace: Option<PathBuf>,
}

impl Scheduler {
    pub(crate) fn new(home: PathBuf, default_workspace: Option<PathBuf>) -> Self {
        Self {
            home,
            default_workspace,
        }
    }

    fn draft(&self, arguments: &Value) -> Result<ScheduleDraft, String> {
        let origin_session_id = string_arg(arguments, "_session_id")
            .map(|value| uuid::Uuid::parse_str(&value).map_err(|_| "invalid host session"))
            .transpose()?;
        let prompt = string_arg(arguments, "prompt").ok_or("prompt is required")?;
        let schedule = parse_schedule(arguments)?;
        schedule.validate()?;
        let goal = string_arg(arguments, "goal");
        let approval_mode = match string_arg(arguments, "approval_mode") {
            Some(mode) => Some(
                willdeep_core::ApprovalMode::parse(&mode)
                    .filter(|mode| *mode != willdeep_core::ApprovalMode::ReadOnly)
                    .ok_or_else(|| {
                        format!(
                            "approval_mode must be one of strict, smart, workspace-write, full-access (got {mode})"
                        )
                    })?
                    .as_str()
                    .to_owned(),
            ),
            None => None,
        };
        let workspace = match string_arg(arguments, "_workspace")
            .or_else(|| string_arg(arguments, "workspace"))
        {
            Some(path) => PathBuf::from(path),
            None => self
                .default_workspace
                .clone()
                .ok_or("workspace is required: pass the absolute path of the project to run in")?,
        };
        if !workspace.is_absolute() || !workspace.is_dir() {
            return Err(format!(
                "workspace must be an existing absolute directory: {}",
                workspace.display()
            ));
        }
        let name = string_arg(arguments, "title")
            .or_else(|| goal.clone())
            .unwrap_or_else(|| prompt.clone())
            .chars()
            .take(48)
            .collect::<String>();
        let settings = ScheduleSettings {
            continue_in_session: origin_session_id.is_some(),
            timezone: string_arg(arguments, "timezone").unwrap_or_else(|| "local".to_owned()),
            profile: string_arg(arguments, "profile"),
            model: string_arg(arguments, "model"),
            max_turns: optional_integer(arguments, "max_turns")?
                .map(|value| u32::try_from(value).map_err(|_| "max_turns is too large"))
                .transpose()?,
            timeout_seconds: optional_integer(arguments, "timeout_seconds")?,
            token_budget: optional_integer(arguments, "token_budget")?,
            ..Default::default()
        };
        let draft = ScheduleDraft {
            origin_session_id,
            name,
            prompt,
            workspace,
            schedule,
            goal,
            approval_mode,
            settings,
        };
        draft.validate()?;
        Ok(draft)
    }

    async fn schedule_task(&self, arguments: &Value) -> Result<String, String> {
        let draft = self.draft(arguments)?;
        if let Some(mode) = draft.approval_mode.as_deref() {
            let requested =
                willdeep_core::ApprovalMode::parse(mode).ok_or("invalid approval mode")?;
            let root = draft
                .workspace
                .canonicalize()
                .map_err(|error| error.to_string())?;
            let workspaces = crate::daemon::remote_workspaces(&self.home)
                .await
                .map_err(|error| error.to_string())?;
            let baseline = workspaces
                .iter()
                .find(|workspace| workspace.root == root)
                .map(|workspace| workspace.access.approval_mode())
                .unwrap_or(willdeep_core::ApprovalMode::Strict);
            if !automatic_approval_allowed(baseline, requested) {
                return Err("A scheduler tool cannot raise unattended permissions. Use the Web or CLI task editor to explicitly select a higher approval mode.".to_owned());
            }
        }
        let result = crate::daemon::schedule_operation(
            &self.home,
            "schedule.create",
            serde_json::to_value(&draft).map_err(|e| e.to_string())?,
            Some(uuid::Uuid::new_v4()),
        )
        .await
        .map_err(|e| format!("{e:#}"))?;
        let view: ScheduledTaskView = serde_json::from_value(result).map_err(|e| e.to_string())?;
        Ok(format!(
            "Scheduled task {:?} created (task_id: {}); {} in timezone {}. Runtime owns the task; nothing runs now. Manage configuration and history in the Web Scheduled Tasks page.",
            view.task.name,
            view.task.id,
            view.task.schedule.summary(),
            view.task.settings.timezone
        ))
    }

    async fn complete_scheduled_task(&self, arguments: &Value) -> Result<String, String> {
        let raw = string_arg(arguments, "task_id").ok_or("task_id is required")?;
        let id = uuid::Uuid::parse_str(&raw).map_err(|_| {
            "task_id must be a scheduled-task id returned by schedule_task".to_owned()
        })?;
        let revision = arguments
            .get("expected_revision")
            .and_then(Value::as_u64)
            .unwrap_or(1);
        crate::daemon::schedule_operation(
            &self.home,
            "schedule.action",
            json!({"id": id, "expected_revision": revision, "action":"complete"}),
            Some(uuid::Uuid::new_v4()),
        )
        .await
        .map_err(|e| format!("{e:#}"))?;
        Ok(format!(
            "Scheduled task {id} completed. Future triggers stopped; configuration and run history retained."
        ))
    }

    async fn list_scheduled_tasks(&self, arguments: &Value) -> Result<String, String> {
        let data = crate::daemon::schedule_operation(&self.home, "schedule.list", json!({}), None)
            .await
            .map_err(|error| format!("{error:#}"))?;
        let mut tasks: Vec<ScheduledTaskView> =
            serde_json::from_value(data).map_err(|error| error.to_string())?;
        if let Some(session) = string_arg(arguments, "_session_id") {
            tasks.retain(|view| {
                view.task
                    .origin_session_id
                    .is_some_and(|id| id.to_string() == session)
            });
        }
        serde_json::to_string_pretty(&tasks).map_err(|error| error.to_string())
    }

    async fn manage(&self, arguments: &Value, history: bool) -> Result<String, String> {
        let id = string_arg(arguments, "task_id").ok_or("task_id is required")?;
        let data =
            crate::daemon::schedule_operation(&self.home, "schedule.get", json!({"id": id}), None)
                .await
                .map_err(|error| format!("{error:#}"))?;
        let view: ScheduledTaskView =
            serde_json::from_value(data).map_err(|error| error.to_string())?;
        if let Some(session) = string_arg(arguments, "_session_id")
            && view
                .task
                .origin_session_id
                .is_none_or(|origin| origin.to_string() != session)
        {
            return Err("task does not belong to this conversation".to_owned());
        }
        let (operation, params) = if history {
            (
                "schedule.history",
                json!({"task_id": id, "limit": 20, "before": string_arg(arguments, "before")}),
            )
        } else {
            let action = string_arg(arguments, "action").ok_or("action is required")?;
            if !matches!(action.as_str(), "pause" | "resume") {
                return Err("action must be pause or resume".to_owned());
            }
            (
                "schedule.action",
                json!({"id": id, "action": action, "expected_revision": view.task.settings.revision}),
            )
        };
        let data = crate::daemon::schedule_operation(
            &self.home,
            operation,
            params,
            (!history).then(uuid::Uuid::new_v4),
        )
        .await
        .map_err(|error| format!("{error:#}"))?;
        serde_json::to_string_pretty(&data).map_err(|error| error.to_string())
    }
}

fn automatic_approval_allowed(
    baseline: willdeep_core::ApprovalMode,
    requested: willdeep_core::ApprovalMode,
) -> bool {
    baseline == requested
        || baseline == willdeep_core::ApprovalMode::FullAccess
        || (baseline != willdeep_core::ApprovalMode::ReadOnly
            && requested == willdeep_core::ApprovalMode::Strict)
}

fn parse_schedule(arguments: &Value) -> Result<Schedule, String> {
    let once = string_arg(arguments, "once_at")
        .map(|value| {
            willdeep_runtime_protocol::schedule::parse_rfc3339_timestamp(&value)
                .ok_or("once_at must be RFC3339 with an explicit timezone")
        })
        .transpose()?;
    let time = |key: &str| -> Result<Option<(u8, u8)>, String> {
        string_arg(arguments, key)
            .map(|raw| parse_hhmm(&raw).ok_or(format!("{key} must be HH:MM (24-hour local time)")))
            .transpose()
    };
    let interval = string_arg(arguments, "interval_minutes")
        .map(|raw| {
            raw.parse::<u32>()
                .ok()
                .filter(|minutes| *minutes > 0)
                .ok_or("interval_minutes must be a positive whole number of minutes".to_owned())
        })
        .transpose()?;
    let daily = time("daily_at")?;
    let weekdays = time("weekdays_at")?;
    let weekly = time("weekly_at")?;
    let chosen = [
        once.is_some(),
        interval.is_some(),
        daily.is_some(),
        weekdays.is_some(),
        weekly.is_some(),
    ]
    .into_iter()
    .filter(|given| *given)
    .count();
    if chosen != 1 {
        return Err(
            "give exactly one schedule: interval_minutes (e.g. \"30\"), daily_at (\"HH:MM\"), weekdays_at (\"HH:MM\"), or weekly_at (\"HH:MM\") with weekday (1 = Sunday … 7 = Saturday)"
                .to_owned(),
        );
    }
    Ok(if let Some(at) = once {
        Schedule::OnceAt { at }
    } else if let Some(minutes) = interval {
        Schedule::Interval { minutes }
    } else if let Some((hour, minute)) = daily {
        Schedule::DailyAt { hour, minute }
    } else if let Some((hour, minute)) = weekdays {
        Schedule::WeekdaysAt { hour, minute }
    } else {
        let (hour, minute) = weekly.expect("one schedule was given");
        let weekday = string_arg(arguments, "weekday")
            .and_then(|raw| raw.parse::<u8>().ok())
            .ok_or("weekly_at needs weekday (1 = Sunday … 7 = Saturday)")?;
        Schedule::WeeklyAt {
            weekday,
            hour,
            minute,
        }
    })
}

fn optional_integer(arguments: &Value, key: &str) -> Result<Option<u64>, String> {
    let Some(value) = arguments.get(key).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .map(Some)
        .ok_or_else(|| format!("{key} must be a positive integer"))
}

#[async_trait]
impl BuiltinPlugin for Scheduler {
    fn server_name(&self) -> &'static str {
        "scheduler"
    }

    fn tools(&self) -> Vec<Value> {
        vec![
            json!({"name":"manage_scheduled_task", "description":"Pause or resume a task belonging to this conversation. Pause stops future triggers and preserves history; an active run continues.", "inputSchema":{"type":"object", "properties":{"task_id":{"type":"string"}, "action":{"type":"string", "enum":["pause","resume"]}}, "required":["task_id","action"], "additionalProperties":false}}),
            json!({"name":"get_scheduled_task_history", "description":"Read paginated actual run statuses, reasons and results for a task in this conversation. Execution completed does not mean verification passed.", "inputSchema":{"type":"object", "properties":{"task_id":{"type":"string"}, "before":{"type":"string"}}, "required":["task_id"], "additionalProperties":false}, "annotations":{"readOnlyHint":true}}),
            json!({
                "name": "schedule_task",
                "description": "Schedule a recurring task that runs on its own later, in this conversation when called from chat; legacy standalone tasks use a fresh session. The WillDeep runtime daemon executes it. Use when the user asks for something to happen repeatedly or to be watched over time (\"every 30 minutes check…\", \"every morning summarize…\", \"watch X until Y\"). Chat tasks continue the current conversation and inherit its permissions. Busy conversations are skipped with a recorded reason. Give exactly one schedule. Optionally give a goal: each run then checks whether it is met and, if so, calls complete_scheduled_task to complete the task while preserving its history. This does NOT run anything now.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prompt": {"type": "string", "description": "What each run should do: target, scope, expected output, safety constraints."},
                        "once_at": {"type":"string", "description":"Run once at RFC3339 datetime with an explicit offset, e.g. 2026-10-13T09:00:00+08:00."},
                        "timezone": {"type":"string", "description":"IANA timezone such as Asia/Shanghai; local means the Runtime's timezone."},
                        "profile": {"type":"string", "description":"Existing provider profile for these runs."},
                        "model": {"type":"string", "description":"Existing model identifier for these runs."},
                        "max_turns": {"type":"integer", "minimum":1, "maximum":1000},
                        "token_budget": {"type":"integer", "minimum":1000, "maximum":10000000},
                        "timeout_seconds": {"type":"integer", "minimum":1, "maximum":86400},
                        "title": {"type": "string", "description": "Short display name. Defaults to the goal or prompt."},
                        "interval_minutes": {"type": "string", "description": "Run every N minutes, e.g. \"30\"."},
                        "daily_at": {"type": "string", "description": "Run every day at local HH:MM."},
                        "weekdays_at": {"type": "string", "description": "Run Monday–Friday at local HH:MM."},
                        "weekly_at": {"type": "string", "description": "Run once a week at local HH:MM; requires weekday."},
                        "weekday": {"type": "string", "description": "For weekly_at: 1 = Sunday … 7 = Saturday."},
                        "goal": {"type": "string", "description": "Optional completion condition, e.g. \"CI on main is green\"."},
                        "approval_mode": {"type": "string", "enum": ["strict", "smart", "workspace-write", "full-access"], "description": "Approval posture for the unattended runs. Defaults to the workspace's mode; pending approvals wait in the inbox."},
                        "workspace": {"type": "string", "description": "Absolute project path the runs work in. Defaults to the current workspace."}
                    },
                    "required": ["prompt"],
                    "additionalProperties": false
                }
            }),
            json!({
                "name": "complete_scheduled_task",
                "description": "Stop future triggers while keeping configuration and run history. A goal-bearing scheduled run calls this with its own task_id once the goal is met; it also cancels a task on the user's request.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": {"type": "string"},
                        "expected_revision": {"type":"integer", "description":"Configuration revision from the scheduled run; completion fails if the task has changed."},
                        "reason": {"type": "string"}
                    },
                    "required": ["task_id", "expected_revision"],
                    "additionalProperties": false
                }
            }),
            json!({
                "name": "list_scheduled_tasks",
                "description": "List scheduled tasks with their schedule, next local run time, workspace, goal and last run session. Read-only.",
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
                "annotations": {"readOnlyHint": true}
            }),
        ]
    }

    async fn call(
        &self,
        name: &str,
        arguments: Value,
        _host: &HostClient,
    ) -> Result<String, String> {
        match name {
            "schedule_task" => self.schedule_task(&arguments).await,
            "complete_scheduled_task" => self.complete_scheduled_task(&arguments).await,
            "list_scheduled_tasks" => self.list_scheduled_tasks(&arguments).await,
            "manage_scheduled_task" => self.manage(&arguments, false).await,
            "get_scheduled_task_history" => self.manage(&arguments, true).await,
            other => Err(format!("unknown tool {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler() -> (Scheduler, PathBuf) {
        let home =
            std::env::temp_dir().join(format!("willdeep-scheduler-{}", uuid::Uuid::new_v4()));
        let workspace = home.join("ws");
        std::fs::create_dir_all(&workspace).unwrap();
        (Scheduler::new(home.clone(), Some(workspace)), home)
    }

    #[test]
    fn builds_a_runtime_owned_request_without_writing_a_schedule_file() {
        let (scheduler, home) = scheduler();
        let draft = scheduler.draft(&json!({"prompt": "Check CI", "interval_minutes": "30", "goal": "CI is green", "timezone": "Asia/Shanghai"})).unwrap();
        assert_eq!(draft.name, "CI is green");
        assert_eq!(draft.schedule, Schedule::Interval { minutes: 30 });
        assert_eq!(draft.settings.timezone, "Asia/Shanghai");
        assert!(!home.join("schedules.json").exists());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn schedules_are_validated() {
        let (scheduler, home) = scheduler();
        for (arguments, needle) in [
            (json!({"prompt": "x"}), "exactly one schedule"),
            (
                json!({"prompt": "x", "interval_minutes": "5", "daily_at": "09:00"}),
                "exactly one schedule",
            ),
            (json!({"prompt": "x", "daily_at": "25:00"}), "HH:MM"),
            (json!({"prompt": "x", "interval_minutes": "0"}), "positive"),
            (json!({"prompt": "x", "weekly_at": "09:00"}), "weekday"),
            (json!({"interval_minutes": "5"}), "prompt is required"),
            (
                json!({"prompt": "x", "daily_at": "09:00", "approval_mode": "read-only"}),
                "approval_mode",
            ),
            (
                json!({"prompt": "x", "daily_at": "09:00", "workspace": "relative/path"}),
                "absolute",
            ),
        ] {
            let error = scheduler.draft(&arguments).unwrap_err();
            assert!(error.contains(needle), "{arguments}: {error}");
        }
        let weekly = scheduler
            .draft(&json!({"prompt": "x", "weekly_at": "09:30", "weekday": "2", "approval_mode": "full-access"}))
            .unwrap();
        assert_eq!(weekly.schedule.summary(), "every Monday at 09:30");
        assert_eq!(weekly.approval_mode.as_deref(), Some("full-access"));
        assert!(!home.join("schedules.json").exists());
        let _ = std::fs::remove_dir_all(&home);
    }
}
