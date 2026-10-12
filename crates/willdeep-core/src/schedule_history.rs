//! Durable scheduled-run records. The index contains no prompts and can be rebuilt.
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use willdeep_runtime_protocol::schedule::{
    ScheduleHistoryPage, ScheduleHistoryQuery, ScheduleRun, ScheduleRunStatus,
};

const DEFAULT_PAGE_SIZE: usize = 25;
const MAX_PAGE_SIZE: usize = 100;
const SECONDS_PER_DAY: u64 = 86_400;
const INDEX_VERSION: u32 = 1;

#[derive(Clone)]
pub struct ScheduleHistoryStore {
    root: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
struct RunIndex {
    #[serde(default)]
    sequence: u64,
    id: Uuid,
    cursor: String,
    status: ScheduleRunStatus,
    revision: u64,
    finished_at: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    runs: Vec<RunIndex>,
}

impl ScheduleHistoryStore {
    pub fn retain_deleted_task(
        &self,
        task: &willdeep_runtime_protocol::schedule::ScheduledTask,
        now: u64,
    ) -> Result<(), String> {
        let _guard = self.lock(task.id)?;
        atomic_json(
            &self.directory(task.id).join("deleted-task.json"),
            &willdeep_runtime_protocol::schedule::DeletedScheduledTask {
                task: task.clone(),
                deleted_at: now,
            },
        )
    }
    pub fn deleted_tasks(
        &self,
    ) -> Result<Vec<willdeep_runtime_protocol::schedule::DeletedScheduledTask>, String> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut tasks = Vec::new();
        for directory in std::fs::read_dir(&self.root)
            .map_err(|error| format!("list retained task metadata: {error}"))?
        {
            let directory =
                directory.map_err(|error| format!("read retained task directory: {error}"))?;
            if directory
                .file_name()
                .to_str()
                .and_then(|name| Uuid::parse_str(name).ok())
                .is_none()
            {
                continue;
            }
            match std::fs::read(directory.path().join("deleted-task.json")) {
                Ok(bytes) => tasks.push(
                    serde_json::from_slice(&bytes)
                        .map_err(|error| format!("parse retained task metadata: {error}"))?,
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("read retained task metadata: {error}")),
            }
        }
        Ok(tasks)
    }
    pub fn new(home: &Path) -> Self {
        Self {
            root: home.join("runtime/schedules/runs"),
        }
    }

    fn directory(&self, task_id: Uuid) -> PathBuf {
        self.root.join(task_id.to_string())
    }

    fn lock(&self, task_id: Uuid) -> Result<File, String> {
        let directory = self.directory(task_id);
        std::fs::create_dir_all(&directory).map_err(|e| format!("create schedule history: {e}"))?;
        let handle = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(directory.join("history.lock"))
            .map_err(|e| format!("open schedule history lock: {e}"))?;
        handle
            .lock()
            .map_err(|e| format!("lock schedule history: {e}"))?;
        Ok(handle)
    }

    pub fn get(&self, task_id: Uuid, run_id: Uuid) -> Result<Option<ScheduleRun>, String> {
        let path = self.directory(task_id).join(format!("{run_id}.json"));
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| format!("parse schedule run: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("read schedule run: {e}")),
        }
    }

