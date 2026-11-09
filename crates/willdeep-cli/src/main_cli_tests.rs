use super::*;

#[test]
fn workflow_events_and_control_receipts_preserve_structure_and_redact_display_strings() {
    let mut value = serde_json::json!({
        "id":uuid::Uuid::new_v4(), "parent_session_id":uuid::Uuid::new_v4(),
        "definition_id":"review-v1", "spec":willdeep_core::workflow::example_spec(),
        "input":null, "limits":willdeep_core::workflow::WorkflowLimits::default(),
        "status":"running", "nodes":[], "output":null,
        "report":"first line\n--password fixture-secret", "error":null,
        "created_at_ms":1, "finished_at_ms":null, "source_run_id":null, "acknowledged":false,
        "revision":1
    });
    let run = serde_json::from_value(value.clone()).unwrap();
    let event = agent_event_json(AgentEvent::WorkflowUpdated(Box::new(run)));
    assert_eq!(event["type"], "workflow_updated");
    assert!(event["workflow"]["report"].as_str().unwrap().contains('\n'));
    assert!(!event.to_string().contains("fixture-secret"));
    assert!(event["workflow"]["limits"].is_object());
    value["report"] = serde_json::Value::Null;
    let call = willdeep_core::ToolCall {
        id: "workflow-call".into(),
        name: "workflow_control".into(),
        arguments: "{}".into(),
    };
    let receipt = agent_event_json(AgentEvent::ToolCompleted {
        call,
        output: serde_json::json!({"workflow":value}).to_string(),
        is_error: false,
    });
    assert_eq!(receipt["workflow"]["id"], value["id"]);
}

/// 成功的工具只报名字；失败的才带上参数和输出，否则事后排查只剩一个 task_id。
/// 参数里的凭据按命令审批的同一套规则打码，长输出留首尾。
#[test]
fn only_failed_tools_carry_their_command_and_output() {
    let call = willdeep_core::ToolCall {
        id: "call-1".to_owned(),
        name: "run_command".to_owned(),
        arguments: r#"{"command":"deploy --password hunter2"}"#.to_owned(),
    };

    let ok = agent_event_json(AgentEvent::ToolCompleted {
        call: call.clone(),
        output: "fine".to_owned(),
        is_error: false,
    });
    assert_eq!(ok.get("arguments"), None);
    assert_eq!(ok.get("output"), None);

    let background = agent_event_json(AgentEvent::ToolCompleted {
        call: call.clone(),
        output:
            "[background-delay seconds=240]\nBackground job started: job_fixture\nprivate output"
                .into(),
        is_error: false,
    });
    assert_eq!(background["background_delay_seconds"], 240);
    assert_eq!(background.get("output"), None);
    assert!(!background.to_string().contains("private output"));

    let failed = agent_event_json(AgentEvent::ToolCompleted {
        call,
        output: "boom".to_owned(),
        is_error: true,
    });
    let arguments = failed["arguments"].as_str().expect("arguments");
    assert!(arguments.contains("deploy"), "{arguments}");
    assert!(!arguments.contains("hunter2"), "凭据必须打码: {arguments}");
    assert_eq!(failed["output"], "boom");

    // 摘要有上界，且掐掉的部分要说清楚掐了多少。
    let long = "x".repeat(5_000);
    let excerpt = failure_excerpt(&long);
    assert!(excerpt.chars().count() < long.chars().count());
    assert!(excerpt.contains("3000 chars omitted"), "{excerpt}");
    assert_eq!(failure_excerpt("short"), "short");
}

/// `-w` is the short form of `--workspace`, and stays global: it must
/// work before a subcommand, after one, and in the bare TUI form.
#[test]
fn workspace_accepts_the_short_flag_everywhere() {
    let expected = Some(std::path::Path::new("/tmp"));
    for argv in [
        vec!["willdeep", "-w", "/tmp"],
        vec!["willdeep", "-w", "/tmp", "run", "inspect"],
        vec!["willdeep", "run", "-w", "/tmp", "inspect"],
    ] {
        let cli = Cli::try_parse_from(argv.clone())
            .unwrap_or_else(|error| panic!("parse {argv:?}: {error}"));
        assert_eq!(cli.workspace.as_deref(), expected, "for {argv:?}");
    }
    // The long form keeps working unchanged.
    let cli = Cli::try_parse_from(["willdeep", "--workspace", "/tmp"]).expect("long form");
    assert_eq!(cli.workspace.as_deref(), expected);
}

