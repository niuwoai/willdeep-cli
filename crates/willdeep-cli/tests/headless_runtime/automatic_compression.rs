use super::*;

#[test]
fn local_automatic_compression_preserves_constraints_batches_and_usage() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let provider = MockProvider::start();
    let config = root.join("config.toml");
    write_private_config(&config, provider.api_base());
    let mut contents = std::fs::read_to_string(&config).unwrap();
    contents.push_str("context_window = 32000\n");
    std::fs::write(&config, contents).unwrap();
    let _guard = TestGuard::new(root, home.clone());
    let id = uuid::Uuid::new_v4();
    let constraint =
        "Only inspect src/allowed.rs; never change protected.txt; preserve this exact constraint.";
    let mut messages = vec![serde_json::json!({"role":"user","content":constraint})];
    for index in 0..10 {
        messages.push(serde_json::json!({"role":"assistant","content":format!("old-inspection-{index}: {}", "historical evidence ".repeat(1000))}));
    }
    let arguments = serde_json::json!({"path":"src/allowed.rs","offset":17,"limit":3}).to_string();
    messages.push(serde_json::json!({"role":"assistant","content":"recent inspection","tool_calls":[{"id":"recent-read","name":"read_file","arguments":arguments}]}));
    messages.push(serde_json::json!({"role":"tool","content":"recent tool result","tool_call_id":"recent-read"}));
    for index in 0..3 {
        messages.push(
            serde_json::json!({"role":"assistant","content":format!("Recent observation {index}")}),
        );
    }
    write_json(
        &home.join("sessions").join(format!("{id}.json")),
        serde_json::json!({"version":1,"id":id,"title":"Automatic compression","workspace":workspace,"profile":"mock","created_at":1,"updated_at":1,"messages":messages}),
    );
    let output = willdeep(&home)
        .args([
            "run",
            "--local",
            "--config",
            path_text(&config),
            "--workspace",
            path_text(&workspace),
            "--session",
            &id.to_string(),
            "--output",
            "ndjson",
            "Continue the original inspection without changing any files",
        ])
        .output()
        .unwrap();
    assert_success(&output, "automatic threshold compression");
    let events: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for kind in ["compression_started", "compression_completed", "completed"] {
        assert!(
            events.iter().any(|event| event["type"] == kind),
            "missing {kind}"
        );
    }
    let requests = provider.captured_requests.lock().unwrap();
    // 这段历史（约 5 万 token）本来就比窗口（3.2 万）大，所以摘要必然分块：
    // 若干条摘要请求，最后一条才是任务本身。分块之前这里是「一口吞」，对着真
    // provider 就是一个 400——正是会话被锁死的那条路。
    assert!(
        requests.len() >= 2,
        "expected summary requests plus one task request, got {}",
        requests.len()
    );
    let (task, summaries) = requests.split_last().unwrap();
    for summary in summaries {
        let estimated = summary["messages"].to_string().chars().count() / 4;
        assert!(
            estimated < 32_000,
            "summary request of ~{estimated} tokens must stay inside the 32000 window"
        );
    }
    let summary_source = summaries[0]["messages"].to_string();
    assert!(summary_source.contains("old-inspection-0"));
    let task = task["messages"].as_array().unwrap();
    assert!(
        task.iter()
            .any(|message| message["role"] == "user" && message["content"] == constraint)
    );
    assert!(
        !serde_json::to_string(task)
            .unwrap()
            .contains("old-inspection-0")
    );
    let call_index = task
        .iter()
        .position(|message| message["tool_calls"][0]["id"] == "recent-read")
        .expect("recent tool call preserved");
    assert_eq!(
        task[call_index]["tool_calls"][0]["function"]["arguments"],
        arguments
    );
    assert_eq!(task[call_index + 1]["role"], "tool");
    assert_eq!(task[call_index + 1]["tool_call_id"], "recent-read");
    assert_eq!(task[call_index + 1]["content"], "recent tool result");
    let session = willdeep_core::SessionStore::new(&home).load(id).unwrap();
    let checkpoint = session.execution_checkpoint.unwrap();
    // 每次 provider 调用都要记一笔账——分块摘要多打了几次，账就得多几笔，
    // 写死的数字只会在块数变化时掩盖真正的漏记。
    let calls = requests.len() as u64;
    assert_eq!(
        (checkpoint.input_tokens, checkpoint.output_tokens),
        (5 * calls, 3 * calls)
    );
    assert_eq!(session.manual_compression_usage.reported_calls, 0);
    assert!(
        session
            .messages
            .iter()
            .any(|message| message.role == willdeep_core::Role::User
                && message.content == constraint)
    );
}

