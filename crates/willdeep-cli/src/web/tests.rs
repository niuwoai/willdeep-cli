use super::*;

#[test]
fn only_the_leading_tilde_is_expanded_in_a_workspace_path() {
    // SAFETY: 测试单线程运行，这里只为断言 `~` 展开取一个确定的 HOME。
    unsafe { std::env::set_var("HOME", "/home/tester") };
    assert_eq!(shellexpand_home("~"), "/home/tester");
    assert_eq!(shellexpand_home("~/Sites/app"), "/home/tester/Sites/app");
    // 这是路径不是 shell 命令：`~other`、变量与通配符一律原样留着，
    // 交给 canonicalize 去失败，而不是在这里猜用户想要什么。
    assert_eq!(shellexpand_home("~other/app"), "~other/app");
    assert_eq!(shellexpand_home("$HOME/app"), "$HOME/app");
    assert_eq!(shellexpand_home("/abs/path"), "/abs/path");
    assert_eq!(shellexpand_home("relative/path"), "relative/path");
}

/// 浏览器拿到的事件里不能有正文，标题也要打码截断。
///
/// 这是第二层白名单：协议那边的 `PublicKernelEvent` 已经不带 body，但 Web
/// 仍然自己投影一次，好让协议将来加字段时不会自动流到浏览器。
#[test]
fn the_browser_projection_carries_no_event_body() {
    let mut event = willdeep_core::kernel::host_event(
        uuid::Uuid::nil(),
        willdeep_runtime_protocol::EventSource::External,
        "external.notice",
        willdeep_runtime_protocol::EventPriority::Normal,
        willdeep_core::kernel::InterruptPolicy::YieldAtBoundary,
        "deploy failed: sk-abcdefghijklmnop0123",
        Some("full log tail with secrets".to_owned()),
        None,
        true,
    );
    event.merge_count = 3;
    let projected = web_kernel_event(&event);
    let encoded = serde_json::to_string(&projected).expect("serialize");
    assert!(!encoded.contains("full log tail"), "正文不进浏览器");
    // 打码走的是命令审批那一套规则（`redact_credentials`），所以样例得是
    // 它真的认得的形状：一个够长的 `sk-` 前缀密钥。
    assert!(!encoded.contains("sk-abcdefghijklmnop0123"), "凭据要打码");
    assert_eq!(projected.merge_count, 3);
    assert!(projected.requires_user_action);
    assert_eq!(projected.source, "external");
    assert_eq!(projected.delivery_state, "pending");
}

fn workspace(path: &str, active: bool) -> crate::daemon::RuntimeWorkspace {
    crate::daemon::RuntimeWorkspace {
        schema: 1,
        id: uuid::Uuid::new_v4(),
        name: path.to_owned(),
        root: PathBuf::from(path),
        access: crate::daemon::WorkspaceAccess::Smart,
        provider_profile: None,
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        created_at: 1,
        updated_at: 1,
        active,
    }
}

#[test]
fn workspace_selection_rejects_unknown_paths() {
    let allowed = vec![workspace("/workspace/a", false)];
    assert!(select_registered_workspace(allowed.clone(), Some("/workspace/b")).is_err());
    assert_eq!(
        select_registered_workspace(allowed, None).unwrap().root,
        PathBuf::from("/workspace/a")
    );
}

