use super::*;
use crate::provider::{ProviderEvent, ProviderEventSink};
use crate::tools::ApprovalMode;
use crate::types::{Completion, Role, ToolDefinition};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

struct Script {
    replies: Mutex<VecDeque<Result<Completion, ProviderError>>>,
    requests: Mutex<Vec<Vec<Message>>>,
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for Script {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        unreachable!("event-aware entry required")
    }

    async fn complete_with_events(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
        events: &dyn ProviderEventSink,
    ) -> Result<Completion, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(messages.to_vec());
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected model call");
        if let Err(ProviderError::StreamInterrupted { partial, .. }) = &reply {
            events
                .emit(ProviderEvent::TextDelta(partial.content.clone()))
                .await;
            if let Some(usage) = &partial.usage {
                events.emit(ProviderEvent::Usage(usage.clone())).await;
            }
        }
        reply
    }
}

struct Fixture(std::path::PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("stream-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn agent(
        &self,
        replies: Vec<Result<Completion, ProviderError>>,
        max_turns: usize,
        token_budget: Option<u64>,
    ) -> (Agent, Arc<Script>) {
        let provider = Arc::new(Script {
            replies: Mutex::new(replies.into()),
            requests: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
        });
        let agent = Agent::new(
            provider.clone(),
            ToolRegistry::new(&self.0, ApprovalMode::FullAccess).unwrap(),
            AgentConfig {
                max_turns,
                token_budget,
                system_prompt: "test".into(),
                context_window: 32_000,
            },
        );
        (agent, provider)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn completion(text: &str) -> Completion {
    Completion {
        reasoning: None,
        content: text.into(),
        tool_calls: Vec::new(),
        usage: Some(Usage {
            input_tokens: Some(4),
            output_tokens: Some(2),
            total_tokens: Some(6),
            ..Default::default()
        }),
        finish_reason: Some("stop".into()),
    }
}

fn interrupted() -> ProviderError {
    ProviderError::StreamInterrupted {
        source: Box::new(ProviderError::InvalidResponse(
            "provider emitted a stream error: api_error: Codex: connection lost mid-reply (unexpected EOF)".into(),
        )),
        partial: Box::new(completion("unfinished")),
    }
}

#[tokio::test]
async fn stream_recovery_continues_from_partial_text_and_accounts_both_calls() {
    let fixture = Fixture::new();
    let (agent, provider) = fixture.agent(
        vec![Err(interrupted()), Ok(completion("finished"))],
        3,
        None,
    );
    let outcome = agent.run("answer").await.unwrap();
    assert_eq!(outcome.final_text, "finished");
    assert_eq!((outcome.input_tokens, outcome.output_tokens), (8, 4));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    let requests = provider.requests.lock().unwrap();
    assert!(
        requests[1]
            .iter()
            .any(|m| m.role == Role::Assistant && m.content == "unfinished")
    );
    assert!(
        requests[1]
            .iter()
            .any(|m| m.content.contains("[stream-recovery]"))
    );
    assert!(
        !outcome
            .messages
            .iter()
            .any(|m| m.content.contains("[stream-recovery]"))
    );
}

#[tokio::test]
async fn stream_recovery_stops_after_two_recoveries_and_preserves_the_error() {
    let fixture = Fixture::new();
    let (agent, provider) = fixture.agent(
        vec![Err(interrupted()), Err(interrupted()), Err(interrupted())],
        10,
        None,
    );
    assert!(matches!(
        agent.run("answer").await,
        Err(AgentError::Provider(
            ProviderError::StreamInterrupted { .. }
        ))
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn stream_recovery_respects_remaining_turns_and_permanent_errors() {
    let fixture = Fixture::new();
    for error in [interrupted(), ProviderError::MissingApiKey] {
        let (agent, provider) = fixture.agent(vec![Err(error)], 1, None);
        assert!(matches!(
            agent.run("answer").await,
            Err(AgentError::Provider(_))
        ));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
    let (agent, provider) = fixture.agent(vec![Err(ProviderError::MissingApiKey)], 10, None);
    assert!(matches!(
        agent.run("answer").await,
        Err(AgentError::Provider(ProviderError::MissingApiKey))
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stream_recovery_does_not_spend_past_token_budget_or_unknown_usage() {
    let fixture = Fixture::new();
    let (agent, provider) = fixture.agent(vec![Err(interrupted())], 3, Some(6));
    assert!(matches!(
        agent.run("answer").await,
        Err(AgentError::TokenBudgetExceeded { .. })
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let mut error = interrupted();
    if let ProviderError::StreamInterrupted { partial, .. } = &mut error {
        partial.usage = None;
    }
    let (agent, provider) = fixture.agent(vec![Err(error)], 3, Some(100));
    assert!(matches!(
        agent.run("answer").await,
        Err(AgentError::TokenBudgetUsageUnknown)
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stream_recovery_backoff_obeys_run_deadline() {
    let fixture = Fixture::new();
    let (agent, provider) = fixture.agent(vec![Err(interrupted())], 3, None);
    let outcome = agent
        .with_time_budget(Duration::from_millis(100))
        .run("answer")
        .await
        .unwrap();
    assert!(!outcome.stop_reason.is_complete());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(outcome.messages.iter().any(|m| m.content == "unfinished"));
}

#[tokio::test]
async fn stream_recovery_backoff_can_be_cancelled_without_another_request() {
    struct Waiting(tokio::sync::Notify);
    #[async_trait]
    impl EventSink for Waiting {
        async fn emit(&self, event: AgentEvent) {
            if matches!(
                event,
                AgentEvent::ProviderProgress(ProviderEvent::RetryWait { .. })
            ) {
                self.0.notify_one();
            }
        }
    }
    let fixture = Fixture::new();
    let (agent, provider) = fixture.agent(vec![Err(interrupted())], 3, None);
    let waiting = Arc::new(Waiting(tokio::sync::Notify::new()));
    let agent = agent.with_event_sink(waiting.clone());
    let mut run = Box::pin(agent.run("answer"));
    tokio::select! {
        _ = waiting.0.notified() => {}
        result = &mut run => panic!("run ended before recovery: {result:?}"),
    }
    drop(run);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stream_recovery_preserves_completed_tools_and_never_executes_partial_calls() {
    let fixture = Fixture::new();
    let call = ToolCall {
        id: "completed".into(),
        name: "create_file".into(),
        arguments: r#"{"path":"once.txt","content":"saved"}"#.into(),
    };
    let mut first = completion("");
    first.tool_calls.push(call.clone());
    let mut error = interrupted();
    if let ProviderError::StreamInterrupted { partial, .. } = &mut error {
        partial.tool_calls.push(ToolCall {
            id: "incomplete".into(),
            name: "create_file".into(),
            arguments: r#"{"path":"forbidden.txt","content":"must not execute"}"#.into(),
        });
    }
    let (agent, provider) = fixture.agent(
        vec![Ok(first), Err(error), Ok(completion("finished"))],
        3,
        None,
    );
    let outcome = agent.run("create once.txt and report").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.0.join("once.txt")).unwrap(),
        "saved"
    );
    assert!(!fixture.0.join("forbidden.txt").exists());
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[2]
            .iter()
            .filter(|m| m.tool_call_id.as_deref() == Some("completed"))
            .count(),
        1
    );
    assert!(
        !requests[2]
            .iter()
            .any(|m| m.tool_calls.iter().any(|c| c.id == "incomplete"))
    );
    assert_eq!(
        outcome
            .messages
            .iter()
            .filter(|m| m.tool_call_id.as_deref() == Some("completed"))
            .count(),
        1
    );
}
