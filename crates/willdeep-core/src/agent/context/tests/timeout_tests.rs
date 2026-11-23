use super::*;
use crate::types::Completion;

struct TimedProvider {
    requests: Mutex<Vec<Vec<Message>>>,
    delay: Duration,
    stall_after: usize,
    content: String,
}

impl TimedProvider {
    fn new(delay_ms: u64, stall_after: usize, content: &str) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            delay: Duration::from_millis(delay_ms),
            stall_after,
            content: content.to_owned(),
        })
    }

    fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[async_trait]
impl Provider for TimedProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        let call = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(messages.to_vec());
            requests.len() - 1
        };
        if call >= self.stall_after {
            std::future::pending::<()>().await;
        }
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok(Completion {
            reasoning: None,
            content: self.content.clone(),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".into()),
            usage: Some(Usage {
                input_tokens: Some(3),
                output_tokens: Some(2),
                total_tokens: Some(5),
                ..Usage::default()
            }),
        })
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<AgentEvent>>);

#[async_trait]
impl EventSink for Events {
    async fn emit(&self, event: AgentEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Events {
    fn assert_failed(&self) {
        let events = self.0.lock().unwrap();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::CompressionStarted { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AgentEvent::CompressionCompleted { .. }))
        );
    }
}

fn short_budget(total_ms: u64, candidate_ms: u64) -> CompressionBudget {
    CompressionBudget::new(
        Duration::from_millis(total_ms),
        Duration::from_millis(candidate_ms),
    )
}

#[tokio::test]
async fn timed_out_candidate_is_skipped_across_chunks_and_reduction() {
    let (agent, main) = agent(WINDOW);
    let stalled = TimedProvider::new(0, 0, "unreachable");
    let fallback = TimedProvider::new(1, usize::MAX, "summary");
    let events = Arc::new(Events::default());
    let agent = agent
        .with_compression_settings(CompressionSettings {
            candidates: vec![(stalled.clone(), false), (fallback.clone(), false)],
            context_window: Some(WINDOW / 4),
        })
        .with_event_sink(events.clone());
    let source = history();
    let settings = agent.compression_settings().unwrap();
    let chunks = chunk_for_summary(&source, agent.summary_input_budget_for(&settings)).len();
    assert!(chunks > 1);
    let mut bills = Vec::new();
    let result = agent
        .summarize_history_with_settings(
            &source,
            &settings,
            &mut short_budget(2_000, 20),
            &mut |usage| {
                bills.push(usage.clone());
                Ok(())
            },
        )
        .await
        .unwrap();
    assert_eq!(result, "summary");
    assert_eq!(
        stalled.calls(),
        1,
        "later chunks must not repeat the timeout"
    );
    assert_eq!(
        fallback.calls(),
        chunks + 1,
        "chunks and their final merge must use the fallback"
    );
    assert_eq!(bills.len(), fallback.calls());
    assert!(main.requests.lock().unwrap().is_empty());
    let events = events.0.lock().unwrap();
    let reasons: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ModelFallback { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
        .collect();
    assert!(reasons[0].contains("history summary chunk 1/"));
    assert!(reasons[0].contains("candidate limit 20ms"));
    assert!(
        reasons
            .iter()
            .skip(1)
            .all(|reason| reason.contains("skipped after earlier timeout"))
    );
}

#[tokio::test]
async fn fallback_candidates_share_the_total_deadline() {
    let (agent, _) = agent(WINDOW);
    let first = TimedProvider::new(0, 0, "unreachable");
    let second = TimedProvider::new(0, 0, "unreachable");
    let third = TimedProvider::new(0, usize::MAX, "must not run");
    let settings = CompressionSettings {
        candidates: vec![
            (first.clone(), false),
            (second.clone(), false),
            (third.clone(), false),
        ],
        context_window: None,
    };
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        agent.summarize_once(
            "old history".into(),
            &settings,
            &mut short_budget(100, 60),
            "candidate fallback test",
            &mut |_| Ok(()),
        ),
    )
    .await
    .expect("the shared deadline must end candidate fallback")
    .unwrap_err();
    assert_eq!(first.calls(), 1);
    assert_eq!(second.calls(), 1);
    assert_eq!(third.calls(), 0);
    let reason = error.to_string();
    assert!(reason.contains("candidate fallback test"), "{reason}");
    assert!(
        reason.contains("shared 100ms deadline exhausted"),
        "{reason}"
    );
}

#[tokio::test]
async fn chunk_bills_survive_when_the_shared_deadline_expires_during_reduction() {
    let (agent, _) = agent(WINDOW);
    let provider = TimedProvider::new(25, 2, "summary");
    let settings = CompressionSettings {
        candidates: vec![(provider.clone(), false)],
        context_window: None,
    };
    let input_budget = agent.summary_input_budget_for(&settings);
    let source = vec![Message::assistant("x".repeat(input_budget as usize * 3), vec![]); 2];
    assert_eq!(chunk_for_summary(&source, input_budget).len(), 2);
    let mut bills = Vec::new();
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        agent.summarize_history_with_settings(
            &source,
            &settings,
            &mut short_budget(160, 1_000),
            &mut |usage| {
                bills.push(usage.clone());
                Ok(())
            },
        ),
    )
    .await
    .expect("reduction must keep the chunks' original deadline")
    .unwrap_err();
    assert_eq!(provider.calls(), 3);
    assert_eq!(
        bills.len(),
        2,
        "already received bills must survive a later timeout"
    );
    let reason = error.to_string();
    assert!(reason.contains("summary reduction round 1"), "{reason}");
    assert!(
        reason.contains("shared 160ms deadline exhausted"),
        "{reason}"
    );
}

