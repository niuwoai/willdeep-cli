use super::*;

fn workflow_fixture() -> crate::workflow::WorkflowRun {
    crate::workflow::WorkflowRun {
        id: uuid::Uuid::new_v4(),
        revision: 1,
        parent_session_id: uuid::Uuid::new_v4(),
        definition_id: "review-v1".into(),
        spec: crate::workflow::example_spec(),
        input: serde_json::Value::Null,
        limits: Default::default(),
        status: crate::workflow::WorkflowRunStatus::Running,
        nodes: Vec::new(),
        output: None,
        report: None,
        error: None,
        created_at_ms: 1,
        finished_at_ms: None,
        source_run_id: None,
        acknowledged: false,
    }
}

#[test]
fn workflow_control_receipts_refresh_runs_and_keep_retry_cards() {
    let call = crate::types::ToolCall {
        id: "workflow-1".into(),
        name: "workflow_control".into(),
        arguments: "{}".into(),
    };
    let mut run = workflow_fixture();
    let mut messages = vec![
        Message::assistant("", vec![call.clone()]),
        Message::tool(&call, serde_json::json!({"workflow":run}).to_string()),
    ];
    run.status = crate::workflow::WorkflowRunStatus::Failed;
    run.revision += 1;
    messages.push(Message::tool(
        &call,
        serde_json::json!({"workflow":run}).to_string(),
    ));
    let mut stale = run.clone();
    stale.revision -= 1;
    stale.status = crate::workflow::WorkflowRunStatus::Running;
    messages.push(Message::tool(
        &call,
        serde_json::json!({"workflow":stale}).to_string(),
    ));
    let mut retry = run.clone();
    retry.id = uuid::Uuid::new_v4();
    retry.source_run_id = Some(run.id);
    retry.status = crate::workflow::WorkflowRunStatus::Running;
    messages.push(Message::tool(
        &call,
        serde_json::json!({"workflow":retry}).to_string(),
    ));
    let cards: Vec<_> = project(&messages, None)
        .into_iter()
        .filter(|item| item.role == "workflow")
        .collect();
    assert_eq!(cards.len(), 2);
    assert_eq!(
        cards[0].workflow.as_ref().unwrap().status,
        crate::workflow::WorkflowRunStatus::Failed
    );
    assert_eq!(
        cards[1].workflow.as_ref().unwrap().source_run_id,
        Some(run.id)
    );
    assert!(
        serde_json::to_value(&cards[0])
            .unwrap()
            .get("workflow")
            .is_some()
    );
}

#[test]
fn workflow_projection_ignores_read_prose_errors_and_invalid_receipts() {
    let run = workflow_fixture();
    let content = serde_json::json!({"workflow":run}).to_string();
    let read = crate::types::ToolCall {
        id: "read".into(),
        name: "workflow_read".into(),
        arguments: "{}".into(),
    };
    let control = crate::types::ToolCall {
        id: "control".into(),
        name: "workflow_control".into(),
        arguments: "{}".into(),
    };
    let messages = vec![
        Message::user(&content),
        Message::assistant(&content, vec![read.clone(), control.clone()]),
        Message::tool(&read, &content),
        Message::tool(&control, "{\"workflow\":{\"id\":\"bad\"}}"),
        Message::tool(
            &control,
            serde_json::json!({"is_error":true,"workflow":run}).to_string(),
        ),
    ];
    assert!(
        project(&messages, None)
            .iter()
            .all(|item| item.workflow.is_none())
    );
}

