use super::*;

fn capture() -> Arc<CaptureProvider> {
    Arc::new(CaptureProvider {
        requests: Mutex::new(Vec::new()),
        stall_after: std::sync::atomic::AtomicUsize::new(usize::MAX),
    })
}

#[tokio::test]
async fn the_same_agent_switches_compressors_and_clears_the_old_window() {
    let (agent, main) = agent(WINDOW);
    let main_before = agent.provider().unwrap();
    let first = capture();
    let second = capture();
    let session_budget = agent.summary_input_budget();
    agent
        .set_compression_settings(CompressionSettings {
            candidates: vec![(first.clone(), true)],
            context_window: Some(WINDOW / 4),
        })
        .unwrap();
    let small_budget = agent.summary_input_budget();
    assert!(small_budget < session_budget);
    agent.compress_history(history()).await.unwrap();
    let first_calls = first.requests.lock().unwrap().len();
    assert!(
        first_calls > 1,
        "small windows must actually chunk this history"
    );
    assert!(first.requests.lock().unwrap().iter().all(|request| {
        !request[0].content.starts_with("Summarize this older")
            && estimate_tokens(request) <= small_budget + SUMMARY_OVERHEAD_TOKENS
    }));

    agent
        .set_compression_settings(CompressionSettings {
            candidates: vec![(second.clone(), false)],
            context_window: None,
        })
        .unwrap();
    assert_eq!(agent.summary_input_budget(), session_budget);
    agent.compress_history(history()).await.unwrap();
    let second_calls = second.requests.lock().unwrap().len();
    assert!(second_calls > 0);
    assert!(second_calls < first_calls);
    assert!(
        second.requests.lock().unwrap()[0][0]
            .content
            .starts_with("Summarize this older")
    );
    assert_eq!(first.requests.lock().unwrap().len(), first_calls);

    // A later /local turn also sees the refreshed compression settings.
    agent.request_messages(&history(), &mut None).await.unwrap();
    assert!(second.requests.lock().unwrap().len() > second_calls);
    assert!(main.requests.lock().unwrap().is_empty());
    assert!(Arc::ptr_eq(&main_before, &agent.provider().unwrap()));
}

struct PausedCompressor {
    requests: Mutex<Vec<Vec<Message>>>,
    started: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

#[async_trait]
impl Provider for PausedCompressor {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<crate::types::Completion, ProviderError> {
        let first = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(messages.to_vec());
            requests.len() == 1
        };
        if first {
            self.started.notify_one();
            self.resume.notified().await;
        }
        Ok(crate::types::Completion {
            reasoning: None,
            content: "OBJECTIVE: preserve the original compression snapshot".to_owned(),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_owned()),
            usage: None,
        })
    }
}

#[tokio::test]
async fn refreshing_during_a_request_keeps_the_original_snapshot_for_all_chunks() {
    let (agent, main) = agent(WINDOW);
    let first = Arc::new(PausedCompressor {
        requests: Mutex::new(Vec::new()),
        started: tokio::sync::Notify::new(),
        resume: tokio::sync::Notify::new(),
    });
    let second = capture();
    let agent = Arc::new(agent.with_compression_settings(CompressionSettings {
        candidates: vec![(first.clone(), true)],
        context_window: Some(WINDOW / 4),
    }));
    let budget = agent.summary_input_budget();
    let running_agent = agent.clone();
    let running = tokio::spawn(async move { running_agent.compress_history(history()).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), first.started.notified())
        .await
        .unwrap();
    agent
        .set_compression_settings(CompressionSettings {
            candidates: vec![(second.clone(), false)],
            context_window: None,
        })
        .unwrap();
    first.resume.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    {
        let requests = first.requests.lock().unwrap();
        assert!(requests.len() > 1);
        assert!(requests.iter().all(|request| {
            !request[0].content.starts_with("Summarize this older")
                && estimate_tokens(request) <= budget + SUMMARY_OVERHEAD_TOKENS
        }));
    }
    assert!(second.requests.lock().unwrap().is_empty());
    agent.compress_history(history()).await.unwrap();
    assert!(!second.requests.lock().unwrap().is_empty());
    assert!(main.requests.lock().unwrap().is_empty());
}
