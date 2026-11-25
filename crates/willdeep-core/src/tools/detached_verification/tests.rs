use super::*;

const BAD: &str = "cargo test -p willdeep-cli tui::tests::viewport";
const GOOD: &str = "cargo test -p willdeep tui::tests::viewport";
const DIAGNOSTIC: &str = "error: package ID specification `willdeep-cli` did not match any packages\n\nhelp: a package with a similar name exists: `willdeep`\n";

struct Fixture {
    root: PathBuf,
    jobs: Arc<DetachedJobStore>,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("detached-verification-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let jobs = Arc::new(DetachedJobStore::new(&root).with_owner("verification-session"));
        Self { root, jobs }
    }

    fn tools(&self) -> ToolRegistry {
        ToolRegistry::new(&self.root, ApprovalMode::FullAccess)
            .unwrap()
            .with_detached_jobs(self.jobs.clone())
            .with_verification_snapshot(|| Some("current".into()))
    }

    fn job(&self, command: &str, cwd: Option<&str>) -> DetachedJob {
        let workspace = cwd.map_or_else(|| self.root.clone(), |cwd| self.root.join(cwd));
        std::fs::create_dir_all(&workspace).unwrap();
        let job = DetachedJob {
            id: format!("job_{}", uuid::Uuid::new_v4().simple()),
            command: command.into(),
            label: command.into(),
            workspace,
            // A deterministic live process, without starting a shell or Cargo.
            pid: std::process::id(),
            started_marker: None,
            created_at: 1,
            owner: Some("verification-session".into()),
            preview: false,
        };
        let dir = self.jobs.directory().join(&job.id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("meta.json"), serde_json::to_vec(&job).unwrap()).unwrap();
        job
    }

    fn track(&self, tools: &ToolRegistry, command: &str, cwd: Option<&str>) -> DetachedJob {
        let job = self.job(command, cwd);
        let evidence = cwd.map_or_else(
            || command.to_owned(),
            |cwd| format!("[cwd:{cwd}] {command}"),
        );
        let start = tools.prepare_detached_verification(command, &evidence, &job.workspace, true);
        assert!(
            tools
                .observe_detached_verification(self.jobs.clone(), job.clone(), start, 1)
                .is_empty()
        );
        job
    }

    fn finish(
        &self,
        job: &DetachedJob,
        status: BackgroundTaskStatus,
        exit_code: Option<i32>,
        output: &str,
    ) {
        crate::detached_job::record_result(
            &self.jobs.directory().join(&job.id),
            TaskResult {
                status,
                exit_code,
                output: output.into(),
            },
        )
        .unwrap();
    }