#[test]
fn ask_user_tool_rows_keep_the_question() {
    // 对话框会因为交接而失效；工具行里留着问题，回放时还找得回来。
    assert_eq!(
        tool_detail(
            "ask_user",
            r#"{"question":"  推哪个远端？\n","options":["a"]}"#
        ),
        Some("推哪个远端？".to_owned())
    );
    assert_eq!(tool_detail("ask_user", r#"{"options":["a"]}"#), None);
    assert_eq!(tool_detail("ask_user", r#"{"question":"   "}"#), None);
}

/// 点开后看完整命令：路径原样保留，凭据照旧打码，长度不截断。
#[test]
fn tool_expansion_keeps_the_whole_command_and_still_redacts_secrets() {
    let command = format!(
        "ssh -o BatchMode=yes q4-pub '/home/ml-data/run.py' token=secret {}",
        "x".repeat(200)
    );
    let arguments = serde_json::json!({"command": command}).to_string();
    let expanded = tool_expansion("run_command", &arguments).unwrap();
    assert!(expanded.contains("/home/ml-data/run.py"), "{expanded}");
    assert!(
        expanded.contains(&"x".repeat(200)),
        "expansion must not truncate"
    );
    assert!(!expanded.contains("secret"), "{expanded}");

    // 摘要行相反：路径换成省略号，超长截断。路径包在引号里也要认出来。
    let summary = tool_detail("run_command", &arguments).unwrap();
    assert!(!summary.contains("/home/ml-data"), "{summary}");
    assert!(summary.ends_with('\u{2026}'), "{summary}");
    let quoted = tool_detail(
        "run_command",
        r#"{"command":"cat \"/etc/hosts\" '~/notes'"}"#,
    )
    .unwrap();
    assert!(!quoted.contains("/etc/hosts"), "{quoted}");
    assert!(!quoted.contains("~/notes"), "{quoted}");

    assert_eq!(tool_expansion("read_file", r#"{"path":"/tmp/a"}"#), None);
}

/// 工具行回放：只带工具调用的 assistant 消息也进投影，每条调用带名字、脱敏摘要
/// 与有无结果；原始参数不出投影。
#[test]
fn tool_only_assistant_messages_project_their_calls_with_results() {
    let call = |id: &str, name: &str, arguments: &str| crate::types::ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: arguments.to_owned(),
    };
    let messages = [
        Message::user("go"),
        Message::assistant(
            "",
            vec![
                call(
                    "a",
                    "run_command",
                    r#"{"command":"cargo test -p x --manifest-path /secret/Cargo.toml"}"#,
                ),
                call("b", "read_file", r#"{"path":"/etc/hosts"}"#),
            ],
        ),
        Message::tool(&call("a", "run_command", "{}"), "ok"),
        Message::assistant("done", Vec::new()),
    ];
    let items = project(&messages, None);
    assert_eq!(
        items.iter().map(|item| item.role).collect::<Vec<_>>(),
        vec!["user", "assistant", "assistant"]
    );
    assert_eq!(items[1].content, "");
    assert_eq!(
        items[1].tools,
        vec![
            ToolTrace {
                name: "run_command".into(),
                detail: Some("cargo test -p x --manifest-path …".into()),
                completed: true,
            },
            ToolTrace {
                name: "read_file".into(),
                detail: None,
                completed: false,
            },
        ]
    );
    assert!(items[2].tools.is_empty());
    let json = serde_json::to_string(&items[1]).unwrap();
    assert!(
        !json.contains("/secret") && !json.contains("/etc/hosts"),
        "{json}"
    );
    assert!(!json.contains("arguments"), "{json}");
}

#[test]
fn recorded_model_is_projected_per_message_and_absent_models_stay_absent() {
    let messages = [
        Message::user("first"),
        Message::assistant("from glm", Vec::new()).with_model(Some("glm-5".into())),
        Message::user("second"),
        Message::assistant("from qwen", Vec::new())
            .with_model(Some("  tokenhub-tsingfly/Qwen3.8-27B  ".into())),
        // 旧会话和派生消息没有模型：投影必须留空，不能拿别的消息的模型填。
        Message::assistant("no model recorded", Vec::new()),
        Message::assistant("blank model", Vec::new()).with_model(Some("   ".into())),
    ];
    let items = project(&messages, None);
    let models: Vec<_> = items.iter().map(|item| item.model.as_deref()).collect();
    assert_eq!(
        models,
        vec![
            None,
            Some("glm-5"),
            None,
            Some("tokenhub-tsingfly/Qwen3.8-27B"),
            None,
            None,
        ]
    );
    // 用户消息不带模型，序列化时这个字段根本不出现。
    let user_json = serde_json::to_string(&items[0]).unwrap();
    assert!(!user_json.contains("model"), "{user_json}");
    let assistant_json = serde_json::to_string(&items[1]).unwrap();
    assert!(
        assistant_json.contains("\"model\":\"glm-5\""),
        "{assistant_json}"
    );
}

#[test]
fn message_without_a_model_field_still_loads() {
    let legacy = r#"{"role":"assistant","content":"old reply"}"#;
    let message: Message = serde_json::from_str(legacy).unwrap();
    assert_eq!(message.model, None);
    let stored = serde_json::to_string(&message).unwrap();
    assert!(
        !stored.contains("model"),
        "unset model must not be written: {stored}"
    );
    let stamped = message.with_model(Some("glm-5".into()));
    let restored: Message =
        serde_json::from_str(&serde_json::to_string(&stamped).unwrap()).unwrap();
    assert_eq!(restored.model.as_deref(), Some("glm-5"));
}

const PLAN: &str = "```plan\n1. A-TRACE-1: 核验接口\n2. D-REL-1: 发布\n```";
const PROGRESS: &str = "全部步骤已结束。\n```progress\nA-TRACE-1: done\nD-REL-1: skipped\n```";

#[test]
fn host_source_survives_round_trip_without_changing_provider_role() {
    let message = Message::host_instruction("The previous Goal phase is complete.");
    let restored: Message =
        serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
    assert_eq!(restored.role, Role::User);
    let items = project(&[restored], None);
    assert_eq!(items[0].role, "system");
    assert!(items[0].content.is_empty());
    assert_eq!(items[0].details, vec![message.content]);
}

#[test]
fn user_pasted_host_prompt_and_plan_are_not_reclassified() {
    let content = format!("The previous Goal phase is complete.\n{PLAN}");
    let legacy: Message =
        serde_json::from_value(serde_json::json!({"role":"user", "content":content})).unwrap();
    for message in [legacy, Message::user(&content)] {
        let items = project(&[message], None);
        assert_eq!(items[0].role, "user");
        assert_eq!(items[0].content, content);
    }
}

#[test]
fn repeated_progress_updates_one_card_and_retains_original_records() {
    let messages = [
        Message::assistant(PLAN, vec![]),
        Message::assistant(PROGRESS, vec![]),
        Message::assistant(PROGRESS, vec![]),
    ];
    let items = project(&messages, None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].details, vec![PLAN, PROGRESS]);
    let plan = items[0].plan.as_ref().unwrap();
    assert_eq!(plan.steps[0].status, StepStatus::Done);
    assert_eq!(plan.steps[1].status, StepStatus::Skipped);
    assert_eq!(messages[0].content, PLAN);
}

#[test]
fn malformed_or_incomplete_blocks_remain_visible_and_do_not_partially_update() {
    let (plan, _) = parse_plan_reply(PLAN, None).unwrap();
    for reply in [
        "```progress\n1: done\n2: invented\n```",
        "```progress\n1: done\n99: done\n```",
        "```progress\n1: done",
        "```rust\n1: done\n```",
        "```plan\n2. missing first step\n```",
        "```progress\n```",
        "all steps done",
    ] {
        assert!(parse_plan_reply(reply, Some(&plan)).is_none(), "{reply}");
        let items = project(
            &[
                Message::assistant(PLAN, vec![]),
                Message::assistant(reply, vec![]),
            ],
            None,
        );
        assert_eq!(items[1].content, reply);
        assert_eq!(
            items[0].plan.as_ref().unwrap().steps[0].status,
            StepStatus::Pending
        );
    }
}

#[test]
fn a_new_phase_reuses_the_card_and_clears_previous_details() {
    let replacement = "```plan\n1. 下一阶段\n```";
    let items = project(
        &[
            Message::assistant(PLAN, vec![]),
            Message::assistant(PROGRESS, vec![]),
            Message::assistant(replacement, vec![]),
        ],
        None,
    );
    assert_eq!(items.len(), 1);
    let plan = items[0].plan.as_ref().unwrap();
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].text, "下一阶段");
    assert_eq!(plan.steps[0].status, StepStatus::Pending);
    assert_eq!(items[0].details, vec![replacement]);
}

fn plan_detail_cases() -> Vec<(&'static str, Plan, bool)> {
    let (plan, _) = parse_plan_reply(PLAN, None).unwrap();
    let mut progress = plan.clone();
    progress.steps[0].status = StepStatus::InProgress;
    let mut renamed = plan.clone();
    renamed.steps[1].text = "新的任务".into();
    let mut reidentified = plan.clone();
    reidentified.steps[1].id = "replacement".into();
    let mut reduced = plan.clone();
    reduced.steps.pop();
    let mut extended = plan.clone();
    let mut added = plan.steps[0].clone();
    added.id = "3".into();
    added.text = "新增任务".into();
    extended.steps.push(added);
    let mut reordered = plan;
    reordered.steps.swap(0, 1);
    vec![
        ("progress", progress, false),
        ("renamed", renamed, true),
        ("reidentified", reidentified, true),
        ("removed", reduced, true),
        ("added", extended, true),
        ("reordered", reordered, true),
    ]
}

#[test]
fn structured_plan_changes_reuse_the_card_and_discard_unrelated_details() {
    let call = crate::types::ToolCall {
        id: "plan-update".into(),
        name: "update_plan".into(),
        arguments: "{}".into(),
    };
    for (case, plan, changed_steps) in plan_detail_cases() {
        for replaced in [false, true] {
            let receipt = crate::planning::Receipt::new(plan.clone(), replaced);
            let messages = [
                Message::assistant(PLAN, vec![]),
                Message::assistant(PROGRESS, vec![]),
                Message::assistant("执行中", vec![call.clone()]),
                Message::tool(&call, serde_json::to_string(&receipt).unwrap()),
            ];
            let items = project(&messages, None);
            assert_eq!(items.len(), 2, "{case}, replaced={replaced}");
            assert_eq!(items[0].plan.as_ref(), Some(&plan), "{case}");
            assert_eq!(items[1].content, "执行中", "{case}");
            let expected = if replaced || changed_steps {
                Vec::<&str>::new()
            } else {
                vec![PLAN, PROGRESS]
            };
            assert_eq!(items[0].details, expected, "{case}, replaced={replaced}");
            assert_eq!(messages[0].content, PLAN);
            assert_eq!(messages[1].content, PROGRESS);
        }
    }
}

#[test]
fn persisted_plan_changes_reuse_the_card_and_discard_unrelated_details() {
    for (case, plan, changed_steps) in plan_detail_cases() {
        let items = project(
            &[
                Message::assistant(PLAN, vec![]),
                Message::assistant(PROGRESS, vec![]),
                Message::user("继续"),
            ],
            Some(&plan),
        );
        assert_eq!(items.len(), 2, "{case}");
        assert_eq!(items[0].plan.as_ref(), Some(&plan), "{case}");
        assert_eq!(items[1].content, "继续", "{case}");
        let expected = if changed_steps {
            Vec::<&str>::new()
        } else {
            vec![PLAN, PROGRESS]
        };
        assert_eq!(items[0].details, expected, "{case}");
    }
    let (plan, _) = parse_plan_reply(PLAN, None).unwrap();
    let items = project(&[], Some(&plan));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].plan.as_ref(), Some(&plan));
    assert!(items[0].details.is_empty());
}

#[test]
fn persisted_host_state_overrides_model_progress() {
    let (mut plan, _) = parse_plan_reply(PLAN, None).unwrap();
    plan.steps[0].status = StepStatus::Failed;
    let items = project(
        &[
            Message::assistant(PLAN, vec![]),
            Message::assistant(PROGRESS, vec![]),
        ],
        Some(&plan),
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].plan.as_ref().unwrap(), &plan);
}