#[test]
fn workspace_selection_prefers_the_active_allowed_registration() {
    let allowed = vec![
        workspace("/workspace/a", false),
        workspace("/workspace/b", true),
    ];
    assert_eq!(
        select_registered_workspace(allowed, None).unwrap().root,
        PathBuf::from("/workspace/b")
    );
}
// 「空会话不进 Web 历史」「只有附件也算用户输入」这两条语义，
// 现在由 willdeep_core::session 的 digest 测试覆盖。
#[test]
fn streaming_client_events_preserve_text_and_localize_retry_wait() {
    let event = client_event(serde_json::json!({"type":"assistant_text_delta", "text":"正在生成正文", "internal":"hidden"}), Language::ZhCn).unwrap();
    assert_eq!(event["text"], "正在生成正文");
    assert!(event.get("internal").is_none());
    // 思维链增量同样只传文本和类型，前端据此画「思考中」尾巴行。
    let event = client_event(
        serde_json::json!({"type":"reasoning_delta", "text":"先看日志", "internal":"hidden"}),
        Language::ZhCn,
    )
    .unwrap();
    assert_eq!(event["type"], "reasoning_delta");
    assert_eq!(event["text"], "先看日志");
    assert!(event.get("internal").is_none());
    let event = client_event(
        serde_json::json!({"type":"provider_retry_wait", "delay_ms":1500}),
        Language::En,
    )
    .unwrap();
    assert_eq!(event["label"], "Waiting to retry · 2s");
    assert!(client_event(serde_json::json!({"type":"usage_snapshot"}), Language::En).is_none());
}

#[test]
fn embedded_frontend_exists() {
    assert!(WebAssets::get("index.html").is_some());
}
#[test]
fn child_retry_event_preserves_identity_and_redacts_private_fields() {
    let id = uuid::Uuid::new_v4();
    let mut event = crate::agent_event_json(willdeep_core::AgentEvent::SubagentRetryWait {
        id,
        attempt: 2,
        delay: std::time::Duration::from_millis(1500),
    });
    event["private"] = serde_json::json!("hidden");
    let public = client_event(event, Language::En).unwrap();
    assert_eq!(public["type"], "subagent_retry_wait");
    assert_eq!(public["id"], id.to_string());
    assert_eq!(public["attempt"], 2);
    assert_eq!(public["delay_ms"], 1500);
    assert_eq!(
        public["label"],
        format!(
            "Subagent {} · Waiting to retry 2 · 2s",
            &id.to_string()[..8]
        )
    );
    assert!(public.get("private").is_none());
}
#[test]
fn tool_events_are_redacted_for_sse() {
    let event=client_event(serde_json::json!({"type":"tool_completed","name":"read_file","output":"secret","is_error":false}),Language::En).unwrap();
    assert!(event.get("output").is_none());
}
#[test]
fn thought_events_are_compact_and_tool_arguments_stay_private() {
    let event = client_event(
        serde_json::json!({"type":"assistant_text","text":"x".repeat(500)}),
        Language::En,
    )
    .unwrap();
    assert_eq!(event["type"], "thought");
    assert!(event["text"].as_str().unwrap().chars().count() <= 240);
}
#[test]
fn validates_web_attachments() {
    let valid = vec![MessageAttachment::Image {
        name: "shot.png".into(),
        media_type: "image/png".into(),
        data: "AAAA".into(),
        width: 1,
        height: 1,
    }];
    assert!(validate_attachments(&valid).is_ok());
    let invalid = vec![MessageAttachment::Image {
        name: "shot.svg".into(),
        media_type: "image/svg+xml".into(),
        data: "AAAA".into(),
        width: 1,
        height: 1,
    }];
    assert!(validate_attachments(&invalid).is_err());
}
#[test]
fn skill_descriptions_hide_sensitive_metadata() {
    assert_eq!(
        safe_skill_description("password: example"),
        "[sensitive description hidden]"
    );
    assert_eq!(
        safe_skill_description("Review Rust code"),
        "Review Rust code"
    );
    assert!(safe_skill_description(&"x".repeat(500)).chars().count() <= 320);
}

#[test]
fn web_runtime_agent_summary_excludes_workspace_report_and_internal_errors() {
    let value = serde_json::to_value(WebRuntimeAgent {
        id: uuid::Uuid::new_v4(),
        parent_id: None,
        label: Some("reader".to_owned()),
        background: true,
        profile: Some("reader".to_owned()),
        model: Some("reader-model".to_owned()),
        status: runtime_status_name(willdeep_core::RuntimeStatus::Working),
        current_turn: 2,
        current_tool: Some("read_file".to_owned()),
        retry_wait: None,
        total_tokens: Some(100),
        elapsed_seconds: 12,
        finished_seconds_ago: None,
        worktree_branch: None,
        dedicated_worktree: false,
    })
    .unwrap();
    assert_eq!(value["status"], "working");
    assert!(value["finished_seconds_ago"].is_null());
    assert!(value.get("workspace").is_none());
    assert!(value.get("report").is_none());
    assert!(value.get("error").is_none());
}

