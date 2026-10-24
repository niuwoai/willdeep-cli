use super::*;

const ID: &str = "00000000-0000-0000-0000-000000000001";

fn queries() -> Vec<Vec<&'static str>> {
    vec![
        vec!["daemon", "tasks"],
        vec!["daemon", "task", ID],
        vec!["daemon", "sessions"],
        vec!["daemon", "session", ID],
        vec!["daemon", "search-sessions", "example"],
        vec!["daemon", "export-session", ID],
        vec!["daemon", "turns", ID],
        vec!["daemon", "turn", ID],
        vec!["daemon", "agents"],
        vec!["daemon", "agent", ID],
        vec!["daemon", "agent-metrics"],
        vec!["daemon", "agent-team", ID],
        vec!["daemon", "agent-runs", ID],
        vec!["daemon", "agent-worktree-review", ID],
        vec!["daemon", "worktrees-audit"],
        vec!["daemon", "workspaces"],
        vec!["daemon", "pending"],
        vec!["daemon", "capabilities"],
        vec!["session", "list"],
        vec!["session", "get", ID],
        vec!["session", "turns", ID],
        vec!["schedule", "list"],
        vec!["schedule", "get", ID],
        vec!["schedule", "cleanup", ID, "1"],
        vec!["api", "task.list"],
        vec!["api", "session.list"],
        vec!["api", "artifact.list"],
        vec!["api", "event.stream", "--ndjson"],
        vec!["api", "agent_library.list"],
    ]
}

fn assert_queries_do_not_start(home: &Path, expected: &str) {
    let runtime = home.join("runtime");
    let before = runtime_files(&runtime);
    for args in queries() {
        let result = willdeep(home).args(&args).output().expect("run query");
        assert!(
            !result.status.success(),
            "{args:?} must report unavailable Runtime"
        );
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
        assert_eq!(
            runtime_files(&runtime),
            before,
            "{args:?} changed Runtime files"
        );
    }
}

fn runtime_files(path: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    if !path.exists() {
        return Default::default();
    }
    std::fs::read_dir(path)
        .expect("read fixture Runtime directory")
        .map(|entry| {
            let path = entry.expect("read fixture entry").path();
            let bytes = std::fs::read(&path).expect("read fixture file");
            (path, bytes)
        })
        .collect()
}

#[test]
fn readonly_queries_leave_stopped_runtime_absent() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let _guard = TestGuard::new(root, home.clone());
    assert_queries_do_not_start(&home, "Runtime is not running");
    assert!(!home.join("runtime").exists());
}

#[test]
fn readonly_queries_preserve_stale_runtime_and_pending_records() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let runtime = home.join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    // Use a freshly released loopback address for the unreachable fixture.
    let socket = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    write_json(
        &runtime.join("daemon.json"),
        serde_json::json!({
            "schema": 1, "version": "0.114.0-rc5", "pid": 0,
            "address": address.to_string(), "token": "isolated-test-token", "started_at": 1
        }),
    );
    std::fs::write(runtime.join("daemon.lock"), b"preserve stale lease").unwrap();
    std::fs::write(
        runtime.join("interactions.json"),
        b"[{\"status\":\"pending\"}]",
    )
    .unwrap();
    std::fs::write(
        runtime.join("tasks.json"),
        b"[{\"status\":\"waiting_approval\"}]",
    )
    .unwrap();
    std::fs::write(runtime.join("sessions.json"), b"[]").unwrap();
    // No daemon was started by this test; cleanup must not operate on the fake lease.
    let mut guard = TestGuard::new(root, home.clone());
    guard.daemon_stopped = true;
    assert_queries_do_not_start(&home, "Runtime");
    std::fs::write(runtime.join("daemon.json"), b"invalid state").unwrap();
    assert_queries_do_not_start(&home, "Runtime");
}

#[test]
fn readonly_queries_use_running_runtime_without_replacing_lease() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    std::fs::create_dir_all(&root).unwrap();
    let mut guard = TestGuard::new(root.clone(), home.clone());
    let start = willdeep(&home)
        .args(["daemon", "create-session", "--workspace", path_text(&root)])
        .output()
        .unwrap();
    assert_success(
        &start,
        "create-session still starts isolated fixture Runtime",
    );
    let state_path = home.join("runtime/daemon.json");
    let before = std::fs::read(&state_path).unwrap();
    for args in [
        vec!["daemon", "tasks"],
        vec!["daemon", "sessions"],
        vec!["daemon", "agents"],
        vec!["daemon", "pending"],
        vec!["daemon", "capabilities"],
        vec!["session", "list"],
        vec!["api", "task.list"],
        vec!["api", "session.list"],
    ] {
        let result = willdeep(&home).args(&args).output().unwrap();
        assert_success(&result, &format!("{args:?}"));
        assert_eq!(std::fs::read(&state_path).unwrap(), before);
    }
    guard.stop_daemon();
}