/// 压缩结果随会话落盘：同一段历史的下一次 run 不再重摘。
///
/// 以前摘要缓存只活在单次 run 里，durable 历史又从不因压缩变短，于是一轮七八
/// 分钟的压缩每来一条消息就要重跑一遍——这个会话在 150 万 token 时正是这样。
#[test]
fn persisted_summary_spares_the_next_run_from_recompressing() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(home.join("sessions")).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let provider = MockProvider::start();
    let config = root.join("config.toml");
    write_private_config(&config, provider.api_base());
    let mut contents = std::fs::read_to_string(&config).unwrap();
    contents.push_str("context_window = 32000\n");
    std::fs::write(&config, contents).unwrap();
    let _guard = TestGuard::new(root, home.clone());
    let id = uuid::Uuid::new_v4();
    let mut messages =
        vec![serde_json::json!({"role":"user","content":"Keep the migration history intact."})];
    for index in 0..10 {
        messages.push(serde_json::json!({"role":"assistant","content":format!("old-inspection-{index}: {}", "historical evidence ".repeat(1000))}));
    }
    write_json(
        &home.join("sessions").join(format!("{id}.json")),
        serde_json::json!({"version":1,"id":id,"title":"Persisted summary","workspace":workspace,"profile":"mock","created_at":1,"updated_at":1,"messages":messages}),
    );

    let run = |prompt: &str| {
        let output = willdeep(&home)
            .args([
                "run",
                "--local",
                "--config",
                path_text(&config),
                "--workspace",
                path_text(&workspace),
                "--session",
                &id.to_string(),
                "--output",
                "ndjson",
                prompt,
            ])
            .output()
            .unwrap();
        assert_success(&output, "conversation turn");
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>()
    };

    let first = run("Start the inspection");
    assert!(
        first
            .iter()
            .any(|event| event["type"] == "compression_started"),
        "the first turn must compress the oversized history"
    );
    let after_first = {
        let requests = provider.captured_requests.lock().unwrap();
        requests.len()
    };
    assert!(
        after_first >= 2,
        "expected chunked summaries plus the task request, got {after_first}"
    );
    let session = willdeep_core::SessionStore::new(&home).load(id).unwrap();
    assert!(
        session.context_summary.is_some(),
        "the summary must be persisted with the session"
    );

    let second = run("Continue the inspection");
    assert!(
        !second
            .iter()
            .any(|event| event["type"] == "compression_started"),
        "the persisted summary must cover the unchanged history"
    );
    assert!(
        second.iter().any(|event| event["type"] == "completed"),
        "the second turn must still finish"
    );
    let after_second = {
        let requests = provider.captured_requests.lock().unwrap();
        requests.len()
    };
    assert_eq!(
        after_second,
        after_first + 1,
        "only the task request itself may be sent on the next turn"
    );
    // 请求数对得上不算数：摘要正文空着、历史被截短，都长上面那个样子。
    let task = {
        let requests = provider.captured_requests.lock().unwrap();
        requests.last().cloned().expect("the second turn must call")
    };
    let body = task["messages"].to_string();
    assert!(
        body.contains(MOCK_REPLY),
        "the reused summary must carry the summarizer's text, not just its shell"
    );
    assert!(
        body.contains("<context-summary"),
        "the reused summary must reach the model"
    );
    assert!(
        !body.contains("old-inspection-0"),
        "the summarized prefix must not be sent again"
    );
    let reloaded = willdeep_core::SessionStore::new(&home).load(id).unwrap();
    assert_eq!(
        reloaded.context_summary, session.context_summary,
        "the summary must survive the second turn unchanged"
    );
    assert_eq!(
        reloaded.messages.len(),
        session.messages.len() + 2,
        "the second turn must append its own exchange, not truncate history"
    );
}
