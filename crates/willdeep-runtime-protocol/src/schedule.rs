//! Runtime scheduled task definitions shared by every client.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

const SECONDS_PER_DAY: i64 = 86_400;

/// 何时触发。本地时间按调用方给的 UTC 偏移（秒）换算。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Schedule {
    /// One run at an absolute UTC timestamp.
    OnceAt { at: u64 },
    /// 每 N 分钟，从上次运行（没跑过则从创建时）算起。
    Interval { minutes: u32 },
    /// 每天本地 HH:MM。
    DailyAt { hour: u8, minute: u8 },
    /// 周一到周五本地 HH:MM。
    WeekdaysAt { hour: u8, minute: u8 },
    /// 每周某天本地 HH:MM。`weekday` 1 = 周日 … 7 = 周六（与 Foundation 一致）。
    WeeklyAt { weekday: u8, hour: u8, minute: u8 },
}

impl Schedule {
    /// 校验取值范围；建任务时调用，坏数据不落盘。
    pub fn validate(&self) -> Result<(), String> {
        let time_ok = |hour: u8, minute: u8| hour <= 23 && minute <= 59;
        match *self {
            Self::OnceAt { at } if at == 0 || i64::try_from(at).is_err() => {
                Err("once_at must be a valid positive UTC timestamp".to_owned())
            }
            Self::Interval { minutes: 0 } => Err("interval_minutes must be at least 1".to_owned()),
            Self::Interval { .. } => Ok(()),
            Self::DailyAt { hour, minute } | Self::WeekdaysAt { hour, minute }
                if !time_ok(hour, minute) =>
            {
                Err("time must be HH:MM in 24-hour local time".to_owned())
            }
            Self::WeeklyAt {
                weekday,
                hour,
                minute,
            } if !(1..=7).contains(&weekday) || !time_ok(hour, minute) => {
                Err("weekly schedules need weekday 1-7 (1 = Sunday) and HH:MM".to_owned())
            }
            _ => Ok(()),
        }
    }

    /// 一行人读的描述。
    pub fn summary(&self) -> String {
        match *self {
            Self::OnceAt { at } => format!("once at {at} UTC"),
            Self::Interval { minutes } if minutes % 60 == 0 && minutes >= 60 => {
                match minutes / 60 {
                    1 => "every hour".to_owned(),
                    hours => format!("every {hours} hours"),
                }
            }
            Self::Interval { minutes } => format!("every {minutes} minutes"),
            Self::DailyAt { hour, minute } => format!("daily at {hour:02}:{minute:02}"),
            Self::WeekdaysAt { hour, minute } => {
                format!("weekdays at {hour:02}:{minute:02}")
            }
            Self::WeeklyAt {
                weekday,
                hour,
                minute,
            } => format!("every {} at {hour:02}:{minute:02}", weekday_name(weekday)),
        }
    }

    /// `after`（Unix 秒）之后下一次该触发的时刻（Unix 秒）。`utc_offset` 是
    /// 本地时间相对 UTC 的秒数。
    pub fn next_fire(&self, after: u64, utc_offset: i64) -> Option<u64> {
        match *self {
            Self::OnceAt { at } => (at > after).then_some(at),
            Self::Interval { minutes } => (minutes > 0)
                .then(|| after.checked_add(u64::from(minutes) * 60))
                .flatten(),
            Self::DailyAt { hour, minute } => next_local(after, utc_offset, hour, minute, |_| true),
            Self::WeekdaysAt { hour, minute } => {
                next_local(after, utc_offset, hour, minute, |weekday| {
                    (2..=6).contains(&weekday)
                })
            }
            Self::WeeklyAt {
                weekday,
                hour,
                minute,
            } => next_local(after, utc_offset, hour, minute, |day| day == weekday),
        }
    }
}

fn weekday_name(weekday: u8) -> &'static str {
    match weekday {
        1 => "Sunday",
        2 => "Monday",
        3 => "Tuesday",
        4 => "Wednesday",
        5 => "Thursday",
        6 => "Friday",
        7 => "Saturday",
        _ => "?",
    }
}

