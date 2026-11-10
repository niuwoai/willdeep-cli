use super::*;

/// 会话记着的目标与状态对得上就原样恢复；换了目标从零开始；会话没记
/// 目标（Web）时只停放，要等本轮信封带着同一句目标来才接回。
#[test]
fn persisted_goal_state_is_restored_only_for_the_same_goal() {
    let mut session = willdeep_core::Session::new(std::env::temp_dir(), None, "restore goal");
    let mut saved = willdeep_core::GoalState::new("ship rc7");
    saved.continuations = 11;
    session.goal = Some("ship rc7".to_owned());
    session.goal_state = Some(saved.clone());

    let same = willdeep_core::GoalContinuation::new();
    restore_goal(&same, &session);
    assert_eq!(same.snapshot().unwrap().continuations, 11);

    session.goal = Some("write docs".to_owned());
    let changed = willdeep_core::GoalContinuation::new();
    restore_goal(&changed, &session);
    let state = changed.snapshot().unwrap();
    assert_eq!(
        (state.statement.as_str(), state.continuations),
        ("write docs", 0)
    );

    session.goal = None;
    let web = willdeep_core::GoalContinuation::new();
    restore_goal(&web, &session);
    assert!(
        !web.is_active(),
        "no goal on the session: nothing is activated"
    );
    web.activate("ship rc7", willdeep_core::GoalBudget::default());
    assert_eq!(web.snapshot().unwrap().continuations, 11);

    let mut finished = saved;
    finished.status = willdeep_core::GoalStatus::Complete;
    session.goal = Some("ship rc7".to_owned());
    session.goal_state = Some(finished);
    let reopened = willdeep_core::GoalContinuation::new();
    restore_goal(&reopened, &session);
    assert_eq!(
        reopened.snapshot().unwrap().continuations,
        0,
        "a finished goal restarts from zero when set again"
    );
}

#[test]
fn approval_mode_defaults_to_smart_and_keeps_explicit_selections() {
    for (value, expected) in [
        (None, ApprovalMode::Smart),
        (Some("smart"), ApprovalMode::Smart),
        (Some("strict"), ApprovalMode::Strict),
        (Some("workspace-write"), ApprovalMode::WorkspaceAccess),
        (Some("full-access"), ApprovalMode::FullAccess),
    ] {
        let config: crate::config::ConfigFile = toml::from_str("version = 1\n[agent]\n").unwrap();
        let mut agent = config.agent;
        agent.approval = value.map(str::to_owned);
        assert_eq!(configured_approval_mode(&agent).unwrap(), expected);
    }
}

mod gateway_capabilities {
    use super::*;
    use willdeep_core::provider::capabilities::parse_model_capabilities;

