use super::*;
use std::sync::Mutex;

/// 够放下 `history()` 整段前缀的窗口。
///
/// 再小，前缀本身就超过一次摘要请求的预算，压缩会走分块路径——那条路另有
/// 专门的测试，混进来只会让「缓存命中几次」「记了几笔账」这些断言跟着跳。
pub(super) const WINDOW: u64 = 12_288;

mod recovery_tests;
mod usage_tests;

#[tokio::test]
async fn compression_preserves_unknown_effects_and_their_replay_guard() {
    let (agent, _) = agent(8192);
    let pending = ToolCall {
        id: "uncertain-write".into(),
        name: "run_command".into(),
        arguments: r#"{"command":"external-operation"}"#.into(),
    };
    let mut original = history();
    original.insert(2, Message::assistant("interrupted", vec![pending.clone()]));
    // Compress directly after interruption, before run_inner has repaired pairs.
    let mut manual = agent.compress_history(original).await.unwrap();
    assert_pairs(&manual);
    let guard = uncertain::UncertainCalls::recover(&mut manual);
    assert!(guard.needs_approval(&pending));
    assert!(manual.iter().any(|m| {
        m.tool_calls
            .iter()
            .any(|call| call.id == pending.id && call.arguments == pending.arguments)
    }));

    // Even the last-resort archive pass cannot remove recovery constraints.
    agent.archive_old_batches(&mut manual, 1).unwrap();
    assert_pairs(&manual);
    let guard = uncertain::UncertainCalls::recover(&mut manual);
    assert!(guard.needs_approval(&pending));
    assert!(uncertain::has_unknown_effect(&manual));
}

#[tokio::test]
async fn automatic_compression_keeps_unknown_call_parameters() {
    let (agent, _) = agent(8192);
    let pending = ToolCall {
        id: "unknown-command".into(),
        name: "run_command".into(),
        arguments: r#"{"command":"external-operation"}"#.into(),
    };
    let mut messages = history();
    messages.insert(2, Message::assistant("interrupted", vec![pending.clone()]));
    uncertain::UncertainCalls::recover(&mut messages);
    let mut compressed = agent.request_messages(&messages, &mut None).await.unwrap();
    assert_pairs(&compressed);
    assert!(uncertain::UncertainCalls::recover(&mut compressed).needs_approval(&pending));
}

struct CaptureProvider {
    requests: Mutex<Vec<Vec<Message>>>,
    /// 从第几条请求起挂住不回（模拟跑不完的一次压缩）。
    stall_after: std::sync::atomic::AtomicUsize,
}