/// 严格晚于 `after` 的第一个「本地 HH:MM 且星期满足条件」的时刻。
fn next_local(
    after: u64,
    utc_offset: i64,
    hour: u8,
    minute: u8,
    weekday_ok: impl Fn(u8) -> bool,
) -> Option<u64> {
    let local_after = after as i64 + utc_offset;
    let day_start = local_after.div_euclid(SECONDS_PER_DAY) * SECONDS_PER_DAY;
    let at = i64::from(hour) * 3_600 + i64::from(minute) * 60;
    for day in 0..=8 {
        let candidate = day_start + day * SECONDS_PER_DAY + at;
        if candidate <= local_after {
            continue;
        }
        // 1970-01-01 是周四；Foundation 的 weekday 1 = 周日。
        let weekday = ((candidate.div_euclid(SECONDS_PER_DAY) + 4).rem_euclid(7) + 1) as u8;
        if weekday_ok(weekday) {
            return u64::try_from(candidate - utc_offset).ok();
        }
    }
    None
}

/// 一个定时任务。新增字段一律 `serde(default)`：旧文件照常能读。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledTask {
    pub id: Uuid,
    pub name: String,
    /// 每次触发时新会话的第一句话。必须自给自足：新会话看不到别的上下文。
    pub prompt: String,
    /// 新会话绑定的工作区。
    pub workspace: PathBuf,
    pub schedule: Schedule,
    /// 自我完成的目标。有它时每次运行都被要求判断是否达成。
    #[serde(default)]
    pub goal: Option<String>,
    /// 这类无人值守运行的审批档位（`strict` / `smart` / `workspace-write` /
    /// `full-access`）。不写按工作区档位。
    #[serde(default)]
    pub approval_mode: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub created_at: u64,
    #[serde(default)]
    pub last_run_at: Option<u64>,
    #[serde(default)]
    pub last_session_id: Option<Uuid>,
    /// 在哪个会话里用 `schedule_task` 建的（从侧栏 / CLI 建的为空）。
    #[serde(default)]
    pub origin_session_id: Option<Uuid>,
    #[serde(flatten)]
    pub settings: ScheduleSettings,
}

fn default_enabled() -> bool {
    true
}

impl ScheduledTask {
    /// 下一次该触发的时刻；停用或调度退化时为 `None`。
    pub fn next_fire(&self, utc_offset: i64) -> Option<u64> {
        if !self.enabled
            || self.settings.archived_at.is_some()
            || self.settings.completed_at.is_some()
        {
            return None;
        }
        let after = self.last_run_at.unwrap_or(self.created_at);
        if self.settings.timezone == "local" && !self.settings.managed {
            self.schedule.next_fire(after, utc_offset)
        } else {
            self.schedule
                .next_fire_in_timezone(after, &self.settings.timezone)
        }
    }

    /// 到 `now` 为止是否到期。停机期间错过多少次都只算一次：调度器触发后把
    /// `last_run_at` 设成 `now`，下一次从 `now` 往后算。
    pub fn is_due(&self, now: u64, utc_offset: i64) -> bool {
        self.next_fire(utc_offset).is_some_and(|due| due <= now)
    }

