use super::*;

const SAVED_SUMMARY: &str = "SAVED_STATE_ONLY: inspected old prefix; preserve constraints";

fn contains_message(messages: &[Message], expected: &Message) -> bool {
    let expected = serde_json::to_value(expected).unwrap();
    messages
        .iter()
        .any(|message| serde_json::to_value(message).unwrap() == expected)
}

fn stable_system(agent: &Agent) -> Message {
    let rules = crate::project_rules::ProjectRules::new(agent.tools.workspace()).unwrap();
    agent.stable_system_message(&rules).unwrap()
}

fn saved(
    agent: &Agent,
    messages: &[Message],
    through: usize,
) -> crate::context_summary::ContextSummary {
    let mut stable = messages.to_vec();
    stable[0] = stable_system(agent);
    stable.retain(|message| !crate::checkpoint::is_federation_notice(message));
    SummaryCache::new(
        messages,
        through,
        SAVED_SUMMARY.into(),
        &system_fingerprint(&stable),
    )
    .persist()
}

fn capacity(agent: &Agent) -> u64 {
    message_capacity(
        agent.effective_context_window(),
        &agent.request_definitions(),
        agent.completion_reserve(),
    )
    .unwrap()
}

async fn compress(
    agent: &Agent,
    messages: Vec<Message>,
    summary: crate::context_summary::ContextSummary,
) -> Result<Vec<Message>, AgentError> {
    agent
        .compress_history_with_summary_recorded(messages, Some(summary), &mut |_| Ok(()))
        .await
}

#[tokio::test]
async fn manual_tail_fallback_sends_first_summary_and_uncovered_batch_only() {
    let (agent, provider) = agent(WINDOW);
    let agent = agent.with_compressor(provider.clone(), true);
    let mut messages = vec![
        Message::system("Never expose credentials"),
        Message::user("Keep my dirty files; do not push"),
    ];
    for index in 0..8 {
        messages.push(Message::assistant(format!("RAW_PREFIX_{index}"), vec![]));
    }
    let call = ToolCall {
        id: "tail-call".into(),
        name: "read_file".into(),
        arguments: r#"{"path":"tail.rs"}"#.into(),
    };
    let target = capacity(&agent) * COMPRESSION_TRIGGER_PERCENT / 100;
    messages.push(
        Message::assistant("RAW_TAIL_ONLY", vec![call.clone()])
            .with_reasoning(Some("r".repeat(target as usize * 4))),
    );
    messages.push(Message::tool(&call, "tail result"));
    messages.push(Message::user("continue"));
    let result = agent.compress_history(messages.clone()).await.unwrap();
    let requests = provider.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "fixture must exercise the single tail fallback"
    );
    let first: Vec<Message> = serde_json::from_str(&requests[0][0].content).unwrap();
    let second: Vec<Message> = serde_json::from_str(&requests[1][0].content).unwrap();
    assert!(
        first
            .iter()
            .any(|message| message.content == "RAW_PREFIX_0")
    );
    assert!(
        !second
            .iter()
            .any(|message| message.content == "RAW_PREFIX_0")
    );
    assert_eq!(
        second
            .iter()
            .filter(|message| message.content.contains("<context-summary"))
            .count(),
        1
    );
    assert!(
        second
            .iter()
            .any(|message| message.content == "RAW_TAIL_ONLY")
    );
    assert_pairs(&second);
    assert_pairs(&result);
    assert!(estimate_tokens(&result) <= capacity(&agent));
    assert!(result.iter().any(Message::requires_reasoning_replay));
    for protected in messages
        .iter()
        .filter(|message| matches!(message.role, Role::System | Role::User))
    {
        assert!(contains_message(&result, protected));
    }
}