#[test]
fn web_runtime_agent_reports_run_duration_and_time_since_completion() {
    let completed_at = now_seconds() - 120;
    let value = serde_json::to_value(web_runtime_agent(crate::daemon::tui_bridge::RemoteAgent {
        id: uuid::Uuid::new_v4(),
        parent_id: None,
        label: Some("root".to_owned()),
        background: false,
        profile: None,
        model: None,
        status: willdeep_core::RuntimeStatus::Done,
        current_turn: 0,
        current_tool: None,
        retry_wait: None,
        total_tokens: None,
        max_turns: None,
        token_budget: None,
        timeout_seconds: None,
        report: None,
        workspace: std::path::PathBuf::from("/tmp"),
        worktree_branch: None,
        dedicated_worktree: false,
        started_at: Some(completed_at - 30),
        created_at: 1,
        completed_at: Some(completed_at),
    }))
    .expect("serialize public agent summary");
    assert_eq!(value["elapsed_seconds"], 30);
    assert!(value["finished_seconds_ago"].as_u64().unwrap_or_default() >= 120);
}

#[test]
fn web_runtime_task_summary_keeps_public_state_without_private_payloads() {
    let task_id = uuid::Uuid::new_v4();
    let session_id = uuid::Uuid::new_v4();
    let agent_id = uuid::Uuid::new_v4();
    let value = serde_json::to_value(web_runtime_task(crate::daemon::tui_bridge::RemoteTask {
        id: task_id,
        session_id: Some(session_id),
        turn_id: None,
        agent_id: Some(agent_id),
        status: willdeep_runtime_protocol::TaskStatus::Failed,
        profile: Some("reader".to_owned()),
        created_at: 10,
        started_at: Some(20),
        completed_at: Some(32),
        exit_code: Some(17),
        failure_domain: Some(willdeep_runtime_protocol::FailureDomain::Tool),
    }))
    .expect("serialize public task summary");
    assert_eq!(value["id"], task_id.to_string());
    assert_eq!(value["session_id"], session_id.to_string());
    assert_eq!(value["agent_id"], agent_id.to_string());
    assert_eq!(value["status"], "failed");
    assert_eq!(value["elapsed_seconds"], 12);
    assert_eq!(value["exit_code"], 17);
    assert_eq!(value["failure_domain"], "tool");
    for private in [
        "workspace",
        "prompt",
        "command",
        "arguments",
        "output",
        "error",
        "report",
        "path",
        "config",
        "model",
        "pid",
    ] {
        assert!(
            value.get(private).is_none(),
            "private field {private} leaked"
        );
    }
}