    /// 触发时新会话收到的第一句话：任务 prompt，带 goal 时附上自我完成说明
    /// （与 Xedit `AppStateAgentRoutines` 同一段话术）。
    pub fn run_prompt(&self) -> String {
        let mut prompt = format!(
            "[scheduled-task id={} name={:?} schedule={:?}] This is an unattended scheduled run; no one is watching live.",
            self.id,
            self.name,
            self.schedule.summary()
        );
        if self.settings.continue_in_session {
            prompt.push_str("\nContinue the source conversation using its prior results. Check actual evidence before proposing improvements, and report what changed and what remains unverified.");
        }
        if let Some(goal) = self.goal.as_deref().filter(|goal| !goal.trim().is_empty()) {
            prompt.push_str(&format!(
                "\n\n[scheduled-task-goal]\nGoal: {goal}\nAfter doing the work above, judge whether this goal is now met:\n- Met → call the `complete_scheduled_task` tool with task_id \"{}\" and expected_revision {} to stop future runs while preserving the task and its history.\n- Not met → briefly report current progress. Do not call complete_scheduled_task.",
                self.id, self.settings.revision
            ));
        }
        format!(
            "{}\n{}\n{}\n{}",
            crate::ATTACHED_CONTEXT_HEADER,
            prompt.replace(crate::USER_MESSAGE_BOUNDARY, "[user-message boundary]"),
            crate::USER_MESSAGE_BOUNDARY,
            self.prompt
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissedRunPolicy {
    #[default]
    Coalesce,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleSettings {
    pub config_path: Option<PathBuf>,
    pub managed: bool,
    pub revision: u64,
    pub updated_at: u64,
    pub timezone: String,
    pub profile: Option<String>,
    pub model: Option<String>,
    pub max_turns: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub token_budget: Option<u64>,
    pub missed_policy: MissedRunPolicy,
    pub failure_limit: u32,
    pub consecutive_failures: u32,
    pub retention_days: u32,
    /// Opt-in only: legacy tasks keep independent run sessions.
    pub continue_in_session: bool,
    pub archived_at: Option<u64>,
    pub completed_at: Option<u64>,
    pub migration_reason: Option<String>,
}

impl Default for ScheduleSettings {
    fn default() -> Self {
        Self {
            config_path: None,
            managed: false,
            revision: 1,
            updated_at: 0,
            timezone: "local".to_owned(),
            profile: None,
            model: None,
            max_turns: None,
            timeout_seconds: None,
            token_budget: None,
            missed_policy: MissedRunPolicy::Coalesce,
            failure_limit: 3,
            consecutive_failures: 0,
            retention_days: 90,
            continue_in_session: false,
            archived_at: None,
            completed_at: None,
            migration_reason: None,
        }
    }
}

impl ScheduleSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.timezone != "local" && self.timezone.parse::<chrono_tz::Tz>().is_err() {
            return Err("timezone must be an IANA timezone or local".to_owned());
        }
        if !(1..=100).contains(&self.failure_limit) || !(1..=3650).contains(&self.retention_days) {
            return Err("failure_limit must be 1-100 and retention_days 1-3650".to_owned());
        }
        if self.max_turns == Some(0)
            || self.timeout_seconds == Some(0)
            || self.token_budget == Some(0)
        {
            return Err("run budgets must be positive when specified".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleDraft {
    pub name: String,
    pub prompt: String,
    pub workspace: PathBuf,
    pub schedule: Schedule,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub approval_mode: Option<String>,
    #[serde(default)]
    pub origin_session_id: Option<Uuid>,
    #[serde(flatten)]
    pub settings: ScheduleSettings,
}

impl ScheduleDraft {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.name.chars().count() > 120 {
            return Err("schedule name must contain 1-120 characters".to_owned());
        }
        if self.prompt.trim().is_empty() || self.prompt.len() > 1024 * 1024 {
            return Err("schedule prompt must contain 1-1048576 bytes".to_owned());
        }
        if !self.workspace.is_absolute() {
            return Err("schedule workspace must be absolute".to_owned());
        }
        self.schedule.validate()?;
        if self.settings.continue_in_session && self.origin_session_id.is_none() {
            return Err("continuing a session requires origin_session_id".to_owned());
        }
        self.settings.validate()
    }

    pub fn into_task(self, id: Uuid, now: u64) -> ScheduledTask {
        let settings = ScheduleSettings {
            managed: true,
            revision: 1,
            updated_at: now,
            archived_at: None,
            completed_at: None,
            migration_reason: None,
            consecutive_failures: 0,
            ..self.settings
        };
        ScheduledTask {
            id,
            name: self.name,
            prompt: self.prompt,
            workspace: self.workspace,
            schedule: self.schedule,
            goal: self.goal,
            approval_mode: self.approval_mode,
            enabled: true,
            created_at: now,
            last_run_at: None,
            last_session_id: None,
            origin_session_id: self.origin_session_id,
            settings,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleUpdate {
    pub id: Uuid,
    pub expected_revision: u64,
    pub task: ScheduleDraft,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleAction {
    pub id: Uuid,
    pub expected_revision: u64,
    pub action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleRunRequest {
    pub id: Uuid,
    #[serde(default)]
    pub retry_of: Option<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleHistoryQuery {
    pub task_id: Uuid,
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub status: Option<ScheduleRunStatus>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleRunStatus {
    Preparing,
    Queued,
    Running,
    WaitingApproval,
    WaitingAnswer,
    Completed,
    Partial,
    Failed,
    Cancelled,
    Skipped,
    Unknown,
}

impl ScheduleRunStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Partial
                | Self::Failed
                | Self::Cancelled
                | Self::Skipped
                | Self::Unknown
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleTrigger {
    Scheduled,
    Manual,
    Retry,
    Imported,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRunUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub model_calls: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleRun {
    #[serde(default)]
    pub sequence: u64,
    pub id: Uuid,
    pub task_id: Uuid,
    pub key: String,
    pub trigger: ScheduleTrigger,
    pub scheduled_at: u64,
    pub queued_at: Option<u64>,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
    pub session_id: Option<Uuid>,
    pub turn_id: Option<Uuid>,
    pub retry_of: Option<Uuid>,
    pub status: ScheduleRunStatus,
    pub reason: Option<String>,
    pub result: Option<String>,
    pub usage: Option<ScheduleRunUsage>,
    pub verification: Option<String>,
    pub snapshot: ScheduledTask,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleHistoryPage {
    pub runs: Vec<ScheduleRun>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleRunRef {
    pub task_id: Uuid,
    pub run_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleCleanup {
    pub task_id: Uuid,
    pub expected_revision: u64,
    #[serde(default)]
    pub preview: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduledTaskView {
    pub task: ScheduledTask,
    pub next_fire_at: Option<u64>,
    pub last_run: Option<ScheduleRun>,
    pub active_run: Option<ScheduleRun>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScheduleActionOutcome {
    Task(Box<ScheduledTaskView>),
    Deleted {
        deleted: Uuid,
        history_retained: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleCleanupOutcome {
    pub run_ids: Vec<Uuid>,
    pub preview: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeletedScheduledTask {
    pub task: ScheduledTask,
    pub deleted_at: u64,
}

pub fn timezones() -> Vec<String> {
    std::iter::once("local".to_owned())
        .chain(chrono_tz::TZ_VARIANTS.iter().map(|zone| zone.to_string()))
        .collect()
}

pub fn parse_rfc3339_timestamp(value: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|date| u64::try_from(date.timestamp()).ok())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchedulePreview {
    #[serde(default)]
    pub task_id: Option<Uuid>,
    pub schedule: Schedule,
    pub timezone: String,
    pub after: u64,
    #[serde(default)]
    pub count: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleLocalTime {
    pub local_datetime: String,
    pub timezone: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScheduleFormatTime {
    pub at: u64,
    pub timezone: String,
}

pub fn format_local_time(value: &ScheduleFormatTime) -> Result<String, String> {
    use chrono::TimeZone;
    let at = i64::try_from(value.at).map_err(|_| "timestamp out of range".to_owned())?;
    let date = chrono::Utc
        .timestamp_opt(at, 0)
        .single()
        .ok_or_else(|| "timestamp out of range".to_owned())?;
    if value.timezone == "local" {
        Ok(date
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%dT%H:%M")
            .to_string())
    } else {
        let zone = value
            .timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| "invalid IANA timezone".to_owned())?;
        Ok(date
            .with_timezone(&zone)
            .format("%Y-%m-%dT%H:%M")
            .to_string())
    }
}

pub fn resolve_local_time(value: &ScheduleLocalTime) -> Result<u64, String> {
    let local = chrono::NaiveDateTime::parse_from_str(&value.local_datetime, "%Y-%m-%dT%H:%M")
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&value.local_datetime, "%Y-%m-%dT%H:%M:%S")
        })
        .map_err(|_| "local_datetime must be YYYY-MM-DDTHH:MM".to_owned())?;
    fn resolve<T: chrono::TimeZone>(local: &chrono::NaiveDateTime, zone: T) -> Option<u64> {
        zone.from_local_datetime(local)
            .earliest()
            .and_then(|date| u64::try_from(date.timestamp()).ok())
    }
    let time = if value.timezone == "local" {
        resolve(&local, chrono::Local)
    } else {
        resolve(
            &local,
            value
                .timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| "invalid IANA timezone".to_owned())?,
        )
    };
    time.ok_or_else(|| "this local time does not exist in the selected timezone".to_owned())
}

impl Schedule {
    pub fn next_fire_in_timezone(&self, after: u64, zone: &str) -> Option<u64> {
        match *self {
            Self::Interval { .. } | Self::OnceAt { .. } => self.next_fire(after, 0),
            _ if zone == "local" => calendar_next(self, after, chrono::Local),
            _ => calendar_next(self, after, zone.parse::<chrono_tz::Tz>().ok()?),
        }
    }
}

fn calendar_next<T: chrono::TimeZone>(schedule: &Schedule, after: u64, zone: T) -> Option<u64> {
    use chrono::Datelike;
    let date = zone
        .timestamp_opt(i64::try_from(after).ok()?, 0)
        .single()?
        .date_naive();
    let (hour, minute) = match *schedule {
        Schedule::DailyAt { hour, minute }
        | Schedule::WeekdaysAt { hour, minute }
        | Schedule::WeeklyAt { hour, minute, .. } => (hour, minute),
        _ => return None,
    };
    for delta in 0..=8 {
        let day = date.checked_add_days(chrono::Days::new(delta))?;
        let weekday = day.weekday().number_from_sunday() as u8;
        if matches!(schedule, Schedule::WeekdaysAt { .. }) && !(2..=6).contains(&weekday) {
            continue;
        }
        if let Schedule::WeeklyAt {
            weekday: target, ..
        } = schedule
            && *target != weekday
        {
            continue;
        }
        let local = day.and_hms_opt(u32::from(hour), u32::from(minute), 0)?;
        // Spring gaps are skipped; a fall fold runs once at the first occurrence.
        if let Some(next) = zone.from_local_datetime(&local).earliest()
            && let Ok(next) = u64::try_from(next.timestamp())
            && next > after
        {
            return Some(next);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_time_wall_clock_uses_selected_zone_and_rejects_dst_gaps() {
        let local = ScheduleLocalTime {
            local_datetime: "2026-10-13T09:00".to_owned(),
            timezone: "Asia/Tokyo".to_owned(),
        };
        assert_eq!(
            resolve_local_time(&local).unwrap(),
            at("2026-10-13T00:00:00Z")
        );
        assert!(
            resolve_local_time(&ScheduleLocalTime {
                local_datetime: "2026-03-08T02:30".to_owned(),
                timezone: "America/New_York".to_owned()
            })
            .is_err()
        );
    }
    fn at(value: &str) -> u64 {
        chrono::DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp() as u64
    }

    #[test]
    fn spring_gap_skips_nonexistent_local_time() {
        let schedule = Schedule::DailyAt {
            hour: 2,
            minute: 30,
        };
        assert_eq!(
            schedule.next_fire_in_timezone(at("2026-03-08T05:00:00Z"), "America/New_York"),
            Some(at("2026-03-09T06:30:00Z"))
        );
    }

    #[test]
    fn autumn_fold_runs_once_at_the_first_occurrence() {
        let schedule = Schedule::DailyAt {
            hour: 1,
            minute: 30,
        };
        assert_eq!(
            schedule.next_fire_in_timezone(at("2026-11-01T04:00:00Z"), "America/New_York"),
            Some(at("2026-11-01T05:30:00Z"))
        );
        assert_eq!(
            schedule.next_fire_in_timezone(at("2026-11-01T05:45:00Z"), "America/New_York"),
            Some(at("2026-11-02T06:30:00Z"))
        );
    }

    #[test]
    fn once_does_not_repeat_and_interval_does_not_overflow() {
        assert_eq!(Schedule::OnceAt { at: 100 }.next_fire(99, 0), Some(100));
        assert_eq!(Schedule::OnceAt { at: 100 }.next_fire(100, 0), None);
        assert_eq!(
            Schedule::Interval { minutes: 5 }.next_fire(u64::MAX, 0),
            None
        );
    }

    #[test]
    fn timezone_validation_rejects_unknown_zones() {
        let settings = ScheduleSettings {
            timezone: "not/a-zone".to_owned(),
            ..Default::default()
        };
        assert!(settings.validate().is_err());
        assert!(
            ScheduleSettings {
                timezone: "Asia/Shanghai".to_owned(),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }
}