    fn capability(json: &str) -> willdeep_core::provider::capabilities::GatewayModelCapability {
        parse_model_capabilities(format!(r#"{{"data":[{json}]}}"#).as_bytes())
            .unwrap()
            .remove(0)
    }

    #[test]
    fn no_capability_keeps_existing_behaviour() {
        assert_eq!(
            gateway_adjustments(None, ApiDialect::ChatCompletions, false),
            GatewayAdjustments::default()
        );
    }

    #[test]
    fn responses_only_model_switches_dialect_unless_pinned() {
        let only = capability(
            r#"{"id":"m","context_window":2000000,"protocols":{"responses":{"mode":"native","verified":true,"image_input":true,"function_tools":true}}}"#,
        );
        let auto = gateway_adjustments(Some(&only), ApiDialect::ChatCompletions, false);
        assert_eq!(auto.dialect, Some(ApiDialect::Responses));
        // 看图结论跟着切换后的协议走。
        assert_eq!(auto.accepts_images, Some(true));
        assert_eq!(auto.context_window, Some(willdeep_core::CONTEXT_WINDOW_MAX));

        let pinned = gateway_adjustments(Some(&only), ApiDialect::ChatCompletions, true);
        assert_eq!(pinned.dialect, None);
        assert_eq!(pinned.accepts_images, None);
    }

    #[test]
    fn verified_text_only_disables_images_but_unverified_default_does_not() {
        let verified = capability(
            r#"{"id":"m","protocols":{"chat_completions":{"verified":true,"image_input":false}}}"#,
        );
        assert_eq!(
            gateway_adjustments(Some(&verified), ApiDialect::ChatCompletions, false).accepts_images,
            Some(false)
        );
        let unverified = capability(
            r#"{"id":"m","protocols":{"chat_completions":{"verified":false,"image_input":false}}}"#,
        );
        assert_eq!(
            gateway_adjustments(Some(&unverified), ApiDialect::ChatCompletions, false)
                .accepts_images,
            None
        );
    }

    #[test]
    fn context_window_prefers_config_then_gateway_then_model_default() {
        // 档案显式配置压过网关与缺省。
        assert_eq!(
            session_context_window(Some(64_000), Some(1_000_000), "someim-32b"),
            64_000
        );
        // 没配时取网关值，哪怕模型名缺省更大。
        assert_eq!(
            session_context_window(None, Some(32_000), "someim-32b"),
            32_000
        );
        // 都没有时按模型名：someim-* 256K，其余 128K。
        assert_eq!(session_context_window(None, None, "someim-32b"), 262_144);
        assert_eq!(session_context_window(None, None, "glm-5"), 128_000);
    }

    #[test]
    fn api_pinned_ignores_auto() {
        let mut cli = <Cli as clap::Parser>::try_parse_from(["willdeep"]).unwrap();
        assert!(!api_pinned(&cli, None));
        cli.api = Some(ApiArg::Auto);
        assert!(!api_pinned(&cli, None));
        cli.api = Some(ApiArg::Responses);
        assert!(api_pinned(&cli, None));
    }
}

mod fence {
    use super::*;
    use willdeep_core::sandbox::{NetworkPolicy, SandboxPolicy};

    fn home() -> std::path::PathBuf {
        std::path::PathBuf::from(
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .expect("home directory"),
        )
    }

    #[test]
    fn tilde_expands_to_the_home_directory_and_nothing_else() {
        assert_eq!(
            expand_home(std::path::Path::new("~/.cargo/registry")),
            home().join(".cargo/registry")
        );
        assert_eq!(expand_home(std::path::Path::new("~")), home());
        assert_eq!(
            expand_home(std::path::Path::new("/opt/x")),
            std::path::PathBuf::from("/opt/x")
        );
        // `~user` 不是家目录写法，不猜。
        assert_eq!(
            expand_home(std::path::Path::new("~rocky/x")),
            std::path::PathBuf::from("~rocky/x")
        );
    }

    #[test]
    fn toolchain_caches_are_granted_by_default_and_removable() {
        let workspace = std::env::temp_dir();
        let agent = crate::config::AgentSettings::default();
        let roots = sandbox_roots(&agent, &workspace);
        assert!(roots.contains(&home().join(".cargo/registry")));
        assert!(roots.contains(&workspace));

        let trimmed = crate::config::AgentSettings {
            sandbox_toolchain_caches: Some(false),
            sandbox_writable_roots: vec![std::path::PathBuf::from("~/.pyenv")],
            ..Default::default()
        };
        let roots = sandbox_roots(&trimmed, &workspace);
        assert!(!roots.iter().any(|root| root.ends_with(".cargo/registry")));
        assert!(roots.contains(&home().join(".pyenv")));
    }

    #[test]
    fn network_follows_the_tier_unless_configured() {
        let agent = crate::config::AgentSettings::default();
        assert_eq!(
            sandbox_network(&agent, ApprovalMode::Smart),
            NetworkPolicy::Allow
        );
        assert_eq!(
            sandbox_network(&agent, ApprovalMode::Strict),
            NetworkPolicy::Allow
        );
        assert_eq!(
            sandbox_network(&agent, ApprovalMode::WorkspaceAccess),
            NetworkPolicy::Allow
        );
        assert_eq!(
            sandbox_network(&agent, ApprovalMode::ReadOnly),
            NetworkPolicy::Deny
        );
        let deny = crate::config::AgentSettings {
            sandbox_network: Some(crate::config::SandboxNetwork::Deny),
            ..Default::default()
        };
        assert_eq!(
            sandbox_network(&deny, ApprovalMode::Smart),
            NetworkPolicy::Deny
        );
        let allow = crate::config::AgentSettings {
            sandbox_network: Some(crate::config::SandboxNetwork::Allow),
            ..Default::default()
        };
        assert_eq!(
            sandbox_network(&allow, ApprovalMode::WorkspaceAccess),
            NetworkPolicy::Allow
        );
    }

    #[test]
    fn unset_sandbox_follows_backend_availability_and_explicit_false_turns_it_off() {
        let workspace = std::env::temp_dir();
        let agent = crate::config::AgentSettings::default();
        let spec = resolve_sandbox(&agent, ApprovalMode::Smart, &workspace);
        assert_eq!(
            spec.policy.is_enforcing(),
            willdeep_core::sandbox::available()
        );
        assert!(spec.allows_network());

        let off = crate::config::AgentSettings {
            sandbox: Some(false),
            ..Default::default()
        };
        assert_eq!(
            resolve_sandbox(&off, ApprovalMode::Smart, &workspace).policy,
            SandboxPolicy::Off
        );
        assert!(resolve_workspace_sandbox(&off, ApprovalMode::Smart, &workspace).is_none());

        let on = crate::config::AgentSettings {
            sandbox: Some(true),
            ..Default::default()
        };
        let read_only = resolve_sandbox(&on, ApprovalMode::ReadOnly, &workspace);
        assert_eq!(read_only.policy, SandboxPolicy::ReadOnly);
        assert!(!read_only.allows_network());
        if willdeep_core::sandbox::available() {
            let fence = resolve_workspace_sandbox(&on, ApprovalMode::Smart, &workspace)
                .expect("workspace-write fence");
            assert_eq!(fence.policy, SandboxPolicy::WorkspaceWrite);
            assert!(
                fence.allows_network(),
                "workspace-write allows network for judge-reviewed external commands"
            );
        }
    }

    /// 临时目录里的一组 git 仓库；drop 时清掉。
    struct GitScratch(std::path::PathBuf);

    impl GitScratch {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "willdeep-gitdirs-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }

        fn git(&self, dir: &std::path::Path, args: &[&str]) {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "init.defaultBranch=main",
                    "-c",
                    "protocol.file.allow=always",
                ])
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("run git");
            assert!(status.success(), "git {args:?} in {}", dir.display());
        }

        fn repo(&self, name: &str) -> std::path::PathBuf {
            let dir = self.0.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            self.git(&dir, &["init", "-q"]);
            self.git(&dir, &["commit", "-q", "--allow-empty", "-m", "init"]);
            dir
        }
    }

