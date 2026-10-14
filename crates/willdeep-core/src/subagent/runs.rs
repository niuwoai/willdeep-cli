//! Durable execution identities. Read APIs do not recover or mutate live runs.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use willdeep_runtime_protocol::{AgentRun, AgentRunReason, AgentRunsPage, AgentStatus};

use crate::types::Usage;

const VERSION: u32 = 1;
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const PAGE_LIMIT: usize = 64;
const REPORT_LIMIT: usize = 64 * 1024;

mod continuation;
mod retirement;
mod team;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct StoredRun {
    version: u32,
    pub record: AgentRun,
    /// Private dispatch/permission snapshot; never returned by public queries.
    pub dispatch: serde_json::Value,
}

#[derive(Clone)]
pub struct AgentRunStore {
    directory: PathBuf,
    parent: Uuid,
}

#[derive(Clone)]
pub(crate) struct RunRequest {
    pub request_id: Uuid,
    pub agent_id: Uuid,
    pub reason: AgentRunReason,
    pub label: String,
    pub model: Option<String>,
    pub max_turns: Option<u64>,
    pub token_budget: Option<u64>,
    pub timeout_seconds: Option<u64>,
    pub dispatch: serde_json::Value,
}

impl AgentRunStore {
    pub fn needs_recovery(&self) -> Result<bool, String> {
        Ok(self.entries()?.iter().any(|entry| {
            entry.record.is_pending() || (!entry.record.delivered && entry.record.report.is_some())
        }))
    }

    pub fn acknowledge_report(&self, id: Uuid) -> Result<(), String> {
        let run = self.get(id)?.ok_or("Agent run was not found")?;
        if run.is_pending() {
            return Err("a pending Agent run has no final report to acknowledge".into());
        }
        self.mark_delivered(id)
    }
    pub(crate) fn claim(&self) -> Result<std::fs::File, String> {
        self.locked(|| {
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).create(true).truncate(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let file = options
                .open(self.directory.join(".owner"))
                .map_err(storage_error)?;
            if !file.metadata().map_err(storage_error)?.is_file() {
                return Err("Agent team owner lock is not a file".into());
            }
            file.try_lock().map_err(|error| match error {
                std::fs::TryLockError::WouldBlock => {
                    "this Agent team is already owned by another execution host".into()
                }
                std::fs::TryLockError::Error(error) => storage_error(error),
            })?;
            Ok(file)
        })
    }
    pub fn new(home: impl AsRef<Path>, parent: Uuid) -> Self {
        Self {
            directory: home.as_ref().join("team-runs").join(parent.to_string()),
            parent,
        }
    }

    pub(crate) fn create(&self, request: RunRequest) -> Result<(AgentRun, bool), String> {
        self.locked(|| self.create_locked(request))
    }