#[test]
fn standalone_progress_snapshot_is_deduplicated() {
    let items = project(
        &[
            Message::assistant(PROGRESS, vec![]),
            Message::assistant(PROGRESS, vec![]),
        ],
        None,
    );
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].plan.as_ref().unwrap().steps.len(), 2);
}

#[test]
fn persisted_plan_fills_in_titles_for_a_status_only_snapshot() {
    let (mut plan, _) = parse_plan_reply(PLAN, None).unwrap();
    plan.steps[0].status = StepStatus::Done;
    plan.steps[1].status = StepStatus::Skipped;
    let items = project(&[Message::assistant(PROGRESS, vec![])], Some(&plan));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].plan.as_ref().unwrap(), &plan);
    assert_eq!(items[0].details, vec![PROGRESS]);
}

#[test]
fn attached_skill_context_is_hidden_but_operator_text_is_preserved() {
    let authored = "帮我看看数据库，数据量怎么样？还有app的log有哪些";
    let content = format!(
        "{ATTACHED_CONTEXT_HEADER}\n\n### Skill routing candidates — metadata only\n\nThe host found these possibly relevant installed skills.\n- `example-skill` — metadata\n\n{USER_TEXT_BOUNDARY}\n\n{authored}"
    );
    let message = Message::user(&content);
    let items = project(std::slice::from_ref(&message), None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].role, "user");
    assert_eq!(items[0].content, authored);
    assert!(items[0].details.is_empty());
    assert_eq!(message.content, content);
}