    impl Drop for GitScratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
    }

    #[test]
    fn a_plain_repo_adds_no_git_roots_because_its_git_dir_is_inside() {
        if !git_available() {
            return;
        }
        let scratch = GitScratch::new("plain");
        let repo = scratch.repo("plain");
        assert!(external_git_dirs(&repo).is_empty());
        assert!(external_git_dirs(&scratch.0).is_empty(), "not a repo");
    }

    #[test]
    fn a_submodule_workspace_gets_its_git_dir_under_the_parent() {
        if !git_available() {
            return;
        }
        let scratch = GitScratch::new("submodule");
        scratch.repo("upstream");
        let parent = scratch.repo("parent");
        // 相对路径：Windows 上 canonicalize 得到的 `\\?\` 路径 git 不认；没有 remote 时
        // 相对 URL 按超级项目目录解析。期望路径逐段 join：verbatim 路径里 `/` 不是分隔符。
        scratch.git(
            &parent,
            &["submodule", "add", "-q", "../upstream", "sdk/python"],
        );
        let module = parent.join("sdk").join("python");
        let git_dir = parent
            .join(".git")
            .join("modules")
            .join("sdk")
            .join("python");
        let dirs = external_git_dirs(&module);
        assert_eq!(dirs, vec![git_dir.clone()]);
        let roots = sandbox_roots(&crate::config::AgentSettings::default(), &module);
        assert!(roots.contains(&git_dir));
    }

    #[test]
    fn a_worktree_workspace_gets_its_own_and_the_common_git_dir() {
        if !git_available() {
            return;
        }
        let scratch = GitScratch::new("worktree");
        let main = scratch.repo("main");
        let tree = scratch.0.join("tree");
        // 相对路径与逐段 join 的原因同上一条。
        scratch.git(&main, &["worktree", "add", "-q", "../tree"]);
        let dirs = external_git_dirs(&tree);
        assert_eq!(
            dirs,
            vec![
                main.join(".git").join("worktrees").join("tree"),
                main.join(".git")
            ]
        );
    }

    #[test]
    fn a_subdirectory_workspace_gets_the_repo_git_dir_above_it() {
        if !git_available() {
            return;
        }
        let scratch = GitScratch::new("subdir");
        let repo = scratch.repo("repo");
        let sub = repo.join("crates/a");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(external_git_dirs(&sub), vec![repo.join(".git")]);
    }
}

