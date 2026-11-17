use super::super::*;
use crate::background::{BackgroundTaskKind, BackgroundTaskStatus};
use crate::goal::{
    ContinuationDecision, GOAL_COMPLETE_MARKER, GoalBudget, GoalContinuation, GoalStatus,
    RoundObservation,
};
use crate::{ApprovalMode, ToolCall};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;

// Store-only attempts cannot exercise the authority boundary between foreground,
// in-process background and detached jobs. Always enter through ToolRegistry.
const RUNTIME_CHECK: &str = "ruby checks.rb";
const RUNTIME_TIMEOUT_SECONDS: u64 = 10;

struct RuntimeFixture {
    root: PathBuf,
    workspace: PathBuf,
    session_id: String,
}

impl RuntimeFixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("goal-runtime-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("checks.rb"), "puts 'parser passed'\n").unwrap();
        Self {
            root,
            workspace: workspace.canonicalize().unwrap(),
            session_id: Uuid::new_v4().to_string(),
        }
    }

    fn tools(&self) -> ToolRegistry {
        let script = self.workspace.join("checks.rb");
        let tools = ToolRegistry::new(&self.workspace, ApprovalMode::FullAccess)
            .unwrap()
            .with_session_id(Some(self.session_id.clone()))
            .with_fallible_verification_snapshot(move || {
                // checks.rb is the entire synthetic workspace. Supervisor metadata
                // and synchronization files live outside it.
                let bytes = std::fs::read(&script).map_err(|error| error.to_string())?;
                Ok(Some(format!("{:x}", Sha256::digest(bytes))))
            });
        tools
            .require_verifications(&[RUNTIME_CHECK.into()])
            .unwrap();
        tools
    }

    fn goal(&self, tools: &ToolRegistry) -> GoalContinuation {
        let goal = GoalContinuation::new();
        goal.set_evidence_resolver(tools.goal_evidence_handle());
        goal.activate("ship the parser", GoalBudget::default());
        goal.update_plan(
            serde_json::from_value(json!({"criteria":["parser checks pass"]})).unwrap(),
        )
        .unwrap();
        goal
    }
}

impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[cfg(unix)]
struct DetachedCleanup(Arc<crate::detached_job::DetachedJobStore>);