    fn collect(&self, tools: &ToolRegistry, job: &DetachedJob) -> String {
        tools
            .get_job_output(JobOutputArgs {
                job_id: job.id.clone(),
                tail_lines: Some(1),
            })
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn last_record(tools: &ToolRegistry) -> CommandVerification {
    tools
        .verification_records
        .lock()
        .unwrap()
        .recent
        .last()
        .unwrap()
        .clone()
}

fn args(command: &str) -> CommandArgs {
    CommandArgs {
        command: command.into(),
        cwd: None,
        timeout_seconds: Some(1),
        label: None,
        run_in_background: Some(true),
        preview: false,
        network: None,
    }
}

#[cfg(unix)]
fn supervisor(fixture: &mut Fixture) {
    use std::os::unix::fs::PermissionsExt;
    let executable = fixture.root.join("supervisor-fixture.sh");
    // Exercise the real DetachedJobStore::spawn_with_policy handshake. The fixture
    // accepts stdin without executing Cargo; tests publish the supervisor contract.
    std::fs::write(
        &executable,
        "#!/bin/sh\nexec /bin/dd of=/dev/null 2>/dev/null\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    fixture.jobs = Arc::new(
        DetachedJobStore::new(&fixture.root)
            .with_owner("verification-session")
            .with_supervisor_executable(executable),
    );
}

#[cfg(unix)]
#[tokio::test]
async fn detached_run_command_records_selector_correction_only_after_completion() {
    let mut fixture = Fixture::new();
    supervisor(&mut fixture);
    let tools = fixture.tools();
    tools.require_verifications(&[GOOD.into()]).unwrap();
    let notice = tools.run_command(args(BAD)).await.unwrap();
    assert!(notice.contains("Background job started:"), "{notice}");
    assert_eq!(tools.verification_count(), 0);
    assert!(!tools.completion_has_current_evidence(Some("current")));
    let bad = fixture
        .jobs
        .list()
        .into_iter()
        .find(|job| job.command == BAD)
        .unwrap();
    fixture.finish(&bad, BackgroundTaskStatus::Failed, Some(101), DIAGNOSTIC);
    fixture.collect(&tools, &bad);
    assert_eq!(
        last_record(&tools).status,
        VerificationStatus::InvocationError
    );
    assert_eq!(
        tools.verification_count(),
        0,
        "a rejected selector did not run tests"
    );
    assert!(!tools.completion_has_current_evidence(Some("current")));

    tools.run_command(args(GOOD)).await.unwrap();
    assert_eq!(
        tools.verification_count(),
        0,
        "launch is not a passing test"
    );
    let good = fixture
        .jobs
        .list()
        .into_iter()
        .find(|job| job.command == GOOD)
        .unwrap();
    fixture.finish(
        &good,
        BackgroundTaskStatus::Completed,
        Some(0),
        "test result: ok",
    );
    fixture.collect(&tools, &good);
    assert_eq!(last_record(&tools).status, VerificationStatus::Passed);
    assert_eq!(tools.verification_count(), 1);
    assert!(tools.completion_has_current_evidence(Some("current")));
    fixture.collect(&tools, &good);
    assert_eq!(
        tools.verification_count(),
        1,
        "reading output cannot replay a live result"
    );
}

#[tokio::test]
async fn detached_completion_observer_reports_without_get_job_output() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let job = fixture.track(&tools, GOOD, None);
    fixture.finish(&job, BackgroundTaskStatus::Completed, Some(0), "ok");
    tokio::time::timeout(Duration::from_secs(3), async {
        while tools.verification_count() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(last_record(&tools).status, VerificationStatus::Passed);
    assert_eq!(last_record(&tools).snapshot_id.as_deref(), Some("current"));
    assert!(tools.completion_has_current_evidence(Some("current")));
    fixture.collect(&tools, &job);
    assert_eq!(tools.verification_count(), 1);
}

#[tokio::test]
async fn detached_correction_keeps_cwd_and_all_other_arguments() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let bad = fixture.track(&tools, BAD, Some("nested"));
    fixture.finish(&bad, BackgroundTaskStatus::Failed, Some(101), DIAGNOSTIC);
    fixture.collect(&tools, &bad);
    assert_eq!(last_record(&tools).command, format!("[cwd:nested] {BAD}"));
    assert_eq!(
        last_record(&tools).status,
        VerificationStatus::InvocationError
    );
    for (command, cwd) in [
        (GOOD.to_owned(), None),
        (format!("{GOOD} --all-features"), Some("nested")),
    ] {
        let other = fixture.track(&tools, &command, cwd);
        fixture.finish(&other, BackgroundTaskStatus::Completed, Some(0), "ok");
        fixture.collect(&tools, &other);
        assert!(!tools.completion_has_current_evidence(Some("current")));
    }
    let good = fixture.track(&tools, GOOD, Some("nested"));
    fixture.finish(&good, BackgroundTaskStatus::Completed, Some(0), "ok");
    fixture.collect(&tools, &good);
    assert!(tools.completion_has_current_evidence(Some("current")));
}

#[tokio::test]
async fn detached_terminal_outcomes_preserve_real_failure_timeout_launch_and_kill() {
    for (status, code, output, expected) in [
        (
            BackgroundTaskStatus::Failed,
            Some(1),
            "test failed",
            VerificationStatus::Failed,
        ),
        (
            BackgroundTaskStatus::Failed,
            Some(101),
            "error[E0308]: mismatched types",
            VerificationStatus::Failed,
        ),
        (
            BackgroundTaskStatus::TimedOut,
            None,
            "command timed out",
            VerificationStatus::TimedOut,
        ),
        (
            BackgroundTaskStatus::LaunchFailed,
            Some(-1),
            "spawn failed",
            VerificationStatus::LaunchFailed,
        ),
        (
            BackgroundTaskStatus::Killed,
            None,
            "cancelled",
            VerificationStatus::Failed,
        ),
    ] {
        let fixture = Fixture::new();
        let tools = fixture.tools();
        let job = fixture.track(&tools, GOOD, None);
        fixture.finish(&job, status, code, output);
        fixture.collect(&tools, &job);
        let record = last_record(&tools);
        assert_eq!(record.status, expected);
        assert_eq!(record.exit_code, code);
        assert_eq!(record.snapshot_id.as_deref(), Some("current"));
        assert!(!tools.completion_has_current_evidence(Some("current")));
    }
}

#[tokio::test]
async fn detached_zero_exit_requires_readable_unchanged_start_and_end_snapshots() {
    for (before, after) in [
        (Ok(Some("before".to_owned())), Ok(Some("after".to_owned()))),
        (
            Ok(Some("before".to_owned())),
            Err("snapshot read failed".to_owned()),
        ),
        (Ok(Some("before".to_owned())), Ok(None)),
        (
            Err("start snapshot read failed".to_owned()),
            Ok(Some("after".to_owned())),
        ),
        (Ok(None), Ok(None)),
    ] {
        let fixture = Fixture::new();
        let snapshot = Arc::new(Mutex::new(before));
        let tools = fixture.tools().with_fallible_verification_snapshot({
            let snapshot = snapshot.clone();
            move || snapshot.lock().unwrap().clone()
        });
        let job = fixture.track(&tools, GOOD, None);
        *snapshot.lock().unwrap() = after;
        fixture.finish(&job, BackgroundTaskStatus::Completed, Some(0), "ok");
        fixture.collect(&tools, &job);
        assert_eq!(last_record(&tools).status, VerificationStatus::Invalidated);
        assert!(!tools.completion_has_current_evidence(Some("before")));
    }
}

#[tokio::test]
async fn detached_running_jobs_never_create_verification_evidence() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    tools.require_verifications(&[GOOD.into()]).unwrap();
    let job = fixture.track(&tools, GOOD, None);
    let output = fixture.collect(&tools, &job);
    assert!(output.contains("status: running"), "{output}");
    assert_eq!(tools.verification_count(), 0);
    assert!(tools.verification_evidence().is_empty());
    assert!(!tools.completion_has_current_evidence(Some("current")));
}

#[tokio::test]
async fn detached_missing_inconsistent_and_vanished_results_never_pass() {
    for corruption in ["missing", "mismatched", "completed_nonzero", "vanished"] {
        let fixture = Fixture::new();
        let tools = fixture.tools();
        let mut job = fixture.job(GOOD, None);
        if corruption == "vanished" {
            job.pid = 1_000_000_000;
            job.started_marker = Some("not-a-process".into());
            std::fs::write(
                fixture.jobs.directory().join(&job.id).join("meta.json"),
                serde_json::to_vec(&job).unwrap(),
            )
            .unwrap();
        }
        let start = tools.prepare_detached_verification(GOOD, GOOD, &job.workspace, true);
        tools.observe_detached_verification(fixture.jobs.clone(), job.clone(), start, 1);
        let dir = fixture.jobs.directory().join(&job.id);
        match corruption {
            "missing" => std::fs::write(dir.join("exit"), "0").unwrap(),
            "mismatched" => {
                fixture.finish(&job, BackgroundTaskStatus::Completed, Some(0), "ok");
                std::fs::write(dir.join("exit"), "101").unwrap();
            }
            "completed_nonzero" => fixture.finish(
                &job,
                BackgroundTaskStatus::Completed,
                Some(101),
                "impossible success",
            ),
            "vanished" => {}
            _ => unreachable!(),
        }
        fixture.collect(&tools, &job);
        assert_eq!(
            last_record(&tools).status,
            VerificationStatus::Invalidated,
            "{corruption}"
        );
        assert!(!tools.completion_has_current_evidence(Some("current")));
    }
}

#[test]
fn detached_pending_journal_after_runtime_restart_never_guesses_passed() {
    let fixture = Fixture::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let job = runtime.block_on(async {
        let tools = fixture.tools();
        fixture.track(&tools, GOOD, None)
    });
    drop(runtime); // Cancels the observer; the detached store remains independent.
    assert_eq!(fixture.jobs.state(&job), JobState::Running);
    fixture.finish(
        &job,
        BackgroundTaskStatus::Completed,
        Some(0),
        "ok after restart",
    );
    let restored = fixture.tools();
    fixture.collect(&restored, &job);
    assert_eq!(
        last_record(&restored).status,
        VerificationStatus::Invalidated
    );
    assert!(!restored.completion_has_current_evidence(Some("current")));
}

#[tokio::test]
async fn detached_settled_journal_replays_immutable_observation_once_per_registry() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let job = fixture.track(&tools, GOOD, None);
    fixture.finish(&job, BackgroundTaskStatus::Completed, Some(0), "ok");
    fixture.collect(&tools, &job);
    let restored = fixture.tools();
    fixture.collect(&restored, &job);
    fixture.collect(&restored, &job);
    assert_eq!(restored.verification_count(), 1);
    assert_eq!(last_record(&restored).status, VerificationStatus::Passed);
    assert!(restored.completion_has_current_evidence(Some("current")));
    let changed = fixture
        .tools()
        .with_verification_snapshot(|| Some("changed".into()));
    fixture.collect(&changed, &job);
    assert_eq!(
        last_record(&changed).snapshot_id.as_deref(),
        Some("current")
    );
    assert!(!changed.completion_has_current_evidence(Some("current")));
}