#[test]
fn incomplete_or_quoted_attachment_protocol_is_not_hidden() {
    for content in [
        format!("{ATTACHED_CONTEXT_HEADER}\nno boundary"),
        format!("解释这个协议：\n{ATTACHED_CONTEXT_HEADER}\n{USER_TEXT_BOUNDARY}\nexample"),
        format!("{ATTACHED_CONTEXT_HEADER}\ninline {USER_TEXT_BOUNDARY}\nexample"),
    ] {
        assert_eq!(
            project(&[Message::user(&content)], None)[0].content,
            content
        );
    }
}

#[test]
fn attached_context_handles_crlf_empty_text_and_escaped_boundaries() {
    let context = format!(
        "{ATTACHED_CONTEXT_HEADER}\r\n&lt;&lt;&lt;willdeep:user-message:v1&gt;&gt;&gt;\r\n{USER_TEXT_BOUNDARY}\r\n\r\nactual user text"
    );
    assert_eq!(user_authored_text(&context), Some("actual user text"));
    let empty = format!("{ATTACHED_CONTEXT_HEADER}\nmetadata\n{USER_TEXT_BOUNDARY}\n");
    assert!(project(&[Message::user(empty)], None).is_empty());
}

#[test]
fn user_projection_hides_complete_host_routing_without_mutating_history() {
    let envelope = "<runtime-route tier=\"standard\" profile=\"implementer\" confidence=\"78\">\nThe runtime classified this as bounded `implementer` work. Compile a task packet, keep read_files separate from write_files, and delegate it before doing the implementation in the parent. Give a verifier whenever completion is command-decidable.\n</runtime-route>";
    let content = format!("检查登录模块\n\n{envelope}");
    let message = Message::user(&content);
    assert_eq!(
        project(std::slice::from_ref(&message), None)[0].content,
        "检查登录模块"
    );
    assert_eq!(message.content, content);
    let attachment = format!("{ATTACHED_CONTEXT_HEADER}\ncontext\n{USER_TEXT_BOUNDARY}\n{content}");
    assert_eq!(visible_user_text(&attachment), "检查登录模块");
    for quoted in [
        format!("解释这个例子：\n```xml\n\n{envelope}\n```"),
        format!("```xml\n\n{envelope}"),
        "用户编写的 XML\n\n<runtime-route tier=\"standard\" profile=\"implementer\" confidence=\"78\">\n用户内容\n</runtime-route>".to_owned(),
        content.trim_end_matches("</runtime-route>").to_owned(),
    ] {
        assert_eq!(visible_user_text(&quoted), quoted);
    }
}