#[test]
fn handoff_subcommands_parse_and_forward_global_flags() {
    let cli = Cli::try_parse_from([
        "willdeep",
        "-p",
        "some-im",
        "--full-auto",
        "handoff",
        "accept",
        "20260917-x",
        "--remote",
        "upstream",
        "--no-run",
    ])
    .expect("accept");
    let Some(CliCommand::Handoff {
        action: handoff_cmd::HandoffAction::Accept(ref accept),
    }) = cli.command
    else {
        panic!("expected handoff accept");
    };
    assert_eq!(accept.branch.as_deref(), Some("20260917-x"));
    assert_eq!(accept.remote, "upstream");
    assert!(accept.no_run);
    assert_eq!(
        handoff_forwarded_args(&cli),
        vec![
            std::ffi::OsString::from("--profile"),
            "some-im".into(),
            "--full-auto".into()
        ]
    );

    let watch = Cli::try_parse_from([
        "willdeep",
        "handoff",
        "watch",
        "--accept",
        "--interval",
        "5",
    ])
    .expect("watch");
    let Some(CliCommand::Handoff {
        action:
            handoff_cmd::HandoffAction::Watch {
                interval,
                accept,
                ref remote,
            },
    }) = watch.command
    else {
        panic!("expected handoff watch");
    };
    assert_eq!((interval, accept, remote.as_str()), (5, true, "origin"));
    assert!(Cli::try_parse_from(["willdeep", "handoff", "list", "--json"]).is_ok());
}

/// Short forms for the arguments typed most often. Each must resolve to
/// the same field as its long form, and they must combine.
#[test]
fn high_frequency_arguments_have_short_forms() {
    let short = Cli::try_parse_from([
        "willdeep",
        "-c",
        "/tmp/x.toml",
        "-p",
        "some-im",
        "-m",
        "glm-5",
        "-w",
        "/tmp",
        "-r",
        "latest",
    ])
    .expect("short forms");
    let long = Cli::try_parse_from([
        "willdeep",
        "--config",
        "/tmp/x.toml",
        "--profile",
        "some-im",
        "--model",
        "glm-5",
        "--workspace",
        "/tmp",
        "--resume",
        "latest",
    ])
    .expect("long forms");

    assert_eq!(short.config, long.config);
    assert_eq!(short.profile, long.profile);
    assert_eq!(short.model, long.model);
    assert_eq!(short.workspace, long.workspace);
    assert_eq!(short.resume, long.resume);
    assert_eq!(short.profile.as_deref(), Some("some-im"));
    assert_eq!(short.model.as_deref(), Some("glm-5"));
    assert_eq!(short.resume.as_deref(), Some("latest"));
}

#[test]
fn resume_without_a_value_defaults_to_the_latest_session() {
    for argv in [
        vec!["willdeep", "-r"],
        vec!["willdeep", "--resume"],
        vec!["willdeep", "-r", "--workspace", "/tmp"],
    ] {
        let cli = Cli::try_parse_from(argv.clone())
            .unwrap_or_else(|error| panic!("parse {argv:?}: {error}"));
        assert_eq!(cli.resume.as_deref(), Some("latest"), "for {argv:?}");
    }
}

#[test]
fn resume_still_accepts_an_explicit_session_id() {
    let session_id = uuid::Uuid::new_v4().to_string();
    for flag in ["-r", "--resume"] {
        let cli = Cli::try_parse_from(["willdeep", flag, session_id.as_str()])
            .unwrap_or_else(|error| panic!("parse {flag}: {error}"));
        assert_eq!(cli.resume.as_deref(), Some(session_id.as_str()));
    }
}

/// Without `--workspace`, tools operate on the directory the user ran
/// the command in — not the home directory, not the config's location.
#[test]
fn workspace_defaults_to_the_current_directory() {
    let cli = Cli::try_parse_from(["willdeep"]).expect("bare invocation");
    assert!(cli.workspace.is_none());

    let resolved = harness::resolve_workspace(&cli, None).expect("resolve workspace");
    let cwd = std::env::current_dir()
        .expect("current directory")
        .canonicalize()
        .expect("canonical current directory");
    assert_eq!(resolved, cwd);
}