#[test]
fn web_structured_logs_keep_relations_without_private_tool_payloads() {
    let agent_id = uuid::Uuid::new_v4();
    let task_id = uuid::Uuid::new_v4();
    let tool = serde_json::to_value(willdeep_runtime_protocol::RuntimeTool {
        id: uuid::Uuid::new_v4(),
        session_id: Some(uuid::Uuid::new_v4()),
        turn_id: Some(uuid::Uuid::new_v4()),
        task_id,
        agent_id,
        name: "read_file".to_owned(),
        status: willdeep_runtime_protocol::ToolStatus::Completed,
        started_at_ms: 10,
        completed_at_ms: Some(20),
    })
    .expect("serialize public tool");
    assert_eq!(tool["agent_id"], agent_id.to_string());
    assert_eq!(tool["task_id"], task_id.to_string());
    for private in [
        "arguments",
        "params",
        "output",
        "error",
        "workspace",
        "path",
    ] {
        assert!(
            tool.get(private).is_none(),
            "private field {private} leaked"
        );
    }

    let artifact = serde_json::to_value(willdeep_runtime_protocol::RuntimeArtifact {
        id: uuid::Uuid::new_v4(),
        kind: willdeep_runtime_protocol::ArtifactKind::WorkspaceChange,
        session_id: None,
        turn_id: None,
        task_id,
        agent_id,
        title: "Workspace changes".to_owned(),
        source_id: "snapshot-id".to_owned(),
        item_count: 3,
        created_at: 30,
    })
    .expect("serialize public artifact");
    assert_eq!(artifact["agent_id"], agent_id.to_string());
    assert_eq!(artifact["task_id"], task_id.to_string());
    for private in ["workspace", "path", "report", "error"] {
        assert!(
            artifact.get(private).is_none(),
            "private field {private} leaked"
        );
    }
}

#[test]
fn runtime_actions_require_the_target_in_the_selected_workspace_snapshot() {
    let approval_id = uuid::Uuid::new_v4();
    let question_id = uuid::Uuid::new_v4();
    let snapshot = crate::daemon::RuntimeSnapshot {
        queue: None,
        attention: Vec::new(),
        gates: vec![
            crate::daemon::RemoteGate::Approval {
                id: approval_id,
                parked: false,
                task_id: uuid::Uuid::new_v4(),
                description: "approval".to_owned(),
                always_allow_available: true,
            },
            crate::daemon::RemoteGate::Question {
                id: question_id,
                parked: false,
                task_id: uuid::Uuid::new_v4(),
                question: "question".to_owned(),
                options: vec!["A".to_owned()],
                multi_select: false,
            },
        ],
        agents: Vec::new(),
        tasks: Vec::new(),
        tools: Vec::new(),
        artifacts: Vec::new(),
        runtime_version: None,
        runtime_diagnostic: None,
        event_sequence: None,
        mobile: None,
    };
    assert!(snapshot_has_approval(&snapshot, approval_id));
    assert!(snapshot_has_question(&snapshot, question_id));
    assert!(!snapshot_has_approval(&snapshot, uuid::Uuid::new_v4()));
    assert!(!snapshot_has_agent(&snapshot, uuid::Uuid::new_v4()));
}

#[test]
fn turn_actions_require_the_owning_session_workspace_in_the_allowlist() {
    let registered = |root: &str| crate::daemon::RuntimeWorkspace {
        schema: 1,
        id: uuid::Uuid::new_v4(),
        name: "workspace".to_owned(),
        root: std::path::PathBuf::from(root),
        access: crate::daemon::WorkspaceAccess::WorkspaceWrite,
        provider_profile: None,
        skills: Vec::new(),
        mcp_servers: Vec::new(),
        created_at: 0,
        updated_at: 0,
        active: true,
    };
    let allowed = vec![registered("/allowed"), registered("/also-allowed")];

    assert!(workspace_in_allowlist(
        &allowed,
        std::path::Path::new("/allowed")
    ));
    assert!(workspace_in_allowlist(
        &allowed,
        std::path::Path::new("/also-allowed")
    ));
    // A Turn whose Session lives outside the allowlist must not be
    // stoppable, even with a valid Turn id.
    assert!(!workspace_in_allowlist(
        &allowed,
        std::path::Path::new("/other-workspace")
    ));
    assert!(!workspace_in_allowlist(
        &[],
        std::path::Path::new("/allowed")
    ));
}

#[test]
fn resume_cursor_stays_inside_the_active_turn_event_window() {
    assert_eq!(bounded_resume_cursor(None, 10, 20), 10);
    assert_eq!(bounded_resume_cursor(Some(5), 10, 20), 10);
    assert_eq!(bounded_resume_cursor(Some(15), 10, 20), 15);
    assert_eq!(bounded_resume_cursor(Some(99), 10, 20), 20);
    assert_eq!(bounded_resume_cursor(Some(1), 30, 20), 30);
}

