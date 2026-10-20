use super::*;

pub(super) fn response(body: &[u8], index: usize, cancellation: bool) -> String {
    let text = String::from_utf8_lossy(body);
    let call = if index == 0 && cancellation {
        Some((
            "run_command",
            serde_json::json!({"command":"sleep 60", "run_in_background":true,"timeout_seconds":90}),
        ))
    } else if index == 1 && cancellation {
        let regex = regex::Regex::new(r"job_[a-f0-9]{32}").unwrap();
        let id = regex
            .find(&text)
            .expect("started job in tool response")
            .as_str();
        Some(("kill_job", serde_json::json!({"job_id":id})))
    } else if index == 0 {
        Some(("read_file", serde_json::json!({"path":"facts.txt"})))
    } else {
        None
    };
    if let Some((name, args)) = call {
        return serde_json::json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":format!("control-{index}"),"type":"function","function":{"name":name,"arguments":args.to_string()}}]},"finish_reason":"tool_calls"}]}).to_string();
    }
    let answer = if !cancellation && index == 1 {
        "facts.txt:1: boundary_marker is read-only evidence."
    } else {
        MOCK_REPLY
    };
    serde_json::json!({"choices":[{"message":{"content":answer},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}).to_string()
}

#[test]
fn read_only_task_automatically_creates_a_child_with_default_routing() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("facts.txt"),
        "boundary_marker is read-only evidence.\n",
    )
    .unwrap();
    let _guard = TestGuard::new(root.clone(), home.clone());
    let provider = MockProvider::start_with_mode(MockMode::AutomaticReadOnly);
    let config = root.join("config.toml");
    write_private_config(&config, provider.api_base());
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("auto_dispatch_read_only = false\n", "");
    std::fs::write(&config, settings).unwrap();
    let result = willdeep(&home)
        .args([
            "run",
            "--config",
            path_text(&config),
            "--workspace",
            path_text(&workspace),
            "定位 boundary_marker 在哪个文件",
        ])
        .output()
        .unwrap();
    assert_success(&result, "automatic read-only delegation");
    let agents = read_json_array(&home.join("runtime/agents.json"));
    let child = agents
        .iter()
        .find(|a| a["parent_id"].is_string())
        .expect("real child Agent record");
    assert_eq!(child["profile"], "scout");
    assert_eq!(child["status"], "completed");
    assert!(
        agents
            .iter()
            .any(|a| a["id"] == child["parent_id"] && a["parent_id"].is_null())
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("facts.txt")).unwrap(),
        "boundary_marker is read-only evidence.\n"
    );
    let events = read_ndjson_values(&home.join("runtime/events.ndjson"));
    assert!(events.iter().any(|event| {
        event["message"]
            .as_str()
            .is_some_and(|text| text.contains("subagent_started"))
    }));
    assert!(
        provider
            .captured_requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.to_string().contains("worker-report"))
    );
}

#[cfg(unix)]
#[test]
fn full_access_cancels_own_detached_job_without_runtime_approval() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _guard = TestGuard::new(root.clone(), home.clone());
    let provider = MockProvider::start_with_mode(MockMode::CancelOwnJob);
    let config = root.join("config.toml");
    write_private_config(&config, provider.api_base());
    let params = root.join("workspace.json");
    write_json(
        &params,
        serde_json::json!({"root":workspace,"access":"full_access"}),
    );
    let registered = willdeep(&home)
        .args([
            "api",
            "workspace.register",
            "--params-file",
            path_text(&params),
        ])
        .output()
        .unwrap();
    assert_success(&registered, "register full-access workspace");
    let result = willdeep(&home)
        .args([
            "run",
            "--config",
            path_text(&config),
            "--workspace",
            path_text(&workspace),
            "start then cancel one background job",
        ])
        .output()
        .unwrap();
    assert_success(&result, "cancel own detached job");
    let interactions_path = home.join("runtime/interactions.json");
    let interactions = if interactions_path.exists() {
        read_json_array(&interactions_path)
    } else {
        Vec::new()
    };
    assert!(
        !interactions
            .iter()
            .any(|item| item["kind"]["type"] == "approval")
    );
    let sessions = read_json_array(&home.join("runtime/tasks.json"));
    assert!(sessions.iter().all(|task| task["status"] == "completed"));
    let jobs = willdeep_core::DetachedJobStore::new(&home);
    assert_eq!(jobs.list().len(), 1);
    let job = &jobs.list()[0];
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while jobs.state(job) == willdeep_core::JobState::Running {
        assert!(
            std::time::Instant::now() < deadline,
            "cancelled job must stop"
        );
        thread::sleep(Duration::from_millis(25));
    }
}
