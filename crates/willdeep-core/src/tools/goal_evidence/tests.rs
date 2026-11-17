use super::*;

struct Fixture {
    store: Arc<GoalEvidenceStore>,
    snapshot: Arc<Mutex<Result<Option<String>, String>>>,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let workspace = std::env::temp_dir().join(format!("goal-evidence-{}", Uuid::new_v4()));
        let store = Arc::new(GoalEvidenceStore::default());
        let snapshot = Arc::new(Mutex::new(Ok(Some("whole-workspace-a".into()))));
        let captured = snapshot.clone();
        store.set_snapshot(Some(Arc::new(move || captured.lock().unwrap().clone())));
        store.bind_scope(Some("session-a"), &workspace);
        store.bind_goal(Some("goal-a"));
        Self {
            store,
            snapshot,
            workspace,
        }
    }

    fn run(
        &self,
        command: &str,
        status: VerificationStatus,
        exit: Option<i32>,
    ) -> GoalEvidenceRecord {
        let attempt = self.store.begin_command(command).unwrap();
        self.store.finish_command(attempt, status, exit).unwrap()
    }

    fn accepts(&self, record: &GoalEvidenceRecord) -> bool {
        self.store.validate("goal-a", &[&record.reference]).is_ok()
    }
}

#[test]
fn opaque_records_contain_execution_identity_and_strict_snapshots() {
    let fixture = Fixture::new();
    let record = fixture.run("cargo test", VerificationStatus::Passed, Some(0));
    assert!(fixture.accepts(&record));
    assert!(Uuid::parse_str(&record.record_id).is_ok());
    assert!(Uuid::parse_str(&record.attempt_id).is_ok());
    assert_ne!(record.record_id, record.attempt_id);
    assert_eq!(
        record.reference,
        format!("goal-evidence:{}", record.record_id)
    );
    assert_eq!(record.session_id, "session-a");
    assert_eq!(record.workspace, fixture.workspace);
    assert_eq!(record.goal_id, "goal-a");
    assert_eq!(record.start_snapshot, record.end_snapshot);
    assert_eq!(record.source, GoalEvidenceSource::Command);
    let receipt = serde_json::to_value(&record).unwrap();
    assert_eq!(receipt["evidence_ref"], record.reference);
    assert!(
        fixture
            .store
            .validate("goal-a", &["cargo test: passed"])
            .is_err()
    );
    let forged = format!("goal-evidence:{}", Uuid::new_v4());
    assert!(fixture.store.validate("goal-a", &[&forged]).is_err());
}

#[test]
fn exit_status_cannot_be_laundered_by_a_successful_result() {
    for exit in [Some(7), Some(-1), None] {
        let fixture = Fixture::new();
        let record = fixture.run("cargo test", VerificationStatus::Passed, exit);
        assert_eq!(record.status, VerificationStatus::Failed);
        assert!(!fixture.accepts(&record));
    }
}

#[test]
fn failed_timed_out_launch_failed_and_invalidated_records_never_resolve() {
    for status in [
        VerificationStatus::Failed,
        VerificationStatus::TimedOut,
        VerificationStatus::LaunchFailed,
        VerificationStatus::Invalidated,
    ] {
        let fixture = Fixture::new();
        let record = fixture.run("cargo test", status, Some(0));
        assert!(!fixture.accepts(&record), "{status:?}");
    }
}

#[test]
fn later_failure_supersedes_success_even_when_snapshots_are_restored() {
    let fixture = Fixture::new();
    let passed = fixture.run("cargo test", VerificationStatus::Passed, Some(0));
    *fixture.snapshot.lock().unwrap() = Ok(Some("whole-workspace-b".into()));
    fixture.run("cargo test", VerificationStatus::Failed, Some(2));
    *fixture.snapshot.lock().unwrap() = Ok(Some("whole-workspace-a".into()));
    assert!(!fixture.accepts(&passed));
    let repaired = fixture.run("cargo test", VerificationStatus::Passed, Some(0));
    assert!(fixture.accepts(&repaired));
    assert!(
        !fixture.accepts(&passed),
        "a fresh success does not resurrect a superseded reference"
    );
}

#[test]
fn a_running_or_later_attempt_cannot_be_overwritten_by_an_older_finish() {
    let fixture = Fixture::new();
    let old = fixture.store.begin_command("cargo test").unwrap();
    let later = fixture.store.begin_command("cargo test").unwrap();
    let failed = fixture
        .store
        .finish_command(later, VerificationStatus::Failed, Some(7))
        .unwrap();
    let old = fixture
        .store
        .finish_command(old, VerificationStatus::Passed, Some(0))
        .unwrap();
    assert!(!fixture.accepts(&failed));
    assert!(!fixture.accepts(&old));
    let passed = fixture.run("cargo test", VerificationStatus::Passed, Some(0));
    let _running = fixture.store.begin_command("cargo test").unwrap();
    assert!(!fixture.accepts(&passed));
}

#[test]
fn no_start_end_or_current_snapshot_fails_closed() {
    for missing in [Ok(None), Ok(Some(String::new())), Err("unreadable".into())] {
        let fixture = Fixture::new();
        *fixture.snapshot.lock().unwrap() = missing.clone();
        let attempt = fixture.store.begin_command("cargo test").unwrap();
        *fixture.snapshot.lock().unwrap() = Ok(Some("whole-workspace-a".into()));
        let no_start = fixture
            .store
            .finish_command(attempt, VerificationStatus::Passed, Some(0))
            .unwrap();
        assert_eq!(no_start.status, VerificationStatus::Invalidated);
        assert!(!fixture.accepts(&no_start));

        let attempt = fixture.store.begin_command("cargo test").unwrap();
        *fixture.snapshot.lock().unwrap() = missing.clone();
        let no_end = fixture
            .store
            .finish_command(attempt, VerificationStatus::Passed, Some(0))
            .unwrap();
        assert_eq!(no_end.status, VerificationStatus::Invalidated);
        *fixture.snapshot.lock().unwrap() = Ok(Some("whole-workspace-a".into()));
        assert!(!fixture.accepts(&no_end));

        let passed = fixture.run("cargo test", VerificationStatus::Passed, Some(0));
        *fixture.snapshot.lock().unwrap() = missing;
        assert!(!fixture.accepts(&passed));
    }
}