impl CaptureProvider {
    fn stall_from(&self, call: usize) {
        self.stall_after
            .store(call, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait]
impl Provider for CaptureProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<crate::types::Completion, ProviderError> {
        let call = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(messages.to_vec());
            requests.len() - 1
        };
        if call >= self.stall_after.load(std::sync::atomic::Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        Ok(crate::types::Completion {
            reasoning: None,
            content: "OBJECTIVE: repair bug. USER CONSTRAINTS: preserve dirty files. COMPLETED: inspected. REMAINING: validate.".to_owned(),
            tool_calls: Vec::new(), finish_reason: Some("stop".to_owned()), usage: None,
        })
    }
}

fn agent(window: u64) -> (Agent, Arc<CaptureProvider>) {
    let workspace = std::env::temp_dir().join(format!("context-tests-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&workspace).unwrap();
    let provider = Arc::new(CaptureProvider {
        requests: Mutex::new(Vec::new()),
        stall_after: std::sync::atomic::AtomicUsize::new(usize::MAX),
    });
    let tools = ToolRegistry::new(workspace, crate::ApprovalMode::Strict)
        .unwrap()
        .with_allowed_tools(["read_file".to_owned()]);
    (
        Agent::new(
            provider.clone(),
            tools,
            AgentConfig {
                max_turns: 3,
                system_prompt: "system".to_owned(),
                context_window: window,
                token_budget: None,
            },
        ),
        provider,
    )
}

fn history() -> Vec<Message> {
    let mut messages = vec![
        Message::system("Never expose credentials"),
        Message::user("Fix login; preserve my dirty config; do not push"),
    ];
    for index in 0..20 {
        let call = ToolCall {
            id: format!("call-{index}"),
            name: "read_file".to_owned(),
            arguments: format!(r#"{{"path":"src/file-{index}.rs"}}"#),
        };
        messages.push(Message::assistant("inspect", vec![call.clone()]));
        messages.push(Message::tool(
            &call,
            format!("file-{index}: {}", "x".repeat(1_900)),
        ));
    }
    messages.push(Message::user("continue with the original constraints"));
    messages
}

fn assert_pairs(messages: &[Message]) {
    let mut pending = std::collections::HashSet::new();
    for message in messages {
        match message.role {
            Role::Assistant => {
                assert!(pending.is_empty());
                pending.extend(message.tool_calls.iter().map(|call| call.id.as_str()));
            }
            Role::Tool => {
                assert!(pending.remove(message.tool_call_id.as_deref().unwrap()));
            }
            _ => assert!(pending.is_empty()),
        }
    }
    assert!(pending.is_empty());
}

#[tokio::test]
async fn automatic_compression_preserves_constraints_and_tool_protocol() {
    let (agent, provider) = agent(WINDOW);
    let messages = history();
    let result = agent.request_messages(&messages, &mut None).await.unwrap();
    assert!(
        result
            .iter()
            .any(|message| message.content.contains("<context-summary"))
    );
    for protected in messages
        .iter()
        .filter(|message| matches!(message.role, Role::System | Role::User))
    {
        assert!(
            result
                .iter()
                .any(|message| message.role == protected.role
                    && message.content == protected.content)
        );
    }
    assert_pairs(&result);
    let requests = provider.requests.lock().unwrap();
    assert!(requests[0][0].content.contains("src/file-0.rs"));
    assert!(requests[0][0].content.contains("call-0"));
    assert!(requests[0][0].content.contains("UNVERIFIED CLAIMS"));
    assert_eq!(
        messages.len(),
        43,
        "automatic compression must not mutate durable history"
    );
}

#[tokio::test]
async fn oversized_instructions_are_rejected_instead_of_silently_cropped() {
    let (agent, provider) = agent(4_096);
    for role in [Role::System, Role::User] {
        let mut message = Message::user(format!(
            "HEAD{}MUST NOT DELETE CONFIG{}TAIL",
            "x".repeat(10_000),
            "y".repeat(10_000)
        ));
        message.role = role;
        assert!(matches!(
            agent.request_messages(&[message], &mut None).await,
            Err(AgentError::ContextCapacity { .. })
        ));
    }
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn oversized_tool_middle_is_retrievable_without_a_summary_request() {
    let (agent, provider) = agent(WINDOW);
    let call = ToolCall {
        id: "call".to_owned(),
        name: "read_file".to_owned(),
        arguments: "{}".to_owned(),
    };
    let original = format!(
        "{}IMPORTANT-MIDDLE{}",
        "h".repeat(20_000),
        "t".repeat(20_000)
    );
    let messages = vec![
        Message::system("system"),
        Message::user("inspect"),
        Message::assistant("", vec![call.clone()]),
        Message::tool(&call, &original),
    ];
    let result = agent.request_messages(&messages, &mut None).await.unwrap();
    assert!(provider.requests.lock().unwrap().is_empty());
    let id = agent.tools.archive_output(&original).unwrap();
    assert!(result[3].content.contains(&id));
    let page = agent
        .tools
        .execute(&ToolCall {
            id: "page".to_owned(),
            name: "read_tool_output".to_owned(),
            arguments: serde_json::json!({"id":id,"offset":20_000,"limit":16}).to_string(),
        })
        .await
        .unwrap();
    assert!(page.ends_with("IMPORTANT-MIDDLE"));
    assert_pairs(&result);
}

#[tokio::test]
async fn manual_compression_keeps_user_instructions_and_whole_tool_batches() {
    let (agent, _) = agent(WINDOW);
    let original = history();
    let compressed = agent.compress_history(original.clone()).await.unwrap();
    assert!(compressed.len() < original.len());
    assert_pairs(&compressed);
    assert!(
        compressed
            .iter()
            .any(|message| message.content == original[1].content)
    );
    assert_eq!(
        compressed.last().unwrap().content,
        original.last().unwrap().content
    );
}

#[test]
fn estimates_include_tool_arguments_text_attachments_and_non_ascii() {
    let plain = Message::assistant("", Vec::new());
    let mut tool = plain.clone();
    tool.tool_calls.push(ToolCall {
        id: "x".to_owned(),
        name: "create_file".to_owned(),
        arguments: "x".repeat(4_000),
    });
    assert!(estimate_tokens(&[tool]) >= estimate_tokens(&[plain]) + 1_000);
    let attached = Message::user_with_attachments(
        "",
        vec![MessageAttachment::Text {
            name: "paste".to_owned(),
            content: "约束".repeat(1_000),
        }],
    );
    assert!(estimate_tokens(&[attached]) >= 4_000);
    let definition = ToolDefinition {
        name: "large".to_owned(),
        description: "x".repeat(8_000),
        parameters: serde_json::json!({}),
    };
    assert!(message_capacity(1_000, &[definition]).is_err());
}

#[tokio::test]
async fn compression_starts_at_the_reserved_capacity_watermark_even_for_short_histories() {
    let (agent, provider) = agent(WINDOW);
    let capacity =
        message_capacity(agent.config.context_window, &agent.tools.definitions()).unwrap();
    let payload_tokens = capacity * COMPRESSION_TRIGGER_PERCENT / 100;
    let messages = vec![
        Message::system("system"),
        Message::user("preserve constraints"),
        Message::assistant("x".repeat(payload_tokens as usize * 4), Vec::new()),
        Message::user("continue"),
    ];
    agent.request_messages(&messages, &mut None).await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    let (agent, provider) = self::agent(WINDOW);
    agent
        .request_messages(
            &[Message::system("system"), Message::user("short question")],
            &mut None,
        )
        .await
        .unwrap();
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn urgent_recent_batches_are_archived_as_groups() {
    let (agent, _) = agent(4_096);
    let mut messages = history();
    let removed = agent.archive_old_batches(&mut messages, 600).unwrap();
    assert!(removed > 0);
    assert_pairs(&messages);
    assert!(
        messages
            .iter()
            .any(|message| message.content.contains("preserve my dirty config"))
    );
    assert_eq!(
        messages.last().unwrap().content,
        "continue with the original constraints"
    );
}

/// 压缩会丢掉带思维链的工具批次，留下来的尾部又常常没吐过思维链。
/// 摘要自己得留下回传标记，否则下一轮 Responses 请求不再补 `reasoning_text`。
#[tokio::test]
async fn compression_keeps_reasoning_replay_marker_after_real_reasoning_is_dropped() {
    let (agent, _) = agent(WINDOW);
    let mut messages = history();
    let reasoned_until = messages.len().saturating_sub(8);
    for message in &mut messages[..reasoned_until] {
        if message.role == Role::Assistant {
            message.reasoning = Some("先看这个文件".into());
        }
    }
    let mut cache = None;
    let result = agent.request_messages(&messages, &mut cache).await.unwrap();
    assert_replay_marker_is_only_on_the_summary(&result);

    push_batch(&mut messages, 100);
    let again = agent.request_messages(&messages, &mut cache).await.unwrap();
    assert_replay_marker_is_only_on_the_summary(&again);
}

/// 从没出现过思维链的会话不能凭空多出这个标记，否则严格校验的上游会看到未知字段。
#[tokio::test]
async fn compression_does_not_invent_a_reasoning_replay_marker() {
    let (agent, _) = agent(WINDOW);
    let result = agent.request_messages(&history(), &mut None).await.unwrap();
    assert!(
        result
            .iter()
            .any(|message| message.content.contains("<context-summary")),
        "the fixture must actually compress"
    );
    assert!(result.iter().all(|message| message.reasoning.is_none()));
}

#[tokio::test]
async fn archived_batch_keeps_reasoning_replay_marker_only_if_that_batch_had_reasoning() {
    let (agent, _) = agent(4_096);
    let reasoned = ToolCall {
        id: "reasoned".into(),
        name: "read_file".into(),
        arguments: "{}".into(),
    };
    let plain = ToolCall {
        id: "plain".into(),
        name: "read_file".into(),
        arguments: "{}".into(),
    };
    let mut messages = vec![
        Message::user("go"),
        Message::assistant("", vec![reasoned.clone()]).with_reasoning(Some("先看".into())),
        Message::tool(&reasoned, "r".repeat(8_000)),
        Message::assistant("", vec![plain.clone()]),
        Message::tool(&plain, "p".repeat(8_000)),
        Message::user("tail stays"),
    ];
    let removed = agent.archive_old_batches(&mut messages, 600).unwrap();
    assert!(removed > 0);
    assert_eq!(messages[1].reasoning.as_deref(), Some(""));
    assert!(messages[1].content.contains("Earlier tool batch"));
    assert!(messages[2].reasoning.is_none());
    assert!(messages[2].content.contains("Earlier tool batch"));
    assert_eq!(messages.last().unwrap().content, "tail stays");
    assert_pairs(&messages);
}

fn assert_replay_marker_is_only_on_the_summary(messages: &[Message]) {
    let summaries: Vec<_> = messages
        .iter()
        .filter(|message| message.content.contains("<context-summary"))
        .collect();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].reasoning.as_deref(), Some(""));
    assert!(
        messages.iter().all(|message| {
            message.content.contains("<context-summary") || message.reasoning.is_none()
        }),
        "kept messages must not grow a reasoning field of their own"
    );
}

#[tokio::test]
async fn hosted_compressor_receives_structured_transcript_without_duplicate_prompt() {
    let (mut agent, _) = agent(WINDOW);
    let compressor = Arc::new(CaptureProvider {
        requests: Mutex::new(Vec::new()),
        stall_after: std::sync::atomic::AtomicUsize::new(usize::MAX),
    });
    agent = agent.with_compressor(compressor.clone(), true);
    agent.request_messages(&history(), &mut None).await.unwrap();
    let requests = compressor.requests.lock().unwrap();
    assert!(!requests[0][0].content.contains("Summarize this older"));
    let transcript: Vec<Message> = serde_json::from_str(&requests[0][0].content).unwrap();
    assert!(
        transcript
            .iter()
            .any(|message| !message.tool_calls.is_empty())
    );
}

fn push_batch(messages: &mut Vec<Message>, index: usize) {
    let call = ToolCall {
        id: format!("step-{index}"),
        name: "read_file".to_owned(),
        arguments: format!(r#"{{"path":"src/step-{index}.rs"}}"#),
    };
    messages.push(Message::assistant("inspect", vec![call.clone()]));
    messages.push(Message::tool(
        &call,
        format!("step-{index}: {}", "y".repeat(1_900)),
    ));
}

#[tokio::test]
async fn consecutive_steps_above_the_watermark_summarize_incrementally() {
    const STEPS: usize = 30;
    let (agent, provider) = agent(WINDOW);
    let capacity =
        message_capacity(agent.config.context_window, &agent.tools.definitions()).unwrap();
    let mut messages = history();
    let mut cache = None;
    for step in 0..STEPS {
        push_batch(&mut messages, step);
        assert!(
            estimate_tokens(&messages) >= capacity * COMPRESSION_TRIGGER_PERCENT / 100,
            "every step must be above the raw watermark"
        );
        let result = agent.request_messages(&messages, &mut cache).await.unwrap();
        assert_pairs(&result);
        assert!(estimate_tokens(&result) <= capacity);
        assert!(
            result
                .iter()
                .any(|m| m.content.contains("<context-summary"))
        );
    }
    let requests = provider.requests.lock().unwrap();
    // The old split-keyed cache re-summarized the whole prefix on every step.
    assert!(
        requests.len() <= 1 + STEPS / 4,
        "{} summarizer calls for {STEPS} steps",
        requests.len()
    );
    assert!(
        requests.len() >= 2,
        "growth must eventually refresh the summary"
    );
    assert!(requests[0][0].content.contains("src/file-0.rs"));
    for incremental in &requests[1..] {
        let source = &incremental[0].content;
        assert!(
            source.contains("<context-summary"),
            "must carry the prior summary"
        );
        assert!(
            !source.contains("src/file-0.rs"),
            "must not resend the summarized prefix"
        );
        assert!(source.len() < requests[0][0].content.len());
    }
}

#[tokio::test]
async fn rewritten_prefix_invalidates_the_request_summary_cache() {
    let (agent, provider) = agent(WINDOW);
    let mut messages = history();
    let mut cache = None;
    agent.request_messages(&messages, &mut cache).await.unwrap();
    messages[3].content = "rewritten tool result".to_owned();
    agent.request_messages(&messages, &mut cache).await.unwrap();
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1][0].content.contains("src/file-0.rs"));
}

/// system 提示（全局规则、项目规则）改了，旧摘要必须作废。
///
/// 摘要会概括 system 的内容，而且现在跨运行复用：规则收紧或者删掉一条授权之后
/// 还拿旧摘要，等于让已经删掉的指令继续随每次请求发出去。以前 system 不参与
/// 失效判定，只能靠改前缀或者手动 `/compress` 才能甩掉它。
#[tokio::test]
async fn rewritten_system_prompt_invalidates_the_request_summary_cache() {
    let (agent, provider) = agent(WINDOW);
    let mut messages = history();
    let mut cache = None;
    agent.request_messages(&messages, &mut cache).await.unwrap();
    messages[0].content.push_str("\nrules refreshed");
    agent.request_messages(&messages, &mut cache).await.unwrap();
    let requests = provider.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "a changed system prompt must re-summarize"
    );
    assert!(requests[1][0].content.contains("src/file-0.rs"));
}

/// 会话落盘再读回来的摘要，下一次 run 必须直接生效。
///
/// 自动压缩不改写 durable 历史，所以没有这一层，同一段前缀每来一条消息就要
/// 从头再摘一遍：历史到 150 万 token 时，那是一轮七八分钟的串行模型调用。
#[tokio::test]
async fn persisted_summary_is_reused_by_the_next_run() {
    let (agent, provider) = agent(WINDOW);
    let messages = history();
    let mut cache = None;
    agent.request_messages(&messages, &mut cache).await.unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    let persisted = cache
        .as_ref()
        .expect("compression must leave a summary behind")
        .persist();

    // 下一次 run：从会话文件里读回同一份摘要，历史没动就一次调用都不发。
    let mut next_run = Some(SummaryCache::restore(persisted.clone()));
    let reused = agent
        .request_messages(&messages, &mut next_run)
        .await
        .unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert!(
        reused
            .iter()
            .any(|message| message.content.contains("<context-summary"))
    );

    // 历史往前长到投影重新越过水位线，只摘新长出来的那一段：请求里带旧摘要，
    // 不重发被摘过的老前缀。
    let mut grown = messages;
    for index in 100..140 {
        push_batch(&mut grown, index);
    }
    agent.request_messages(&grown, &mut next_run).await.unwrap();
    let requests = provider.requests.lock().unwrap();
    assert!(
        requests.len() >= 2,
        "growth past the watermark must refresh the summary"
    );
    let refreshes = &requests[1..];
    assert!(
        refreshes
            .iter()
            .any(|request| request[0].content.contains("<context-summary")),
        "the refresh must build on the persisted summary"
    );
    assert!(
        refreshes
            .iter()
            .all(|request| !request[0].content.contains("src/file-0.rs")),
        "the summarized prefix must not be sent again"
    );
}

/// 重复的 host instruction 不能作废已落盘的摘要。
///
/// 指纹锚在 durable 历史的前缀上，而不是分页投影：`page_tool_outputs` 对重复的
/// host instruction 只留最后一条（守护进程每一轮唤醒追加的是同一个提示），投影
/// 下标会整体前移。指纹算在投影上的话，每次唤醒都要把整份摘要作废、重新按块摘
/// 一遍——正好把这个缓存要省的那笔账又花了回去。
#[tokio::test]
async fn repeated_host_instruction_keeps_the_persisted_summary() {
    const WAKE: &str = "Runtime events arrived while you were away. Review them and continue.";
    let (agent, provider) = agent(WINDOW);
    let mut messages = history();
    messages.push(Message::host_instruction(WAKE));
    let mut cache = None;
    agent.request_messages(&messages, &mut cache).await.unwrap();
    // 本轮继续干活，把 wake prompt 推到 6 条尾巴之外，再触发一次增量压缩。
    for index in 0..16 {
        push_batch(&mut messages, 200 + index);
    }
    agent.request_messages(&messages, &mut cache).await.unwrap();
    let persisted = cache
        .as_ref()
        .expect("compression must leave a summary behind")
        .persist();
    assert!(
        persisted.through > 44,
        "wake prompt must fall inside the summarized prefix (through={})",
        persisted.through
    );
    let calls_before = provider.requests.lock().unwrap().len();

    // 下一次 wake：同一条 host instruction 再追加一次，分页投影少一条。
    messages.push(Message::host_instruction(WAKE));
    let capacity =
        message_capacity(agent.config.context_window, &agent.tools.definitions()).unwrap();
    let projection = agent.page_tool_outputs(&messages, capacity).unwrap();
    assert_eq!(
        projection.messages.len(),
        messages.len() - 1,
        "the earlier wake prompt must be deduplicated away"
    );
    assert_eq!(
        projection.projected_index(persisted.through),
        persisted.through - 1,
        "the projection index must follow the removed duplicate"
    );
    let mut restored = Some(SummaryCache::restore(persisted));
    agent
        .request_messages(&messages, &mut restored)
        .await
        .unwrap();
    let calls_after = provider.requests.lock().unwrap().len();
    assert_eq!(
        calls_after, calls_before,
        "persisted summary must survive host-instruction dedup"
    );
}

/// 分页上限跟着 capacity 变，但摘要的锚点不能跟着动。
///
/// 换模型、挂上或断开 MCP 服务都会改 `message_capacity`，工具输出的缩写长度
/// 随之改变，投影内容也就跟着变。指纹只算 durable 前缀，所以这些变化不该把
/// 已经付过钱的摘要作废——否则「换个模型继续聊」先卡十分钟。
#[tokio::test]
async fn a_repaged_projection_keeps_the_persisted_summary_valid() {
    let (agent, _) = agent(WINDOW);
    let messages = history();
    let mut cache = None;
    agent.request_messages(&messages, &mut cache).await.unwrap();
    let persisted = cache
        .expect("compression must leave a summary behind")
        .persist();
    let narrow = agent.page_tool_outputs(&messages, 256).unwrap();
    let wide = agent.page_tool_outputs(&messages, 4_096).unwrap();
    let size = |paged: &PagedHistory| -> usize {
        paged
            .messages
            .iter()
            .map(|message| message.content.len())
            .sum()
    };
    assert_ne!(
        size(&narrow),
        size(&wide),
        "the paging limit must actually rewrite tool output"
    );
    assert!(
        SummaryCache::restore(persisted).matches(&messages),
        "repaging must not invalidate a summary anchored on durable history"
    );
}

/// 存下来的摘要只对它的那段前缀有效：历史被换掉、或者索引越界，只能作废重摘。
#[tokio::test]
async fn stale_persisted_summary_is_discarded() {
    let (agent, provider) = agent(WINDOW);
    let messages = history();
    let mut cache = None;
    agent.request_messages(&messages, &mut cache).await.unwrap();
    let valid = cache.as_ref().unwrap().persist();

    // 索引落在历史之外：不能 panic，也不能当成命中。
    let mut out_of_range = valid.clone();
    out_of_range.through = messages.len() + 8;
    let mut restored = Some(SummaryCache::restore(out_of_range));
    agent
        .request_messages(&messages, &mut restored)
        .await
        .unwrap();

    // 指纹对不上：宁可整段重摘，也不能把别的历史的摘要喂给模型。
    let mut rewritten = messages;
    rewritten[5].content = "different evidence".to_owned();
    let mut restored = Some(SummaryCache::restore(valid));
    agent
        .request_messages(&rewritten, &mut restored)
        .await
        .unwrap();
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
}

/// 真实故障的回归：会话历史涨到 94 万 token 后，压缩把整段前缀塞进一条 user
/// 消息，被 provider 以 400 顶回来。压缩是每轮必经之路，于是会话永久锁死。
/// 送出去的每一条摘要请求都必须自己封顶，与 durable 历史有多长无关。
#[tokio::test]
async fn summary_requests_stay_within_budget_however_long_the_history_grows() {
    let (agent, provider) = agent(WINDOW);
    let budget = agent.summary_input_budget();
    let mut messages = history();
    for index in 20..80 {
        let call = ToolCall {
            id: format!("call-{index}"),
            name: "read_file".to_owned(),
            arguments: format!(r#"{{"path":"src/file-{index}.rs"}}"#),
        };
        messages.push(Message::assistant("inspect", vec![call.clone()]));
        messages.push(Message::tool(
            &call,
            format!("file-{index}: {}", "x".repeat(1_900)),
        ));
    }
    messages.push(Message::user("continue"));
    assert!(
        estimate_tokens(&messages) > budget * 3,
        "the fixture must be far larger than one summary request"
    );

    agent.request_messages(&messages, &mut None).await.unwrap();

    let requests = provider.requests.lock().unwrap();
    assert!(
        requests.len() > 1,
        "an oversized prefix must be summarized in chunks, not in one shot"
    );
    for request in requests.iter() {
        let estimated = estimate_tokens(request);
        assert!(
            estimated <= budget + SUMMARY_OVERHEAD_TOKENS,
            "summary request of {estimated} tokens exceeds the {budget} budget"
        );
    }
}

/// 主模型在云端、压缩器在本机时，摘要原料按压缩器自己的窗口封顶。按会话窗口
/// 切出来的块本地端点收不下，整轮压缩会一直失败，而压缩是每轮必经之路。
#[tokio::test]
async fn a_bound_compressor_window_caps_every_summary_request() {
    let (local, provider) = agent(WINDOW);
    let local = local.with_compressor_window(WINDOW / 4);
    let budget = local.summary_input_budget();
    let (session, _) = agent(WINDOW);
    assert!(
        budget < session.summary_input_budget(),
        "绑定更小的压缩窗口必须真的缩小摘要预算"
    );

    let mut messages = history();
    for index in 20..80 {
        let call = ToolCall {
            id: format!("call-{index}"),
            name: "read_file".to_owned(),
            arguments: format!(r#"{{"path":"src/file-{index}.rs"}}"#),
        };
        messages.push(Message::assistant("inspect", vec![call.clone()]));
        messages.push(Message::tool(
            &call,
            format!("file-{index}: {}", "x".repeat(1_900)),
        ));
    }
    messages.push(Message::user("continue"));

    local.request_messages(&messages, &mut None).await.unwrap();

    let requests = provider.requests.lock().unwrap();
    assert!(
        requests.len() > 1,
        "a history far larger than the compressor window must be chunked"
    );
    for request in requests.iter() {
        let estimated = estimate_tokens(request);
        assert!(
            estimated <= budget + SUMMARY_OVERHEAD_TOKENS,
            "summary request of {estimated} tokens exceeds the compressor budget {budget}"
        );
    }
}

/// 单条消息自己就超预算时不能原样放行，否则分块只是把超限往下挪了一层。
#[tokio::test]
async fn one_oversized_message_is_truncated_instead_of_being_sent_whole() {
    let (agent, provider) = agent(WINDOW);
    let budget = agent.summary_input_budget();
    let mut messages = history();
    // 工具结果在摘要之前已被 `page_tool_outputs` 归档成摘录，所以这里要的是一条
    // 没人替它兜底、又确实能进摘要的 assistant 长消息。
    messages.insert(
        2,
        Message::assistant(
            format!("recap: {}", "y".repeat(budget as usize * 8)),
            Vec::new(),
        ),
    );

    agent.request_messages(&messages, &mut None).await.unwrap();

    let requests = provider.requests.lock().unwrap();
    assert!(!requests.is_empty());
    for request in requests.iter() {
        let estimated = estimate_tokens(request);
        assert!(
            estimated <= budget + SUMMARY_OVERHEAD_TOKENS,
            "summary request of {estimated} tokens exceeds the {budget} budget"
        );
    }
    assert!(
        requests
            .iter()
            .any(|request| request[0].content.contains("truncated for summarization")),
        "the oversized paste must be marked as truncated, not silently dropped"
    );
}

/// 超时/取消不能把已经落盘的有效摘要抹成 None。
///
/// `request_messages_accounted` 先把缓存槽位 `take()` 走再 await 重摘；预算耗尽
/// 时整个 future 被丢弃，槽位回不来。以前这里照写 `None`，等于把上一轮付过钱
/// 的摘要从会话里删掉：下一轮没有基线，只能把整段 durable 前缀重新分块摘要，
/// 预算型会话于是每轮都停在 BudgetLimited，永远出不来。
#[tokio::test]
async fn a_cancelled_refresh_keeps_the_previous_summary() {
    let (agent, provider) = agent(WINDOW);
    let mut messages = vec![Message::system("system")];
    messages.extend(history().into_iter().skip(1));
    let mut cache = None;
    agent.request_messages(&messages, &mut cache).await.unwrap();
    let persisted = cache
        .expect("compression must leave a summary behind")
        .persist();

    // 历史长到投影再次越过水位线：这一轮本要做一次增量重摘。
    let mut grown = messages;
    for index in 100..140 {
        push_batch(&mut grown, index);
    }
    let calls = provider.requests.lock().unwrap().len();
    provider.stall_from(calls);
    let agent = agent.with_time_budget(std::time::Duration::from_millis(400));
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        agent.run_checkpointed_with_summary(
            grown,
            Message::user("continue"),
            None,
            Some(persisted.clone()),
        ),
    )
    .await
    .expect("the time budget must end the run, not hang it")
    .expect("a spent budget is an outcome, not an error");
    assert_eq!(
        outcome.stop_reason,
        crate::agent::AgentStopReason::BudgetLimited
    );
    assert!(
        provider.requests.lock().unwrap().len() > calls,
        "the refresh must have started before the budget ran out"
    );
    assert_eq!(
        outcome.context_summary,
        Some(persisted),
        "a cancelled refresh must hand the previous summary back untouched"
    );
}

#[test]
fn clamping_keeps_both_ends_and_respects_the_token_budget() {
    let text = format!("HEAD{}TAIL", "中".repeat(20_000));
    let clamped = clamp_text(&text, 1_024);
    assert!(text_tokens(&clamped) <= 1_024 + text_tokens("\n[…truncated for summarization…]\n"));
    assert!(clamped.starts_with("HEAD"));
    assert!(clamped.ends_with("TAIL"));
    assert_eq!(clamp_text("short", 1_024), "short");
}