#[cfg(unix)]
impl Drop for DetachedCleanup {
    fn drop(&mut self) {
        let jobs = self.0.list();
        for job in &jobs {
            if let Err(error) = self.0.kill(&job.id) {
                eprintln!("failed to stop private test supervisor {}: {error}", job.id);
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let running = || {
            jobs.iter().any(|job| {
                // SAFETY: signal zero only probes the test-owned PID; no signal is sent.
                unsafe { libc::kill(job.pid as libc::pid_t, 0) == 0 }
            })
        };
        while running() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if running() {
            if std::thread::panicking() {
                eprintln!("private test supervisor did not exit during cleanup");
            } else {
                panic!("private test supervisor did not exit during cleanup");
            }
        }
    }
}

async fn execute_runtime_tool(tools: &ToolRegistry, name: &str, arguments: Value) -> String {
    tools
        .execute(&ToolCall {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            arguments: arguments.to_string(),
        })
        .await
        .unwrap()
}

fn runtime_receipt(output: &str) -> Value {
    serde_json::from_str(
        output
            .rsplit_once("\n[goal-evidence] ")
            .expect("executor-issued Goal receipt")
            .1,
    )
    .unwrap()
}

fn check_runtime_receipt(goal: &GoalContinuation, receipt: &Value) -> Result<String, String> {
    goal.update_plan(
        serde_json::from_value(json!({"checklist":[{
            "index":1,"done":true,"evidence_ref":receipt["evidence_ref"]
        }]}))
        .unwrap(),
    )
}

#[cfg(unix)]
#[tokio::test]
async fn detached_run_command_revokes_goal_ref_and_journal_recovery_cannot_restore_authority() {
    use crate::detached_job::{DetachedJobStore, JobState};
    use crate::tools::RunVerification;
    use std::os::unix::fs::PermissionsExt;

    let fixture = RuntimeFixture::new();
    let executable = fixture.root.join("supervisor.rb");
    // Implement only the supervisor transport contract, executing the actual local
    // command after the test has inspected the pending attempt. No CLI or model.
    std::fs::write(
        &executable,
        r#"#!/usr/bin/env ruby
require 'json'
require 'open3'
require 'timeout'
length = STDIN.read(4).unpack1('N')
request = JSON.parse(STDIN.read(length))
Timeout.timeout(request.fetch('timeout_seconds')) do
  release = File.join(File.dirname(request.fetch('workspace')), 'release-detached')
  sleep 0.01 until File.exist?(release)
  out, err, status = Open3.capture3('/bin/sh', '-c', request.fetch('command'), chdir: request.fetch('workspace'))
  directory = request.fetch('detached_directory')
  File.write(File.join(directory, 'stdout.log'), out)
  File.write(File.join(directory, 'stderr.log'), err)
  File.write(File.join(directory, 'result.json'), JSON.generate({status: status.success? ? 'completed' : 'failed', exit_code: status.exitstatus}))
  File.write(File.join(directory, 'exit.pending'), status.exitstatus.to_s)
  File.rename(File.join(directory, 'exit.pending'), File.join(directory, 'exit'))
end
"#,
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let jobs = Arc::new(
        DetachedJobStore::new(fixture.root.join("home"))
            .with_owner(&fixture.session_id)
            .with_supervisor_executable(executable),
    );
    let _cleanup = DetachedCleanup(jobs.clone());
    let tools = fixture.tools().with_detached_jobs(jobs.clone());
    let goal = fixture.goal(&tools);
    let output =
        execute_runtime_tool(&tools, "run_command", json!({"command":RUNTIME_CHECK})).await;
    assert!(output.contains("parser passed"), "{output}");
    let old = runtime_receipt(&output);
    assert_eq!(old["status"], "passed");
    assert_eq!(old["exit_code"], 0);
    check_runtime_receipt(&goal, &old).unwrap();
    let saved_goal = goal.snapshot().unwrap();
    assert!(saved_goal.open_criteria().is_empty());
    let snapshot = old["end_snapshot"].as_str().unwrap();

    let notice = execute_runtime_tool(
        &tools,
        "run_command",
        json!({"command":format!("  {RUNTIME_CHECK}  "),"cwd":".","run_in_background":true,"timeout_seconds":RUNTIME_TIMEOUT_SECONDS}),
    )
    .await;
    assert!(notice.contains("Background job started:"), "{notice}");
    assert!(!notice.contains("[goal-evidence]"));
    assert_eq!(tools.verification_baseline().as_deref(), Some(snapshot));
    assert!(
        check_runtime_receipt(&goal, &old)
            .unwrap_err()
            .contains("superseded")
    );
    assert_eq!(goal.snapshot().unwrap().goal_id, saved_goal.goal_id);
    assert_eq!(goal.snapshot().unwrap().open_criteria().len(), 1);
    assert_eq!(tools.goal_evidence.records.lock().unwrap().records.len(), 1);
    assert_eq!(
        tools.verification_count(),
        1,
        "launch is not a terminal result"
    );
    let launched = jobs.list();
    assert_eq!(launched.len(), 1);
    let job = &launched[0];
    assert_eq!(job.workspace, fixture.workspace);
    assert_eq!(job.command.trim(), RUNTIME_CHECK);
    assert_eq!(jobs.state(job), JobState::Running);
    let journal_path = jobs.directory().join(&job.id).join("verification.json");
    let pending_journal = std::fs::read(&journal_path).unwrap();
    let pending: Value = serde_json::from_slice(&pending_journal).unwrap();
    assert!(pending["result"].is_null());

    std::fs::write(fixture.root.join("release-detached"), "release").unwrap();
    tokio::time::timeout(Duration::from_secs(RUNTIME_TIMEOUT_SECONDS), async {
        while jobs.state(job) == JobState::Running {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("local supervisor must finish");
    assert_eq!(jobs.state(job), JobState::Finished { exit_code: 0 });
    let output = execute_runtime_tool(&tools, "get_job_output", json!({"job_id":job.id})).await;
    assert!(output.contains("parser passed"), "{output}");
    assert!(!output.contains("[goal-evidence]"));
    assert_eq!(
        tools.run_verification(Some(snapshot), 0),
        (RunVerification::Passed, 2)
    );
    assert_eq!(tools.goal_evidence.records.lock().unwrap().records.len(), 1);
    assert!(check_runtime_receipt(&goal, &old).is_err());
    assert!(matches!(
        goal.evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::CompletionRejected {
            open_criteria: 1,
            ..
        })
    ));
    let settled_journal = std::fs::read(&journal_path).unwrap();
    let settled: Value = serde_json::from_slice(&settled_journal).unwrap();
    assert_eq!(settled["result"]["status"], "passed");
    assert_eq!(settled["result"]["exit_code"], 0);

    // Simulate restart at both persistence boundaries, retaining the exact session,
    // Goal ID, cwd and workspace bytes. Only the ordinary ledger may be replayed.
    for (journal, expected) in [
        (settled_journal, RunVerification::Passed),
        (pending_journal, RunVerification::Stale),
    ] {
        std::fs::write(&journal_path, journal).unwrap();
        let recovered_tools = fixture.tools().with_detached_jobs(jobs.clone());
        let recovered_goal = GoalContinuation::new();
        recovered_goal.set_evidence_resolver(recovered_tools.goal_evidence_handle());
        recovered_goal.restore(saved_goal.clone(), GoalBudget::default());
        for _ in 0..2 {
            let output =
                execute_runtime_tool(&recovered_tools, "get_job_output", json!({"job_id":job.id}))
                    .await;
            assert!(output.contains("parser passed"), "{output}");
            assert!(!output.contains("[goal-evidence]"));
            assert_eq!(
                recovered_tools.run_verification(Some(snapshot), 0),
                (expected, 1)
            );
            assert!(
                recovered_tools
                    .goal_evidence
                    .records
                    .lock()
                    .unwrap()
                    .records
                    .is_empty()
            );
            assert!(check_runtime_receipt(&recovered_goal, &old).is_err());
            assert_eq!(
                recovered_goal.snapshot().unwrap().goal_id,
                saved_goal.goal_id
            );
            assert_eq!(recovered_goal.snapshot().unwrap().open_criteria().len(), 1);
        }
        assert!(matches!(
            recovered_goal.evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
            Some(ContinuationDecision::CompletionRejected {
                open_criteria: 1,
                ..
            })
        ));
    }
}

#[tokio::test]
async fn in_process_background_shell_issues_a_terminal_goal_receipt_and_completes_goal() {
    let fixture = RuntimeFixture::new();
    let tools = fixture.tools();
    let goal = fixture.goal(&tools);
    let output =
        execute_runtime_tool(&tools, "run_command", json!({"command":RUNTIME_CHECK})).await;
    let old = runtime_receipt(&output);
    check_runtime_receipt(&goal, &old).unwrap();
    let mut events = tools.background.subscribe();
    let notice = execute_runtime_tool(
        &tools,
        "run_command",
        json!({"command":format!("  {RUNTIME_CHECK}  "),"cwd":".","run_in_background":true,"timeout_seconds":RUNTIME_TIMEOUT_SECONDS}),
    )
    .await;
    assert!(notice.contains("Background task started:"), "{notice}");
    assert!(!notice.contains("[goal-evidence]"));
    assert!(
        check_runtime_receipt(&goal, &old)
            .unwrap_err()
            .contains("superseded")
    );
    assert_eq!(goal.snapshot().unwrap().open_criteria().len(), 1);
    assert_eq!(tools.goal_evidence.records.lock().unwrap().records.len(), 1);

    let event = tokio::time::timeout(Duration::from_secs(RUNTIME_TIMEOUT_SECONDS), events.recv())
        .await
        .expect("background shell must settle")
        .unwrap();
    assert!(notice.contains(&event.snapshot.id));
    assert_eq!(event.snapshot.kind, BackgroundTaskKind::Shell);
    assert_eq!(event.snapshot.status, BackgroundTaskStatus::Completed);
    assert_eq!(event.snapshot.exit_code, Some(0));
    let output = execute_runtime_tool(
        &tools,
        "get_job_output",
        json!({"job_id":event.snapshot.id}),
    )
    .await;
    assert!(output.contains("parser passed"), "{output}");
    let fresh = runtime_receipt(&output);
    assert_eq!(fresh["source"], "command");
    assert_eq!(fresh["command"], RUNTIME_CHECK);
    assert_eq!(fresh["status"], "passed");
    assert_eq!(fresh["exit_code"], 0);
    assert_eq!(fresh["goal_id"], old["goal_id"]);
    assert_eq!(fresh["session_id"], fixture.session_id);
    assert_eq!(fresh["workspace"], json!(fixture.workspace));
    assert_eq!(fresh["start_snapshot"], old["end_snapshot"]);
    assert_eq!(fresh["start_snapshot"], fresh["end_snapshot"]);
    assert_ne!(fresh["attempt_id"], old["attempt_id"]);
    assert_ne!(fresh["record_id"], old["record_id"]);
    assert!(Uuid::parse_str(fresh["attempt_id"].as_str().unwrap()).is_ok());
    assert!(Uuid::parse_str(fresh["record_id"].as_str().unwrap()).is_ok());
    assert_eq!(tools.goal_evidence.records.lock().unwrap().records.len(), 2);
    assert!(check_runtime_receipt(&goal, &old).is_err());
    check_runtime_receipt(&goal, &fresh).unwrap();
    assert!(goal.snapshot().unwrap().open_criteria().is_empty());
    assert_eq!(
        goal.evaluate(GOAL_COMPLETE_MARKER, &RoundObservation::default()),
        Some(ContinuationDecision::Complete)
    );
    assert_eq!(goal.snapshot().unwrap().status, GoalStatus::Complete);
}