#[tokio::test]
async fn manual_compression_materializes_the_actual_automatic_summary_without_requests() {
    let (mut agent, provider) = agent(WINDOW);
    // Exercise the scheduling contract in the shared stable-system builder too.
    agent.tools = ToolRegistry::new(agent.tools.workspace(), crate::ApprovalMode::Strict)
        .unwrap()
        .with_allowed_tools(["read_file".to_owned(), "update_plan".to_owned()]);
    let outcome = agent.run_with_history(history(), "continue").await.unwrap();
    let summary = outcome
        .context_summary
        .expect("automatic compression must persist a summary");
    let calls = provider.requests.lock().unwrap().len();
    let result = compress(&agent, outcome.messages.clone(), summary.clone())
        .await
        .unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), calls);
    assert!(result.len() < outcome.messages.len());
    assert_eq!(
        result
            .iter()
            .filter(|message| message.content.contains("<context-summary"))
            .count(),
        1
    );
    assert!(
        result
            .iter()
            .any(|message| message.content.contains(&summary.summary))
    );
    assert_pairs(&result);
}

#[tokio::test]
async fn manual_reuse_validates_durable_indices_before_notice_removal_and_host_deduplication() {
    let (agent, provider) = agent(WINDOW);
    let mut messages = history();
    messages.insert(2, Message::host_instruction("same steering"));
    let mut notice = Message::system("display only");
    notice.source = Some(MessageSource::FederationNotice);
    messages.insert(3, notice.clone());
    messages.insert(4, Message::host_instruction("same steering"));
    messages[0] = stable_system(&agent);
    let mut cache = None;
    agent.request_messages(&messages, &mut cache).await.unwrap();
    let cached = cache.unwrap().persist();
    let calls = provider.requests.lock().unwrap().len();
    let result = compress(&agent, messages.clone(), cached.clone())
        .await
        .unwrap();
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        calls,
        "no raw prefix may be resent"
    );
    assert!(contains_message(&result, &notice));
    assert_eq!(
        result
            .iter()
            .filter(|message| message.content == "same steering")
            .count(),
        1
    );
    assert_eq!(
        result
            .iter()
            .filter(|message| message.content.contains(&cached.summary))
            .count(),
        1
    );
    let mut protocol = result;
    protocol.retain(|message| !crate::checkpoint::is_federation_notice(message));
    assert_pairs(&protocol);
}

#[tokio::test]
async fn manual_reuse_ignores_live_goal_and_budget_pins_but_preserves_constraints() {
    let (agent, provider) = agent(WINDOW);
    let goal = Arc::new(crate::GoalContinuation::new());
    let agent = agent.with_goal_continuation(goal.clone());
    let mut messages = history();
    let through = batch_boundary(&messages, messages.len() - KEEP_RECENT_MESSAGES);
    let cached = saved(&agent, &messages, through);
    messages[0]
        .content
        .push_str("\n<active-goal>old goal</active-goal>\n[execution-budget] old budget");
    goal.activate("a different live goal", crate::GoalBudget::default());
    let result = compress(&agent, messages.clone(), cached).await.unwrap();
    assert!(provider.requests.lock().unwrap().is_empty());
    assert!(contains_message(&result, &messages[0]));
    assert!(contains_message(&result, &messages[1]));
}

#[tokio::test]
async fn manual_incremental_summary_sends_saved_state_once_and_only_new_material() {
    let (agent, provider) = agent(WINDOW);
    let agent = agent.with_compressor(provider.clone(), true);
    let mut messages = history();
    let through = messages.len();
    let cached = saved(&agent, &messages, through);
    for index in 0..10 {
        messages.push(Message::assistant(
            format!("NEW_MATERIAL_{index}: {}", "n".repeat(4_000)),
            vec![],
        ));
    }
    let result = compress(&agent, messages, cached).await.unwrap();
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let source: Vec<Message> = serde_json::from_str(&requests[0][0].content).unwrap();
    assert_eq!(
        source
            .iter()
            .filter(|message| message.content.contains(SAVED_SUMMARY))
            .count(),
        1
    );
    assert!(
        source
            .iter()
            .any(|message| message.content.contains("NEW_MATERIAL_0"))
    );
    assert!(!requests[0][0].content.contains("file-0"));
    assert_pairs(&result);
    assert!(estimate_tokens(&result) <= capacity(&agent));
}