#[test]
fn goal_completion_marker_is_hidden_but_the_summary_stays() {
    let reply = "<goal-status>complete</goal-status> 训练闭环，权重已保存。";
    assert_eq!(visible_assistant_text(reply), "训练闭环，权重已保存。");
    assert_eq!(
        visible_assistant_text("\n\n<goal-status>complete</goal-status>\n\n已交付。"),
        "已交付。"
    );
    // 只有标签：没有正文可显示。
    assert_eq!(
        visible_assistant_text("<goal-status>complete</goal-status>"),
        ""
    );
    // 只认开头：正文里引用的标签是模型自己的话，原样保留。
    for inline in [
        "标记 <goal-status>complete</goal-status> 只出现在正文里",
        "普通回复",
        "<goal-status>complete</goal-status 少了右括号",
        "",
    ] {
        assert_eq!(visible_assistant_text(inline), inline);
    }
}

#[test]
fn projection_hides_the_goal_completion_marker_without_mutating_history() {
    let message = Message::assistant("<goal-status>complete</goal-status> 训练闭环。", Vec::new());
    let items = project(std::slice::from_ref(&message), None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].content, "训练闭环。");
    assert_eq!(
        message.content,
        "<goal-status>complete</goal-status> 训练闭环。"
    );
    // 纯标记的回复不产生空气泡。
    let bare = Message::assistant("<goal-status>complete</goal-status>", Vec::new());
    assert!(project(std::slice::from_ref(&bare), None).is_empty());
}

#[test]
fn federation_notice_projects_as_a_system_item() {
    let mut note = Message::system("[联邦] 自动回复已暂停");
    note.source = Some(crate::types::MessageSource::FederationNotice);
    let items = project(&[Message::system("internal"), note], None);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].role, "system");
    // 群说明要给人看到正文；它不像 host instruction 那样藏进 details。
    assert_eq!(items[0].content, "[联邦] 自动回复已暂停");
    assert!(items[0].details.is_empty());
}

#[test]
fn goal_envelope_is_hidden_but_the_typed_text_stays() {
    let content = "<goal>\n请继续，最终以完成训练为目标\n</goal>\nContinue until this goal is genuinely complete.\n\n请继续，最终以完成训练为目标";
    assert_eq!(visible_user_text(content), "请继续，最终以完成训练为目标");
    // 用户自己贴的半截或包在代码围栏里的，原样保留。
    for quoted in [
        "<goal>\n只有开头",
        "<goal>\n目标\n</goal>\nContinue until this goal is genuinely complete.\n\n```\n未闭合",
        "解释一下这个格式：\n\n<goal>\n目标\n</goal>",
    ] {
        assert_eq!(visible_user_text(quoted), quoted);
    }
}
