use super::*;
use crate::types::Completion;
use std::sync::atomic::{AtomicUsize, Ordering};

const TOOL_TURNS: usize = 5;

struct ReadLoopProvider {
    requests: Mutex<Vec<Vec<Message>>>,
    calls: AtomicUsize,
    compressor: Arc<CaptureProvider>,
    summary_calls: Mutex<Vec<usize>>,
    live_plan: bool,
}

#[async_trait]
impl Provider for ReadLoopProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        self.requests.lock().unwrap().push(messages.to_vec());
        self.summary_calls
            .lock()
            .unwrap()
            .push(self.compressor.requests.lock().unwrap().len());
        let turn = self.calls.fetch_add(1, Ordering::SeqCst);
        let tool_calls = if self.live_plan && turn < 2 {
            let status = if turn == 0 { "in_progress" } else { "done" };
            vec![ToolCall {
                id: format!("fresh-plan-{turn}"),
                name: "update_plan".into(),
                arguments: serde_json::json!({
                    "steps": [{"id": "inspect", "title": "Inspect", "status": status}]
                })
                .to_string(),
            }]
        } else if turn < TOOL_TURNS {
            vec![ToolCall {
                id: format!("fresh-read-{turn}"),
                name: "read_file".into(),
                arguments: r#"{"path":"current.txt"}"#.into(),
            }]
        } else {
            Vec::new()
        };
        let (content, finish_reason) = if tool_calls.is_empty() {
            let content = if self.live_plan {
                "<goal-status>complete</goal-status> done"
            } else {
                "done"
            };
            (content, "stop")
        } else {
            ("inspect", "tool_calls")
        };
        Ok(Completion {
            reasoning: None,
            content: content.into(),
            finish_reason: Some(finish_reason.into()),
            tool_calls,
            usage: None,
        })
    }
}

fn loop_agent(live_plan: bool) -> (Agent, Arc<ReadLoopProvider>, Arc<CaptureProvider>) {
    let (mut agent, compressor) = agent(WINDOW);
    if live_plan {
        agent.tools = ToolRegistry::new(agent.tools.workspace(), crate::ApprovalMode::Strict)
            .unwrap()
            .with_allowed_tools(["read_file".to_owned(), "update_plan".to_owned()]);
    }
    std::fs::write(
        agent.tools.workspace().join("current.txt"),
        "small new output",
    )
    .unwrap();
    let provider = Arc::new(ReadLoopProvider {
        requests: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
        compressor: compressor.clone(),
        summary_calls: Mutex::new(Vec::new()),
        live_plan,
    });
    agent.provider = crate::provider::MainModelHandle::new(provider.clone());
    agent.config.max_turns = TOOL_TURNS + 1;
    let agent = agent.with_compressors(vec![(compressor.clone(), false)]);
    (agent, provider, compressor)
}