#[test]
fn run_subcommand_accepts_global_options_and_explicit_output() {
    let cli = Cli::try_parse_from([
        "willdeep",
        "run",
        "--profile",
        "coding",
        "--workspace",
        ".",
        "--output",
        "json",
        "inspect",
        "this",
    ])
    .unwrap();
    assert_eq!(cli.profile.as_deref(), Some("coding"));
    assert_eq!(cli.workspace.as_deref(), Some(std::path::Path::new(".")));
    let Some(CliCommand::Run(args)) = cli.command else {
        panic!("expected run command");
    };
    assert_eq!(args.prompt, ["inspect", "this"]);
    assert_eq!(args.output, RunOutput::Json);
}

#[test]
fn run_subcommand_rejects_prompt_and_input_together() {
    assert!(Cli::try_parse_from(["willdeep", "run", "--input", "prompt.txt", "inline"]).is_err());
}

#[test]
fn daemon_upgrade_accepts_bounded_wait_and_force_handoff() {
    let cli = Cli::try_parse_from([
        "willdeep",
        "daemon",
        "upgrade",
        "--timeout",
        "45",
        "--force",
    ])
    .unwrap();
    let Some(CliCommand::Daemon {
        action: daemon::DaemonAction::Upgrade { timeout, force },
    }) = cli.command
    else {
        panic!("expected daemon upgrade command");
    };
    assert_eq!(timeout, 45);
    assert!(force);
    assert!(Cli::try_parse_from(["willdeep", "daemon", "upgrade", "--timeout", "0"]).is_err());
    assert!(Cli::try_parse_from(["willdeep", "daemon", "upgrade", "--timeout", "86401"]).is_err());
}

#[test]
fn top_level_session_commands_parse_stable_targets() {
    let id = uuid::Uuid::new_v4();
    for action in ["get", "turns", "stop"] {
        let cli = Cli::try_parse_from(["willdeep", "session", action, &id.to_string()]).unwrap();
        assert!(matches!(cli.command, Some(CliCommand::Session { .. })));
    }
    let cli = Cli::try_parse_from(["willdeep", "session", "list"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(CliCommand::Session {
            action: daemon::SessionAction::List
        })
    ));
}

#[test]
fn doctor_accepts_machine_output_and_global_context_options() {
    let cli = Cli::try_parse_from([
        "willdeep",
        "doctor",
        "--json",
        "--bundle",
        "diagnostics.zip",
        "--workspace",
        ".",
        "--profile",
        "coding",
    ])
    .unwrap();
    assert_eq!(cli.workspace.as_deref(), Some(std::path::Path::new(".")));
    assert_eq!(cli.profile.as_deref(), Some("coding"));
    assert!(matches!(
        cli.command,
        Some(CliCommand::Doctor {
            json: true,
            bundle: Some(ref path)
        })
        if path == std::path::Path::new("diagnostics.zip")
    ));
}