    fn create_locked(&self, request: RunRequest) -> Result<(AgentRun, bool), String> {
        if let Some(mut existing) = self.load(request.request_id)? {
            if existing.record.agent_id != request.agent_id
                || existing.dispatch != request.dispatch
                || existing.record.reason != request.reason
            {
                return Err("Agent request ID was already used for a different request".into());
            }
            if existing.record.status == AgentStatus::Queued && existing.record.max_turns.is_none()
            {
                existing.record.max_turns = request.max_turns;
                existing.record.token_budget = request.token_budget;
                existing.record.timeout_seconds = request.timeout_seconds;
                self.save(&existing)?;
            }
            if existing.record.is_pending() {
                self.ensure_agent_available(request.agent_id)?;
            }
            return Ok((existing.record, false));
        }
        self.ensure_agent_available(request.agent_id)?;
        let entries = self.entries()?;
        let policy = self.team_policy()?;
        let defaults = super::scheduler::AgentTeamSettings::default();
        let capacity = policy.map_or(defaults.max_concurrent + defaults.max_queued, |policy| {
            policy.max_concurrent + policy.max_queued
        });
        if entries
            .iter()
            .filter(|entry| entry.record.is_pending())
            .count()
            >= capacity
        {
            return Err("Agent team queue capacity is exhausted".into());
        }
        if entries
            .iter()
            .any(|entry| entry.record.agent_id == request.agent_id && entry.record.is_pending())
        {
            return Err("this Agent already has an unfinished run".into());
        }
        let sequence = entries
            .iter()
            .map(|entry| entry.record.sequence)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("Agent run sequence exhausted")?;
        let record = AgentRun {
            id: request.request_id,
            request_id: request.request_id,
            agent_id: request.agent_id,
            parent_session_id: self.parent,
            sequence,
            reason: request.reason,
            status: AgentStatus::Queued,
            label: request.label,
            model: request.model,
            queued_at_ms: crate::usage_ledger::now_millis(),
            started_at_ms: None,
            finished_at_ms: None,
            max_turns: request.max_turns,
            token_budget: request.token_budget,
            timeout_seconds: request.timeout_seconds,
            current_turn: 0,
            current_tool: None,
            usage: Default::default(),
            report: None,
            delivered: false,
        };
        self.save(&StoredRun {
            version: VERSION,
            record: record.clone(),
            dispatch: request.dispatch,
        })?;
        Ok((record, true))
    }

    pub fn get(&self, id: Uuid) -> Result<Option<AgentRun>, String> {
        Ok(self.load(id)?.map(|entry| entry.record))
    }

    pub(crate) fn latest(&self, agent: Uuid) -> Result<Option<StoredRun>, String> {
        Ok(self
            .entries()?
            .into_iter()
            .filter(|entry| entry.record.agent_id == agent)
            .max_by_key(|entry| entry.record.sequence))
    }

    pub(crate) fn workspace_was_used(&self, agent: Uuid, current: Uuid) -> Result<bool, String> {
        let entries = self.entries()?;
        Ok(entries.iter().any(|entry| {
            entry.record.agent_id == agent
                && entry.record.id != current
                && entry.record.started_at_ms.is_some()
        }) || entries
            .iter()
            .find(|entry| entry.record.id == current)
            .is_some_and(|entry| {
                entry.record.reason != AgentRunReason::Spawn
                    && !entries.iter().any(|previous| {
                        previous.record.agent_id == agent && previous.record.id != current
                    })
            }))
    }

    pub fn list(
        &self,
        agent: Option<Uuid>,
        after: Option<u64>,
        limit: usize,
    ) -> Result<AgentRunsPage, String> {
        let mut runs: Vec<_> = self
            .entries()?
            .into_iter()
            .map(|entry| entry.record)
            .filter(|run| {
                agent.is_none_or(|id| run.agent_id == id)
                    && after.is_none_or(|sequence| run.sequence > sequence)
            })
            .collect();
        runs.sort_by_key(|run| run.sequence);
        let limit = limit.clamp(1, PAGE_LIMIT);
        let next = (runs.len() > limit).then(|| runs[limit - 1].sequence);
        runs.truncate(limit);
        Ok(AgentRunsPage {
            runs,
            next_after_sequence: next,
        })
    }

    pub(crate) fn pending(&self) -> Result<Vec<StoredRun>, String> {
        let mut entries = self.entries()?;
        entries.retain(|entry| entry.record.status == AgentStatus::Queued);
        entries.sort_by_key(|entry| entry.record.sequence);
        Ok(entries)
    }

    /// Only the owner of the parent session may invoke recovery, never a query.
    pub(crate) fn interrupt_running(&self) -> Result<usize, String> {
        self.locked(|| {
            let mut count = 0;
            for mut entry in self.entries()? {
                if entry.record.status != AgentStatus::Running {
                    continue;
                }
                entry.record.status = AgentStatus::Interrupted;
                entry.record.usage.uncertain_execution = true;
                entry.record.finished_at_ms = Some(crate::usage_ledger::now_millis());
                entry.record.current_tool = None;
                entry.record.report = Some("The execution host restarted. This run was interrupted; uncertain tools were not replayed. Inspect the existing workspace before explicitly continuing or retrying.".into());
                self.save(&entry)?;
                count += 1;
            }
            Ok(count)
        })
    }