fn parent_config(kind: ProviderKind, model: &str) -> ProviderConfig {
    ProviderConfig::new(
        kind,
        ApiDialect::ChatCompletions,
        "https://some.im/v1".to_owned(),
        "placeholder".to_owned(),
        model.to_owned(),
    )
}

fn bound_models(
    source: &str,
    kind: ProviderKind,
    parent_model: &str,
) -> Vec<(String, String, u64)> {
    bound_models_following(source, kind, parent_model, None)
}

/// 同 [`bound_models`]，但在解析之后把主模型切到 `switched_to`，看绑定报
/// 出来的是哪个模型。
fn bound_models_following(
    source: &str,
    kind: ProviderKind,
    parent_model: &str,
    switched_to: Option<&str>,
) -> Vec<(String, String, u64)> {
    let file: crate::config::ConfigFile =
        toml::from_str(source).expect("parse tier config fixture");
    let parent = parent_config(kind, parent_model);
    let main_model = willdeep_core::provider::MainModelHandle::new(
        build_provider(parent.clone()).expect("build parent provider"),
    );
    let bindings = resolve_tier_bindings(&file, &parent, &main_model.follower(), kind, 400_000)
        .expect("resolve tier bindings");
    if let Some(model) = switched_to {
        main_model.set_model(model).expect("switch main model");
    }
    bindings
        .into_iter()
        .map(|(tier, binding)| {
            (
                tier.as_str().to_owned(),
                binding
                    .model
                    .or_else(|| willdeep_core::provider::provider_model(binding.provider.as_ref()))
                    .unwrap_or_default(),
                binding.window,
            )
        })
        .collect()
}

#[test]
fn the_expert_fallback_follows_a_model_switch_but_explicit_bindings_do_not() {
    // `/model` 之后，「回落父模型」的专家档必须跟着换；显式绑定的不动。
    let bound = bound_models_following(
        "version = 1\n",
        ProviderKind::Anthropic,
        "opus-5",
        Some("opus-6"),
    );
    assert_eq!(
        bound,
        vec![("expert".to_owned(), "opus-6".to_owned(), 400_000)]
    );
    let pinned = bound_models_following(
        "version = 1\n[worker_tiers.expert]\nmodel = \"opus-5\"\n",
        ProviderKind::Anthropic,
        "opus-5",
        Some("opus-6"),
    );
    assert_eq!(
        pinned,
        vec![("expert".to_owned(), "opus-5".to_owned(), 400_000)]
    );
}

#[test]
fn the_gateway_tier_table_is_wired_not_just_documented() {
    // 0.51 的毛病：`default_hosted_model()` 那张表只有测试引用，运行时
    // 压根没查过它，于是进阶档不换模型、专家档默默用了会话自己的模型。
    let bound = bound_models("version = 1\n", ProviderKind::SomeIm, "glm-5");
    assert_eq!(
        bound,
        vec![
            (
                "advanced".to_owned(),
                "deepseek-v4-flash".to_owned(),
                262_144
            ),
            // 专家档兑现整个会话预算，这正是它要票据的理由。
            ("expert".to_owned(), "gpt-5.6-sol".to_owned(), 400_000),
        ],
        "基础档不该被绑定——那会盖掉用户在 [subagents.*] 里的选择"
    );
}

