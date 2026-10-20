use super::*;

#[cfg(unix)]
const STEERING: &str = "operator-message-during-silent-background-wait";

pub(super) fn response(body: &[u8], index: usize, monitor: bool, delayed: bool) -> String {
    let text = String::from_utf8_lossy(body);
    if text.contains("<verdict>") {
        return serde_json::json!({"choices":[{"message":{"content":"<verdict>YES</verdict>"},"finish_reason":"stop"}]}).to_string();
    }
    if index == 0 {
        let mut arguments = serde_json::json!({
            "command": "while [ ! -f release-background ]; do sleep 0.1; done",
            "label": "silent background steering regression",
            "timeout_seconds": 60,
        });
        if delayed {
            arguments["command"] = "sleep 30; cd .; printf 'delayed-%s\\n' once >> automatic-once; while [ ! -f release-background ]; do sleep 0.1; done".into();
        } else if !monitor {
            arguments["run_in_background"] = true.into();
        }
        let name = if monitor { "monitor" } else { "run_command" };
        return serde_json::json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"silent-background","type":"function","function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]}).to_string();
    }
    serde_json::json!({"choices":[{"message":{"content":MOCK_REPLY},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}).to_string()
}

#[cfg(unix)]
struct ReleaseOnDrop(PathBuf);

#[cfg(unix)]
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "release");
    }
}

#[cfg(unix)]
fn wait_for(label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(std::time::Instant::now() < deadline, "timed out: {label}");
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn check_steering(monitor: bool, delayed: bool) {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let _guard = TestGuard::new(root.clone(), home.clone());
    let provider = MockProvider::start_with_mode(if delayed {
        MockMode::LeadingSleepThenSuccess
    } else {
        MockMode::IdleBackgroundThenSuccess { monitor }
    });
    let config = root.join("config.toml");
    write_private_config(&config, provider.api_base());
    let settings = std::fs::read_to_string(&config)
        .unwrap()
        .replace("max_turns = 4", "max_turns = 1");
    std::fs::write(&config, settings).unwrap();
    if delayed {
        // The Runtime workspace policy overrides the config's approval mode.
        // This regression tests background delivery, not OS fence availability:
        // Linux runners without a usable fence must not wait on approval here.
        let registration = root.join("workspace.json");
        std::fs::write(
            &registration,
            serde_json::json!({"root": workspace, "access": "full_access"}).to_string(),
        )
        .unwrap();
        let registered = willdeep(&home)
            .args([
                "api",
                "workspace.register",
                "--params-file",
                path_text(&registration),
            ])
            .output()
            .unwrap();
        assert_success(&registered, "register delayed-command test workspace");
    }
    let mut client = ChildGuard(Some(
        willdeep(&home)
            .args([
                "run",
                "--config",
                path_text(&config),
                "--workspace",
                path_text(&workspace),
                "start silent background task",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    ));
    let release = ReleaseOnDrop(workspace.join("release-background"));
    wait_for("background task registered", || {
        !provider.captured_requests.lock().unwrap().is_empty()
    });
    let list = willdeep(&home).args(["api", "task.list"]).output().unwrap();
    assert_success(&list, "task.list");
    let tasks: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    let session_id: uuid::Uuid = tasks["data"][0]["session_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let store = willdeep_core::SessionStore::new(&home);
    // This checkpoint is written only after the model loop has returned at
    // its turn limit. Steering cannot accidentally reach the previous loop.
    wait_for("model loop ended while background remains silent", || {
        store
            .load(session_id)
            .ok()
            .and_then(|s| s.execution_checkpoint)
            .is_some_and(|c| c.status == willdeep_core::checkpoint::CheckpointStatus::Partial)
    });
    assert!(!release.0.exists());
    wait_for(
        "Runtime publishes the background-only send boundary",
        || {
            let events =
                std::fs::read_to_string(home.join("runtime/events.ndjson")).unwrap_or_default();
            events
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .any(|event| {
                    event["message"]
                        .as_str()
                        .and_then(|message| message.split_once(' '))
                        .and_then(|(_, payload)| {
                            serde_json::from_str::<serde_json::Value>(payload).ok()
                        })
                        .is_some_and(|value| {
                            value["type"] == "background_wait" && value["waiting"] == true
                        })
                })
        },
    );
    let params = root.join("steer.json");
    std::fs::write(
        &params,
        serde_json::json!({"session_id":session_id,"message":STEERING}).to_string(),
    )
    .unwrap();
    let steer = willdeep(&home)
        .args(["api", "turn.steer", "--params-file", path_text(&params)])
        .output()
        .unwrap();
    assert_success(&steer, "turn.steer");
    let ack: serde_json::Value = serde_json::from_slice(&steer.stdout).unwrap();
    assert_eq!(ack["data"]["delivered"], true);
    wait_for("model reads steering before background exits", || {
        provider.captured_requests.lock().unwrap().iter().any(|r| {
            r["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["role"] == "user" && m["content"] == STEERING)
        })
    });
    assert!(
        !release.0.exists(),
        "background must remain active until steering was read"
    );
    if delayed {
        assert!(
            !workspace.join("automatic-once").exists(),
            "steering must be read before the leading sleep completes"
        );
    }
    std::fs::write(&release.0, "release").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(if delayed { 45 } else { 15 });
    while client.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "Runtime client must finish"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(client.0.as_mut().unwrap().wait().unwrap().success());
    let saved = store.load(session_id).unwrap();
    assert_eq!(
        saved
            .messages
            .iter()
            .filter(|m| m.role == willdeep_core::Role::User && m.content == STEERING)
            .count(),
        1
    );
    if delayed {
        assert_eq!(
            std::fs::read_to_string(workspace.join("automatic-once")).unwrap(),
            "delayed-once\n"
        );
    }
}

#[cfg(unix)]
#[test]
fn runtime_steering_wakes_a_silent_monitor_after_the_turn_limit() {
    check_steering(true, false);
}

#[cfg(unix)]
#[test]
fn runtime_steering_wakes_a_silent_detached_job_after_the_turn_limit() {
    check_steering(false, false);
}

#[cfg(unix)]
#[test]
fn runtime_automatically_backgrounds_a_leading_sleep_and_reads_steering_before_it_finishes() {
    check_steering(false, true);
}