    pub fn dispatch_lock(&self, task_id: Uuid, run_id: Uuid) -> Result<File, String> {
        std::fs::create_dir_all(self.directory(task_id))
            .map_err(|e| format!("create schedule dispatch directory: {e}"))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.directory(task_id).join(format!("{run_id}.lock")))
            .map_err(|e| format!("open schedule dispatch lock: {e}"))?;
        lock.lock()
            .map_err(|e| format!("lock schedule dispatch: {e}"))?;
        Ok(lock)
    }

    pub fn insert(&self, run: &ScheduleRun) -> Result<ScheduleRun, String> {
        let _lock = self.lock(run.task_id)?;
        if let Some(existing) = self.get(run.task_id, run.id)? {
            if existing.key != run.key {
                return Err("schedule run ID already belongs to another trigger".to_owned());
            }
            return Ok(existing);
        }
        let mut run = run.clone();
        let path = self.directory(run.task_id).join("sequence.json");
        let saved = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<u64>(&bytes)
                .map_err(|e| format!("read schedule sequence: {e}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(format!("read schedule sequence: {error}")),
        };
        let previous = self
            .index(run.task_id)?
            .iter()
            .map(|row| row.sequence)
            .max()
            .unwrap_or(0)
            .max(saved);
        run.sequence = previous
            .checked_add(1)
            .ok_or("schedule sequence exhausted")?;
        atomic_json(&path, &run.sequence)?;
        self.write_run(&run)?;
        Ok(run)
    }

    pub fn put(&self, run: &ScheduleRun) -> Result<(), String> {
        let _lock = self.lock(run.task_id)?;
        if let Some(existing) = self.get(run.task_id, run.id)? {
            if existing.key != run.key
                || existing.snapshot != run.snapshot
                || existing.sequence != run.sequence
            {
                return Err(
                    "schedule run identity and configuration snapshot are immutable".to_owned(),
                );
            }
            if existing.status.is_terminal() && existing.status != run.status {
                return Err(
                    "a terminal run cannot become active again; create a retry record".to_owned(),
                );
            }
        }
        self.write_run(run)
    }

    fn write_run(&self, run: &ScheduleRun) -> Result<(), String> {
        let directory = self.directory(run.task_id);
        let mut index = self.index(run.task_id)?;
        atomic_json(&directory.join("index-dirty.json"), &run.id)?;
        atomic_json(&directory.join(format!("{}.json", run.id)), run)?;
        index.retain(|row| row.id != run.id);
        index.push(RunIndex {
            sequence: run.sequence,
            id: run.id,
            cursor: cursor(run),
            status: run.status,
            revision: run.snapshot.settings.revision,
            finished_at: run.finished_at,
        });
        index.sort_by(|a, b| b.cursor.cmp(&a.cursor));
        atomic_json(
            &directory.join("index.json"),
            &IndexFile {
                version: INDEX_VERSION,
                runs: index,
            },
        )?;
        std::fs::remove_file(directory.join("index-dirty.json"))
            .map_err(|e| format!("finish schedule index transaction: {e}"))
    }

    fn index(&self, task_id: Uuid) -> Result<Vec<RunIndex>, String> {
        let directory = self.directory(task_id);
        if !directory.exists() {
            return Ok(Vec::new());
        }
        if !directory.join("index-dirty.json").exists() {
            match std::fs::read(directory.join("index.json")) {
                Ok(bytes) => {
                    if let Ok(file) = serde_json::from_slice::<IndexFile>(&bytes)
                        && file.version == INDEX_VERSION
                    {
                        return Ok(file.runs);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("read schedule history index: {error}")),
            }
        }
        let mut ids = Vec::new();
        for entry in
            std::fs::read_dir(&directory).map_err(|e| format!("list schedule runs: {e}"))?
        {
            let entry = entry.map_err(|e| format!("read schedule history entry: {e}"))?;
            if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
                && let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str())
                && let Ok(id) = Uuid::parse_str(stem)
            {
                ids.push(id);
            }
        }
        let mut index = Vec::new();
        for id in ids {
            if let Some(run) = self.get(task_id, id)? {
                index.push(RunIndex {
                    sequence: run.sequence,
                    id,
                    cursor: cursor(&run),
                    status: run.status,
                    revision: run.snapshot.settings.revision,
                    finished_at: run.finished_at,
                });
            }
        }
        index.sort_by(|a, b| b.cursor.cmp(&a.cursor));
        atomic_json(
            &directory.join("index.json"),
            &IndexFile {
                version: INDEX_VERSION,
                runs: index.clone(),
            },
        )?;
        if directory.join("index-dirty.json").exists() {
            std::fs::remove_file(directory.join("index-dirty.json"))
                .map_err(|e| format!("recover schedule history index: {e}"))?;
        }
        Ok(index)
    }

    pub fn list(&self, query: &ScheduleHistoryQuery) -> Result<ScheduleHistoryPage, String> {
        let _lock = self.lock(query.task_id)?;
        let limit = query
            .limit
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        let index = self.index(query.task_id)?;
        let rows = index
            .iter()
            .filter(|row| {
                query.status.is_none_or(|status| row.status == status)
                    && query
                        .before
                        .as_ref()
                        .is_none_or(|before| row.cursor < *before)
            })
            .take(limit + 1)
            .collect::<Vec<_>>();
        let next_cursor = (rows.len() > limit).then(|| rows[limit - 1].cursor.clone());
        let mut runs = Vec::new();
        for row in rows.into_iter().take(limit) {
            if let Some(run) = self.get(query.task_id, row.id)? {
                runs.push(run);
            }
        }
        Ok(ScheduleHistoryPage { runs, next_cursor })
    }

    pub fn active(&self, task_id: Uuid) -> Result<Vec<ScheduleRun>, String> {
        let _lock = self.lock(task_id)?;
        let mut runs = Vec::new();
        for row in self
            .index(task_id)?
            .into_iter()
            .filter(|row| !row.status.is_terminal())
        {
            if let Some(run) = self.get(task_id, row.id)? {
                runs.push(run);
            }
        }
        Ok(runs)
    }

    pub fn outcome_summary(
        &self,
        task_id: Uuid,
        revision: u64,
        failure_limit: u32,
    ) -> Result<(Option<ScheduleRun>, u32), String> {
        let _lock = self.lock(task_id)?;
        let mut latest = None;
        let mut failures = 0;
        let mut streak_ended = false;
        for row in self
            .index(task_id)?
            .into_iter()
            .filter(|row| row.status.is_terminal() && row.revision == revision)
        {
            if latest.is_some()
                && !matches!(
                    row.status,
                    ScheduleRunStatus::Completed
                        | ScheduleRunStatus::Failed
                        | ScheduleRunStatus::Partial
                        | ScheduleRunStatus::Cancelled
                        | ScheduleRunStatus::Unknown
                )
            {
                continue;
            }
            let Some(run) = self.get(task_id, row.id)? else {
                continue;
            };
            if run.snapshot.settings.revision != revision {
                continue;
            }
            if latest.is_none() {
                latest = Some(run.clone());
            }
            if matches!(
                run.status,
                ScheduleRunStatus::Completed
                    | ScheduleRunStatus::Partial
                    | ScheduleRunStatus::Cancelled
                    | ScheduleRunStatus::Unknown
            ) {
                streak_ended = true;
            }
            if !streak_ended && run.status == ScheduleRunStatus::Failed {
                failures += 1;
            }
            if streak_ended || failures >= failure_limit {
                break;
            }
        }
        Ok((latest, failures))
    }

    pub fn cleanup(
        &self,
        task_id: Uuid,
        now: u64,
        retention_days: u32,
        preview: bool,
    ) -> Result<Vec<Uuid>, String> {
        let _lock = self.lock(task_id)?;
        let cutoff = now.saturating_sub(u64::from(retention_days) * SECONDS_PER_DAY);
        let mut index = self.index(task_id)?;
        let deleted = index
            .iter()
            .filter(|row| row.status.is_terminal() && row.finished_at.is_some_and(|at| at < cutoff))
            .map(|row| row.id)
            .collect::<Vec<_>>();
        if !preview && !deleted.is_empty() {
            let directory = self.directory(task_id);
            atomic_json(&directory.join("index-dirty.json"), &deleted)?;
            for id in &deleted {
                std::fs::remove_file(directory.join(format!("{id}.json")))
                    .map_err(|e| format!("remove expired schedule run: {e}"))?;
                let lock = directory.join(format!("{id}.lock"));
                if lock.exists() {
                    std::fs::remove_file(lock)
                        .map_err(|e| format!("remove expired schedule lock: {e}"))?;
                }
            }
            let ids = deleted.iter().collect::<std::collections::HashSet<_>>();
            index.retain(|row| !ids.contains(&row.id));
            atomic_json(
                &directory.join("index.json"),
                &IndexFile {
                    version: INDEX_VERSION,
                    runs: index,
                },
            )?;
            std::fs::remove_file(directory.join("index-dirty.json"))
                .map_err(|e| format!("finish history cleanup: {e}"))?;
        }
        Ok(deleted)
    }
}