#[tokio::test]
async fn detached_invalidated_observation_cannot_be_relabelled_after_restart() {
    let fixture = Fixture::new();
    let snapshot = Arc::new(Mutex::new("before".to_owned()));
    let tools = fixture.tools().with_verification_snapshot({
        let snapshot = snapshot.clone();
        move || Some(snapshot.lock().unwrap().clone())
    });
    let job = fixture.track(&tools, GOOD, None);
    *snapshot.lock().unwrap() = "during".into();
    fixture.finish(&job, BackgroundTaskStatus::Completed, Some(0), "ok");
    fixture.collect(&tools, &job);
    let restored = fixture
        .tools()
        .with_verification_snapshot(|| Some("before".into()));
    fixture.collect(&restored, &job);
    assert_eq!(
        last_record(&restored).status,
        VerificationStatus::Invalidated
    );
    assert_eq!(
        last_record(&restored).observed_snapshot_id.as_deref(),
        Some("during")
    );
    assert!(!restored.completion_has_current_evidence(Some("before")));
}

#[tokio::test]
async fn detached_foreign_workspace_owner_and_unjournaled_jobs_are_output_only() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let job = fixture.track(&tools, GOOD, None);
    fixture.finish(&job, BackgroundTaskStatus::Completed, Some(0), "ok");
    fixture.collect(&tools, &job);
    let other_workspace = fixture.root.join("other");
    std::fs::create_dir_all(&other_workspace).unwrap();
    let other = ToolRegistry::new(&other_workspace, ApprovalMode::FullAccess)
        .unwrap()
        .with_detached_jobs(fixture.jobs.clone())
        .with_verification_snapshot(|| Some("current".into()));
    fixture.collect(&other, &job);
    assert!(other.verification_evidence().is_empty());
    let foreign_owner = fixture.tools().with_detached_jobs(Arc::new(
        DetachedJobStore::new(&fixture.root).with_owner("another-session"),
    ));
    fixture.collect(&foreign_owner, &job);
    assert!(foreign_owner.verification_evidence().is_empty());
    let legacy = fixture.job(GOOD, None);
    fixture.finish(
        &legacy,
        BackgroundTaskStatus::Completed,
        Some(0),
        "old result",
    );
    fixture.collect(&tools, &legacy);
    assert_eq!(tools.verification_count(), 1);
}