    pub(crate) fn start(&self, id: Uuid) -> Result<AgentRun, String> {
        self.update(id, |run| {
            if run.status != AgentStatus::Queued {
                return Err("only a queued Agent run may start".into());
            }
            run.status = AgentStatus::Running;
            run.started_at_ms = Some(crate::usage_ledger::now_millis());
            Ok(())
        })
    }

    pub(crate) fn finish(
        &self,
        id: Uuid,
        status: AgentStatus,
        report: &str,
    ) -> Result<AgentRun, String> {
        if matches!(
            status,
            AgentStatus::Queued
                | AgentStatus::Running
                | AgentStatus::WaitingAnswer
                | AgentStatus::WaitingApproval
        ) {
            return Err("Agent terminal status is required".into());
        }
        self.update(id, |run| {
            if !run.is_pending() {
                return Err("Agent run has already settled".into());
            }
            run.status = status;
            run.finished_at_ms = Some(crate::usage_ledger::now_millis());
            run.current_tool = None;
            run.report = Some(crate::background::keep_head_and_tail(
                report,
                REPORT_LIMIT * 3 / 4,
                REPORT_LIMIT / 4,
            ));
            Ok(())
        })
    }

    pub(crate) fn mark_delivered(&self, id: Uuid) -> Result<(), String> {
        self.update(id, |run| {
            run.delivered = true;
            Ok(())
        })
        .map(|_| ())
    }

    pub(crate) fn usage(&self, id: Uuid, usage: &Usage) -> Result<(), String> {
        self.update(id, |run| {
            run.usage.model_calls = run.usage.model_calls.saturating_add(1);
            if usage.total_tokens.is_none()
                && (usage.input_tokens.is_none() || usage.output_tokens.is_none())
            {
                run.usage.unknown_usage_calls = run.usage.unknown_usage_calls.saturating_add(1);
            }
            add(&mut run.usage.input_tokens, usage.input_tokens);
            add(&mut run.usage.output_tokens, usage.output_tokens);
            add(&mut run.usage.cache_read_tokens, usage.cache_read_tokens);
            let total = usage.total_tokens.or_else(|| {
                usage
                    .input_tokens
                    .zip(usage.output_tokens)
                    .map(|(input, output)| input.saturating_add(output))
            });
            add(&mut run.usage.total_tokens, total);
            run.usage.accounted_tokens = run
                .usage
                .accounted_tokens
                .saturating_add(usage.billable_tokens());
            Ok(())
        })
        .map(|_| ())
    }

    pub(crate) fn progress(
        &self,
        id: Uuid,
        turn: Option<u64>,
        tool: Option<Option<String>>,
    ) -> Result<(), String> {
        self.update(id, |run| {
            if let Some(turn) = turn {
                run.current_turn = turn;
            }
            if let Some(tool) = tool {
                run.current_tool = tool;
            }
            Ok(())
        })
        .map(|_| ())
    }

    fn update(
        &self,
        id: Uuid,
        change: impl FnOnce(&mut AgentRun) -> Result<(), String>,
    ) -> Result<AgentRun, String> {
        self.locked(|| {
            let mut entry = self.load(id)?.ok_or("Agent run was not found")?;
            change(&mut entry.record)?;
            self.save(&entry)?;
            Ok(entry.record)
        })
    }

