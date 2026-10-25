use super::*;

pub(super) fn classification_response(request: &serde_json::Value) -> axum::response::Response {
    use axum::response::IntoResponse;
    let content = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .unwrap()["content"]
        .as_str()
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(content).unwrap();
    let decision = if payload["new_message"]
        .as_str()
        .is_some_and(|text| text.contains("after the original answer"))
    {
        "dependent"
    } else {
        "independent"
    };
    axum::Json(serde_json::json!({"choices":[{"message":{"content":serde_json::json!({"decision":decision,"blocking_ids":[]}).to_string(),"tool_calls":[]},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}})).into_response()
}

#[test]
fn runtime_yields_past_a_dependent_head_then_restores_the_original_question() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let provider = MockProvider::start_with_mode(MockMode::DeferredHumanWait);
    let config = root.join("config.toml");
    write_private_config(&config, provider.api_base());
    let contents = std::fs::read_to_string(&config)
        .unwrap()
        .replace("[agent]", "[agent]\nhuman_wait_yield_seconds = 1");
    std::fs::write(&config, contents).unwrap();
    let mut session = willdeep_core::Session::new(workspace.clone(), None, "deferred human wait");
    session.config = Some(config.clone());
    willdeep_core::SessionStore::new(&home)
        .save(&mut session)
        .unwrap();
    let _guard = TestGuard::new(root, home.clone());
    let run = willdeep(&home)
        .args([
            "run",
            "--config",
            path_text(&config),
            "--workspace",
            path_text(&workspace),
            "--session",
            &session.id.to_string(),
            "original question",
        ])
        .output()
        .unwrap();
    assert_eq!(
        run.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let client = runtime_client(&home);
    let call = |operation: &str, params: serde_json::Value| -> serde_json::Value {
        runtime
            .block_on(client.call::<_, serde_json::Value>(
                operation,
                &params,
                Some(uuid::Uuid::new_v4()),
            ))
            .unwrap()
            .into_result()
            .unwrap()
    };
    let original = call("question.list", serde_json::json!({}))[0]["id"].clone();
    let original_turn =
        call("turn.list", serde_json::json!({"session_id":session.id}))[0]["id"].clone();
    let submit = |prompt| {
        call(
            "turn.submit",
            serde_json::json!({"session_id":session.id,"prompt":prompt,"turn_request_id":uuid::Uuid::new_v4(),"attachments":[]}),
        )
    };
    let dependent = submit("after the original answer");
    for _ in 0..2 {
        let independent = submit("independent explanation");
        wait_until(Duration::from_secs(30), || {
            call("turn.get", serde_json::json!({"id":independent["id"]}))["status"] == "completed"
        });
        wait_until(Duration::from_secs(10), || {
            let questions = call("question.list", serde_json::json!({}));
            questions[0]["id"] == original && questions[0]["parked"] == false
        });
        let questions = call("question.list", serde_json::json!({}));
        assert_eq!(
            questions[0]["id"], original,
            "the original question retains its identity"
        );
        assert_eq!(questions[0]["parked"], false);
        let pending = call(
            "turn.queue.list",
            serde_json::json!({"session_id":session.id}),
        );
        assert!(
            pending
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["id"] == dependent["id"])
        );
    }
    let listen = free_loopback_address();
    let web = willdeep(&home)
        .args([
            "--web",
            "--listen",
            &listen,
            "--config",
            path_text(&config),
            "--workspace",
            path_text(&workspace),
        ])
        .spawn()
        .unwrap();
    let _web = ChildGuard(Some(web));
    let base = format!("http://{listen}");
    runtime.block_on(async {
        let http = reqwest::Client::new();
        await_web_health(&http, &base).await;
        let url = format!("{base}/api/sessions/{}/queue", session.id);
        let response = http.get(&url).send().await.unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert!(status.is_success(), "Web queue read: {status} {body}");
        let pending: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(pending[0]["id"], dependent["id"], "Web reads the Runtime-owned queue");
        let edited = http.patch(&url).json(&serde_json::json!({"action":"edit","message_id":dependent["id"],"expected_text":"after the original answer","text":"after the original answer, edited by Web"})).send().await.unwrap();
        assert!(edited.status().is_success());
        let pending = client.call::<_, serde_json::Value>("turn.queue.list", &serde_json::json!({"session_id":session.id}), None).await.unwrap().into_result().unwrap();
        assert_eq!(pending[0]["text"], "after the original answer, edited by Web");
        let stale = http.patch(&url).json(&serde_json::json!({"action":"edit","message_id":dependent["id"],"expected_text":"after the original answer","text":"stale edit"})).send().await.unwrap();
        assert!(!stale.status().is_success(), "stale cross-client edits cannot overwrite content");
        let removed = http.patch(&url).json(&serde_json::json!({"action":"remove","message_id":dependent["id"]})).send().await.unwrap();
        assert!(removed.status().is_success());
        let request = serde_json::json!({"session_id":session.id,"request_id":uuid::Uuid::new_v4(),"workspace":workspace.canonicalize().unwrap(),"language":"en","prompt":"after the original answer","attachments":[]});
        let mut ids = Vec::new();
        for _ in 0..2 {
            let response = http.post(&url).json(&request).send().await.unwrap();
            let status = response.status();
            let body = response.text().await.unwrap();
            assert!(status.is_success(), "Web enqueue: {status} {body}");
            ids.push(serde_json::from_str::<serde_json::Value>(&body).unwrap()["id"].clone());
        }
        assert_eq!(ids[0], ids[1], "retries share one durable submission");
        let pending = client.call::<_, serde_json::Value>("turn.queue.list", &serde_json::json!({"session_id":session.id}), None).await.unwrap().into_result().unwrap(); assert_eq!(pending.as_array().unwrap().len(), 1);
        assert!(!http.get(format!("{base}/api/sessions/{}/queue", uuid::Uuid::new_v4())).send().await.unwrap().status().is_success());
        let histories: serde_json::Value = http.get(format!("{base}/api/sessions/{}/independent", session.id)).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
        assert_eq!(histories.as_array().unwrap().len(), 2);
        let activity_url = format!("{base}/api/runtime/activity");
        let activity: serde_json::Value = http.get(&activity_url).query(&[("workspace", path_text(&workspace)), ("session", &session.id.to_string())]).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
        assert_eq!(activity["gates"][0]["id"], original, "Web can recover a question raised by a different client in the selected session");
        let foreign = uuid::Uuid::new_v4();
        let activity: serde_json::Value = http.get(&activity_url).query(&[("workspace", path_text(&workspace)), ("session", &foreign.to_string())]).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
        assert_eq!(activity["gates"], serde_json::json!([]));
        let answer_url = format!("{base}/api/runtime/questions/{}/answer", original.as_str().unwrap());
        let wrong = http.post(&answer_url).json(&serde_json::json!({"workspace":workspace,"session":foreign,"answer":"yes"})).send().await.unwrap();
        assert_eq!(wrong.status(), reqwest::StatusCode::NOT_FOUND, "another session cannot answer the original request");
        http.post(&answer_url).json(&serde_json::json!({"workspace":workspace,"session":session.id,"answer":"yes"})).send().await.unwrap().error_for_status().unwrap();
    });
    wait_until(Duration::from_secs(30), || {
        call("question.list", serde_json::json!({}))
            .as_array()
            .unwrap()
            .is_empty()
    });
    wait_until(Duration::from_secs(30), || {
        call("turn.get", serde_json::json!({"id":original_turn}))["status"] == "completed"
    });
    let core = willdeep_core::SessionStore::new(&home)
        .load(session.id)
        .unwrap();
    assert!(
        !core
            .messages
            .iter()
            .any(|message| message.role == willdeep_core::Role::User
                && message.source.is_none()
                && message.content == "independent explanation"),
        "display merging cannot overwrite the suspended execution history"
    );
    assert!(
        provider.requests() >= 6,
        "actual Runtime and tool-free classifier must both call the fixture provider"
    );
}
