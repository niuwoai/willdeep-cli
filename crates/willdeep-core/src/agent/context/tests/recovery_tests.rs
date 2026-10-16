use super::*;

#[tokio::test]
async fn repeated_host_steering_keeps_latest_occurrence_and_all_operator_input() {
    let (agent, _) = agent(WINDOW);
    let steering = "host constraints".repeat(800);
    let original = vec![
        Message::host_instruction(&steering),
        Message::user("repeat this instruction"),
        Message::host_instruction("distinct constraint"),
        Message::host_instruction(&steering),
        Message::user("repeat this instruction"),
    ];
    let result = agent.request_messages(&original, &mut None).await.unwrap();
    assert_eq!(result.iter().filter(|m| m.content == steering).count(), 1);
    assert_eq!(
        result
            .iter()
            .filter(|m| m.content == "repeat this instruction")
            .count(),
        2
    );
    assert!(
        result
            .iter()
            .position(|m| m.content == "distinct constraint")
            .unwrap()
            < result.iter().position(|m| m.content == steering).unwrap()
    );
    assert_eq!(original.len(), 5);
}

#[tokio::test]
async fn manual_compression_removes_accumulated_duplicate_host_instructions() {
    let (agent, _) = agent(WINDOW);
    let steering = "preserve every authorization".repeat(800);
    let mut original = vec![Message::system("system"), Message::user("actual request")];
    for _ in 0..47 {
        original.push(Message::host_instruction(&steering));
    }
    original.push(Message::assistant("done", vec![]));
    let result = agent.compress_history(original).await.unwrap();
    assert_eq!(result.iter().filter(|m| m.content == steering).count(), 1);
    assert!(result.iter().any(|m| m.content == "actual request"));
    assert!(
        estimate_tokens(&result) <= message_capacity(WINDOW, &agent.tools.definitions()).unwrap()
    );
}

#[test]
fn clamp_respects_budget_for_ascii_unicode_and_tiny_limits() {
    for budget in [0, 1, 4, 16, 128, 8192] {
        for text in [
            "ascii".repeat(10_000),
            "约束".repeat(10_000),
            "x约束\\\"".repeat(10_000),
        ] {
            assert!(text_tokens(&clamp_text(&text, budget)) <= budget);
        }
    }
}

#[test]
fn estimate_includes_reasoning_sent_back_to_thinking_models() {
    let plain = Message::assistant("inspect", vec![]);
    let thinking = plain.clone().with_reasoning(Some("r".repeat(40_000)));
    assert_eq!(
        estimate_tokens(&[thinking]) - estimate_tokens(&[plain]),
        10_000
    );
}

#[tokio::test]
async fn automatic_compression_handles_a_recent_thinking_response() {
    let (agent, provider) = agent(WINDOW);
    let original = vec![
        Message::system("Never expose credentials"),
        Message::user("Preserve dirty files; no push"),
        Message::assistant("inspected", vec![]).with_reasoning(Some("r".repeat(80_000))),
        Message::user("continue"),
    ];
    let snapshot = serde_json::to_string(&original).unwrap();
    let mut cache = None;
    let result = agent.request_messages(&original, &mut cache).await.unwrap();
    let capacity = message_capacity(WINDOW, &agent.tools.definitions()).unwrap();
    assert!(estimate_tokens(&result) <= capacity);
    assert!(result.iter().any(|m| m.content == original[1].content));
    assert!(result.iter().any(|m| m.content == "continue"));
    assert!(!provider.requests.lock().unwrap().is_empty());
    assert_eq!(serde_json::to_string(&original).unwrap(), snapshot);
    let calls = provider.requests.lock().unwrap().len();
    agent.request_messages(&original, &mut cache).await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), calls);
}

#[tokio::test]
async fn manual_compression_handles_short_but_oversized_history() {
    let (agent, _) = agent(WINDOW);
    let original = vec![
        Message::system("Never expose credentials"),
        Message::user("Preserve my dirty files"),
        Message::assistant("inspected", vec![]).with_reasoning(Some("r".repeat(80_000))),
    ];
    let result = agent.compress_history(original).await.unwrap();
    assert!(
        estimate_tokens(&result) < message_capacity(WINDOW, &agent.tools.definitions()).unwrap()
    );
    assert!(
        result
            .iter()
            .any(|m| m.content == "Preserve my dirty files")
    );
}

#[tokio::test]
async fn manual_compression_folds_oversized_recent_tail_too() {
    let (agent, _) = agent(WINDOW);
    let mut original = history();
    original.push(Message::assistant("tail", vec![]).with_reasoning(Some("r".repeat(80_000))));
    let result = agent.compress_history(original).await.unwrap();
    assert!(
        estimate_tokens(&result) <= message_capacity(WINDOW, &agent.tools.definitions()).unwrap()
    );
    assert_pairs(&result);
}

#[derive(Default)]
struct Events(Mutex<Vec<AgentEvent>>);

#[async_trait]
impl EventSink for Events {
    async fn emit(&self, event: AgentEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[tokio::test]
async fn uncompressible_user_input_never_emits_false_success() {
    let (agent, _) = agent(WINDOW);
    let sink = Arc::new(Events::default());
    let agent = agent.with_event_sink(sink.clone());
    let mut original = history();
    original.push(Message::user("constraint".repeat(12_000)));
    assert!(matches!(
        agent.compress_history(original.clone()).await,
        Err(AgentError::ContextCapacity { .. })
    ));
    assert!(
        !sink
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, AgentEvent::CompressionCompleted { .. }))
    );
    sink.0.lock().unwrap().clear();
    assert!(matches!(
        agent.request_messages(&original, &mut None).await,
        Err(AgentError::ContextCapacity { .. })
    ));
    assert!(
        !sink
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, AgentEvent::CompressionCompleted { .. }))
    );
}

#[tokio::test]
async fn single_chunk_summary_uses_bounded_serialized_fields() {
    let (agent, provider) = agent(WINDOW);
    let message = Message::assistant("short", vec![]).with_reasoning(Some("\\\"".repeat(80_000)));
    agent
        .summarize_history(&[message], &mut |_| Ok(()))
        .await
        .unwrap();
    let requests = provider.requests.lock().unwrap();
    assert!(!requests.is_empty());
    assert!(requests.iter().all(|request| estimate_tokens(request)
        <= agent.summary_input_budget() + SUMMARY_OVERHEAD_TOKENS));
}