#[test]
fn run_attachments_load_text_and_supported_image_metadata() {
    let root = std::env::temp_dir().join(format!("willdeep-run-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let text_path = root.join("notes.txt");
    std::fs::write(&text_path, "evidence").unwrap();
    let image_path = root.join("pixel.png");
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 3)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    std::fs::write(&image_path, png.into_inner()).unwrap();

    let attachments = load_run_attachments(&[text_path, image_path]).unwrap();
    assert!(matches!(
        &attachments[0],
        willdeep_core::MessageAttachment::Text { content, .. } if content == "evidence"
    ));
    assert!(matches!(
        &attachments[1],
        willdeep_core::MessageAttachment::Image {
            media_type,
            width: 2,
            height: 3,
            ..
        } if media_type == "image/png"
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn run_exit_codes_are_stable_by_failure_domain() {
    assert_eq!(stable_exit_code(&invalid_run_input("bad input")), 2);
    assert_eq!(
        stable_exit_code(&anyhow::Error::new(
            willdeep_core::provider::ProviderError::MissingApiKey
        )),
        3
    );
    assert_eq!(
        stable_exit_code(&anyhow::Error::new(willdeep_core::AgentError::Tool(
            willdeep_core::tools::ToolError::ApprovalDenied("test".to_owned())
        ))),
        4
    );
    assert_eq!(
        stable_exit_code(&anyhow::Error::new(willdeep_core::AgentError::MaxTurns(3))),
        5
    );
    assert_eq!(stable_exit_code(&anyhow::anyhow!("unknown")), 1);
    assert_eq!(
        stable_exit_code(&anyhow::Error::new(HeadlessRuntimeExecutionError(
            daemon::HeadlessRuntimeStatus::Failed(Some(
                willdeep_runtime_protocol::FailureDomain::Provider
            ))
        ))),
        3
    );
    assert_eq!(
        stable_exit_code(&anyhow::Error::new(HeadlessRuntimeExecutionError(
            daemon::HeadlessRuntimeStatus::WaitingApproval
        ))),
        4
    );
}

#[test]
fn run_completion_json_has_one_stable_machine_readable_envelope() {
    let session_id = uuid::Uuid::new_v4();
    let value = completion_json(
        &harness::HarnessOutcome {
            stop_reason: willdeep_core::AgentStopReason::Finished,
            final_text: "done".to_owned(),
            turns: 4,
            compressed: false,
        },
        session_id,
    );
    assert_eq!(value["type"], "completed");
    assert_eq!(value["turns"], 4);
    assert_eq!(value["text"], "done");
    assert_eq!(value["session_id"], session_id.to_string());
    assert_eq!(value["stop_reason"], "finished");
    assert!(
        value["prompt_variant"].is_null(),
        "no variant in this process"
    );
}

#[test]
fn partial_completion_json_preserves_each_stop_reason() {
    use willdeep_core::AgentStopReason;
    for reason in [
        AgentStopReason::BudgetLimited,
        AgentStopReason::MaxTurns,
        AgentStopReason::Incomplete,
    ] {
        let value = completion_json(
            &harness::HarnessOutcome {
                stop_reason: reason,
                final_text: "saved partial work".to_owned(),
                turns: 3,
                compressed: false,
            },
            uuid::Uuid::new_v4(),
        );
        assert_eq!(value["type"], "partial");
        assert_eq!(value["stop_reason"], reason.as_str());
        assert_eq!(value["text"], "saved partial work");
    }
}

#[test]
fn every_supported_shell_generates_a_completion_script() {
    for shell in [
        clap_complete::Shell::Bash,
        clap_complete::Shell::Zsh,
        clap_complete::Shell::Fish,
        clap_complete::Shell::PowerShell,
    ] {
        let mut command = Cli::command();
        let mut output = Vec::new();
        clap_complete::generate(shell, &mut command, "willdeep", &mut output);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("willdeep"), "missing command for {shell:?}");
        assert!(output.len() > 100, "completion too short for {shell:?}");
    }
}

#[test]
fn generated_man_page_documents_run_and_runtime_commands() {
    let mut output = Vec::new();
    clap_mangen::Man::new(Cli::command())
        .render(&mut output)
        .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains(".TH willdeep"));
    assert!(output.contains("Run one non\\-interactive coding\\-agent turn"));
    assert!(output.contains("Manage the persistent local Runtime Daemon"));
    assert!(output.contains("Diagnose local configuration and runtime readiness"));
}

#[test]
fn automatic_api_selection_matches_provider() {
    assert_eq!(
        resolve_dialect(ApiArg::Auto, ProviderKind::Anthropic),
        ApiDialect::AnthropicMessages
    );
    assert_eq!(
        resolve_dialect(ApiArg::Auto, ProviderKind::SomeIm),
        ApiDialect::ChatCompletions
    );
}

#[test]
fn explicit_api_overrides_provider_default() {
    assert_eq!(
        resolve_dialect(ApiArg::Responses, ProviderKind::SomeIm),
        ApiDialect::Responses
    );
}

/// 动作摘要够回答「执行了什么」，又不把路径和凭据带出去。
#[test]
fn tool_detail_answers_what_ran_without_leaking_paths_or_secrets() {
    let detail = tool_detail(
        "run_command",
        r#"{"command":"cargo test --manifest-path /Users/rocky/secret/Cargo.toml"}"#,
    )
    .expect("run_command detail");
    assert_eq!(detail, "cargo test --manifest-path …");
    assert!(
        !detail.contains("/Users/rocky"),
        "路径不下发浏览器: {detail}"
    );

    // 路径出现在任意位置都要省略，不能只看前几个词。
    let flagged = tool_detail(
        "run_command",
        r#"{"command":"cargo --manifest-path=/Users/rocky/Cargo.toml test"}"#,
    )
    .expect("detail");
    assert_eq!(flagged, "cargo --manifest-path=… test");

    // 裸路径参数同样要藏起来。
    let bare =
        tool_detail("run_command", r#"{"command":"ls /Users/rocky/private"}"#).expect("detail");
    assert!(!bare.contains("/Users"), "{bare}");

    // 循环头不能只剩前三个词，否则界面上像半截命令。
    let loop_command = tool_detail(
        "run_command",
        r#"{"command":"for u in a b; do echo $u; done"}"#,
    )
    .expect("loop detail");
    assert_eq!(loop_command, "for u in a b; do echo $u; done");

    // 凭据按命令审批同一套规则打码。
    let secret = tool_detail(
        "run_command",
        r#"{"command":"deploy sk-abcdefghijklmnop0123"}"#,
    )
    .expect("detail");
    assert!(!secret.contains("sk-abcdefghijklmnop0123"), "{secret}");

    // Worker 报职责与标签，这两样本来就是给人看的。
    let worker = tool_detail(
        "spawn_agent",
        r#"{"profile":"tester","label":"run the suite","prompt":"..."}"#,
    )
    .expect("spawn detail");
    assert_eq!(worker, "tester · run the suite");

    // 认不出的工具宁可不显示，也不要漏一段不知道含什么的参数。
    assert_eq!(tool_detail("edit_file", r#"{"path":"/etc/hosts"}"#), None);
}

#[test]
fn agent_event_json_has_stable_runtime_schema() {
    assert_eq!(
        agent_event_json(AgentEvent::TurnStarted { turn: 3 }),
        serde_json::json!({"type": "turn_started", "turn": 3})
    );
    assert_eq!(
        agent_event_json(AgentEvent::AssistantText {
            text: "ready".to_string(),
            model: None,
        }),
        serde_json::json!({"type": "assistant_text", "text": "ready", "model": null})
    );
    assert_eq!(
        agent_event_json(AgentEvent::RouteDecided {
            tier: willdeep_core::RoutingTier::Worker,
            profile: Some("scout".to_owned()),
            confidence: 90,
            auto_dispatched: true,
            reason: "bounded read-only work".to_owned(),
            model: Some("someim-32b".to_owned()),
            fallback: None,
        }),
        serde_json::json!({
            "type": "route_decided",
            "tier": "worker",
            "profile": "scout",
            "confidence": 90,
            "auto_dispatched": true,
            "reason": "bounded read-only work",
            "model": "someim-32b",
            "fallback": null
        })
    );
    assert_eq!(
        agent_event_json(AgentEvent::ModelFallback {
            purpose: "compressor".to_owned(),
            skipped: Some("someim-32b-compressor".to_owned()),
            used: Some("glm-5".to_owned()),
            reason: "someim-32b-compressor: HTTP 502".to_owned(),
        }),
        serde_json::json!({
            "type": "model_fallback",
            "purpose": "compressor",
            "skipped": "someim-32b-compressor",
            "used": "glm-5",
            "reason": "someim-32b-compressor: HTTP 502"
        })
    );
    assert_eq!(
        agent_event_json(AgentEvent::BackgroundShellCompleted {
            id: "job_123".to_owned(),
            status: willdeep_core::BackgroundTaskStatus::TimedOut,
            exit_code: None,
            elapsed_millis: 60_000,
            output_bytes: 512,
        }),
        serde_json::json!({
            "type": "background_shell_completed",
            "id": "job_123",
            "status": "timed_out",
            "exit_code": null,
            "elapsed_millis": 60_000,
            "output_bytes": 512
        })
    );
}

/// 零配置：钥匙决定 Provider，some.im 优先；OpenAI-compatible 没端点不猜。
#[test]
fn provider_is_inferred_from_whichever_key_is_present() {
    assert_eq!(
        provider_from_keys(true, true, true),
        Some(ProviderArg::SomeIm)
    );
    assert_eq!(
        provider_from_keys(false, true, true),
        Some(ProviderArg::Anthropic)
    );
    assert_eq!(
        provider_from_keys(false, false, true),
        Some(ProviderArg::OpenAiCompatible)
    );
    assert_eq!(provider_from_keys(false, false, false), None);
    assert_eq!(default_model(ProviderKind::SomeIm), Some("glm-5"));
    assert_eq!(
        default_model(ProviderKind::Anthropic),
        Some("claude-sonnet-4-5")
    );
    assert_eq!(default_model(ProviderKind::OpenAiCompatible), None);
    assert_eq!(provider_arg_name(ProviderArg::SomeIm), "some-im");
}
