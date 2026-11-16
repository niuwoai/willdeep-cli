//! Verification for detached jobs. The supervisor and its restart contract stay unchanged.
//! Only a live executor can bind an exit result to its snapshot callbacks. Settled records
//! are replayable; a pending journal recovered after restart can never become Passed.
use super::*;
use crate::detached_job::{DetachedJob, DetachedJobStore, JobState};
use std::collections::HashMap;
use std::time::Duration;

const JOURNAL: &str = "verification.json";
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SUPERVISOR_GRACE_SECONDS: u64 = 10;

#[derive(Default)]
pub(super) struct Tracker(Mutex<HashMap<String, Entry>>);

struct Entry {
    journal: Journal,
    recovered: bool,
    reported: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Start {
    workspace: PathBuf,
    command_workspace: PathBuf,
    command: String,
    evidence_command: String,
    snapshot_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Journal {
    job: DetachedJob,
    start: Start,
    result: Option<Settled>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Settled {
    exit_code: Option<i32>,
    status: VerificationStatus,
    observed_snapshot_id: Option<String>,
    output: String,
}

#[derive(Deserialize)]
struct RecordedResult {
    status: BackgroundTaskStatus,
    exit_code: Option<i32>,
}

#[derive(Clone)]
struct Observer {
    tracker: Arc<Tracker>,
    workspace: PathBuf,
    reporter: Option<VerificationReporter>,
    snapshot: Option<VerificationSnapshot>,
}

impl ToolRegistry {
    pub(super) fn prepare_detached_verification(
        &self,
        command: &str,
        evidence_command: &str,
        command_workspace: &Path,
        accepted: bool,
    ) -> Option<Start> {
        (accepted && !contains_sensitive_command(command)).then(|| Start {
            workspace: self.workspace.clone(),
            command_workspace: command_workspace.to_owned(),
            command: command.to_owned(),
            evidence_command: evidence_command.to_owned(),
            snapshot_id: verification::capture_accepted_snapshot(
                self.verification_snapshot.as_ref(),
                true,
            ),
        })
    }

    pub(super) fn report_detached_launch_failure(
        &self,
        start: Option<&Start>,
        error: &std::io::Error,
    ) {
        if let Some(start) = start {
            verification::report_accepted_verification(
                self.verification_reporter.as_ref(),
                &start.evidence_command,
                None,
                VerificationStatus::LaunchFailed,
                &error.to_string(),
                start.snapshot_id.clone(),
                None,
            );
        }
    }

    pub(super) fn observe_detached_verification(
        &self,
        jobs: Arc<DetachedJobStore>,
        job: DetachedJob,
        start: Option<Start>,
        timeout: u64,
    ) -> String {
        let Some(start) = start else {
            return String::new();
        };
        let journal = Journal {
            job: job.clone(),
            start,
            result: None,
        };
        let saved = save(&jobs, &journal);
        self.detached_verifications
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                job.id.clone(),
                Entry {
                    journal,
                    recovered: false,
                    reported: false,
                },
            );
        let observer = self.detached_verification_observer();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now()
                + Duration::from_secs(timeout.saturating_add(SUPERVISOR_GRACE_SECONDS));
            loop {
                // Avoid invoking process_start_marker/ps on every polling tick. The exit
                // marker is published only after the supervisor's logs and result.json.
                if jobs.directory().join(&job.id).join("exit").exists()
                    || tokio::time::Instant::now() >= deadline
                {
                    observer.collect(&jobs, &job);
                    break;
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        });
        saved.err().map_or_else(String::new, |error| format!(
            "\n[verification] Could not persist verification recovery metadata: {error}. The job is running; verification collection is available in this executor only."
        ))
    }

    pub(super) fn collect_detached_verification(&self, jobs: &DetachedJobStore, job: &DetachedJob) {
        self.detached_verification_observer().collect(jobs, job);
    }

    fn detached_verification_observer(&self) -> Observer {
        Observer {
            tracker: self.detached_verifications.clone(),
            workspace: self.workspace.clone(),
            reporter: self.verification_reporter.clone(),
            snapshot: self.verification_snapshot.clone(),
        }
    }
}

impl Observer {
    fn collect(&self, jobs: &DetachedJobStore, job: &DetachedJob) {
        let mut entries = self.tracker.0.lock().unwrap_or_else(|e| e.into_inner());
        if !entries.contains_key(&job.id) {
            // Reading another session's output must not import its evidence. Legacy
            // unowned jobs and jobs predating the launch journal remain output-only.
            if !jobs.owns(job) {
                return;
            }
            let Some(journal) = std::fs::read(jobs.directory().join(&job.id).join(JOURNAL))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Journal>(&bytes).ok())
            else {
                return;
            };
            entries.insert(
                job.id.clone(),
                Entry {
                    journal,
                    recovered: true,
                    reported: false,
                },
            );
        }
        let entry = entries
            .get_mut(&job.id)
            .expect("inserted detached verification");
        if entry.reported || !matches_job(&entry.journal, job, &self.workspace) {
            return;
        }
        if entry.journal.result.is_none() {
            let state = jobs.state(job);
            if state == JobState::Running {
                return;
            }
            let mut result = terminal_result(jobs, job, state);
            if result.status == VerificationStatus::Passed {
                if entry.recovered
                    || entry.journal.start.snapshot_id.is_none()
                    || self.snapshot.is_none()
                {
                    result.status = VerificationStatus::Invalidated;
                    result.output.push_str("\n[verification-invalidated] A live start/end snapshot pair is unavailable for this detached check. Rerun the check on a stable workspace.");
                } else {
                    // Detached recovery deliberately uses the whole-workspace snapshot:
                    // no path-list approximation may turn an uncertain result into Passed.
                    let finished = verification::finish_verification(
                        self.snapshot.as_ref(),
                        &job.command,
                        &entry.journal.start.snapshot_id,
                        result.status,
                        &mut result.output,
                        None,
                    );
                    result.status = finished.status;
                    result.observed_snapshot_id = finished.observed_snapshot_id;
                }
            }
            entry.journal.result = Some(result);
        }
        // Keep the immutable observed result in memory even if persistence fails. A
        // pending journal recovered elsewhere is invalidated, never guessed to Passed.
        if let Err(error) = save(jobs, &entry.journal) {
            let result = entry.journal.result.as_mut().expect("settled result");
            result.output.push_str(&format!(
                "\n[verification] Could not persist detached verification result: {error}."
            ));
        }
        entry.reported = true;
        let journal = entry.journal.clone();
        drop(entries);
        let result = journal.result.expect("settled result");
        verification::report_accepted_verification(
            self.reporter.as_ref(),
            &journal.start.evidence_command,
            result.exit_code,
            result.status,
            &result.output,
            journal.start.snapshot_id,
            result.observed_snapshot_id,
        );
    }
}

fn matches_job(journal: &Journal, job: &DetachedJob, workspace: &Path) -> bool {
    journal.start.workspace == workspace
        && journal.start.command_workspace == job.workspace
        && journal.start.command == job.command
        && journal.job.id == job.id
        && journal.job.command == job.command
        && journal.job.workspace == job.workspace
        && journal.job.pid == job.pid
        && journal.job.created_at == job.created_at
        && journal.job.started_marker == job.started_marker
        && journal.job.owner == job.owner
}

fn terminal_result(jobs: &DetachedJobStore, job: &DetachedJob, state: JobState) -> Settled {
    let mut result = Settled {
        exit_code: None,
        status: VerificationStatus::Invalidated,
        observed_snapshot_id: None,
        output: jobs.output(&job.id, crate::detached_job::MAX_JOB_OUTPUT_BYTES),
    };
    if let JobState::Finished { exit_code } = state {
        let recorded = std::fs::read(jobs.directory().join(&job.id).join("result.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RecordedResult>(&bytes).ok());
        if let Some(recorded) = recorded {
            let expected = match recorded.status {
                BackgroundTaskStatus::TimedOut => 124,
                BackgroundTaskStatus::Killed => 137,
                _ => recorded.exit_code.unwrap_or(125),
            };
            if expected == exit_code {
                result.exit_code = recorded.exit_code;
                result.status = verification::verification_status(&recorded.status);
                if result.status == VerificationStatus::Passed && exit_code != 0 {
                    result.status = VerificationStatus::Invalidated;
                }
                return result;
            }
        }
    }
    result.output.push_str("\n[verification-invalidated] The detached job has no consistent supervisor exit result; its output is not passing verification evidence.");
    result
}

fn save(jobs: &DetachedJobStore, journal: &Journal) -> std::io::Result<()> {
    crate::detached_job::write_private_atomic(
        &jobs.directory().join(&journal.job.id).join(JOURNAL),
        &serde_json::to_vec(journal)?,
    )
}

#[cfg(test)]
mod tests;
