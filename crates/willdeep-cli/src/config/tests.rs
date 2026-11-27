use super::*;

#[test]
fn automatic_read_only_dispatch_defaults_on_and_preserves_an_explicit_opt_out() {
    let defaults = AgentSettings::default();
    assert!(defaults.runtime_parameters(128).auto_dispatch_read_only);
    let disabled = AgentSettings {
        auto_dispatch_read_only: Some(false),
        ..defaults
    };
    assert!(!disabled.runtime_parameters(128).auto_dispatch_read_only);
}

#[test]
fn goal_budget_settings_map_onto_the_goal_budget() {
    let defaults: ConfigFile = toml::from_str("[agent]\n").unwrap();
    assert_eq!(
        defaults.agent.goal_budget(),
        willdeep_core::GoalBudget::default()
    );
    assert_eq!(defaults.agent.goal_auto_resume, None);
    let configured: ConfigFile = toml::from_str(
        "[agent]\ngoal_token_budget = 2000000\ngoal_wall_clock_minutes = 30\ngoal_max_continuations = 10\ngoal_auto_resume = false\n",
    )
    .unwrap();
    let budget = configured.agent.goal_budget();
    assert_eq!(budget.max_tokens, Some(2_000_000));
    assert_eq!(
        budget.wall_clock,
        Some(std::time::Duration::from_secs(1_800))
    );
    assert_eq!(budget.max_continuations, 10);
    assert_eq!(configured.agent.goal_auto_resume, Some(false));
}