#[tokio::test]
async fn detached_journal_identity_must_match_the_actual_job() {
    let fixture = Fixture::new();
    let tools = fixture.tools();
    let mut job = fixture.track(&tools, GOOD, None);
    fixture.finish(&job, BackgroundTaskStatus::Completed, Some(0), "ok");
    job.command = BAD.into();
    std::fs::write(
        fixture.jobs.directory().join(&job.id).join("meta.json"),
        serde_json::to_vec(&job).unwrap(),
    )
    .unwrap();
    fixture.collect(&tools, &job);
    assert!(tools.verification_evidence().is_empty());
    let restored = fixture.tools();
    fixture.collect(&restored, &job);
    assert!(restored.verification_evidence().is_empty());
}

#[tokio::test]
async fn detached_spawn_failure_is_recorded_as_launch_failed() {
    let fixture = Fixture::new();
    let tools = fixture.tools().with_detached_jobs(Arc::new(
        DetachedJobStore::new(&fixture.root)
            .with_owner("verification-session")
            .with_supervisor_executable(fixture.root.join("missing-supervisor")),
    ));
    assert!(matches!(
        tools.run_command(args(GOOD)).await,
        Err(ToolError::Io(_))
    ));
    assert_eq!(last_record(&tools).status, VerificationStatus::LaunchFailed);
    assert_eq!(tools.verification_count(), 1);
    assert!(!tools.completion_has_current_evidence(Some("current")));
}

#[cfg(unix)]
#[tokio::test]
async fn detached_non_verification_jobs_do_not_create_journals_or_evidence() {
    let mut fixture = Fixture::new();
    supervisor(&mut fixture);
    let tools = fixture.tools();
    tools.run_command(args("printf hello")).await.unwrap();
    let job = fixture.jobs.list().pop().unwrap();
    assert!(
        !fixture
            .jobs
            .directory()
            .join(&job.id)
            .join(JOURNAL)
            .exists()
    );
    fixture.finish(&job, BackgroundTaskStatus::Completed, Some(0), "hello");
    fixture.collect(&tools, &job);
    assert!(tools.verification_evidence().is_empty());
}