#[tokio::test]
async fn failed_incremental_refresh_keeps_the_previous_cache_and_reports_no_success() {
    let (agent, _) = agent(WINDOW);
    let mut messages = history();
    let mut cache = None;
    agent.request_messages(&messages, &mut cache).await.unwrap();
    let persisted = cache.as_ref().unwrap().persist();
    for index in 100..140 {
        push_batch(&mut messages, index);
    }
    let snapshot = serde_json::to_string(&messages).unwrap();
    for timeout in [true, false] {
        let provider = TimedProvider::new(0, if timeout { 0 } else { usize::MAX }, "");
        let events = Arc::new(Events::default());
        // The same old cache must remain usable after either a timeout or a
        // received but unusable (billed) completion.
        let mut cache = Some(SummaryCache::restore(persisted.clone()));
        let mut bills = Vec::new();
        let (observed, _) = super::agent(WINDOW);
        let observed = observed
            .with_compressors(vec![(provider.clone(), false)])
            .with_event_sink(events.clone());
        let result = observed
            .request_messages_with_budget(
                &messages,
                &mut cache,
                (None, ""),
                &mut |usage| {
                    bills.push(usage.clone());
                    Ok(())
                },
                &mut short_budget(100, 20),
            )
            .await;
        let reason = result.unwrap_err().to_string();
        if timeout {
            assert!(reason.contains("history summary"), "{reason}");
            assert!(reason.contains("candidate limit 20ms"), "{reason}");
        }
        assert_eq!(provider.calls(), 1);
        assert_eq!(bills.len(), usize::from(!timeout));
        assert_eq!(cache.as_ref().unwrap().persist(), persisted);
        assert_eq!(serde_json::to_string(&messages).unwrap(), snapshot);
        events.assert_failed();
    }
}

#[tokio::test]
async fn automatic_and_manual_second_pass_share_the_deadline_and_preserve_history() {
    let mut messages = vec![
        Message::system("system"),
        Message::user("preserve my files"),
        Message::assistant("older evidence", vec![]),
    ];
    for _ in 0..5 {
        messages.push(Message::assistant("x".repeat(12_000), vec![]));
    }
    messages.push(Message::user("continue"));
    let snapshot = serde_json::to_string(&messages).unwrap();
    for manual in [false, true] {
        let (agent, _) = agent(WINDOW);
        let capacity = message_capacity(
            WINDOW,
            &agent.request_definitions(),
            agent.completion_reserve(),
        )
        .unwrap();
        assert!(estimate_tokens(&compact_prefix(&messages, 3, "summary")) > capacity);
        let provider = TimedProvider::new(25, 1, "summary");
        let events = Arc::new(Events::default());
        let agent = agent
            .with_compressors(vec![(provider.clone(), false)])
            .with_event_sink(events.clone());
        let mut bills = Vec::new();
        let mut cache = None;
        let result = tokio::time::timeout(Duration::from_secs(1), async {
            let mut budget = short_budget(120, 1_000);
            let mut record = |usage: &Usage| {
                bills.push(usage.clone());
                Ok(())
            };
            if manual {
                agent
                    .compress_history_with_budget(messages.clone(), None, &mut record, &mut budget)
                    .await
            } else {
                agent
                    .request_messages_with_budget(
                        &messages,
                        &mut cache,
                        (None, ""),
                        &mut record,
                        &mut budget,
                    )
                    .await
            }
        })
        .await
        .expect("the second summary pass must keep the first pass's deadline");
        let reason = result.unwrap_err().to_string();
        assert!(
            reason.contains("shared 120ms deadline exhausted"),
            "{reason}"
        );
        assert_eq!(provider.calls(), 2);
        assert_eq!(bills.len(), 1);
        let requests = provider.requests.lock().unwrap();
        assert!(requests[0][0].content.contains("older evidence"));
        assert!(requests[1][0].content.contains("<context-summary"));
        assert!(
            cache.is_none(),
            "a partial first pass must not replace the old cache"
        );
        assert_eq!(serde_json::to_string(&messages).unwrap(), snapshot);
        events.assert_failed();
    }
}

#[tokio::test]
async fn reduction_limit_rejects_leftover_parts_instead_of_returning_the_last_one() {
    let (agent, _) = agent(WINDOW);
    // With this small window each request merges exactly two parts. The minimum
    // 257 parts leave two summaries after eight rounds.
    let part = "x".repeat(4_000);
    let provider = TimedProvider::new(0, usize::MAX, &part);
    let settings = CompressionSettings {
        candidates: vec![(provider.clone(), false)],
        context_window: Some(WINDOW / 4),
    };
    let error = agent
        .merge_summary_parts(
            vec![part; (1 << MAX_SUMMARY_REDUCE_ROUNDS) + 1],
            &settings,
            &mut CompressionBudget::default(),
            &mut |_| Ok(()),
        )
        .await
        .unwrap_err();
    let reason = error.to_string();
    assert!(
        reason.contains("2 parts remain after 8 reduction rounds"),
        "{reason}"
    );
    assert!(reason.contains("history was not replaced"), "{reason}");
    assert_eq!(provider.calls(), (1 << MAX_SUMMARY_REDUCE_ROUNDS) - 1);
}