#[test]
fn other_providers_keep_the_parent_model_for_the_expert_tier() {
    // 别处没有网关那张表里的模型。回落父模型是正交化之前的行为，保住它
    // 至少不会让票据白扣；进阶档则宁可不换，也不偷偷换成一个没人指定的。
    let bound = bound_models("version = 1\n", ProviderKind::Anthropic, "opus-5");
    assert_eq!(
        bound,
        vec![("expert".to_owned(), "opus-5".to_owned(), 400_000)]
    );
}

#[test]
fn explicit_worker_tier_config_outranks_the_gateway_defaults() {
    let bound = bound_models(
        r#"
version = 1

[worker_tiers.standard]
model = "qwen3-32b"
context_window = 131072

[worker_tiers.expert]
model = "opus-5"
context_window = 500000
"#,
        ProviderKind::SomeIm,
        "glm-5",
    );
    assert_eq!(
        bound,
        vec![
            ("standard".to_owned(), "qwen3-32b".to_owned(), 131_072),
            // 没配的档仍然回落网关默认表。
            (
                "advanced".to_owned(),
                "deepseek-v4-flash".to_owned(),
                262_144
            ),
            ("expert".to_owned(), "opus-5".to_owned(), 500_000),
        ]
    );
}

#[test]
fn a_tier_bound_to_a_legacy_alias_still_defers_to_the_relay_prompt() {
    // 用户仍然可以显式指回那七个退役别名。指回去了，网关就还是会 prepend
    // 职责提示词，客户端这时必须闭嘴，否则同一份职责说两遍。
    let file: crate::config::ConfigFile =
        toml::from_str("version = 1\n[worker_tiers.advanced]\nmodel = \"someim-32b-reader\"\n")
            .expect("parse legacy alias config");
    let parent = parent_config(ProviderKind::SomeIm, "glm-5");
    let follow_main =
        willdeep_core::provider::MainModelHandle::new(build_provider(parent.clone()).unwrap())
            .follower();
    let bindings =
        resolve_tier_bindings(&file, &parent, &follow_main, ProviderKind::SomeIm, 400_000)
            .expect("resolve tier bindings");
    let advanced = bindings
        .iter()
        .find(|(tier, _)| *tier == willdeep_core::WorkerTier::Advanced)
        .expect("advanced tier");
    assert!(advanced.1.hosted_job_prompt);
    // 而网关默认的那两个模型都不是托管别名。
    let expert = bindings
        .iter()
        .find(|(tier, _)| *tier == willdeep_core::WorkerTier::Expert)
        .expect("expert tier");
    assert!(!expert.1.hosted_job_prompt);
}

fn compressor_candidates(source: &str, kind: ProviderKind) -> Vec<(String, String, bool)> {
    let file: crate::config::ConfigFile =
        toml::from_str(source).expect("parse compressor config fixture");
    let loaded = crate::config::LoadedConfig { file };
    compressor_candidate_configs(&loaded, &parent_config(kind, "glm-5"), kind)
        .expect("resolve compressor candidates")
        .into_iter()
        .map(|(config, hosted)| (config.base_url.clone(), config.model.clone(), hosted))
        .collect()
}

/// 本地辅助模型曾经可以排在压缩候选链最前面（`[local_model]
/// prefer_for_context_summaries`）。这个能力已经撤掉：键还认，但不许再把本地
/// 端点塞进压缩链——弱模型压坏一次，丢掉的是整段不可逆的长期上下文。
const RETIRED_LOCAL_PREFERENCE: &str = r#"
version = 1

[local_model]
enabled = true
base_url = "http://127.0.0.1:11434/v1"
prefer_for_context_summaries = true
"#;

#[test]
fn the_retired_local_preference_never_reaches_the_compressor_chain() {
    // some.im 会话：只剩托管压缩器，本地端点不在链上。
    let someim = compressor_candidates(RETIRED_LOCAL_PREFERENCE, ProviderKind::SomeIm);
    assert_eq!(
        someim,
        vec![(
            "https://some.im/v1".to_owned(),
            SOMEIM_CONTEXT_COMPRESSOR_MODEL.to_owned(),
            true
        )]
    );

    // 其它 Provider：会话模型兜底由调用方追加，这里一个候选都不该有。
    assert!(
        compressor_candidates(RETIRED_LOCAL_PREFERENCE, ProviderKind::OpenAiCompatible).is_empty()
    );

    // 显式 compressor_model 仍然赢过托管压缩器，本地端点照样进不来。
    let explicit = compressor_candidates(
        &format!("{RETIRED_LOCAL_PREFERENCE}\n[agent]\ncompressor_model = \"glm-5-air\"\n"),
        ProviderKind::SomeIm,
    );
    assert_eq!(
        explicit,
        vec![(
            "https://some.im/v1".to_owned(),
            "glm-5-air".to_owned(),
            false
        )]
    );

    for candidates in [someim, explicit] {
        assert!(
            candidates
                .iter()
                .all(|(base_url, ..)| !base_url.contains("127.0.0.1")),
            "本地辅助端点不得出现在压缩候选链里"
        );
    }
}