#[test]
fn load_runtime_reuses_last_good_config_after_parse_failure() {
    let root = std::env::temp_dir().join(format!("wd-config-resilient-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("config.toml");
    std::fs::write(&path, "version = 1\n[agent]\nmax_turns = 4\n").unwrap();
    let first = LoadedConfig::load_runtime(Some(&path)).unwrap();
    assert_eq!(first.file.agent.max_turns, Some(4));
    std::fs::write(&path, "version = 1\nnot valid toml [[[\n").unwrap();
    let second = LoadedConfig::load_runtime(Some(&path)).unwrap();
    assert_eq!(second.file.agent.max_turns, Some(4));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn parses_explicit_verification_commands_and_defaults_to_no_contract() {
    let configured: ConfigFile =
        toml::from_str("[agent]\nverification_commands = ['cargo test --workspace']\n").unwrap();
    assert_eq!(
        configured.agent.verification_commands,
        vec!["cargo test --workspace"]
    );
    let defaults: ConfigFile = toml::from_str("[agent]\nmax_turns = 4\n").unwrap();
    assert!(defaults.agent.verification_commands.is_empty());
}

/// 子代理的回合上限与主会话同一个天花板。曾经卡在 32：配置页不校验区间，
/// 存得下更大的值，任务加载时才拒绝。
#[test]
fn subagent_max_turns_shares_the_agent_ceiling() {
    let parsed: ConfigFile =
        toml::from_str("version = 1\n[subagents.implementer]\nmax_turns = 128\n").unwrap();
    assert_eq!(
        parsed.subagents["implementer"].max_turns,
        Some(128),
        "128 is a value the settings panel can store"
    );
    let complaint = validate(&parsed, Path::new("config.toml"))
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(
        !complaint.contains("max_turns"),
        "a storable value must load: {complaint}"
    );

    let over: ConfigFile = toml::from_str(&format!(
        "version = 1\n[subagents.implementer]\nmax_turns = {}\n",
        MAX_TURNS_CEILING + 1
    ))
    .unwrap();
    let error = validate(&over, Path::new("config.toml"))
        .expect_err("above the ceiling is rejected")
        .to_string();
    assert!(
        error.contains(&format!(
            "subagents.implementer.max_turns must be between 1 and {MAX_TURNS_CEILING}"
        )),
        "{error}"
    );
}

/// 轮次上限放到 200 之后，token 预算是唯一的自动闸门：区间必须校验，不写不限。
#[test]
fn agent_token_budget_is_optional_and_range_checked() {
    let unset: ConfigFile = toml::from_str("version = 1\n").unwrap();
    assert_eq!(unset.agent.token_budget, None);

    let parsed: ConfigFile =
        toml::from_str("version = 1\n[agent]\ntoken_budget = 2000000\n").unwrap();
    assert_eq!(parsed.agent.token_budget, Some(2_000_000));
    let complaint = validate(&parsed, Path::new("config.toml"))
        .err()
        .map(|error| error.to_string())
        .unwrap_or_default();
    assert!(!complaint.contains("token_budget"), "{complaint}");

    for bad in ["10", "99999999999"] {
        let parsed: ConfigFile =
            toml::from_str(&format!("version = 1\n[agent]\ntoken_budget = {bad}\n")).unwrap();
        let error = validate(&parsed, Path::new("config.toml"))
            .expect_err("out-of-range budget is rejected")
            .to_string();
        assert!(
            error.contains("agent.token_budget must be between"),
            "{error}"
        );
    }
}

/// 旧写法仍能解析，但必须被点名；正名与不写都不算旧写法。
#[test]
fn legacy_approval_spellings_are_reported_not_silently_mapped() {
    for spelled in ["ask", "request-every-time"] {
        let parsed: ConfigFile =
            toml::from_str(&format!("version = 1\n[agent]\napproval = \"{spelled}\"\n")).unwrap();
        let notes = deprecations(&parsed);
        assert_eq!(
            notes,
            vec![Deprecation::LegacyApproval {
                spelled: spelled.to_owned()
            }]
        );
        assert!(notes[0].summary().contains("legacy spelling of \"strict\""));
    }
    for fine in [
        "version = 1\n",
        "version = 1\n[agent]\napproval = \"smart\"\n",
    ] {
        let parsed: ConfigFile = toml::from_str(fine).unwrap();
        assert!(deprecations(&parsed).is_empty());
    }
}

#[test]
fn parses_multiple_provider_profiles() {
    let parsed: ConfigFile = toml::from_str(
        r#"
version = 1
default_provider = "some-im"

[agent]
max_turns = 30
approval = "smart"

[providers.some-im]
provider = "some-im"
api = "chat-completions"
api_base = "https://some.im/v1"
api_key_env = "SOMEIM_API_KEY"
model = "deepseek-v4-flash"

[providers.anthropic]
provider = "anthropic"
api = "anthropic-messages"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-5"
"#,
    )
    .expect("parse config");
    assert_eq!(parsed.providers.len(), 2);
    assert_eq!(parsed.default_provider.as_deref(), Some("some-im"));
    assert_eq!(parsed.agent.max_turns, Some(30));
    let loaded = LoadedConfig {
        file: parsed,
        mcp_skipped: Vec::new(),
    };
    let selected = loaded
        .select_provider(None)
        .expect("select default")
        .expect("default provider");
    assert_eq!(selected.model.as_deref(), Some("deepseek-v4-flash"));
}

#[test]
fn rejects_unknown_fields() {
    let result = toml::from_str::<ConfigFile>(
        r#"
version = 1
[providers.test]
base_url = "https://example.com/v1"
"#,
    );
    assert!(result.is_err());
}

#[test]
fn safety_judge_defaults_to_on_and_stays_configurable() {
    let default: ConfigFile = toml::from_str("version = 1\n[agent]\napproval = \"smart\"\n")
        .expect("parse minimal config");
    assert_eq!(default.agent.safety_judge, None);
    assert!(
        default.agent.safety_judge.unwrap_or(true),
        "an unset safety_judge must mean the judge is active"
    );

    let configured: ConfigFile =
        toml::from_str("version = 1\n[agent]\nsafety_judge = false\njudge_model = \"glm-5\"\n")
            .expect("parse configured judge");
    assert_eq!(configured.agent.safety_judge, Some(false));
    assert_eq!(configured.agent.judge_model.as_deref(), Some("glm-5"));
}

#[test]
fn a_compressor_profile_moves_compression_without_moving_the_session() {
    let parsed: ConfigFile = toml::from_str(
        r#"
version = 1

[agent]
compressor_profile = "local-compressor"
compressor_context_window = 32768

[providers.local-compressor]
provider = "openai-compatible"
api = "chat-completions"
api_base = "http://127.0.0.1:8080/v1"
api_key_env = "WILLDEEP_LOCAL_KEY"
model = "qwen3.5-9b"
"#,
    )
    .expect("parse compressor profile");
    assert_eq!(
        parsed.agent.compressor_profile.as_deref(),
        Some("local-compressor")
    );
    assert_eq!(parsed.agent.compressor_context_window, Some(32_768));
    validate(&parsed, Path::new("config.toml")).expect("compressor profile validates");
}

#[test]
fn an_unknown_compressor_profile_is_rejected_at_startup() {
    let parsed: ConfigFile =
        toml::from_str("version = 1\n[agent]\ncompressor_profile = \"nope\"\n")
            .expect("parse unknown compressor profile");
    let error = validate(&parsed, Path::new("config.toml"))
        .expect_err("a profile that does not exist must fail, not fall back");
    assert!(error.to_string().contains("nope"));
}

#[test]
fn a_compressor_window_outside_the_supported_range_is_rejected() {
    let parsed: ConfigFile =
        toml::from_str("version = 1\n[agent]\ncompressor_context_window = 1024\n")
            .expect("parse out-of-range compressor window");
    let error = validate(&parsed, Path::new("config.toml"))
        .expect_err("a window smaller than any usable budget must be rejected");
    assert!(error.to_string().contains("compressor_context_window"));
}

#[test]
fn compression_timeouts_keep_the_total_at_least_as_long_as_one_candidate() {
    let accept = |text: &str| {
        let parsed: ConfigFile = toml::from_str(text).expect("parse compression timeouts");
        validate(&parsed, Path::new("config.toml"))
    };
    accept("version = 1\n").expect("unset timeouts use the defaults");
    accept(
        "version = 1\n[agent]\ncompressor_candidate_timeout_seconds = 90\ncompressor_timeout_seconds = 90\n",
    )
    .expect("a total equal to the candidate wait is the shortest that can finish one chunk");

    let too_short = accept(
        "version = 1\n[agent]\ncompressor_candidate_timeout_seconds = 90\ncompressor_timeout_seconds = 60\n",
    )
    .expect_err("the shared budget must cover at least one candidate wait");
    assert!(
        too_short
            .to_string()
            .contains("at least the candidate timeout")
    );

    let below_range = accept("version = 1\n[agent]\ncompressor_candidate_timeout_seconds = 14\n")
        .expect_err("14 seconds is below the candidate range");
    assert!(below_range.to_string().contains("between 15 and 600"));

    accept(
        "version = 1\n[agent]\ncompressor_candidate_timeout_seconds = 600\ncompressor_timeout_seconds = 3600\n",
    )
    .expect("both timeout maxima are valid");
    let above_candidate_cap = accept(
        "version = 1\n[agent]\ncompressor_candidate_timeout_seconds = 601\ncompressor_timeout_seconds = 3600\n",
    )
    .expect_err("a candidate timeout above 600 seconds must be rejected");
    assert!(
        above_candidate_cap
            .to_string()
            .contains("between 15 and 600")
    );

    let above_total_cap = accept("version = 1\n[agent]\ncompressor_timeout_seconds = 3601\n")
        .expect_err("a configured total timeout above 3600 seconds must be rejected");
    assert!(above_total_cap.to_string().contains("at most 3600"));
}

#[test]
fn local_model_defaults_match_the_desktop_auxiliary_policy() {
    let parsed: ConfigFile = toml::from_str("version = 1\n").expect("parse minimal config");
    assert!(!parsed.local_model.enabled);
    assert_eq!(parsed.local_model.base_url, "http://127.0.0.1:11434/v1");
    assert_eq!(parsed.local_model.summary_model, "gemma4:e4b-it-qat");
    assert!(parsed.local_model.prefer_for_titles);
    assert!(parsed.local_model.prefer_for_worker_routing);
}

#[test]
fn retired_context_summary_preference_still_parses_and_is_ignored() {
    // 上下文压缩不再支持本地模型，但写过这一行的 config.toml 不能因此
    // 启动失败：本段是 deny_unknown_fields，删字段等于把旧配置判死。
    for value in ["true", "false"] {
        let parsed: ConfigFile = toml::from_str(&format!(
            "version = 1\n[local_model]\nenabled = true\nprefer_for_context_summaries = {value}\n"
        ))
        .expect("旧配置里的已废弃键必须仍然能解析");
        validate(&parsed, Path::new("config.toml")).expect("已废弃键不该让校验失败");
        assert!(parsed.local_model.enabled, "同段的其它键照旧生效");
    }
}

#[test]
fn enabled_local_model_accepts_domain_lan_and_loopback_endpoints_without_a_key() {
    for base_url in [
        "https://models.home.example/v1",
        "http://192.168.50.20:11434/v1",
        "http://127.0.0.1:11434/v1",
    ] {
        let source =
            format!("version = 1\n[local_model]\nenabled = true\nbase_url = {base_url:?}\n");
        let parsed: ConfigFile = toml::from_str(&source).expect("parse auxiliary model config");
        validate(&parsed, Path::new("config.toml")).expect("accept auxiliary endpoint");
    }

    let parsed: ConfigFile = toml::from_str(
        "version = 1\n[local_model]\nenabled = true\nbase_url = \"file:///tmp/model\"\n",
    )
    .expect("parse invalid auxiliary model config");
    assert!(validate(&parsed, Path::new("config.toml")).is_err());
}

#[test]
fn subagent_sections_resolve_canonical_first_then_legacy_names() {
    let file: ConfigFile = toml::from_str(
        "version = 1\n[subagents.deep]\nmodel = \"d\"\n[subagents.judge]\nmodel = \"j\"\n[subagents.tester]\nmodel = \"t\"\n",
    )
    .expect("parse legacy sections");
    assert_eq!(subagent_section_id(&file, "generalist"), Some("deep"));
    assert_eq!(subagent_section_id(&file, "reviewer"), Some("judge"));
    assert_eq!(subagent_section_id(&file, "tester"), Some("tester"));
    assert_eq!(subagent_section_id(&file, "implementer"), None);
    assert_eq!(
        subagent_settings(&file, "reviewer").and_then(|s| s.model.as_deref()),
        Some("j")
    );

    let both: ConfigFile = toml::from_str(
        "version = 1\n[subagents.generalist]\nmodel = \"g\"\n[subagents.deep]\nmodel = \"d\"\n",
    )
    .expect("parse both sections");
    assert_eq!(subagent_section_id(&both, "generalist"), Some("generalist"));
}

#[test]
fn deep_budget_is_bounded_and_can_be_disabled() {
    let disabled: ConfigFile =
        toml::from_str("version = 1\n[agent]\nmax_deep_calls_per_harness = 0\n")
            .expect("parse disabled deep budget");
    validate(&disabled, Path::new("config.toml")).expect("zero disables deep");

    let excessive: ConfigFile =
        toml::from_str("version = 1\n[agent]\nmax_deep_calls_per_harness = 17\n")
            .expect("parse excessive deep budget");
    assert!(validate(&excessive, Path::new("config.toml")).is_err());
}

#[test]
fn worker_tier_sections_take_only_canonical_names_and_real_providers() {
    let source = r#"
version = 1
default_provider = "some-im"

[providers.some-im]
provider = "some-im"
api_key_env = "SOMEIM_API_KEY"
model = "glm-5"

[providers.anthropic]
provider = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
model = "opus-5"

[worker_tiers.advanced]
model = "deepseek-v4-flash"

[worker_tiers.expert]
provider_profile = "anthropic"
model = "opus-5"
context_window = 400000
"#;
    let parsed: ConfigFile = toml::from_str(source).expect("parse worker tiers");
    validate(&parsed, Path::new("config.toml")).expect("accept worker tiers");
    assert_eq!(parsed.worker_tiers.len(), 2);
    assert_eq!(
        parsed.worker_tiers["expert"].provider_profile.as_deref(),
        Some("anthropic")
    );

    // 段名只认正名。`deep` 做参数值仍然可用，做段名则会和 `expert` 撞成
    // 同一档——与其定一条谁赢的规则，不如直接拒绝。
    for bad in [
        "[worker_tiers.deep]\nmodel = \"opus-5\"\n",
        "[worker_tiers.basic]\nmodel = \"glm-5\"\n",
        "[worker_tiers.expert]\nmodel = \"\"\n",
        "[worker_tiers.expert]\ncontext_window = 2000\n",
        "[worker_tiers.expert]\nprovider_profile = \"nope\"\n",
    ] {
        let parsed: ConfigFile =
            toml::from_str(&format!("version = 1\n{bad}")).expect("parse rejected tier");
        assert!(
            validate(&parsed, Path::new("config.toml")).is_err(),
            "should reject: {bad}"
        );
    }
}

#[test]
fn feedback_defaults_to_hash_only_and_parses_opt_in() {
    let parsed: ConfigFile = toml::from_str("").expect("parse empty");
    assert!(parsed.feedback.enabled);
    assert!(!parsed.feedback.store_text);
    assert_eq!(parsed.feedback.retain_months, 12);
    let parsed: ConfigFile =
        toml::from_str("[feedback]\nenabled = false\nstore_text = true\n").expect("parse");
    assert!(!parsed.feedback.enabled);
    assert!(parsed.feedback.store_text);
}

#[test]
fn input_suggestions_toggle_parses_and_is_unset_by_default() {
    let parsed: ConfigFile =
        toml::from_str("[agent]\ninput_suggestions = false\n").expect("parse toggle");
    assert_eq!(parsed.agent.input_suggestions, Some(false));
    let parsed: ConfigFile = toml::from_str("[agent]\n").expect("parse empty agent");
    assert!(parsed.agent.input_suggestions.is_none());
}

#[test]
fn sandbox_settings_parse_and_relative_roots_are_rejected() {
    // TOML 基本字符串里的反斜杠要转义；`/opt/cache` 在 Windows 上不算绝对路径。
    let absolute_root = if cfg!(windows) {
        r"C:\\opt\\cache"
    } else {
        "/opt/cache"
    };
    let parsed: ConfigFile = toml::from_str(&format!(
        "[agent]\nsandbox_network = \"deny\"\nsandbox_toolchain_caches = false\nsandbox_writable_roots = [\"~/.pyenv\", \"{absolute_root}\"]\n",
    ))
    .expect("parse sandbox keys");
    assert_eq!(parsed.agent.sandbox_network, Some(SandboxNetwork::Deny));
    assert_eq!(parsed.agent.sandbox_toolchain_caches, Some(false));
    assert!(parsed.agent.sandbox.is_none());
    validate(&parsed, Path::new("config.toml")).expect("tilde and absolute roots are fine");

    let relative: ConfigFile =
        toml::from_str("[agent]\nsandbox_writable_roots = [\"cache\"]\n").expect("parse");
    let error = validate(&relative, Path::new("config.toml"))
        .expect_err("relative roots are ambiguous")
        .to_string();
    assert!(error.contains("must be absolute"), "{error}");
    assert!(toml::from_str::<ConfigFile>("[agent]\nsandbox_network = \"maybe\"\n").is_err());
}

#[test]
fn example_config_stays_valid() {
    let parsed: ConfigFile = toml::from_str(include_str!("../../../../config.example.toml"))
        .expect("parse config.example.toml");
    assert!(parsed.mcp_servers.is_empty());
    assert_eq!(parsed.providers.len(), 3);
    assert_eq!(
        parsed.notifications.sound.as_deref(),
        Some("system-default")
    );
    assert_eq!(
        parsed.notifications.webhook_url.as_deref(),
        Some("http://127.0.0.1:8787/willdeep")
    );
    let some_im = parsed.providers.get("some-im").expect("some.im profile");
    assert_eq!(some_im.model.as_deref(), Some("glm-5"));
    assert_eq!(parsed.agent.small_model_routing, Some(true));
    assert_eq!(parsed.agent.auto_dispatch_read_only, Some(true));
    assert_eq!(parsed.agent.max_deep_calls_per_harness, Some(1));
    for hosted in [
        "scout",
        "reader",
        "editor",
        "test_fixer",
        "build_fixer",
        "log_inspector",
        "git_detective",
    ] {
        let profile = parsed.subagents.get(hosted).expect("hosted worker policy");
        assert_eq!(profile.provider_profile, None, "{hosted} provider override");
        assert_eq!(profile.model, None, "{hosted} model override");
    }
    let generalist = parsed
        .subagents
        .get("generalist")
        .expect("generalist profile");
    assert_eq!(generalist.model.as_deref(), Some("deepseek-v4-flash"));
    // 示例只写正名：改名前的 `deep` / `judge` 段仍能读，但不该再被示范。
    for legacy in ["deep", "judge"] {
        assert!(
            !parsed.subagents.contains_key(legacy),
            "示例里不该再写 [subagents.{legacy}]"
        );
    }
    assert!(parsed.subagents.contains_key("reviewer"));
    // 这条测试的名字说的是「stays valid」，那就真的校验一遍——只解析
    // 不校验的话，示例里写错一个 provider_profile 也照样绿。
    validate(&parsed, Path::new("config.example.toml")).expect("example config validates");
    assert_eq!(
        parsed.worker_tiers["expert"].provider_profile.as_deref(),
        Some("anthropic"),
        "示例要示范「专家档单独换端点」这件事"
    );
}

#[test]
fn notification_schema_matches_the_macos_app() {
    let parsed: ConfigFile = toml::from_str(
        r#"
version = 1

[notifications]
sound = "custom"
custom_sound_file = "WillDeep-Custom-Alert.mp3"
custom_sound_display_name = "message.mp3"
webhook_enabled = true
webhook_url = "http://127.0.0.1:8787/willdeep"
webhook_on_task_completed = true
webhook_on_attention_required = false
"#,
    )
    .expect("parse shared notifications section");

    validate(&parsed, Path::new("config.toml")).expect("validate local webhook");
    assert_eq!(parsed.notifications.sound.as_deref(), Some("custom"));
    assert_eq!(parsed.notifications.webhook_enabled, Some(true));
    assert_eq!(
        parsed.notifications.webhook_on_attention_required,
        Some(false)
    );
}

#[test]
fn notifications_tolerate_fields_only_the_desktop_app_knows() {
    // WillDeep.app must be able to ship a new key into the shared file
    // without bricking every CLI that has not been upgraded yet.
    let parsed: ConfigFile = toml::from_str(
        "version = 1\n[notifications]\nsound = \"custom\"\nwebhook_retry_count = 3\n",
    )
    .expect("ignore unknown desktop-only fields");

    assert_eq!(parsed.notifications.sound.as_deref(), Some("custom"));
    validate(&parsed, Path::new("config.toml")).expect("unknown fields stay valid");
}

#[test]
fn unknown_keys_outside_notifications_are_still_rejected() {
    let error = toml::from_str::<ConfigFile>("version = 1\nnot_a_real_key = true\n")
        .expect_err("top-level typos must still fail loudly");
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn enabled_webhook_rejects_non_http_urls() {
    let parsed: ConfigFile = toml::from_str(
        "version = 1\n[notifications]\nwebhook_enabled = true\nwebhook_url = \"file:///tmp/hook\"\n",
    )
    .expect("parse notifications section");

    let error = validate(&parsed, Path::new("config.toml")).expect_err("reject file URL");
    assert!(error.to_string().contains("must use http or https"));
}

fn channels_config(section: &str) -> String {
    format!("version = 1\n[notifications]\n{section}")
}

fn channel_error(section: &str) -> String {
    let parsed: ConfigFile = toml::from_str(&channels_config(section)).expect("parse channels");
    validate(&parsed, Path::new("config.toml"))
        .expect_err("must be rejected")
        .to_string()
}

#[test]
fn every_channel_kind_parses_and_validates() {
    let parsed: ConfigFile = toml::from_str(&channels_config(
        r#"
[[notifications.channels]]
kind = "webhook"
name = "dashboard"
url = "https://example.com/willdeep"
secret_env = "DASHBOARD_SECRET"

[[notifications.channels]]
kind = "lark"
name = "team-bot"
webhook_url = "https://open.feishu.cn/open-apis/bot/v2/hook/xxxx"
events = ["task_completed"]

[[notifications.channels]]
kind = "email"
name = "ops-mail"
from = "WillDeep <bot@example.com>"
to = ["ops@example.com", "oncall@example.com"]
smtp = { host = "smtp.example.com", port = 587, tls = "starttls", username = "bot@example.com", password_env = "SMTP_PASSWORD" }

[[notifications.channels]]
kind = "email"
name = "ops-resend"
from = "bot@example.com"
to = ["ops@example.com"]
resend = { api_key_env = "RESEND_API_KEY" }
"#,
    ))
    .expect("parse every channel kind");

    validate(&parsed, Path::new("config.toml")).expect("validate every channel kind");
    assert_eq!(parsed.notifications.channels.len(), 4);
    let NotificationChannel::Lark { events, .. } = &parsed.notifications.channels[1] else {
        panic!("the second channel is the lark one");
    };
    assert_eq!(
        events,
        &[NotificationEventKind::TaskCompleted],
        "an explicit events list is kept as written"
    );
}

#[test]
fn an_email_channel_must_choose_exactly_one_backend() {
    let both = channel_error(
        r#"
[[notifications.channels]]
kind = "email"
from = "bot@example.com"
to = ["ops@example.com"]
smtp = { host = "smtp.example.com" }
resend = { api_key_env = "RESEND_API_KEY" }
"#,
    );
    assert!(
        both.contains("cannot configure both smtp and resend"),
        "{both}"
    );

    let neither = channel_error(
        r#"
[[notifications.channels]]
kind = "email"
from = "bot@example.com"
to = ["ops@example.com"]
"#,
    );
    assert!(neither.contains("needs either smtp or resend"), "{neither}");
}

#[test]
fn smtp_credentials_need_both_username_and_a_password_env() {
    let error = channel_error(
        r#"
[[notifications.channels]]
kind = "email"
from = "bot@example.com"
to = ["ops@example.com"]
smtp = { host = "smtp.example.com", username = "bot@example.com" }
"#,
    );
    assert!(error.contains("both username and"), "{error}");
}

#[test]
fn plaintext_smtp_credentials_are_only_allowed_on_loopback() {
    let remote = channel_error(
        r#"
[[notifications.channels]]
kind = "email"
from = "bot@example.com"
to = ["ops@example.com"]
smtp = { host = "smtp.example.com", tls = "none", username = "bot@example.com", password_env = "SMTP_PASSWORD" }
"#,
    );
    assert!(remote.contains("plaintext"), "{remote}");

    // 本机中继（MailHog、Postfix on localhost）走明文可以，密码不出这台机器。
    let parsed: ConfigFile = toml::from_str(&channels_config(
        r#"
[[notifications.channels]]
kind = "email"
from = "bot@example.com"
to = ["ops@example.com"]
smtp = { host = "127.0.0.1", port = 1025, tls = "none", username = "bot", password_env = "SMTP_PASSWORD" }
"#,
    ))
    .expect("parse loopback smtp");
    validate(&parsed, Path::new("config.toml")).expect("loopback plaintext is allowed");
}

#[test]
fn channel_events_must_not_be_empty_or_repeat() {
    let empty = channel_error(
        r#"
[[notifications.channels]]
kind = "webhook"
url = "https://example.com/willdeep"
events = []
"#,
    );
    assert!(empty.contains("events is empty"), "{empty}");

    let repeated = channel_error(
        r#"
[[notifications.channels]]
kind = "lark"
webhook_url = "https://open.feishu.cn/open-apis/bot/v2/hook/xxxx"
events = ["task_completed", "task_completed"]
"#,
    );
    assert!(repeated.contains("same event twice"), "{repeated}");
}

#[test]
fn channel_names_must_be_unique_and_mailboxes_must_be_addresses() {
    let duplicate = channel_error(
        r#"
[[notifications.channels]]
kind = "webhook"
name = "ops"
url = "https://example.com/one"

[[notifications.channels]]
kind = "lark"
name = "ops"
webhook_url = "https://open.feishu.cn/open-apis/bot/v2/hook/xxxx"
"#,
    );
    assert!(duplicate.contains("duplicates the name"), "{duplicate}");

    let bad_mailbox = channel_error(
        r#"
[[notifications.channels]]
kind = "email"
from = "not an address"
to = ["ops@example.com"]
resend = { api_key_env = "RESEND_API_KEY" }
"#,
    );
    assert!(
        bad_mailbox.contains("must be an email address"),
        "{bad_mailbox}"
    );
}

#[test]
fn a_non_http_lark_endpoint_is_rejected() {
    let error = channel_error(
        r#"
[[notifications.channels]]
kind = "lark"
webhook_url = "file:///tmp/hook"
"#,
    );
    assert!(error.contains("must use http or https"), "{error}");
}

#[test]
fn redacts_inline_secrets_and_keeps_environment_references() {
    let mut value: toml::Value = toml::from_str(
        r#"
[providers.inline]
api_key = "secret"
model = "model-a"

[providers.environment]
api_key_env = "PROVIDER_API_KEY"

[[notifications.channels]]
kind = "webhook"
url = "https://example.com/willdeep"
secret = "hook-secret"
secret_env = "HOOK_SECRET_ENV"

[[notifications.channels]]
kind = "email"
from = "bot@example.com"
to = ["ops@example.com"]
smtp = { host = "smtp.example.com", username = "bot", password = "smtp-secret", password_env = "SMTP_PASSWORD" }

[[notifications.channels]]
kind = "email"
from = "bot@example.com"
to = ["ops@example.com"]
resend = { api_key = "re-secret", api_key_env = "RESEND_API_KEY" }
"#,
    )
    .unwrap();
    redact_inline_secrets(&mut value);
    assert_eq!(
        value["providers"]["inline"]["api_key"].as_str(),
        Some("[REDACTED]")
    );
    assert_eq!(
        value["providers"]["environment"]["api_key_env"].as_str(),
        Some("PROVIDER_API_KEY")
    );
    assert_eq!(
        value["notifications"]["channels"][0]["secret"].as_str(),
        Some("[REDACTED]")
    );
    assert_eq!(
        value["notifications"]["channels"][0]["secret_env"].as_str(),
        Some("HOOK_SECRET_ENV")
    );
    assert_eq!(
        value["notifications"]["channels"][1]["smtp"]["password"].as_str(),
        Some("[REDACTED]")
    );
    assert_eq!(
        value["notifications"]["channels"][1]["smtp"]["password_env"].as_str(),
        Some("SMTP_PASSWORD")
    );
    assert_eq!(
        value["notifications"]["channels"][2]["resend"]["api_key"].as_str(),
        Some("[REDACTED]")
    );
    assert_eq!(
        value["notifications"]["channels"][2]["resend"]["api_key_env"].as_str(),
        Some("RESEND_API_KEY")
    );
}

#[test]
fn redacts_an_unmigrated_mcp_token_and_keeps_the_env_name() {
    let mut value: toml::Value = toml::from_str(
        r#"
[mcp_servers.panel]
url = "https://panel.example.com/mcp"
token = "inline-secret-value"
bearer_token_env = "PANEL_TOKEN"
"#,
    )
    .unwrap();
    redact_inline_secrets(&mut value);
    assert_eq!(
        value["mcp_servers"]["panel"]["token"].as_str(),
        Some("[REDACTED]")
    );
    assert_eq!(
        value["mcp_servers"]["panel"]["bearer_token_env"].as_str(),
        Some("PANEL_TOKEN")
    );
}

#[test]
fn inline_secrets_are_accepted_for_every_secret_slot() {
    let parsed: ConfigFile = toml::from_str(&channels_config(
        r#"
[[notifications.channels]]
kind = "webhook"
name = "dashboard"
url = "https://example.com/willdeep"
secret = "webhook-secret"

[[notifications.channels]]
kind = "lark"
name = "team-bot"
webhook_url = "https://open.feishu.cn/open-apis/bot/v2/hook/xxxx"
secret = "lark-secret"

[[notifications.channels]]
kind = "email"
name = "ops-mail"
from = "bot@example.com"
to = ["ops@example.com"]
smtp = { host = "smtp.example.com", username = "bot@example.com", password = "smtp-secret" }

[[notifications.channels]]
kind = "email"
name = "ops-resend"
from = "bot@example.com"
to = ["ops@example.com"]
resend = { api_key = "re_secret" }
"#,
    ))
    .expect("parse inline secrets");
    validate_notification_channels(&parsed.notifications.channels)
        .expect("inline secrets are valid");
}

#[test]
fn a_secret_cannot_be_both_inline_and_an_environment_variable() {
    for section in [
        r#"
[[notifications.channels]]
kind = "webhook"
url = "https://example.com/willdeep"
secret = "inline"
secret_env = "DASHBOARD_SECRET"
"#,
        r#"
[[notifications.channels]]
kind = "lark"
webhook_url = "https://open.feishu.cn/open-apis/bot/v2/hook/xxxx"
secret = "inline"
secret_env = "LARK_SECRET"
"#,
        r#"
[[notifications.channels]]
kind = "email"
from = "bot@example.com"
to = ["ops@example.com"]
smtp = { host = "smtp.example.com", username = "bot", password = "inline", password_env = "SMTP_PASSWORD" }
"#,
        r#"
[[notifications.channels]]
kind = "email"
from = "bot@example.com"
to = ["ops@example.com"]
resend = { api_key = "inline", api_key_env = "RESEND_API_KEY" }
"#,
    ] {
        let parsed: ConfigFile = toml::from_str(&channels_config(section)).expect("parse");
        let error = validate_notification_channels(&parsed.notifications.channels)
            .expect_err("both sources must be rejected")
            .to_string();
        assert!(error.contains("cannot define both"), "{error}");
    }
}

#[test]
fn a_blank_inline_secret_is_rejected() {
    let parsed: ConfigFile = toml::from_str(&channels_config(
        r#"
[[notifications.channels]]
kind = "lark"
webhook_url = "https://open.feishu.cn/open-apis/bot/v2/hook/xxxx"
secret = "   "
"#,
    ))
    .expect("parse blank secret");
    let error = validate_notification_channels(&parsed.notifications.channels)
        .expect_err("blank secret must be rejected")
        .to_string();
    assert!(error.contains("secret cannot be empty"), "{error}");
}

#[test]
fn resend_needs_a_key_from_somewhere() {
    let parsed: ConfigFile = toml::from_str(&channels_config(
        r#"
[[notifications.channels]]
kind = "email"
from = "bot@example.com"
to = ["ops@example.com"]
resend = { api_base = "https://api.resend.com" }
"#,
    ))
    .expect("parse missing key");
    let error = validate_notification_channels(&parsed.notifications.channels)
        .expect_err("a key is required")
        .to_string();
    assert!(error.contains("needs api_key or api_key_env"), "{error}");
}

#[cfg(unix)]
#[test]
fn an_inline_notification_secret_requires_a_private_config_file() {
    use std::os::unix::fs::PermissionsExt;

    let root =
        std::env::temp_dir().join(format!("willdeep-config-secret-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("config.toml");
    let contents = channels_config(
        r#"
[[notifications.channels]]
kind = "lark"
webhook_url = "https://open.feishu.cn/open-apis/bot/v2/hook/xxxx"
secret = "inline-secret"
"#,
    );
    std::fs::write(&path, &contents).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let parsed: ConfigFile = toml::from_str(&contents).unwrap();

    let error = validate(&parsed, &path)
        .expect_err("world-readable config must be rejected")
        .to_string();
    assert!(error.contains("chmod 600"), "{error}");
    assert!(!error.contains("inline-secret"));

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    validate(&parsed, &path).expect("0600 config is accepted");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn intake_public_url_is_optional_but_must_be_an_origin() {
    let mut settings = IntakeSettings::default();
    validate_intake(&settings).expect("an empty public_url is fine");

    settings.public_url = "https://intake.example.com".to_owned();
    validate_intake(&settings).expect("an https origin is accepted");

    settings.public_url = "https://intake.example.com/".to_owned();
    validate_intake(&settings).expect("a single trailing slash is accepted");

    settings.public_url = "http://127.0.0.1:9857".to_owned();
    validate_intake(&settings).expect("an http origin with a port is accepted");

    settings.public_url = "https://example.com:443".to_owned();
    validate_intake(&settings).expect("an explicit default port is accepted");

    settings.public_url = "https://例子.测试".to_owned();
    validate_intake(&settings).expect("an internationalized host is accepted");

    for value in [
        "example.com",
        "ftp://example.com",
        "https://example.com/intake",
        "https://example.com?next=1",
        "https://example.com\\",
        "https://user:secret@example.com",
    ] {
        settings.public_url = value.to_owned();
        let error = validate_intake(&settings).expect_err("not a bare origin");
        assert!(error.to_string().contains("public_url"), "{value}: {error}");
    }
}

#[test]
fn intake_tunnel_requires_https_and_a_fixed_loopback_port() {
    let mut settings = IntakeSettings {
        enabled: true,
        ..IntakeSettings::default()
    };
    assert!(!settings.cloudflare_tunnel);
    settings.public_url = "http://example.com".to_owned();
    validate_intake(&settings).unwrap();
    settings.cloudflare_tunnel = true;
    for (listen, url, accepted) in [
        ("127.0.0.1:9857", "", false),
        ("127.0.0.1:9857", "http://example.com", false),
        ("127.0.0.1:9857", "HTTPS://example.com/", true),
        ("[::1]:9857", "https://example.com", true),
        ("0.0.0.0:9857", "https://example.com", false),
        ("127.0.0.1:0", "https://example.com", false),
    ] {
        settings.listen = listen.to_owned();
        settings.public_url = url.to_owned();
        assert_eq!(
            validate_intake(&settings).is_ok(),
            accepted,
            "{listen} {url}"
        );
    }
}

/// 测试里的「绝对路径」必须是当前平台的绝对路径：Windows 上 `/srv/work`
/// 既不是绝对路径也没有盘符，配置校验会拒绝它。
fn platform_absolute(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

#[test]
fn intake_number_bounds_accept_integers_and_floats() {
    let mut file: ConfigFile = toml::from_str(
        "[intake]\n\
         enabled = false\n\
         listen = \"127.0.0.1:9857\"\n\
         [[intake.forms]]\n\
         slug = \"apply\"\n\
         title = \"申请\"\n\
         workspace = \".\"\n\
         session = \"new\"\n\
         prompt = \"收到一份申请\"\n\
         [[intake.forms.fields]]\n\
         name = \"age\"\n\
         label = \"年龄\"\n\
         type = \"number\"\n\
         min = 18\n\
         max = 99.5\n\
         [[intake.forms.fields]]\n\
         name = \"phone\"\n\
         label = \"电话\"\n\
         type = \"tel\"\n",
    )
    .unwrap();
    // 绝对路径按平台取：Windows 上 `/srv/work` 不是绝对路径。
    file.intake.forms[0].workspace = platform_absolute("willdeep-intake-bounds");
    let age = &file.intake.forms[0].fields[0];
    assert_eq!(age.min, Some(18.0));
    assert_eq!(age.max, Some(99.5));
    assert!(file.intake.forms[0].fields[1].min.is_none());
    validate_intake(&file.intake).expect("number bounds and the new field types are accepted");
}

#[test]
fn intake_number_options_only_apply_to_number_fields_and_must_be_ordered() {
    let field = |kind: &str, min: Option<f64>, max: Option<f64>| IntakeField {
        name: "age".to_owned(),
        label: "年龄".to_owned(),
        kind: kind.to_owned(),
        required: false,
        options: Vec::new(),
        allow_decimal: false,
        min,
        max,
    };
    let mut settings = IntakeSettings::default();
    settings.forms.push(IntakeForm {
        slug: "apply".to_owned(),
        title: "申请".to_owned(),
        workspace: platform_absolute("willdeep-intake-number-options"),
        session: "new".to_owned(),
        prompt: "收到一份申请".to_owned(),
        fields: vec![field("number", Some(10.0), Some(1.0))],
    });
    let error = validate_intake(&settings).expect_err("min greater than max");
    assert!(
        error.to_string().contains("min greater than max"),
        "{error}"
    );

    settings.forms[0].fields = vec![field("text", Some(1.0), None)];
    let error = validate_intake(&settings).expect_err("bounds on a text field");
    assert!(error.to_string().contains("number options"), "{error}");

    settings.forms[0].fields = vec![field("number", Some(1.0), Some(10.0))];
    validate_intake(&settings).expect("an ordered range is accepted");
}

#[test]
fn federation_requires_the_intake_listener_and_a_public_address() {
    let mut intake = IntakeSettings::default();
    let mut federation = FederationSettings::default();
    validate_federation(&federation, &intake).expect("disabled federation needs nothing");

    federation.enabled = true;
    let error = validate_federation(&federation, &intake).expect_err("intake must be on");
    assert!(
        error.to_string().contains("requires [intake] enabled"),
        "{error}"
    );

    intake.enabled = true;
    let error = validate_federation(&federation, &intake).expect_err("public_url is required");
    assert!(error.to_string().contains("public_url"), "{error}");

    intake.public_url = "https://intake.example.com".to_owned();
    federation.workspace = "relative/path".to_owned();
    let error = validate_federation(&federation, &intake).expect_err("workspace must be absolute");
    assert!(error.to_string().contains("absolute"), "{error}");

    federation.workspace = platform_absolute("willdeep-federation")
        .display()
        .to_string();
    federation.reply = "sometimes".to_owned();
    let error = validate_federation(&federation, &intake).expect_err("reply must be known");
    assert!(error.to_string().contains("reply"), "{error}");

    federation.reply = "always".to_owned();
    federation.max_auto_turns = 0;
    assert!(validate_federation(&federation, &intake).is_err());

    federation.max_auto_turns = 6;
    validate_federation(&federation, &intake).expect("a complete federation section is accepted");
}

#[test]
fn web_listen_defaults_and_only_accepts_an_ip_and_port() {
    assert_eq!(WebSettings::default().listen, DEFAULT_WEB_LISTEN);

    let mut settings = WebSettings::default();
    for value in ["127.0.0.1:9900", "[::1]:9900"] {
        settings.listen = value.to_owned();
        validate_web(&settings)
            .unwrap_or_else(|error| panic!("{value} is a valid listen: {error}"));
    }
    for value in ["localhost:9900", "127.0.0.1"] {
        settings.listen = value.to_owned();
        let error = validate_web(&settings).expect_err("not IP:PORT");
        assert!(
            error.to_string().contains("web.listen must be IP:PORT"),
            "{value}: {error}"
        );
    }
}

#[test]
fn web_listen_resolution_prefers_explicit_then_the_enabled_service_then_config() {
    let explicit: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
    let enabled: std::net::SocketAddr = "127.0.0.1:2".parse().unwrap();
    let config = "127.0.0.1:3";

    assert_eq!(
        resolve_web_listen(Some(explicit), Some(enabled), config).unwrap(),
        explicit,
        "an explicit address wins over everything"
    );
    assert_eq!(
        resolve_web_listen(None, Some(enabled), config).unwrap(),
        enabled,
        "an enabled service wins over the config"
    );
    assert_eq!(
        resolve_web_listen(None, None, config).unwrap(),
        "127.0.0.1:3".parse().unwrap(),
        "with no enabled service the config address is used"
    );
    let error = resolve_web_listen(None, None, "localhost:3").expect_err("config must be IP:PORT");
    assert!(error.to_string().contains("web.listen"), "{error}");
}