fn cursor(run: &ScheduleRun) -> String {
    format!("{:020}:{}", run.sequence, run.id)
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|e| format!("create schedule record: {e}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("write schedule record: {e}"))?;
    std::fs::rename(&temporary, path).map_err(|e| format!("replace schedule record: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use willdeep_runtime_protocol::schedule::{Schedule, ScheduleDraft, ScheduleTrigger};
    struct Fixture {
        root: PathBuf,
        store: ScheduleHistoryStore,
        task: willdeep_runtime_protocol::schedule::ScheduledTask,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("schedule-history-{}", Uuid::new_v4()));
            let task = ScheduleDraft {
                name: "test".to_owned(),
                prompt: "check".to_owned(),
                workspace: std::env::temp_dir(),
                schedule: Schedule::Interval { minutes: 5 },
                goal: None,
                approval_mode: None,
                settings: Default::default(),
            }
            .into_task(Uuid::new_v4(), 1);
            Self {
                store: ScheduleHistoryStore::new(&root),
                root,
                task,
            }
        }
        fn run(&self, time: u64) -> ScheduleRun {
            let key = format!("scheduled:{}:{time}", self.task.id);
            ScheduleRun {
                sequence: 0,
                id: Uuid::new_v5(&Uuid::NAMESPACE_OID, key.as_bytes()),
                task_id: self.task.id,
                key,
                trigger: ScheduleTrigger::Scheduled,
                scheduled_at: time,
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
                snapshot: self.task.clone(),
            }
        }
        fn query(&self) -> ScheduleHistoryQuery {
            ScheduleHistoryQuery {
                task_id: self.task.id,
                before: None,
                limit: Some(1),
                status: None,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn repeated_trigger_recovers_the_original_record_and_snapshot() {
        let fixture = Fixture::new();
        let mut run = fixture.store.insert(&fixture.run(100)).unwrap();
        run.snapshot.prompt = "different".to_owned();
        assert_eq!(fixture.store.insert(&run).unwrap().snapshot.prompt, "check");
        assert!(fixture.store.put(&run).is_err());
    }

    #[test]
    fn missing_index_rebuilds_and_pagination_has_no_duplicates() {
        let fixture = Fixture::new();
        fixture.store.insert(&fixture.run(100)).unwrap();
        fixture.store.insert(&fixture.run(200)).unwrap();
        std::fs::remove_file(fixture.store.directory(fixture.task.id).join("index.json")).unwrap();
        let first = fixture.store.list(&fixture.query()).unwrap();
        assert_eq!(first.runs[0].scheduled_at, 200);
        let next = fixture
            .store
            .list(&ScheduleHistoryQuery {
                before: first.next_cursor,
                ..fixture.query()
            })
            .unwrap();
        assert_eq!(next.runs[0].scheduled_at, 100);
        assert!(next.next_cursor.is_none());
    }

    #[test]
    fn dirty_transaction_recovers_new_record_status_from_stale_index() {
        let fixture = Fixture::new();
        let mut run = fixture.store.insert(&fixture.run(100)).unwrap();
        run.status = ScheduleRunStatus::Failed;
        run.finished_at = Some(101);
        let directory = fixture.store.directory(fixture.task.id);
        atomic_json(&directory.join("index-dirty.json"), &run.id).unwrap();
        atomic_json(&directory.join(format!("{}.json", run.id)), &run).unwrap();
        let page = fixture.store.list(&fixture.query()).unwrap();
        assert_eq!(page.runs[0].status, ScheduleRunStatus::Failed);
        assert!(fixture.store.active(fixture.task.id).unwrap().is_empty());
        assert!(!directory.join("index-dirty.json").exists());
    }

    #[test]
    fn terminal_history_cannot_be_reused_as_a_retry() {
        let fixture = Fixture::new();
        let mut run = fixture.run(100);
        run.status = ScheduleRunStatus::Failed;
        let mut run = fixture.store.insert(&run).unwrap();
        run.status = ScheduleRunStatus::Running;
        assert!(fixture.store.put(&run).unwrap_err().contains("terminal"));
    }

    #[test]
    fn creation_order_survives_clock_skew_and_cleanup() {
        let fixture = Fixture::new();
        let mut first = fixture.run(200);
        first.status = ScheduleRunStatus::Completed;
        first.finished_at = Some(200);
        let first = fixture.store.insert(&first).unwrap();
        let mut later = fixture.run(100);
        later.status = ScheduleRunStatus::Failed;
        later.finished_at = Some(201);
        let later = fixture.store.insert(&later).unwrap();
        assert!(later.sequence > first.sequence);
        let (latest, failures) = fixture
            .store
            .outcome_summary(fixture.task.id, 1, 3)
            .unwrap();
        assert_eq!(latest.unwrap().id, later.id);
        assert_eq!(failures, 1);
        fixture
            .store
            .cleanup(fixture.task.id, SECONDS_PER_DAY * 100, 90, false)
            .unwrap();
        let next = fixture.store.insert(&fixture.run(300)).unwrap();
        assert!(next.sequence > later.sequence);
    }

    #[test]
    fn cleanup_preview_preserves_active_and_recent_records() {
        let fixture = Fixture::new();
        fixture.store.insert(&fixture.run(100)).unwrap();
        let mut old = fixture.run(200);
        old.status = ScheduleRunStatus::Completed;
        old.finished_at = Some(200);
        fixture.store.insert(&old).unwrap();
        let now = SECONDS_PER_DAY * 100;
        assert_eq!(
            fixture
                .store
                .cleanup(fixture.task.id, now, 90, true)
                .unwrap(),
            [old.id]
        );
        assert!(
            fixture
                .store
                .get(fixture.task.id, old.id)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            fixture
                .store
                .cleanup(fixture.task.id, now, 90, false)
                .unwrap(),
            [old.id]
        );
        assert_eq!(fixture.store.active(fixture.task.id).unwrap().len(), 1);
    }
}