/// 主模型在云端、压缩跑本机：显式档案把压缩整套搬走，会话 Provider 不动。
#[test]
fn a_compressor_profile_moves_only_the_summary_endpoint() {
    let candidates = compressor_candidates(
        r#"
version = 1

[agent]
compressor_profile = "local-compressor"

[providers.local-compressor]
provider = "openai-compatible"
api = "chat-completions"
api_base = "http://127.0.0.1:8080/v1"
api_key = "local"
model = "qwen3.5-9b"
context_window = 131072
"#,
        ProviderKind::SomeIm,
    );
    assert_eq!(
        candidates,
        vec![(
            "http://127.0.0.1:8080/v1".to_owned(),
            "qwen3.5-9b".to_owned(),
            false
        )],
        "压缩候选必须整条换成档案端点，而不是只换模型名"
    );
}

#[test]
fn compressor_model_still_overrides_the_profile_model() {
    let candidates = compressor_candidates(
        r#"
version = 1

[agent]
compressor_profile = "local-compressor"
compressor_model = "qwen3.5-4b"

[providers.local-compressor]
provider = "openai-compatible"
api_base = "http://127.0.0.1:8080/v1"
api_key = "local"
model = "qwen3.5-9b"
"#,
        ProviderKind::OpenAiCompatible,
    );
    assert_eq!(
        candidates,
        vec![(
            "http://127.0.0.1:8080/v1".to_owned(),
            "qwen3.5-4b".to_owned(),
            false
        )]
    );

    // 档案只写端点、模型留给 compressor_model 也算数：只配一份本地地址
    // 再单独指模型，是比两处都写更自然的写法。
    let without_model = compressor_candidates(
        r#"
version = 1

[agent]
compressor_profile = "local-compressor"
compressor_model = "qwen3.5-4b"

[providers.local-compressor]
provider = "openai-compatible"
api_base = "http://127.0.0.1:8080/v1"
api_key = "local"
"#,
        ProviderKind::OpenAiCompatible,
    );
    assert_eq!(
        without_model,
        vec![(
            "http://127.0.0.1:8080/v1".to_owned(),
            "qwen3.5-4b".to_owned(),
            false
        )]
    );
}

#[test]
fn an_unknown_compressor_profile_fails_loudly() {
    let file: crate::config::ConfigFile =
        toml::from_str("version = 1\n[agent]\ncompressor_profile = \"missing\"\n")
            .expect("parse compressor fixture");
    let loaded = crate::config::LoadedConfig { file };
    let error = compressor_candidate_configs(
        &loaded,
        &parent_config(ProviderKind::SomeIm, "glm-5"),
        ProviderKind::SomeIm,
    )
    .expect_err("an unknown profile must not silently fall back to the session model");
    assert!(error.to_string().contains("missing"));
}

