use super::*;

#[tokio::test]
async fn drain_waits_for_terminal_state_persistence_before_reporting_idle() {
    let root = std::env::temp_dir().join(format!("wd-drain-finish-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let sessions = crate::daemon::tests::test_runtime_session_store(&root);
    let state = crate::daemon::tests::test_server_state(&root, sessions);
    // finish() publishes a terminal task status before it persists the Turn
    // and its final event. Simulate that interval with the same writer gate.
    let persistence = state.tasks.persistence.lock().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), state.tasks.wait_until_idle())
            .await
            .is_err(),
        "drain must wait for an in-flight terminal-state commit"
    );
    drop(persistence);
    tokio::time::timeout(Duration::from_secs(1), state.tasks.wait_until_idle())
        .await
        .expect("drain should finish once terminal persistence is complete");
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