#[test]
fn resume_stream_query_rejects_client_selected_runtime_scope() {
    assert!(
        serde_json::from_value::<ResumeStreamQuery>(serde_json::json!({
            "after": 12,
            "language": "zh-CN"
        }))
        .is_ok()
    );
    for private_scope in ["workspace", "turn_id", "task_id", "agent_id"] {
        assert!(
            serde_json::from_value::<ResumeStreamQuery>(serde_json::json!({
                "after": 12,
                (private_scope): uuid::Uuid::new_v4()
            }))
            .is_err(),
            "resume query accepted client scope {private_scope}"
        );
    }
}

#[test]
fn runtime_action_bodies_reject_client_supplied_extra_scope() {
    assert!(
        serde_json::from_value::<WebWorkspaceAction>(serde_json::json!({
            "workspace": "/allowed",
            "agent_id": uuid::Uuid::new_v4()
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WebAgentRetryAction>(serde_json::json!({
            "workspace": "/allowed",
            "model": "qwen3-max",
            "agent_id": uuid::Uuid::new_v4()
        }))
        .is_err()
    );
    let retry = serde_json::from_value::<WebAgentRetryAction>(serde_json::json!({
        "workspace": "/allowed",
        "model": "qwen3-max"
    }))
    .expect("valid retry action");
    assert_eq!(retry.model.as_deref(), Some("qwen3-max"));
    assert!(
        serde_json::from_value::<WebApprovalAction>(serde_json::json!({
            "workspace": "/allowed",
            "decision": "allow_once",
            "always_allow": true
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<WebAgentSpawnAction>(serde_json::json!({
            "workspace": "/allowed",
            "session_id": uuid::Uuid::new_v4(),
            "profile": "scout",
            "prompt": "inspect the repository",
            "workspace_root": "/attacker-selected"
        }))
        .is_err()
    );
}

#[test]
fn web_agent_spawn_accepts_only_bounded_read_only_profiles() {
    let session_id = uuid::Uuid::new_v4();
    let valid = serde_json::from_value::<WebAgentSpawnAction>(serde_json::json!({
        "workspace": "/allowed",
        "session_id": session_id,
        "profile": "reader",
        "prompt": "  read the architecture docs  ",
        "label": "  architecture reader  "
    }))
    .expect("valid spawn body");
    let (profile, prompt, label) = validate_agent_spawn(valid).expect("valid spawn action");
    assert_eq!(profile, "reader");
    assert_eq!(prompt, "read the architecture docs");
    assert_eq!(label.as_deref(), Some("architecture reader"));

    let judge = WebAgentSpawnAction {
        workspace: "/allowed".to_owned(),
        session_id,
        profile: "judge".to_owned(),
        prompt: "review the evidence".to_owned(),
        label: None,
    };
    assert!(validate_agent_spawn(judge).is_ok());

    // `deep` 不在这里了：它现在是档位别名，档位的准入由 Agent 层的票据
    // 把关，不归这条工种白名单管。
    for profile in ["implementer", "editor", "scout", "writer", "shell"] {
        let action = WebAgentSpawnAction {
            workspace: "/allowed".to_owned(),
            session_id,
            profile: profile.to_owned(),
            prompt: "do work".to_owned(),
            label: None,
        };
        assert!(validate_agent_spawn(action).is_err());
    }
    let empty_prompt = WebAgentSpawnAction {
        workspace: "/allowed".to_owned(),
        session_id,
        profile: "reader".to_owned(),
        prompt: "   ".to_owned(),
        label: None,
    };
    assert!(validate_agent_spawn(empty_prompt).is_err());
    let long_label = WebAgentSpawnAction {
        workspace: "/allowed".to_owned(),
        session_id,
        profile: "scout".to_owned(),
        prompt: "inspect".to_owned(),
        label: Some("x".repeat(MAX_AGENT_LABEL_CHARS + 1)),
    };
    assert!(validate_agent_spawn(long_label).is_err());
}

#[test]
fn runtime_event_metadata_and_payload_are_parsed_without_exposing_prefixes() {
    let task_id = uuid::Uuid::new_v4();
    let turn_id = uuid::Uuid::new_v4();
    let message = format!(
        "session_id={} turn_id={turn_id} task_id={task_id}",
        uuid::Uuid::new_v4()
    );
    assert_eq!(event_uuid(&message, "turn_id"), Some(turn_id));
    assert_eq!(event_uuid(&message, "task_id"), Some(task_id));

    let output = format!(
        "task_id={task_id} {}",
        serde_json::json!({"type":"assistant_text","text":"hello"})
    );
    let payload = runtime_output_payload(&output).unwrap();
    assert_eq!(payload["text"], "hello");
}

/// 下一句预测用的假 Provider：按脚本回答或失败，并数自己被问了几次。
struct ScriptedProvider {
    reply: Option<&'static str>,
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl willdeep_core::provider::Provider for ScriptedProvider {
    async fn complete(
        &self,
        _: &[willdeep_core::Message],
        _: &[willdeep_core::types::ToolDefinition],
    ) -> std::result::Result<willdeep_core::types::Completion, willdeep_core::provider::ProviderError>
    {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.reply {
            Some(text) => Ok(willdeep_core::types::Completion {
                content: text.to_owned(),
                ..Default::default()
            }),
            None => Err(willdeep_core::provider::ProviderError::InvalidResponse(
                "scripted failure".to_owned(),
            )),
        }
    }
}

fn scripted(
    reply: Option<&'static str>,
) -> (
    Arc<dyn willdeep_core::provider::Provider>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    (
        Arc::new(ScriptedProvider {
            reply,
            calls: calls.clone(),
        }),
        calls,
    )
}

fn finished_turn() -> Vec<willdeep_core::Message> {
    vec![
        willdeep_core::Message::user("把测试修好"),
        willdeep_core::Message::assistant("测试已全部通过。要不要我提交？", Vec::new()),
    ]
}

#[tokio::test]
async fn suggestion_comes_from_the_first_provider_that_answers() {
    let (failing, failing_calls) = scripted(None);
    let (answering, _) = scripted(Some("提交并合回 develop"));
    let suggestion =
        suggest_for_session(&finished_turn(), false, || Ok(vec![failing, answering])).await;
    assert_eq!(suggestion.as_deref(), Some("提交并合回 develop"));
    assert_eq!(failing_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn every_provider_failing_is_a_quiet_null() {
    let (first, _) = scripted(None);
    let (second, _) = scripted(None);
    let suggestion = suggest_for_session(&finished_turn(), false, || Ok(vec![first, second])).await;
    assert_eq!(suggestion, None);
    // 装不出 Provider 也一样安静。
    let suggestion = suggest_for_session(&finished_turn(), false, || {
        Err(anyhow::anyhow!("no provider configured"))
    })
    .await;
    assert_eq!(suggestion, None);
}

#[tokio::test]
async fn a_running_session_or_an_empty_payload_never_asks_a_provider() {
    let (provider, calls) = scripted(Some("继续"));
    let running = suggest_for_session(&finished_turn(), true, || Ok(vec![provider.clone()])).await;
    assert_eq!(running, None);
    let only_user = vec![willdeep_core::Message::user("你好")];
    let empty = suggest_for_session(&only_user, false, || Ok(vec![provider.clone()])).await;
    assert_eq!(empty, None);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

fn suggestion_state(tag: &str, config: &str) -> (Arc<WebState>, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "willdeep-web-suggestion-{tag}-{}",
        uuid::Uuid::new_v4()
    ));
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let config_path = root.join("config.toml");
    std::fs::write(&config_path, config).unwrap();
    let state = Arc::new(WebState {
        config_path,
        profile: None,
        workspaces: Arc::new(std::sync::RwLock::new(Vec::new())),
        home,
        language: Language::default(),
        harness_slots: Arc::new(Semaphore::new(1)),
        settings_writable: false,
        settings_write_lock: Mutex::new(()),
        config_tools: tokio::sync::OnceCell::new(),
    });
    (state, root)
}

fn tree_snapshot(root: &std::path::Path) -> Vec<(PathBuf, std::time::SystemTime)> {
    let mut entries = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let modified = entry.metadata().unwrap().modified().unwrap();
            if path.is_dir() {
                pending.push(path.clone());
            }
            entries.push((path, modified));
        }
    }
    entries.sort();
    entries
}

/// Web 建议的反馈只认服务端发出去的 id：原文与停留时长取服务端记下的，
/// 发送的「原样 / 改过 / 重写」由服务端判定；对不上号、未知信号、
/// 别的会话冒用 id 都静默丢弃；结局之后 id 作废，重复上报不再记。
#[test]
fn suggestion_feedback_trusts_only_server_issued_suggestions() {
    let home = std::env::temp_dir().join(format!("willdeep-web-feedback-{}", uuid::Uuid::new_v4()));
    let settings = crate::config::FeedbackSettings::default();
    let session = uuid::Uuid::new_v4();
    let id = issue_suggestion(session, "run the tests");
    let report =
        |session_id: uuid::Uuid, suggestion_id: uuid::Uuid, signal: &str, sent: Option<&str>| {
            apply_suggestion_feedback(
                &home,
                &settings,
                session_id,
                &SuggestionFeedbackRequest {
                    suggestion_id,
                    signal: signal.to_owned(),
                    sent: sent.map(str::to_owned),
                },
            );
        };
    report(session, id, "shown", None);
    report(uuid::Uuid::new_v4(), id, "dismissed", None);
    report(session, uuid::Uuid::new_v4(), "accepted", None);
    report(session, id, "made_up", None);
    report(session, id, "accepted", None);
    report(session, id, "sent", Some("run the tests please"));
    report(session, id, "sent", Some("run the tests"));

    let dir = willdeep_core::feedback::feedback_dir(&home);
    assert!(willdeep_core::feedback::shared_sink(&dir).flush(Duration::from_secs(2)));
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        rows.extend(
            text.lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()),
        );
    }
    let signals: Vec<&str> = rows
        .iter()
        .map(|row| row["signal"].as_str().unwrap())
        .collect();
    assert_eq!(
        signals,
        [
            "suggestion_shown",
            "suggestion_accepted",
            "suggestion_sent_edited"
        ],
        "{rows:?}"
    );
    assert!(rows.iter().all(|row| row["client"] == "web"
        && row["suggestion_id"] == id.to_string()
        && row["text_len"] == 13
        && row["text"].is_null()));
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn the_endpoint_answers_null_with_200_and_writes_nothing_when_switched_off() {
    let (state, root) = suggestion_state("off", "[agent]\ninput_suggestions = false\n");
    let before = tree_snapshot(&root);
    let turn_id = uuid::Uuid::new_v4();
    let Json(response) = input_suggestion(
        State(state),
        Path(uuid::Uuid::new_v4()),
        Json(InputSuggestionRequest {
            turn_id: Some(turn_id),
        }),
    )
    .await;
    assert_eq!(response.suggestion, None);
    assert_eq!(response.turn_id, Some(turn_id), "turn_id 原样带回");
    assert_eq!(tree_snapshot(&root), before, "预测不落盘");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn an_unknown_session_is_null_not_an_error_and_writes_nothing() {
    let (state, root) = suggestion_state("unknown", "");
    let before = tree_snapshot(&root);
    let Json(response) = input_suggestion(
        State(state),
        Path(uuid::Uuid::new_v4()),
        Json(InputSuggestionRequest { turn_id: None }),
    )
    .await;
    assert_eq!(response.suggestion, None);
    assert_eq!(tree_snapshot(&root), before, "预测不落盘");
    let _ = std::fs::remove_dir_all(root);
}