    pub(crate) fn load(&self, id: Uuid) -> Result<Option<StoredRun>, String> {
        let path = self.directory.join(format!("{id}.json"));
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage_error(error)),
        };
        if !file.metadata().map_err(storage_error)?.is_file() {
            return Err("Agent run record is not a file".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(storage_error)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Agent run record exceeds size limit".into());
        }
        let entry: StoredRun =
            serde_json::from_slice(&bytes).map_err(|_| "invalid Agent run record")?;
        if entry.version != VERSION
            || entry.record.id != id
            || entry.record.parent_session_id != self.parent
        {
            return Err("Agent run record identity or version mismatch".into());
        }
        Ok(Some(entry))
    }

    fn entries(&self) -> Result<Vec<StoredRun>, String> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(storage_error(error)),
        };
        let mut result = Vec::new();
        for entry in entries {
            let path = entry.map_err(storage_error)?.path();
            if path.extension().is_none_or(|value| value != "json") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|value| value.to_str())
                .and_then(|value| value.parse().ok())
            else {
                continue;
            };
            if let Some(entry) = self.load(id)? {
                result.push(entry);
            }
        }
        Ok(result)
    }

    fn locked<T>(&self, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&self.directory).map_err(storage_error)?;
        if !std::fs::symlink_metadata(&self.directory)
            .map_err(storage_error)?
            .is_dir()
        {
            return Err("Agent run directory is not a directory".into());
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let lock = options
            .open(self.directory.join(".lock"))
            .map_err(storage_error)?;
        if !lock.metadata().map_err(storage_error)?.is_file() {
            return Err("Agent run lock is not a file".into());
        }
        lock.lock().map_err(storage_error)?;
        work()
    }

    fn save(&self, entry: &StoredRun) -> Result<(), String> {
        let bytes = serde_json::to_vec(entry).map_err(|_| "cannot encode Agent run record")?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("Agent run record exceeds size limit".into());
        }
        let temporary = self.directory.join(format!(".{}.pending", Uuid::new_v4()));
        let result = (|| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary).map_err(storage_error)?;
            file.write_all(&bytes).map_err(storage_error)?;
            file.sync_all().map_err(storage_error)?;
            std::fs::rename(
                &temporary,
                self.directory.join(format!("{}.json", entry.record.id)),
            )
            .map_err(storage_error)?;
            #[cfg(unix)]
            std::fs::File::open(&self.directory)
                .and_then(|file| file.sync_all())
                .map_err(storage_error)?;
            Ok(())
        })();
        if result.is_err()
            && let Err(error) = std::fs::remove_file(&temporary)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(storage_error(error));
        }
        result
    }
}

fn add(total: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or(0).saturating_add(value));
    }
}

fn storage_error(error: impl std::fmt::Display) -> String {
    format!("Agent run storage: {error}")
}

pub(crate) struct RunProgressSink {
    pub store: AgentRunStore,
    pub run_id: Uuid,
    pub parent: std::sync::Arc<dyn crate::EventSink>,
}

#[async_trait::async_trait]
impl crate::EventSink for RunProgressSink {
    async fn emit(&self, event: crate::AgentEvent) {
        self.emit_sequenced(event).await;
    }

    async fn emit_sequenced(&self, event: crate::AgentEvent) -> Option<u64> {
        use crate::AgentEvent;
        let update = match &event {
            AgentEvent::SubagentTurnStarted { turn, .. } => Some((Some(*turn as u64), None)),
            AgentEvent::SubagentToolRequested { name, .. } => {
                Some((None, Some(Some(name.clone()))))
            }
            AgentEvent::SubagentToolCompleted { .. } => Some((None, Some(None))),
            _ => None,
        };
        if let Some((turn, tool)) = update
            && let Err(error) = self.store.progress(self.run_id, turn, tool)
        {
            eprintln!("cannot persist Agent run progress: {error}");
        }
        let sequence = self
            .store
            .get(self.run_id)
            .ok()
            .flatten()
            .map_or(0, |run| run.sequence);
        self.parent
            .emit_sequenced(crate::AgentEvent::RunScoped {
                run_id: self.run_id,
                sequence,
                event: Box::new(event),
            })
            .await
    }
}

#[cfg(test)]
mod tests;
