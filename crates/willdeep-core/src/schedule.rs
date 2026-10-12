//! Runtime 定时任务定义存储；插件和客户端通过 Runtime API 维护。
//!
//! 行为对齐 Xedit 的 Automations（`AgentRoutine`）：每次触发以任务的 prompt 为
//! 第一句开一个**全新会话**；带 `goal` 的任务每次运行都被要求判断目标是否达成，
//! 达成就调 `complete_scheduled_task` 停止后续触发并保留历史。调度 JSON 与 Xedit 同构
//! （`kind` + `minutes` / `hour` / `minute` / `weekday`，weekday 1 = 周日 … 7 = 周六）。
//!
//! 保留 `$WILLDEEP_HOME/schedules.json` 的兼容路径，Runtime 是写入方。
//! 每次读改写都拿 `schedules.lock` 的排他文件锁，整文件原子
//! 替换，权限 0600。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
pub use willdeep_runtime_protocol::schedule::{Schedule, ScheduledTask};

/// 本机在 `at`（Unix 秒）那一刻相对 UTC 的偏移秒数；非 Unix 平台按 UTC。
#[cfg(unix)]
// `tm_gmtoff` 是 `c_long`：64 位平台上已是 i64，32 位上不是。
#[allow(clippy::unnecessary_cast)]
pub fn local_utc_offset(at: u64) -> i64 {
    let time = at as libc::time_t;
    // SAFETY: localtime_r only writes the caller-owned `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { libc::localtime_r(&time, &mut tm) };
    if converted.is_null() {
        0
    } else {
        tm.tm_gmtoff as i64
    }
}

#[cfg(not(unix))]
pub fn local_utc_offset(_at: u64) -> i64 {
    0
}

/// `$WILLDEEP_HOME/schedules.json` 的读写。
#[derive(Clone, Debug)]
pub struct ScheduleStore {
    path: PathBuf,
    lock_path: PathBuf,
}

#[derive(Default, Serialize, Deserialize)]
struct StoreFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    tasks: Vec<ScheduledTask>,
}

