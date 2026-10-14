use super::*;

const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(10);
const SLOW_OBSERVER_STARTUP: Duration = Duration::from_millis(2100);

#[test]
fn root_retry_wait_is_observable_through_public_runtime_api() {
    let _serial = process_test_guard();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let provider = MockProvider::start_with_mode(MockMode::RetryRoot);
    let config = root.join("config.toml");
    write_private_config(&config, provider.api_base());
    let _guard = TestGuard::new(root.clone(), home.clone());
    let child = willdeep(&home)
        .args([
            "run",
            "--config",
            path_text(&config),
            "--workspace",
            path_text(&workspace),
            "--output",
            "json",
            "retry root request",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut process = ChildGuard(Some(child));
    observe("first Provider request", &home, &provider, || {
        provider.requests() > 0
    });
    // Exercise the former race deliberately: observer setup takes longer
    // than Retry-After=2, while the unreleased 429 must keep request one held.
    thread::sleep(SLOW_OBSERVER_STARTUP);
    assert_eq!(
        provider.requests(),
        1,
        "observer barrier holds the first response"
    );
    let client = runtime_client(&home);
    let mut root_id = None;
    // Prepare and exercise the real HTTP observer before the fixture sends
    // its 429. Windows process/runtime startup must not consume the actual
    // two-second RetryWaiting window we intend to inspect.
    observe("observer ready before 429", &home, &provider, || {
        let willdeep_runtime_protocol::ApiResponse::Ok { data, .. } =
            runtime.block_on(client.agents()).unwrap()
        else {
            panic!("agent.list failed");
        };
        let Some(agent) = data.iter().find(|agent| agent.parent_id.is_none()) else {
            return false;
        };
        root_id = Some(agent.id);
        assert!(
            agent.retry_wait.is_none(),
            "429 is still held by the fixture"
        );
        agent.status == willdeep_runtime_protocol::AgentStatus::Running
    });
    let root_id = root_id.unwrap();
    provider
        .release_retry_response
        .store(true, Ordering::SeqCst);
    observe("RetryWaiting visible", &home, &provider, || {
        let willdeep_runtime_protocol::ApiResponse::Ok { data, .. } =
            runtime.block_on(client.agents()).unwrap()
        else {
            panic!("agent.list failed");
        };
        let agent = data.iter().find(|agent| agent.id == root_id).unwrap();
        agent
            .retry_wait
            .as_ref()
            .is_some_and(|wait| wait.attempt == 1 && wait.delay_ms == 2000)
    });
    observe("retry started and wait cleared", &home, &provider, || {
        if provider.requests() != 2 {
            return false;
        }
        let willdeep_runtime_protocol::ApiResponse::Ok { data, .. } =
            runtime.block_on(client.agent(root_id)).unwrap()
        else {
            panic!("agent.get failed");
        };
        data.retry_wait.is_none() && data.status == willdeep_runtime_protocol::AgentStatus::Running
    });
    provider.release_stream.store(true, Ordering::SeqCst);
    observe("root process completed", &home, &provider, || {
        process.0.as_mut().unwrap().try_wait().unwrap().is_some()
    });
    let result = process.0.take().unwrap().wait_with_output().unwrap();
    assert_success(&result, "root Provider retry");
    let willdeep_runtime_protocol::ApiResponse::Ok { data, .. } =
        runtime.block_on(client.agent(root_id)).unwrap()
    else {
        panic!("final agent.get failed");
    };
    assert_eq!(
        data.status,
        willdeep_runtime_protocol::AgentStatus::Completed
    );
    assert!(data.retry_wait.is_none());
    assert_eq!(provider.requests(), 2);
}

fn observe(phase: &str, home: &Path, provider: &MockProvider, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + OBSERVATION_TIMEOUT;
    while std::time::Instant::now() < deadline {
        if condition() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!(
        "phase {phase} did not complete within {OBSERVATION_TIMEOUT:?}; Provider requests={}; {}",
        provider.requests(),
        runtime_turn_report(home)
    );
}
