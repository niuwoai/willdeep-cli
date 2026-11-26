use super::*;

#[test]
fn runtime_title_does_not_delay_terminal_events_or_the_next_turn() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let workspace = workspace.canonicalize().unwrap();
    let provider = MockProvider::start_with_mode(MockMode::TitleUntilReleased);
    let config = root.join("config.toml");
    write_titling_config(&config, provider.api_base());
    let listen = free_loopback_address();
    let mut guard = TestGuard::new(root, home.clone());
    let mut web = ChildGuard(Some(
        willdeep(&home)
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
            .unwrap(),
    ));

    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let client = reqwest::Client::new();
        let base = format!("http://{listen}");
        tokio::time::timeout(Duration::from_secs(10), async {
            while !client
                .get(format!("{base}/health"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("Web must start");
        let submit = |prompt: &str, session_id: Option<&str>| {
            client
                .post(format!("{base}/api/chat/stream"))
                .json(&serde_json::json!({
                    "prompt": prompt, "session_id": session_id, "workspace": workspace,
                    "language": "en", "attachments": []
                }))
        };
        let first = completed(submit("First conversation prompt", None)).await;
        let session_id = first["session_id"].as_str().unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while provider.requests() < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("background title request must start");
        assert_eq!(
            provider.captured_requests.lock().unwrap()[1]["model"],
            "mock-title"
        );

        let next_prompt = "Second conversation prompt";
        let second = completed(submit(next_prompt, Some(session_id))).await;
        assert_ne!(first["turn_id"], second["turn_id"]);
        provider.release_stream.store(true, Ordering::SeqCst);
        let store = willdeep_core::SessionStore::new(&home);
        let id = session_id.parse().unwrap();
        let stored = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let stored = store.load(id).unwrap();
                if stored.title_source == willdeep_core::TitleSource::Summarized {
                    break stored;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("title must be saved after releasing the request");
        assert_eq!(stored.title, MOCK_REPLY);
        assert!(
            stored
                .messages
                .iter()
                .any(|message| message.content == next_prompt)
        );
        assert_eq!(provider.requests(), 3, "two turns and one title request");
    });
    web.stop();
    guard.stop_daemon();
}

async fn completed(request: reqwest::RequestBuilder) -> serde_json::Value {
    let mut body = tokio::time::timeout(Duration::from_secs(5), async {
        request
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .text()
            .await
            .unwrap()
    })
    .await
    .expect("chat stream must close while the title request is blocked");
    while let Some(event) = pop_sse_json(&mut body) {
        assert_ne!(event["type"], "error", "{event}");
        if event["type"] == "completed" {
            assert_eq!(event["text"], MOCK_REPLY);
            return event;
        }
    }
    panic!("expected completed event");
}
