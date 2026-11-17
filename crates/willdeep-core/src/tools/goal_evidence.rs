//! Process-local execution evidence. No model-facing record creation or restore API.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use uuid::Uuid;

use super::{ToolRegistry, VerificationSnapshot, VerificationStatus};
use crate::goal::GoalEvidenceResolver;

const REFERENCE_PREFIX: &str = "goal-evidence:";
const MAX_HUMAN_NOTE_CHARS: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Scope {
    session_id: String,
    workspace: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalEvidenceSource {
    Command,
    HumanObservation,
}

/// Immutable audit data; deserialization does not install records in the store.
#[derive(Clone, Debug, Serialize)]
pub struct GoalEvidenceRecord {
    pub record_id: String,
    #[serde(rename = "evidence_ref")]
    pub reference: String,
    pub session_id: String,
    pub workspace: PathBuf,
    pub goal_id: String,
    pub attempt_id: String,
    pub source: GoalEvidenceSource,
    pub command: Option<String>,
    pub observation: Option<String>,
    pub exit_code: Option<i32>,
    pub status: VerificationStatus,
    pub start_snapshot: Option<String>,
    pub end_snapshot: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CheckKey {
    goal_id: String,
    source: GoalEvidenceSource,
    check: String,
}

/// Unforgeable in model input: only the executor can obtain and consume this token.
pub(crate) struct GoalEvidenceAttempt {
    scope: Scope,
    generation: Uuid,
    goal_id: String,
    attempt_id: String,
    key: CheckKey,
    start_snapshot: Option<String>,
}

struct Records {
    generation: Uuid,
    scope: Option<Scope>,
    goal_id: Option<String>,
    latest: BTreeMap<CheckKey, String>,
    records: BTreeMap<String, GoalEvidenceRecord>,
}

impl Default for Records {
    fn default() -> Self {
        Self {
            generation: Uuid::new_v4(),
            scope: None,
            goal_id: None,
            latest: BTreeMap::new(),
            records: BTreeMap::new(),
        }
    }
}

/// Shared by the registry and its Goal resolver. A new process starts empty.
#[derive(Default)]
pub struct GoalEvidenceStore {
    records: Mutex<Records>,
    snapshot: Mutex<Option<VerificationSnapshot>>,
}

impl GoalEvidenceStore {
    /// Registry construction/session rebinding only; `workspace` is already canonical.
    pub(crate) fn bind_scope(&self, session_id: Option<&str>, workspace: &Path) {
        let scope = session_id
            .filter(|id| !id.trim().is_empty() && workspace.is_absolute())
            .map(|id| Scope {
                session_id: id.to_owned(),
                workspace: workspace.to_owned(),
            });
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        if records.scope != scope {
            *records = Records {
                scope,
                ..Records::default()
            };
        }
    }

    pub(crate) fn set_snapshot(&self, capture: Option<VerificationSnapshot>) {
        *self.snapshot.lock().unwrap_or_else(|e| e.into_inner()) = capture;
    }

    fn capture(&self) -> Result<String, String> {
        let capture = self
            .snapshot
            .lock()
            .map_err(|_| "evidence snapshot unavailable")?
            .clone()
            .ok_or("evidence requires a workspace snapshot")?;
        capture()?
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| "evidence requires a readable workspace snapshot".into())
    }

    pub(crate) fn begin_command(&self, command: &str) -> Option<GoalEvidenceAttempt> {
        self.begin(GoalEvidenceSource::Command, command.trim())
    }

    fn begin(&self, source: GoalEvidenceSource, check: &str) -> Option<GoalEvidenceAttempt> {
        let (scope, generation, goal_id) = {
            let records = self.records.lock().ok()?;
            (
                records.scope.clone()?,
                records.generation,
                records.goal_id.clone()?,
            )
        };
        // Snapshot capture may do IO. Never hold either state mutex across it.
        let start_snapshot = self.capture().ok();
        let mut records = self.records.lock().ok()?;
        if records.scope.as_ref() != Some(&scope)
            || records.generation != generation
            || records.goal_id.as_ref() != Some(&goal_id)
        {
            return None;
        }
        let key = CheckKey {
            goal_id: goal_id.clone(),
            source,
            check: check.to_owned(),
        };
        let attempt_id = Uuid::new_v4().to_string();
        // A later attempt, including a running or failed one, supersedes old success.
        records.latest.insert(key.clone(), attempt_id.clone());
        Some(GoalEvidenceAttempt {
            scope,
            generation,
            goal_id,
            attempt_id,
            key,
            start_snapshot,
        })
    }

    pub(crate) fn finish_command(
        &self,
        attempt: GoalEvidenceAttempt,
        status: VerificationStatus,
        exit_code: Option<i32>,
    ) -> Option<GoalEvidenceRecord> {
        self.finish(attempt, status, exit_code, None)
    }

    fn finish(
        &self,
        attempt: GoalEvidenceAttempt,
        status: VerificationStatus,
        exit_code: Option<i32>,
        observation: Option<String>,
    ) -> Option<GoalEvidenceRecord> {
        let end_snapshot = self.capture().ok();
        let status = if status == VerificationStatus::Passed {
            if attempt.key.source == GoalEvidenceSource::Command && exit_code != Some(0) {
                VerificationStatus::Failed
            } else if attempt.start_snapshot.is_none() || attempt.start_snapshot != end_snapshot {
                VerificationStatus::Invalidated
            } else {
                VerificationStatus::Passed
            }
        } else {
            status
        };
        let record_id = Uuid::new_v4().to_string();
        let record = GoalEvidenceRecord {
            reference: format!("{REFERENCE_PREFIX}{record_id}"),
            record_id,
            session_id: attempt.scope.session_id.clone(),
            workspace: attempt.scope.workspace.clone(),
            goal_id: attempt.goal_id,
            attempt_id: attempt.attempt_id,
            source: attempt.key.source,
            command: (attempt.key.source == GoalEvidenceSource::Command)
                .then_some(attempt.key.check),
            observation,
            exit_code,
            status,
            start_snapshot: attempt.start_snapshot,
            end_snapshot,
        };
        let mut records = self.records.lock().ok()?;
        if records.generation != attempt.generation
            || records.scope.as_ref() != Some(&attempt.scope)
        {
            return None;
        }
        records
            .records
            .insert(record.reference.clone(), record.clone());
        Some(record)
    }

    /// Host-only entry point for an actual human observation. No tool accepts a source
    /// field, and a command reference cannot be reclassified as a human observation.
    pub fn record_human_observation(
        &self,
        goal_id: &str,
        observation: &str,
    ) -> Result<GoalEvidenceRecord, String> {
        let observation = observation.trim();
        if observation.is_empty() || observation.chars().count() > MAX_HUMAN_NOTE_CHARS {
            return Err("human observation must contain 1 to 500 characters".into());
        }
        {
            let records = self
                .records
                .lock()
                .map_err(|_| "evidence store unavailable")?;
            if records.goal_id.as_deref() != Some(goal_id) {
                return Err("human observation does not belong to the current goal".into());
            }
        }
        let attempt = self
            .begin(GoalEvidenceSource::HumanObservation, observation)
            .ok_or("human observation requires an active, scoped goal")?;
        if attempt.goal_id != goal_id {
            return Err("goal changed while recording human observation".into());
        }
        let record = self
            .finish(
                attempt,
                VerificationStatus::Passed,
                None,
                Some(observation.into()),
            )
            .ok_or("scope changed while recording human observation")?;
        if record.status != VerificationStatus::Passed {
            return Err("human observation requires a stable readable workspace snapshot".into());
        }
        Ok(record)
    }
}

impl GoalEvidenceResolver for GoalEvidenceStore {
    fn bind_goal(&self, goal_id: Option<&str>) {
        let mut records = self.records.lock().unwrap_or_else(|e| e.into_inner());
        records.goal_id = goal_id.filter(|id| !id.is_empty()).map(str::to_owned);
    }

    fn validate(&self, goal_id: &str, references: &[&str]) -> Result<(), String> {
        let current = self.capture()?;
        let generation = {
            let records = self
                .records
                .lock()
                .map_err(|_| "evidence store unavailable")?;
            let scope = records
                .scope
                .as_ref()
                .ok_or("evidence has no session/workspace scope")?;
            if records.goal_id.as_deref() != Some(goal_id) {
                return Err("evidence belongs to a different goal".into());
            }
            for reference in references {
                let record = records
                    .records
                    .get(*reference)
                    .ok_or("unknown evidence reference; rerun verification")?;
                let check = match record.source {
                    GoalEvidenceSource::Command => record.command.as_deref(),
                    GoalEvidenceSource::HumanObservation => record.observation.as_deref(),
                }
                .ok_or("incomplete evidence record")?;
                let key = CheckKey {
                    goal_id: goal_id.into(),
                    source: record.source,
                    check: check.into(),
                };
                if record.session_id != scope.session_id
                    || record.workspace != scope.workspace
                    || record.goal_id != goal_id
                {
                    return Err("evidence belongs to a different session, workspace or goal".into());
                }
                if record.status != VerificationStatus::Passed
                    || (record.source == GoalEvidenceSource::Command && record.exit_code != Some(0))
                    || records.latest.get(&key) != Some(&record.attempt_id)
                {
                    return Err("evidence failed or was superseded by a later attempt".into());
                }
                if record.start_snapshot.as_deref() != Some(&current)
                    || record.end_snapshot.as_deref() != Some(&current)
                {
                    return Err("evidence is stale for the current workspace snapshot".into());
                }
            }
            records.generation
        };
        if self.capture()? != current {
            return Err("workspace changed while validating evidence".into());
        }
        // Also detect a new same-check attempt/scope change during snapshot IO.
        let records = self
            .records
            .lock()
            .map_err(|_| "evidence store unavailable")?;
        if records.generation != generation || records.goal_id.as_deref() != Some(goal_id) {
            return Err("evidence scope changed while validating".into());
        }
        for reference in references {
            let record = records
                .records
                .get(*reference)
                .ok_or("evidence disappeared")?;
            let check = record
                .command
                .as_deref()
                .or(record.observation.as_deref())
                .ok_or("incomplete evidence record")?;
            let key = CheckKey {
                goal_id: goal_id.into(),
                source: record.source,
                check: check.into(),
            };
            if records.latest.get(&key) != Some(&record.attempt_id) {
                return Err("evidence was superseded while validating".into());
            }
        }
        Ok(())
    }
}

impl ToolRegistry {
    /// Attach this exact shared authority to the parent GoalContinuation.
    pub fn goal_evidence_handle(&self) -> Arc<GoalEvidenceStore> {
        self.goal_evidence
            .bind_scope(self.session_id.as_deref(), &self.workspace);
        self.goal_evidence
            .set_snapshot(self.verification_snapshot.clone());
        self.goal_evidence.clone()
    }

    pub(super) fn begin_goal_verification(
        &self,
        command: &str,
        accepted: bool,
    ) -> Option<GoalEvidenceAttempt> {
        accepted
            .then(|| self.goal_evidence_handle().begin_command(command))
            .flatten()
    }

    /// Append the returned footer AFTER truncating command logs so the reference survives.
    pub(super) fn finish_goal_verification(
        store: &GoalEvidenceStore,
        attempt: Option<GoalEvidenceAttempt>,
        status: VerificationStatus,
        exit_code: Option<i32>,
    ) -> Option<String> {
        let record = store.finish_command(attempt?, status, exit_code)?;
        Some(format!(
            "\n[goal-evidence] {}",
            serde_json::to_string(&record).ok()?
        ))
    }
}

#[cfg(test)]
mod tests;