#[test]
fn scope_and_goal_identity_are_checked_and_rebinding_revokes_in_flight_tokens() {
    let fixture = Fixture::new();
    let passed = fixture.run("cargo test", VerificationStatus::Passed, Some(0));
    fixture.store.bind_goal(Some("goal-b"));
    assert!(
        fixture
            .store
            .validate("goal-b", &[&passed.reference])
            .is_err()
    );
    assert!(
        fixture
            .store
            .validate("goal-a", &[&passed.reference])
            .is_err()
    );
    fixture.store.bind_goal(Some("goal-a"));
    let in_flight = fixture.store.begin_command("cargo test").unwrap();
    fixture
        .store
        .bind_scope(Some("session-b"), &fixture.workspace);
    fixture.store.bind_goal(Some("goal-a"));
    assert!(!fixture.accepts(&passed));
    assert!(
        fixture
            .store
            .finish_command(in_flight, VerificationStatus::Passed, Some(0))
            .is_none()
    );
    let passed = fixture.run("cargo test", VerificationStatus::Passed, Some(0));
    fixture
        .store
        .bind_scope(Some("session-b"), &fixture.workspace.join("other"));
    fixture.store.bind_goal(Some("goal-a"));
    assert!(!fixture.accepts(&passed));
}

#[test]
fn missing_scope_or_restart_never_recovers_a_record_from_text() {
    let fixture = Fixture::new();
    let passed = fixture.run("cargo test", VerificationStatus::Passed, Some(0));
    let restarted = Fixture::new();
    assert!(!restarted.accepts(&passed));
    fixture.store.bind_scope(None, &fixture.workspace);
    fixture.store.bind_goal(Some("goal-a"));
    assert!(fixture.store.begin_command("cargo test").is_none());
    assert!(!fixture.accepts(&passed));
}

#[test]
fn whole_workspace_changes_invalidate_even_a_narrow_command() {
    let fixture = Fixture::new();
    let attempt = fixture
        .store
        .begin_command("cargo test -p one_package")
        .unwrap();
    *fixture.snapshot.lock().unwrap() = Ok(Some("changed-unrelated-package".into()));
    let record = fixture
        .store
        .finish_command(attempt, VerificationStatus::Passed, Some(0))
        .unwrap();
    assert_eq!(record.status, VerificationStatus::Invalidated);
    assert!(!fixture.accepts(&record));
    *fixture.snapshot.lock().unwrap() = Ok(Some("whole-workspace-a".into()));
    assert!(!fixture.accepts(&record));
}

#[test]
fn host_human_observation_is_separate_from_model_claims_and_command_results() {
    let fixture = Fixture::new();
    let failed = fixture.run("cargo test", VerificationStatus::Failed, Some(2));
    let human = fixture
        .store
        .record_human_observation("goal-a", "layout inspected on device")
        .unwrap();
    assert_eq!(human.source, GoalEvidenceSource::HumanObservation);
    assert!(human.command.is_none());
    assert!(fixture.accepts(&human));
    assert!(!fixture.accepts(&failed));
    assert!(
        fixture
            .store
            .record_human_observation("goal-b", "observed")
            .is_err()
    );
    assert!(
        serde_json::from_value::<crate::goal::ChecklistUpdate>(serde_json::json!({
            "index":1,"done":true,"evidence_ref":failed.reference,"source":"human_observation"
        }))
        .is_err()
    );
    *fixture.snapshot.lock().unwrap() = Ok(Some("changed-after-human-observation".into()));
    assert!(!fixture.accepts(&human));
}

#[test]
fn changes_during_resolution_and_snapshot_io_fail_without_state_lock_io() {
    let fixture = Fixture::new();
    let passed = fixture.run("cargo test", VerificationStatus::Passed, Some(0));
    let weak = Arc::downgrade(&fixture.store);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    fixture.store.set_snapshot(Some(Arc::new(move || {
        let store = weak.upgrade().unwrap();
        assert!(
            store.records.try_lock().is_ok(),
            "snapshot IO must not hold record lock"
        );
        assert!(
            store.snapshot.try_lock().is_ok(),
            "snapshot IO must not hold capture lock"
        );
        let call = captured.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Some(
            if call == 0 {
                "whole-workspace-a"
            } else {
                "changed"
            }
            .into(),
        ))
    })));
    assert!(!fixture.accepts(&passed));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn a_new_attempt_during_resolution_revokes_old_success() {
    let fixture = Fixture::new();
    let passed = fixture.run("cargo test", VerificationStatus::Passed, Some(0));
    let weak = Arc::downgrade(&fixture.store);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    fixture.store.set_snapshot(Some(Arc::new(move || {
        if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            let store = weak.upgrade().unwrap();
            let key = CheckKey {
                goal_id: "goal-a".into(),
                source: GoalEvidenceSource::Command,
                check: "cargo test".into(),
            };
            store
                .records
                .lock()
                .unwrap()
                .latest
                .insert(key, Uuid::new_v4().to_string());
        }
        Ok(Some("whole-workspace-a".into()))
    })));
    assert!(!fixture.accepts(&passed));
}
