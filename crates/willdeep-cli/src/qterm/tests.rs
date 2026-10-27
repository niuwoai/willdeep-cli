use super::*;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    context: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("willdeep-qterm-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("requests")).unwrap();
        fs::create_dir_all(root.join("responses")).unwrap();
        let context = root.join("context.json");
        write_private(
            &context,
            &json!({"protocolVersion":1,"tabID":Uuid::new_v4(),"root":root,
            "token":format!("{}{}",Uuid::new_v4(),Uuid::new_v4())}),
        );
        Self { root, context }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn write_private(path: &std::path::Path, value: &serde_json::Value) {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let pending = path.with_extension(format!("pending-{}", Uuid::new_v4()));
    options
        .open(&pending)
        .unwrap()
        .write_all(&serde_json::to_vec(value).unwrap())
        .unwrap();
    fs::rename(pending, path).unwrap();
}

fn next_request(root: &std::path::Path) -> (PathBuf, serde_json::Value) {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        for entry in fs::read_dir(root.join("requests")).unwrap().flatten() {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                return (
                    entry.path(),
                    serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap(),
                );
            }
        }
        assert!(
            Instant::now() < deadline,
            "status request was not published"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn respond(root: &std::path::Path, path: PathBuf, request: &serde_json::Value, grant: bool) {
    let mut response = json!({"id":request["id"],"ok":true});
    if grant {
        response["agentGrant"] =
            json!({"instanceID":Uuid::new_v4(),"runID":Uuid::new_v4(),"token":"status-only-token"});
    }
    fs::remove_file(path).unwrap();
    write_private(
        &root
            .join("responses")
            .join(format!("{}.json", request["id"].as_str().unwrap())),
        &response,
    );
}

#[test]
fn semantic_mapping_never_equates_idle_partial_cancel_and_success() {
    let session = Uuid::new_v4();
    for (status, activity, block, result) in [
        (RuntimeStatus::Idle, "idle", None, None),
        (RuntimeStatus::Queued, "working", None, None),
        (
            RuntimeStatus::WaitingApproval,
            "blocked",
            Some("approval"),
            None,
        ),
        (
            RuntimeStatus::WaitingAnswer,
            "blocked",
            Some("question"),
            None,
        ),
        (RuntimeStatus::Done, "idle", None, Some("completed")),
        (RuntimeStatus::Failed, "idle", None, Some("failed")),
        (
            RuntimeStatus::Interrupted,
            "idle",
            None,
            Some("interrupted"),
        ),
        (RuntimeStatus::Cancelled, "idle", None, Some("interrupted")),
        (RuntimeStatus::Partial, "idle", None, Some("unknown")),
        (RuntimeStatus::Unknown, "unknown", None, None),
    ] {
        let state = State::new(session, None, None, status);
        assert_eq!(
            (state.activity, state.block_reason, state.result),
            (activity, block, result)
        );
    }
}

#[test]
fn context_and_response_files_must_be_private_and_not_symlinks() {
    let fixture = Fixture::new();
    assert!(transport::Endpoint::open(&fixture.context).is_ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        fs::set_permissions(&fixture.context, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(transport::Endpoint::open(&fixture.context).is_err());
        fs::set_permissions(&fixture.context, fs::Permissions::from_mode(0o600)).unwrap();
        let link = fixture.root.join("link.json");
        symlink(&fixture.context, &link).unwrap();
        assert!(transport::Endpoint::open(&link).is_err());
    }
}

#[test]
fn transport_binds_once_and_sends_metadata_with_increasing_sequence() {
    let fixture = Fixture::new();
    let root = fixture.root.clone();
    let server = std::thread::spawn(move || {
        let (path, request) = next_request(&root);
        assert_eq!(request["action"], "bind_native_agent");
        assert_eq!(request["agentKind"], "willdeep");
        assert!(request.get("authToken").is_none());
        respond(&root, path, &request, true);
        for sequence in 1..=2 {
            let (path, request) = next_request(&root);
            assert_eq!(request["action"], "report_agent_state");
            assert_eq!(request["agentReport"]["sequence"], sequence);
            assert_eq!(request["agentReport"]["context"]["taskID"], "task-1");
            assert_eq!(request["agentReport"]["token"], "status-only-token");
            assert!(request.get("text").is_none());
            respond(&root, path, &request, false);
        }
    });
    let mut endpoint = transport::Endpoint::open(&fixture.context).unwrap();
    let session = Uuid::new_v4();
    endpoint
        .report(&State::new(
            session,
            Some("task-1".into()),
            None,
            RuntimeStatus::Working,
        ))
        .unwrap();
    endpoint
        .report(&State::new(
            session,
            Some("task-1".into()),
            None,
            RuntimeStatus::Done,
        ))
        .unwrap();
    server.join().unwrap();
    assert_eq!(
        fs::read_dir(fixture.root.join("responses"))
            .unwrap()
            .count(),
        0
    );
}

fn memory_reporter() -> Reporter {
    Reporter {
        shared: Some(Arc::new((Mutex::new(Shared::default()), Condvar::new()))),
        worker: None,
    }
}

#[test]
fn runtime_frontend_finishing_does_not_invent_a_local_interruption() {
    let session = Uuid::new_v4();
    let mut view = ViewState::new(memory_reporter());
    view.observe(session, true, true, false, false, 0);
    view.reporter.update(
        session,
        Some("actual-runtime-task".into()),
        None,
        RuntimeStatus::Working,
    );
    // runtime_turn is cleared by the UI before the final Runtime snapshot arrives.
    view.observe(session, false, false, false, false, 0);
    let reported = state(&view.reporter);
    assert_eq!(reported.activity, "working");
    assert_eq!(reported.result, None);
    assert_eq!(
        reported.context.task_id.as_deref(),
        Some("actual-runtime-task")
    );
}

#[tokio::test]
async fn headless_shutdown_drains_final_result_without_retiring_runtime_task() {
    let fixture = Fixture::new();
    let root = fixture.root.clone();
    let server = std::thread::spawn(move || {
        let (path, request) = next_request(&root);
        respond(&root, path, &request, true);
        let (path, request) = next_request(&root);
        assert_eq!(request["agentReport"]["result"], "completed");
        assert_eq!(request["agentReport"]["activity"], "idle");
        respond(&root, path, &request, false);
    });
    let reporter = Reporter::start(transport::Endpoint::open(&fixture.context).unwrap());
    reporter.update(
        Uuid::new_v4(),
        Some("task-final".into()),
        None,
        RuntimeStatus::Done,
    );
    reporter.finish(true).await;
    server.join().unwrap();
    assert_eq!(
        fs::read_dir(fixture.root.join("requests")).unwrap().count(),
        0
    );
}

#[tokio::test]
async fn frontend_shutdown_delivers_final_result_before_exited_and_keeps_result() {
    let fixture = Fixture::new();
    let root = fixture.root.clone();
    let server = std::thread::spawn(move || {
        let (path, request) = next_request(&root);
        respond(&root, path, &request, true);
        for activity in ["idle", "exited"] {
            let (path, request) = next_request(&root);
            assert_eq!(request["agentReport"]["activity"], activity);
            assert_eq!(request["agentReport"]["result"], "failed");
            respond(&root, path, &request, false);
        }
    });
    let reporter = Reporter::start(transport::Endpoint::open(&fixture.context).unwrap());
    reporter.update(
        Uuid::new_v4(),
        Some("task-final".into()),
        None,
        RuntimeStatus::Failed,
    );
    reporter.finish(false).await;
    server.join().unwrap();
}

fn state(reporter: &Reporter) -> State {
    reporter
        .shared
        .as_ref()
        .unwrap()
        .0
        .lock()
        .unwrap()
        .state
        .clone()
        .unwrap()
}

#[test]
fn local_consecutive_identical_gates_get_distinct_episodes() {
    let mut view = ViewState::new(memory_reporter());
    let session = Uuid::new_v4();
    view.observe(session, true, false, true, false, 1);
    let first = state(&view.reporter);
    view.observe(session, true, false, true, false, 2);
    let second = state(&view.reporter);
    assert_eq!(first.activity, "blocked");
    assert_ne!(first.context.gate_id, second.context.gate_id);
    view.observe(session, true, false, false, false, 3);
    assert_eq!(state(&view.reporter).activity, "working");
    view.finished(session, RuntimeStatus::Done);
    view.observe(session, false, false, false, false, 3);
    assert_eq!(state(&view.reporter).result, Some("completed"));
}

#[test]
fn repeated_views_are_deduplicated_and_session_switch_resets_context() {
    let mut view = ViewState::new(memory_reporter());
    let session = Uuid::new_v4();
    view.observe(session, true, false, false, false, 0);
    let first = state(&view.reporter);
    let revision = view
        .reporter
        .shared
        .as_ref()
        .unwrap()
        .0
        .lock()
        .unwrap()
        .revision;
    view.observe(session, true, false, false, false, 0);
    assert_eq!(
        revision,
        view.reporter
            .shared
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .revision
    );
    let next = Uuid::new_v4();
    view.observe(next, false, false, false, false, 0);
    let state = state(&view.reporter);
    assert_eq!(state.context.session_id, next);
    assert_ne!(state.context, first.context);
    assert_eq!(state.result, None);
}

#[test]
fn runtime_unavailable_becomes_unknown_and_preserves_pending_gate_identity() {
    let reporter = memory_reporter();
    let session = Uuid::new_v4();
    reporter.update(
        session,
        Some("task-1".into()),
        Some("gate-1".into()),
        RuntimeStatus::WaitingApproval,
    );
    let snapshot = crate::daemon::RuntimeSnapshot {
        queue: None,
        attention: vec![],
        gates: vec![],
        agents: vec![],
        tasks: vec![],
        tools: vec![],
        artifacts: vec![],
        runtime_version: None,
        runtime_diagnostic: None,
        event_sequence: None,
        mobile: None,
    };
    reporter.runtime_snapshot(&snapshot, session, 0, false);
    let state = state(&reporter);
    assert_eq!(state.activity, "unknown");
    assert_eq!(state.context.gate_id.as_deref(), Some("gate-1"));
}

fn task(
    session: Uuid,
    agent: Uuid,
    created_at: u64,
    status: willdeep_runtime_protocol::TaskStatus,
) -> crate::daemon::tui_bridge::RemoteTask {
    crate::daemon::tui_bridge::RemoteTask {
        id: Uuid::new_v4(),
        session_id: Some(session),
        turn_id: None,
        agent_id: Some(agent),
        status,
        profile: None,
        created_at,
        started_at: Some(created_at),
        completed_at: None,
        exit_code: None,
        failure_domain: None,
    }
}

#[test]
fn background_or_other_session_completion_never_finishes_root() {
    use willdeep_runtime_protocol::TaskStatus;
    let session = Uuid::new_v4();
    let root = Uuid::new_v4();
    let child = Uuid::new_v4();
    let tasks = vec![
        task(session, root, 1, TaskStatus::Running),
        task(session, child, 3, TaskStatus::Completed),
        task(Uuid::new_v4(), root, 4, TaskStatus::Completed),
    ];
    assert_eq!(main_task(&tasks, session, &[root]).unwrap().id, tasks[0].id);
    assert!(main_task(&tasks, Uuid::new_v4(), &[root]).is_none());
}

#[test]
fn active_root_has_priority_over_finished_history() {
    use willdeep_runtime_protocol::TaskStatus;
    let session = Uuid::new_v4();
    let root = Uuid::new_v4();
    let tasks = vec![
        task(session, root, 2, TaskStatus::WaitingApproval),
        task(session, root, 3, TaskStatus::Completed),
    ];
    assert_eq!(
        main_task(&tasks, session, &[root]).unwrap().status,
        TaskStatus::WaitingApproval
    );
}

#[test]
fn final_result_is_not_lost_when_a_local_gate_closes_at_completion() {
    let mut view = ViewState::new(memory_reporter());
    let session = Uuid::new_v4();
    view.observe(session, true, false, true, false, 1);
    view.finished(session, RuntimeStatus::Done);
    view.observe(session, false, false, false, false, 2);
    assert_eq!(state(&view.reporter).result, Some("completed"));
}

#[test]
fn missing_endpoint_keeps_cli_behavior_optional() {
    assert!(
        transport::Endpoint::open(std::path::Path::new("/nonexistent/qterm-context.json")).is_err()
    );
    let reporter = Reporter::default();
    reporter.update(Uuid::new_v4(), None, None, RuntimeStatus::Working);
    assert!(!reporter.take_error());
}

#[test]
fn old_completed_snapshot_cannot_finish_a_new_queued_turn() {
    use willdeep_runtime_protocol::TaskStatus;
    let session = Uuid::new_v4();
    let reporter = memory_reporter();
    reporter.update(
        session,
        Some("new-turn".into()),
        None,
        RuntimeStatus::Working,
    );
    let mut old = task(session, Uuid::new_v4(), 1, TaskStatus::Completed);
    old.agent_id = None;
    let snapshot = crate::daemon::RuntimeSnapshot {
        queue: None,
        attention: vec![],
        gates: vec![],
        agents: vec![],
        tasks: vec![old],
        tools: vec![],
        artifacts: vec![],
        runtime_version: None,
        runtime_diagnostic: None,
        event_sequence: Some(5),
        mobile: None,
    };
    reporter.runtime_snapshot(&snapshot, session, 5, true);
    assert_eq!(state(&reporter).activity, "working");
    assert_eq!(
        state(&reporter).context.task_id.as_deref(),
        Some("new-turn")
    );
}

#[test]
fn adopting_running_runtime_does_not_reset_a_pending_approval() {
    let session = Uuid::new_v4();
    let mut view = ViewState::new(memory_reporter());
    view.observe(session, false, false, false, false, 0);
    view.reporter.update(
        session,
        Some("runtime-task".into()),
        Some("gate".into()),
        RuntimeStatus::WaitingApproval,
    );
    view.observe(session, true, true, true, false, 1);
    assert_eq!(state(&view.reporter).activity, "blocked");
    assert_eq!(
        state(&view.reporter).context.gate_id.as_deref(),
        Some("gate")
    );
}

#[test]
fn runtime_history_does_not_overwrite_a_local_turn() {
    use willdeep_runtime_protocol::TaskStatus;
    let session = Uuid::new_v4();
    let mut view = ViewState::new(memory_reporter());
    view.observe(session, true, false, false, false, 0);
    let mut old = task(session, Uuid::new_v4(), 1, TaskStatus::Completed);
    old.agent_id = None;
    let snapshot = crate::daemon::RuntimeSnapshot {
        queue: None,
        attention: vec![],
        gates: vec![],
        agents: vec![],
        tasks: vec![old],
        tools: vec![],
        artifacts: vec![],
        runtime_version: None,
        runtime_diagnostic: None,
        event_sequence: Some(5),
        mobile: None,
    };
    view.finished(session, RuntimeStatus::Failed);
    view.observe(session, false, false, false, false, 0);
    view.runtime_snapshot(&snapshot, session, 5, false);
    assert_eq!(state(&view.reporter).result, Some("failed"));
}