#[test]
fn complete_compression_settings_preserve_fallback_and_clear_removed_window() {
    let parent = parent_config(ProviderKind::SomeIm, "main-a");
    let main =
        willdeep_core::provider::MainModelHandle::new(build_provider(parent.clone()).unwrap());
    let loaded = LoadedConfig {
        file: toml::from_str(
            r#"version = 1
[agent]
compressor_model = "summary-a"
compressor_context_window = 32768
"#,
        )
        .unwrap(),
    };
    let explicit = build_compression_settings(&loaded, &parent, main.follower()).unwrap();
    assert_eq!(explicit.context_window, Some(32_768));
    assert_eq!(explicit.candidates.len(), 2);
    assert!(!explicit.candidates[0].1);
    assert_eq!(
        explicit.candidates[0].0.ledger_identity().unwrap().model,
        "summary-a"
    );
    main.set_model("main-b").unwrap();
    assert_eq!(
        explicit.candidates[1].0.ledger_identity().unwrap().model,
        "main-b"
    );
    assert_eq!(
        explicit.candidates[0].0.ledger_identity().unwrap().model,
        "summary-a"
    );

    let reset = build_compression_settings(
        &LoadedConfig {
            file: crate::config::ConfigFile::default(),
        },
        &parent,
        main.follower(),
    )
    .unwrap();
    assert_eq!(reset.context_window, None);
    assert_eq!(reset.candidates.len(), 2);
    assert!(reset.candidates[0].1);
    assert_eq!(
        reset.candidates[0].0.ledger_identity().unwrap().model,
        SOMEIM_CONTEXT_COMPRESSOR_MODEL
    );
    assert_eq!(
        reset.candidates[1].0.ledger_identity().unwrap().model,
        "main-b"
    );
    assert_eq!(parent.model, "main-a");
}

#[test]
fn compression_settings_reject_an_unusable_selected_profile() {
    let parent = parent_config(ProviderKind::SomeIm, "main");
    let main =
        willdeep_core::provider::MainModelHandle::new(build_provider(parent.clone()).unwrap());
    for profile in [
        "api_base = 'http://localhost:1/v1'\napi_key = 'fixture'",
        "api_base = 'not-a-url'\napi_key = 'fixture'\nmodel = 'summary'",
        "api_base_env = 'WILLDEEP_TEST_MISSING_COMPRESSOR_ENDPOINT'\napi_key = 'fixture'\nmodel = 'summary'",
    ] {
        let loaded = LoadedConfig {
            file: toml::from_str(&format!(
                "version = 1\n[agent]\ncompressor_profile = 'summary'\n[providers.summary]\nprovider = 'openai-compatible'\n{profile}\n",
            ))
            .unwrap(),
        };
        assert!(build_compression_settings(&loaded, &parent, main.follower()).is_err());
    }
}

/// 摘要预算跟着压缩器走：显式窗口 > 档案的 `context_window` > 会话窗口。
#[test]
fn the_compressor_window_prefers_the_explicit_setting_then_the_profile() {
    let profile: crate::config::ConfigFile = toml::from_str(
        r#"
version = 1

[agent]
compressor_profile = "local-compressor"

[providers.local-compressor]
provider = "openai-compatible"
api_base = "http://127.0.0.1:8080/v1"
api_key = "local"
model = "qwen3.5-9b"
context_window = 65536
"#,
    )
    .expect("parse compressor profile");
    assert_eq!(compressor_context_window(&profile), Some(65_536));

    let explicit: crate::config::ConfigFile = toml::from_str(
        r#"
version = 1

[agent]
compressor_profile = "local-compressor"
compressor_context_window = 32768

[providers.local-compressor]
provider = "openai-compatible"
api_base = "http://127.0.0.1:8080/v1"
api_key = "local"
model = "qwen3.5-9b"
context_window = 65536
"#,
    )
    .expect("parse explicit compressor window");
    assert_eq!(compressor_context_window(&explicit), Some(32_768));

    let unset: crate::config::ConfigFile =
        toml::from_str("version = 1\n").expect("parse minimal config");
    assert_eq!(compressor_context_window(&unset), None);
}

#[test]
fn someim_sessions_judge_with_the_managed_security_guard() {
    assert_eq!(
        default_judge_model(ProviderKind::SomeIm, "someim-auto-flash"),
        "someim-security-guard",
        "a some.im session must reach the gateway's managed safety policy, \
         matching the macOS app"
    );
}

#[test]
fn every_other_provider_reuses_the_session_model() {
    // No second endpoint, no second credential: a judge the session cannot
    // authenticate is a judge that degrades into an approval card.
    assert_eq!(
        default_judge_model(ProviderKind::OpenAiCompatible, "deepseek-v4-flash"),
        "deepseek-v4-flash"
    );
    assert_eq!(
        default_judge_model(ProviderKind::Anthropic, "claude-opus-5"),
        "claude-opus-5"
    );
}
