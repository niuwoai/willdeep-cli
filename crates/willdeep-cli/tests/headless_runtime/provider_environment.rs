use super::*;

#[test]
fn gateway_profile_aliases_reach_foreground_runtime_and_explicit_child() {
    let _serial = process_test_guard();
    for local in [true, false] {
        let root = temporary_root();
        let home = root.join("home");
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let provider = MockProvider::start_with_mode(MockMode::DelegateThenSuccess);
        let config = root.join("config.toml");
        write_private_text(
            &config,
            r#"version=1
default_provider="magpie"
[agent]
auto_title=false
max_turns=4
approval="smart"
[providers.magpie]
provider="openai-compatible"
api="chat-completions"
api_base_env="OPENAI_BASE_URL"
api_key_env="OPENAI_API_KEY"
model="mock-parent"
[providers.worker]
provider="openai-compatible"
api_key_env="OPENAI_API_KEY"
model="mock-worker"
[subagents.scout]
provider_profile="worker"
"#,
        );
        let _guard = TestGuard::new(root, home.clone());
        let mut command = willdeep(&home);
        command
            .env("OPENAI_BASE_URL", provider.api_base())
            .env("OPENAI_API_KEY", "magpie-willdeep");
        command.args([
            "run",
            "--config",
            path_text(&config),
            "--workspace",
            path_text(&workspace),
            "--output",
            "json",
        ]);
        if local {
            command.arg("--local");
        }
        let output = command
            .arg("Delegate a read-only diagnosis then report the result")
            .output()
            .unwrap();
        assert_success(
            &output,
            if local {
                "foreground gateway profile"
            } else {
                "Runtime gateway profile"
            },
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["type"], "completed");
        let requests = provider.captured_requests.lock().unwrap();
        let models = requests
            .iter()
            .map(|request| request["model"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(models, ["mock-parent", "mock-worker", "mock-parent"]);
        if !local {
            let tasks = willdeep(&home).args(["api", "task.list"]).output().unwrap();
            assert_success(&tasks, "Runtime task evidence");
            let value: serde_json::Value = serde_json::from_slice(&tasks.stdout).unwrap();
            assert!(
                !value["data"].as_array().unwrap().is_empty(),
                "must execute through Runtime"
            );
        }
    }
}