impl ScheduleStore {
    pub fn new(home: &Path) -> Self {
        Self {
            path: home.join("schedules.json"),
            lock_path: home.join("schedules.lock"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Result<Vec<ScheduledTask>, String> {
        self.update(|tasks| tasks.clone())
    }

    pub fn try_update<R>(
        &self,
        edit: impl FnOnce(&mut Vec<ScheduledTask>) -> Result<R, String>,
    ) -> Result<R, String> {
        self.update(|tasks| {
            let before = tasks.clone();
            let result = edit(tasks);
            if result.is_err() {
                *tasks = before;
            }
            result
        })?
    }

    /// 持锁读改写：`edit` 改完的列表整文件原子替换回去。
    pub fn update<R>(&self, edit: impl FnOnce(&mut Vec<ScheduledTask>) -> R) -> Result<R, String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create {}: {error}", parent.display()))?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lock_path)
            .map_err(|error| format!("open {}: {error}", self.lock_path.display()))?;
        lock.lock()
            .map_err(|error| format!("lock {}: {error}", self.lock_path.display()))?;
        let mut file = match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str::<StoreFile>(&text)
                .map_err(|error| format!("parse {}: {error}", self.path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreFile::default(),
            Err(error) => return Err(format!("read {}: {error}", self.path.display())),
        };
        let before = file.tasks.clone();
        let result = edit(&mut file.tasks);
        if file.tasks != before {
            file.version = 1;
            self.write(&file)?;
        }
        Ok(result)
    }

    fn write(&self, file: &StoreFile) -> Result<(), String> {
        let text = serde_json::to_vec_pretty(file).map_err(|error| error.to_string())?;
        let temporary = self
            .path
            .with_extension(format!("json.{}.tmp", Uuid::new_v4().simple()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut handle = options
            .open(&temporary)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        handle
            .write_all(&text)
            .and_then(|()| handle.sync_all())
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, &self.path)
            .map_err(|error| format!("replace {}: {error}", self.path.display()))
    }
}

/// 解析 `HH:MM`（24 小时制）。
pub fn parse_hhmm(raw: &str) -> Option<(u8, u8)> {
    let (hour, minute) = raw.trim().split_once(':')?;
    let hour: u8 = hour.trim().parse().ok()?;
    let minute: u8 = minute.trim().parse().ok()?;
    (hour <= 23 && minute <= 59).then_some((hour, minute))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-30T00:00:00Z，周三。
    const WEDNESDAY_MIDNIGHT_UTC: u64 = 1_790_726_400;
    const BEIJING: i64 = 8 * 3_600;

    #[test]
    fn schedules_round_trip_in_the_xedit_shape() {
        let json = serde_json::to_value(Schedule::WeeklyAt {
            weekday: 2,
            hour: 9,
            minute: 30,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "weeklyAt", "weekday": 2, "hour": 9, "minute": 30})
        );
        let interval: Schedule =
            serde_json::from_value(serde_json::json!({"kind": "interval", "minutes": 30})).unwrap();
        assert_eq!(interval, Schedule::Interval { minutes: 30 });
    }

    #[test]
    fn next_fire_follows_local_time() {
        // 北京时间 08:00 周三。
        let now = WEDNESDAY_MIDNIGHT_UTC;
        let daily = Schedule::DailyAt { hour: 9, minute: 0 };
        // 当天 09:00 北京 = 01:00Z。
        assert_eq!(daily.next_fire(now, BEIJING), Some(now + 3_600));
        // 过了点就到明天。
        assert_eq!(
            daily.next_fire(now + 3_600, BEIJING),
            Some(now + 3_600 + 86_400)
        );
        assert_eq!(
            Schedule::Interval { minutes: 30 }.next_fire(now, BEIJING),
            Some(now + 1_800)
        );
        // 周五 18:00 之后的工作日 09:00 是下周一。
        let friday_evening_local = now + 2 * 86_400 + 10 * 3_600; // 周五 18:00 北京
        let weekdays = Schedule::WeekdaysAt { hour: 9, minute: 0 };
        assert_eq!(
            weekdays.next_fire(friday_evening_local, BEIJING),
            Some(now + 5 * 86_400 + 3_600)
        );
        // 每周日 10:00 北京 = 周日 02:00Z。
        let sunday = Schedule::WeeklyAt {
            weekday: 1,
            hour: 10,
            minute: 0,
        };
        assert_eq!(
            sunday.next_fire(now, BEIJING),
            Some(now + 4 * 86_400 + 2 * 3_600)
        );
    }

    #[test]
    fn validation_and_hhmm_parsing() {
        assert!(Schedule::Interval { minutes: 0 }.validate().is_err());
        assert!(
            Schedule::DailyAt {
                hour: 24,
                minute: 0
            }
            .validate()
            .is_err()
        );
        assert!(
            Schedule::WeeklyAt {
                weekday: 8,
                hour: 1,
                minute: 0
            }
            .validate()
            .is_err()
        );
        assert_eq!(parse_hhmm(" 07:05 "), Some((7, 5)));
        assert_eq!(parse_hhmm("7:60"), None);
        assert_eq!(parse_hhmm("noon"), None);
        assert_eq!(
            Schedule::Interval { minutes: 120 }.summary(),
            "every 2 hours"
        );
        assert_eq!(
            Schedule::DailyAt { hour: 9, minute: 5 }.summary(),
            "daily at 09:05"
        );
    }

    fn task(schedule: Schedule) -> ScheduledTask {
        ScheduledTask {
            id: Uuid::new_v4(),
            name: "check CI".to_owned(),
            prompt: "Check CI on main".to_owned(),
            workspace: PathBuf::from("/ws"),
            schedule,
            goal: None,
            approval_mode: None,
            enabled: true,
            created_at: WEDNESDAY_MIDNIGHT_UTC,
            last_run_at: None,
            last_session_id: None,
            origin_session_id: None,
            settings: Default::default(),
        }
    }

    #[test]
    fn missed_runs_collapse_into_one() {
        let mut every_ten = task(Schedule::Interval { minutes: 10 });
        let much_later = WEDNESDAY_MIDNIGHT_UTC + 3 * 3_600;
        assert!(every_ten.is_due(much_later, 0));
        every_ten.last_run_at = Some(much_later);
        assert!(
            !every_ten.is_due(much_later + 60, 0),
            "fires once, not 18 times"
        );
        every_ten.enabled = false;
        assert!(!every_ten.is_due(much_later + 3_600, 0));
    }

    #[test]
    fn goal_tasks_are_told_how_to_complete_themselves() {
        let mut watch = task(Schedule::Interval { minutes: 30 });
        assert!(!watch.run_prompt().contains("complete_scheduled_task"));
        watch.goal = Some("CI on main is green".to_owned());
        let prompt = watch.run_prompt();
        assert!(prompt.contains("Check CI on main"));
        assert!(prompt.contains("CI on main is green"));
        assert!(prompt.contains(&format!("task_id \"{}\"", watch.id)));
    }

    #[test]
    fn the_store_round_trips_and_is_private() {
        let home = std::env::temp_dir().join(format!("willdeep-schedules-{}", Uuid::new_v4()));
        let store = ScheduleStore::new(&home);
        assert!(store.list().unwrap().is_empty());
        let created = task(Schedule::DailyAt { hour: 9, minute: 0 });
        store.update(|tasks| tasks.push(created.clone())).unwrap();
        assert_eq!(store.list().unwrap(), vec![created.clone()]);
        let removed = store
            .update(|tasks| {
                let before = tasks.len();
                tasks.retain(|task| task.id != created.id);
                before != tasks.len()
            })
            .unwrap();
        assert!(removed);
        assert!(store.list().unwrap().is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}