#[tokio::test]
async fn real_agent_tool_turns_reuse_the_compressed_prefix() {
    let (agent, provider, compressor) = loop_agent(false);
    let root = agent.tools.workspace().to_path_buf();
    let result = agent.run_with_history(history(), "continue").await.unwrap();
    let restored = agent
        .run_checkpointed_with_summary(
            result.messages.clone(),
            Message::user("continue again"),
            None,
            result.context_summary.clone(),
        )
        .await;
    std::fs::remove_dir_all(root).unwrap();
    let restored = restored.unwrap();
    assert_eq!(restored.final_text, "done");
    assert_eq!(restored.turns, 1);
    assert_eq!(result.final_text, "done");
    assert_eq!(result.turns, TOOL_TURNS + 1);
    assert!(result.context_summary.is_some());
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), TOOL_TURNS + 2);
    for request in requests.iter() {
        assert_pairs(request);
        assert!(
            request
                .iter()
                .any(|message| message.content.contains("<context-summary"))
        );
        assert!(
            request
                .iter()
                .any(|message| message.content == history()[1].content)
        );
    }
    let summary_calls = provider.summary_calls.lock().unwrap();
    assert!(
        summary_calls[0] > 0,
        "the initial history requires compression"
    );
    assert_eq!(
        *summary_calls,
        vec![summary_calls[0]; TOOL_TURNS + 2],
        "small tool results and a subsequent run must reuse the initial summary"
    );
    for (index, request) in requests.iter().enumerate() {
        if index > 0 {
            let previous = (index - 1).min(TOOL_TURNS - 1);
            let id = format!("fresh-read-{previous}");
            assert!(
                request
                    .iter()
                    .flat_map(|message| &message.tool_calls)
                    .any(|call| { call.id == id && call.arguments == r#"{"path":"current.txt"}"# })
            );
            assert!(request.iter().any(|message| {
                message.tool_call_id.as_deref() == Some(id.as_str())
                    && message.content.contains("small new output")
            }));
        }
        let turn = if index <= TOOL_TURNS { index + 1 } else { 1 };
        assert!(request.last().unwrap().content.contains(&format!(
            "Current provider round: {turn}/{}.",
            TOOL_TURNS + 1
        )));
        let capacity = message_capacity(
            WINDOW,
            &agent.request_definitions(),
            agent.completion_reserve(),
        )
        .unwrap();
        assert!(estimate_tokens(request) <= capacity);
    }
    assert!(
        compressor
            .requests
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .all(|message| { !message.content.contains("[execution-budget]") })
    );
}

#[tokio::test]
async fn real_runs_invalidate_summaries_after_prefix_or_rule_changes() {
    let (mut agent, provider, compressor) = loop_agent(false);
    // These runs finish immediately; only the persisted summary is under test.
    provider.calls.store(TOOL_TURNS, Ordering::SeqCst);
    let mut first = agent.run_with_history(history(), "continue").await.unwrap();
    let first_calls = compressor.requests.lock().unwrap().len();
    let original = first
        .messages
        .iter_mut()
        .find(|message| message.tool_calls.iter().any(|call| call.id == "call-0"))
        .unwrap();
    original.content = "REWRITTEN_PREFIX_CANARY".into();
    let second = agent
        .run_checkpointed_with_summary(
            first.messages,
            Message::user("continue"),
            None,
            first.context_summary,
        )
        .await
        .unwrap();
    let second_calls = compressor.requests.lock().unwrap().len();
    assert!(second_calls > first_calls);
    assert!(
        compressor.requests.lock().unwrap()[first_calls..]
            .iter()
            .flatten()
            .any(|message| message.content.contains("REWRITTEN_PREFIX_CANARY"))
    );

    agent.config.system_prompt = "UPDATED_SYSTEM_RULE_CANARY".into();
    let third = agent
        .run_checkpointed_with_summary(
            second.messages,
            Message::user("continue"),
            None,
            second.context_summary,
        )
        .await;
    std::fs::remove_dir_all(agent.tools.workspace()).unwrap();
    assert_eq!(third.unwrap().final_text, "done");
    let requests = compressor.requests.lock().unwrap();
    assert!(requests.len() > second_calls);
    assert!(
        requests[second_calls..]
            .iter()
            .flatten()
            .any(|message| message.content.contains("UPDATED_SYSTEM_RULE_CANARY"))
    );
}

#[tokio::test]
async fn live_plan_goal_and_deadline_survive_compression_without_invalidating_it() {
    let (agent, provider, compressor) = loop_agent(true);
    let goal = Arc::new(GoalContinuation::new());
    goal.activate("inspect the fixture", crate::goal::GoalBudget::default());
    let agent = agent
        .with_goal_continuation(goal)
        .with_time_budget(std::time::Duration::from_secs(60));
    let result = agent.run_with_history(history(), "continue").await;
    std::fs::remove_dir_all(agent.tools.workspace()).unwrap();
    assert_eq!(result.unwrap().stop_reason, AgentStopReason::GoalComplete);
    let counts = provider.summary_calls.lock().unwrap();
    assert!(counts[0] > 0);
    assert_eq!(*counts, vec![counts[0]; TOOL_TURNS + 1]);
    let requests = provider.requests.lock().unwrap();
    let baseline_system = requests[0][0].content.clone();
    for request in requests.iter() {
        assert_pairs(request);
        assert_eq!(
            request[0].content, baseline_system,
            "the system message must stay byte-identical between turns for prefix caching"
        );
        let live = &request.last().unwrap().content;
        assert!(live.contains("<goal-state>"));
        assert!(live.contains("[run-time-budget]"));
        assert!(
            !request[0].content.contains("<goal-state>"),
            "live state must stay out of the cached system prefix"
        );
        assert!(
            estimate_tokens(request)
                <= message_capacity(
                    WINDOW,
                    &agent.request_definitions(),
                    agent.completion_reserve()
                )
                .unwrap()
        );
    }
    assert!(
        requests[1]
            .last()
            .unwrap()
            .content
            .contains("\"running\":[\"inspect\"]")
    );
    let plan_json = requests[2]
        .last()
        .unwrap()
        .content
        .split_once("<session-plan>")
        .unwrap()
        .1
        .split_once("</session-plan>")
        .unwrap()
        .0;
    let receipt: crate::planning::Receipt = serde_json::from_str(plan_json).unwrap();
    assert_eq!(receipt.plan.steps[0].id, "inspect");
    assert_eq!(
        receipt.plan.steps[0].status,
        crate::conversation::StepStatus::Done
    );
    for message in compressor.requests.lock().unwrap().iter().flatten() {
        for marker in [
            "<session-plan>",
            "<goal-state>",
            "[run-time-budget]",
            "[execution-budget]",
        ] {
            assert!(!message.content.contains(marker));
        }
    }
}

#[tokio::test]
async fn deadline_outcome_and_checkpoint_keep_the_full_system_pin() {
    #[derive(Default)]
    struct Checkpoints(Mutex<Vec<crate::checkpoint::RunCheckpoint>>);
    impl crate::checkpoint::CheckpointSink for Checkpoints {
        fn save(&self, checkpoint: &crate::checkpoint::RunCheckpoint) -> Result<(), String> {
            self.0.lock().unwrap().push(checkpoint.clone());
            Ok(())
        }
    }
    let (mut agent, _, compressor) = loop_agent(false);
    let root = Arc::new(CaptureProvider {
        requests: Mutex::new(Vec::new()),
        stall_after: AtomicUsize::new(0),
    });
    agent.provider = crate::provider::MainModelHandle::new(root.clone());
    let agent = agent.with_time_budget(std::time::Duration::from_secs(2));
    let checkpoints = Checkpoints::default();
    let result = agent
        .run_checkpointed(history(), Message::user("continue"), Some(&checkpoints))
        .await;
    std::fs::remove_dir_all(agent.tools.workspace()).unwrap();
    let result = result.unwrap();
    assert_eq!(result.stop_reason, AgentStopReason::BudgetLimited);
    assert!(!compressor.requests.lock().unwrap().is_empty());
    let requests = root.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let system = &requests[0][0].content;
    let live = &requests[0].last().unwrap().content;
    assert!(live.contains("[execution-budget]"));
    assert!(live.contains("[run-time-budget]"));
    assert!(
        !system.contains("[execution-budget]"),
        "live budgets stay out of the durable system pin"
    );
    assert_eq!(&result.messages[0].content, system);
    assert!(
        result
            .messages
            .iter()
            .all(|message| !message.content.contains("[run-time-budget]")),
        "live state must never enter durable history"
    );
    let saved = checkpoints.0.lock().unwrap();
    assert_eq!(&saved.last().unwrap().messages[0].content, system);
}

#[tokio::test]
async fn live_context_reserves_capacity_without_entering_summary_input() {
    let (agent, compressor) = agent(WINDOW);
    let capacity = message_capacity(
        WINDOW,
        &agent.request_definitions(),
        agent.completion_reserve(),
    )
    .unwrap();
    let live_context = format!("\nLIVE_STATE_CANARY{}", "x".repeat(capacity as usize * 2));
    let messages = vec![
        Message::system("stable rules"),
        Message::user("preserve constraints"),
        Message::assistant("x".repeat(capacity as usize * 2), vec![]),
        Message::user("continue"),
    ];
    assert!(estimate_tokens(&messages) < capacity * COMPRESSION_TRIGGER_PERCENT / 100);
    let mut uncompressed = messages.clone();
    uncompressed.push(Message::host_instruction(&live_context));
    assert!(estimate_tokens(&uncompressed) > capacity);
    let mut result = agent
        .request_messages_accounted(&messages, &mut None, None, &live_context, &mut |_| Ok(()))
        .await
        .unwrap();
    result.push(Message::host_instruction(&live_context));
    assert!(estimate_tokens(&result) <= capacity);
    assert_pairs(&result);
    assert!(!compressor.requests.lock().unwrap().is_empty());
    assert!(
        compressor
            .requests
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .all(|message| { !message.content.contains("LIVE_STATE_CANARY") })
    );

    let calls = compressor.requests.lock().unwrap().len();
    let oversized = "x".repeat(capacity as usize * 4);
    assert!(matches!(
        agent
            .request_messages_accounted(&messages, &mut None, None, &oversized, &mut |_| Ok(()))
            .await,
        Err(AgentError::ContextCapacity { .. })
    ));
    assert_eq!(compressor.requests.lock().unwrap().len(), calls);
    std::fs::remove_dir_all(agent.tools.workspace()).unwrap();
}
