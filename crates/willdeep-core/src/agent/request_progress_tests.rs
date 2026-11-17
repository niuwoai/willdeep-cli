use super::*;
use crate::tools::ApprovalMode;
use crate::types::{Completion, ToolDefinition};
use std::sync::atomic::AtomicUsize;

#[tokio::test]
async fn request_progress_follows_preparation_and_precedes_provider_execution() {
    struct Progress(Arc<AtomicUsize>);
    #[async_trait]
    impl EventSink for Progress {
        async fn emit(&self, event: AgentEvent) {
            match event {
                AgentEvent::TurnStarted { .. } => {
                    assert_eq!(self.0.swap(1, Ordering::SeqCst), 0);
                }
                AgentEvent::ModelRequestStarted => {
                    assert_eq!(self.0.swap(2, Ordering::SeqCst), 1);
                }
                _ => {}
            }
        }
    }
    struct Ready(Arc<AtomicUsize>);
    #[async_trait]
    impl Provider for Ready {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<Completion, ProviderError> {
            // A client can show model waiting before the first response exists,
            // and only after request preparation has actually finished.
            assert_eq!(self.0.swap(3, Ordering::SeqCst), 2);
            Ok(Completion {
                reasoning: None,
                content: "ready".into(),
                tool_calls: Vec::new(),
                usage: None,
                finish_reason: Some("stop".into()),
            })
        }
    }
    let root = std::env::temp_dir().join(format!(
        "willdeep-request-progress-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let phase = Arc::new(AtomicUsize::new(0));
    let agent = Agent::new(
        Arc::new(Ready(phase.clone())),
        ToolRegistry::new(&root, ApprovalMode::ReadOnly).unwrap(),
        AgentConfig {
            max_turns: 1,
            system_prompt: String::new(),
            context_window: 32_000,
            token_budget: None,
        },
    )
    .with_event_sink(Arc::new(Progress(phase.clone())));
    assert_eq!(agent.run("say ready").await.unwrap().final_text, "ready");
    assert_eq!(phase.load(Ordering::SeqCst), 3);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn model_waiting_starts_after_actual_automatic_compression() {
    use crate::types::ToolCall;
    use std::sync::Mutex;

    struct Progress(Arc<Mutex<Vec<&'static str>>>);
    #[async_trait]
    impl EventSink for Progress {
        async fn emit(&self, event: AgentEvent) {
            let name = match event {
                AgentEvent::TurnStarted { .. } => "preparing",
                AgentEvent::CompressionStarted { .. } => "compressing",
                AgentEvent::CompressionCompleted { .. } => "compressed",
                AgentEvent::ModelRequestStarted => "waiting",
                _ => return,
            };
            self.0.lock().unwrap().push(name);
        }
    }
    struct Response {
        trace: Arc<Mutex<Vec<&'static str>>>,
        summary: bool,
    }
    #[async_trait]
    impl Provider for Response {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<Completion, ProviderError> {
            self.trace.lock().unwrap().push(if self.summary {
                "summary request"
            } else {
                "foreground request"
            });
            Ok(Completion {
                reasoning: None,
                content: if self.summary {
                    "OBJECTIVE: inspect files. COMPLETED: read files. REMAINING: report findings."
                } else {
                    "ready"
                }
                .into(),
                tool_calls: Vec::new(),
                usage: None,
                finish_reason: Some("stop".into()),
            })
        }
    }
    let root = std::env::temp_dir().join(format!(
        "willdeep-progress-compression-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let trace = Arc::new(Mutex::new(Vec::new()));
    let agent = Agent::new(
        Arc::new(Response {
            trace: trace.clone(),
            summary: false,
        }),
        ToolRegistry::new(&root, ApprovalMode::ReadOnly)
            .unwrap()
            .with_allowed_tools(["read_file".to_owned()]),
        AgentConfig {
            max_turns: 1,
            system_prompt: "Inspect files".into(),
            context_window: 12_288,
            token_budget: None,
        },
    )
    .with_compressors(vec![(
        Arc::new(Response {
            trace: trace.clone(),
            summary: true,
        }),
        false,
    )])
    .with_event_sink(Arc::new(Progress(trace.clone())));
    let mut history = vec![Message::user("Inspect the files and report")];
    for index in 0..20 {
        let call = ToolCall {
            id: format!("read-{index}"),
            name: "read_file".into(),
            arguments: format!(r#"{{"path":"src/file-{index}.rs"}}"#),
        };
        history.push(Message::assistant("inspect", vec![call.clone()]));
        history.push(Message::tool(
            &call,
            format!("file-{index}: {}", "x".repeat(1_900)),
        ));
    }
    let result = agent.run_with_history(history, "report now").await.unwrap();
    assert_eq!(result.final_text, "ready");
    assert!(
        result.context_summary.is_some(),
        "fixture must actually compress"
    );
    let trace = trace.lock().unwrap();
    let compressed = trace
        .iter()
        .rposition(|name| *name == "compressed")
        .unwrap();
    let waiting = trace.iter().position(|name| *name == "waiting").unwrap();
    let foreground = trace
        .iter()
        .position(|name| *name == "foreground request")
        .unwrap();
    assert!(trace.contains(&"summary request"));
    assert!(compressed < waiting && waiting < foreground, "{trace:?}");
    assert_eq!(trace.iter().filter(|name| **name == "waiting").count(), 1);
    std::fs::remove_dir_all(root).unwrap();
}