#[tokio::test]
async fn manual_reuse_does_not_resummarize_when_only_protected_material_is_new() {
    let (agent, provider) = agent(WINDOW);
    let mut messages = history();
    let cached = saved(&agent, &messages, messages.len());
    let target = capacity(&agent) * COMPRESSION_TRIGGER_PERCENT / 100;
    let constraint = Message::user("u".repeat(target as usize * 4));
    messages.push(constraint.clone());
    let result = compress(&agent, messages.clone(), cached.clone())
        .await
        .unwrap();
    assert!(contains_message(&result, &constraint));
    assert!(provider.requests.lock().unwrap().is_empty());
    let pending = ToolCall {
        id: "new-uncertain-call".into(),
        name: "run_command".into(),
        arguments: r#"{"command":"external-operation"}"#.into(),
    };
    messages.push(Message::assistant("interrupted", vec![pending.clone()]));
    let mut result = compress(&agent, messages.clone(), cached.clone())
        .await
        .unwrap();
    assert!(uncertain::UncertainCalls::recover(&mut result).needs_approval(&pending));
    assert!(
        provider.requests.lock().unwrap().is_empty(),
        "retained UNKNOWN batches are not new replaceable material"
    );
    messages.push(Message::user("u".repeat(capacity(&agent) as usize * 4)));
    assert!(matches!(
        compress(&agent, messages, cached).await,
        Err(AgentError::ContextCapacity { .. })
    ));
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn manual_summary_invalidates_after_durable_history_or_stable_rules_change() {
    for change in ["history", "config", "rules"] {
        let (mut agent, provider) = agent(WINDOW);
        let mut messages = history();
        let through = batch_boundary(&messages, messages.len() - KEEP_RECENT_MESSAGES);
        let cached = saved(&agent, &messages, through);
        compress(&agent, messages.clone(), cached.clone())
            .await
            .unwrap();
        assert!(
            provider.requests.lock().unwrap().is_empty(),
            "the unchanged cache must be valid before each mutation"
        );
        match change {
            "history" => messages[3].content.push_str(" REAL_HISTORY_EDIT"),
            "config" => agent.config.system_prompt.push_str(" new stable rule"),
            "rules" => std::fs::write(
                agent.tools.workspace().join("AGENTS.md"),
                "new project rule",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        compress(&agent, messages, cached).await.unwrap();
        let requests = provider.requests.lock().unwrap();
        assert!(
            !requests.is_empty(),
            "{change} must invalidate the saved state"
        );
        assert!(
            requests
                .iter()
                .flatten()
                .any(|message| message.content.contains("file-0"))
        );
        assert!(
            requests
                .iter()
                .flatten()
                .all(|message| !message.content.contains(SAVED_SUMMARY))
        );
    }
}

#[tokio::test]
async fn manual_reuse_remaps_the_anchor_after_unknown_receipts_are_inserted() {
    let (agent, provider) = agent(WINDOW);
    let pending = ToolCall {
        id: "covered-interrupted-call".into(),
        name: "run_command".into(),
        arguments: r#"{"command":"external-operation"}"#.into(),
    };
    let mut messages = history();
    messages.insert(2, Message::assistant("interrupted", vec![pending.clone()]));
    let through = batch_boundary(&messages, messages.len() - KEEP_RECENT_MESSAGES);
    let cached = saved(&agent, &messages, through);
    let mut notice = Message::system("inside a retained batch");
    notice.source = Some(MessageSource::FederationNotice);
    // A new display-only notice outside the cached prefix must not manufacture
    // an UNKNOWN result or move the cached anchor over the following real batch.
    let last_assistant = messages
        .iter()
        .rposition(|message| message.role == Role::Assistant)
        .unwrap();
    messages.insert(last_assistant + 1, notice.clone());
    let mut result = compress(&agent, messages, cached).await.unwrap();
    assert!(provider.requests.lock().unwrap().is_empty());
    assert!(contains_message(&result, &notice));
    result.retain(|message| !crate::checkpoint::is_federation_notice(message));
    assert_pairs(&result);
    assert!(uncertain::UncertainCalls::recover(&mut result).needs_approval(&pending));
    assert!(
        result
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("call-19")
                && message.content.contains("file-19"))
    );
    assert!(
        !result
            .iter()
            .any(|message| message.content.contains("file-0:"))
    );
    assert_eq!(
        result
            .iter()
            .filter(|message| message.content.contains(SAVED_SUMMARY))
            .count(),
        1
    );
}

#[tokio::test]
async fn manual_reuse_rejects_an_anchor_inside_a_tool_batch_even_with_notices() {
    for through in [3, 4] {
        let (agent, provider) = agent(WINDOW);
        let mut messages = history();
        let mut notice = Message::system("between the call and receipt");
        notice.source = Some(MessageSource::FederationNotice);
        messages.insert(3, notice);
        let cached = saved(&agent, &messages, through);
        compress(&agent, messages, cached).await.unwrap();
        let requests = provider.requests.lock().unwrap();
        assert!(
            !requests.is_empty(),
            "an incomplete batch anchor must be rejected"
        );
        assert!(
            requests
                .iter()
                .flatten()
                .any(|message| message.content.contains("file-0"))
        );
        assert!(
            requests
                .iter()
                .flatten()
                .all(|message| !message.content.contains(SAVED_SUMMARY))
        );
    }
}

#[tokio::test]
async fn manual_reuse_survives_a_changed_tool_output_paging_limit() {
    let (agent, provider) = agent(WINDOW);
    let mut messages = history();
    messages[3].content = format!("OVERSIZED_PREFIX_OUTPUT: {}", "p".repeat(30_000));
    let outcome = agent.run_with_history(messages, "continue").await.unwrap();
    let summary = outcome.context_summary.unwrap();
    assert!(summary.through > 3);
    let calls = provider.requests.lock().unwrap().len();
    let before = agent
        .page_tool_outputs(&outcome.messages, capacity(&agent), true)
        .unwrap();
    assert!(agent.tighten_context_window(Some(WINDOW / 2)));
    let after = agent
        .page_tool_outputs(&outcome.messages, capacity(&agent), true)
        .unwrap();
    assert_ne!(
        before.messages[3].content, after.messages[3].content,
        "the fixture must actually repage the covered output"
    );
    let result = compress(&agent, outcome.messages, summary).await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), calls);
    assert_pairs(&result);
    assert!(
        !result
            .iter()
            .any(|message| message.content.contains("OVERSIZED_PREFIX_OUTPUT"))
    );
}

#[tokio::test]
async fn manual_reuse_restores_scoped_rules_and_invalidates_after_they_change() {
    let (agent, provider) = agent(WINDOW);
    let nested = agent.tools.workspace().join("src");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("AGENTS.md"), "RESTORED_NESTED_RULE").unwrap();
    let outcome = agent.run_with_history(history(), "continue").await.unwrap();
    assert!(outcome.messages[0].content.contains("RESTORED_NESTED_RULE"));
    let summary = outcome.context_summary.unwrap();
    let calls = provider.requests.lock().unwrap().len();
    compress(&agent, outcome.messages.clone(), summary.clone())
        .await
        .unwrap();
    assert_eq!(
        provider.requests.lock().unwrap().len(),
        calls,
        "unchanged historical scope must reuse the automatic summary"
    );
    std::fs::write(nested.join("AGENTS.md"), "CHANGED_RESTORED_RULE").unwrap();
    compress(&agent, outcome.messages, summary).await.unwrap();
    assert!(
        provider.requests.lock().unwrap().len() > calls,
        "manual reuse must reject summaries made under different scoped rules"
    );
}
