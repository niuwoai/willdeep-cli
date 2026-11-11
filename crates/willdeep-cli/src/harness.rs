use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use willdeep_core::hooks::HookRegistry;
use willdeep_core::provider::{ApiDialect, ProviderConfig, ProviderKind};
use willdeep_core::tools::ApprovalTrace;
use willdeep_core::{
    Agent, AgentConfig, ApprovalMode, Approver, BackgroundTaskKind, BackgroundTaskRegistry,
    BackgroundTaskStatus, EventSink, ProviderSafetyJudge, RoutingGuard, RoutingPolicy, SafetyJudge,
    SubagentCatalog, ToolRegistry, WebToolConfig, build_provider,
};

use crate::config::{LoadedConfig, ProviderProfile};
use crate::daemon::{AgentCommandWatcher, RuntimeConnection};
use crate::i18n::Language;
use crate::{
    ApiArg, Cli, TerminalApprover, TerminalSink, daemon, model_accepts_images, projects,
    provider_config_from_profile,
};

mod agent_library;
mod agent_team;
mod delegation;
mod verification;
mod worker_profiles;
pub(crate) mod workflows;

#[cfg(test)]
mod ownership_tests;

/// 把三个 Worker 档位各自解析成一个模型绑定。
///
/// 优先级从高到低：
///
/// 1. 用户显式写的 `[worker_tiers.<档>]`——这是调用点的意志，压过一切默认；
/// 2. some.im 网关上这一档的默认模型（[`willdeep_core::WorkerTier::default_hosted_model`]），
///    与 Xedit 同一张表：同一个人换个客户端不该换个 Worker；
/// 3. 专家档在非网关 provider 上回落父模型——别处没有那张表里的模型，回落到
///    会话自己的模型至少还是「更强的那个」；
/// 4. 其余不绑定：只放宽预算，不偷偷换模型。
///
/// 基础档默认不绑定。它已经是工种自己的模型了，再绑一次只会把用户在
/// `[subagents.*]` 里的选择覆盖掉。
fn resolve_tier_bindings(
    file: &crate::config::ConfigFile,
    parent: &ProviderConfig,
    follow_main: &Arc<dyn willdeep_core::provider::Provider>,
    kind: ProviderKind,
    session_window: u64,
) -> Result<Vec<(willdeep_core::WorkerTier, willdeep_core::TierBinding)>> {
    let mut bindings = Vec::new();
    for tier in willdeep_core::WorkerTier::ALL {
        let configured = file.worker_tiers.get(tier.as_str());
        let window = configured
            .and_then(|value| value.context_window)
            .unwrap_or_else(|| match tier {
                // 专家档兑现的是整个会话预算——这一档贵到要票据，就是因为它
                // 把会话窗口整个交给一个 Worker。
                willdeep_core::WorkerTier::Expert => tier.context_budget().max(session_window),
                _ => tier.context_budget(),
            });
        // 专家档在非网关 provider 上、又没配任何东西时回落「会话模型」：跟着
        // `/model` 走，而不是钉死启动时那个。模型名留空，派工时现取。
        if tier == willdeep_core::WorkerTier::Expert
            && kind != ProviderKind::SomeIm
            && configured
                .is_none_or(|value| value.provider_profile.is_none() && value.model.is_none())
        {
            bindings.push((
                tier,
                willdeep_core::TierBinding {
                    provider: follow_main.clone(),
                    hosted_job_prompt: false,
                    model: None,
                    window,
                },
            ));
            continue;
        }
        let mut provider_config =
            match configured.and_then(|value| value.provider_profile.as_deref()) {
                // 这里是 fail-fast：绑了却建不起来（多半是那个 Profile 的密钥
                // 没配）就不让会话启动。静默跳过更糟——票据照扣，档没升，而
                // 用户要等到某次派工完成之后才可能发现。
                Some(name) => provider_config_from_profile(file, name).with_context(|| {
                    format!("resolve worker_tiers.{}.provider_profile", tier.as_str())
                })?,
                None => parent.clone(),
            };
        let explicit_model = configured.and_then(|value| value.model.as_deref());
        let default_model = (kind == ProviderKind::SomeIm
            && tier != willdeep_core::WorkerTier::Standard)
            .then(|| tier.default_hosted_model());
        let Some(model) = explicit_model
            .or(default_model)
            .map(str::to_owned)
            .or_else(|| {
                // 专家档在别的 provider 上没有网关那张表可查，回落父模型——这是
                // 正交化之前的行为，保住它至少不会让票据白扣。
                (tier == willdeep_core::WorkerTier::Expert).then(|| parent.model.clone())
            })
        else {
            // 不绑定：派工时只放宽预算。Agent 会在那一刻发 `ModelFallback`
            // 说明「要了这一档但模型没换」，不再静默。
            continue;
        };
        provider_config.model = model.clone();
        let binding = willdeep_core::TierBinding {
            provider: build_provider(provider_config).with_context(|| {
                format!("initialize {} worker tier model {model}", tier.as_str())
            })?,
            hosted_job_prompt: willdeep_core::hosts_job_prompt(&model),
            model: Some(model),
            window,
        };
        bindings.push((tier, binding));
    }
    Ok(bindings)
}

use crate::model_defaults::{
    SOMEIM_CONTEXT_COMPRESSOR_MODEL, SOMEIM_SECURITY_GUARD_MODEL, SOMEIM_VISION_FALLBACK_MODEL,
};

/// The judge's model when `[agent] judge_model` is unset.
///
/// On some.im that is the dedicated `someim-security-guard` alias, whose
/// safety policy lives on the gateway and can be tightened server-side
/// without shipping a client. This matches the macOS app, so one operator
/// gets the same verdicts from the CLI and from Xedit. Every other provider
/// reuses the session's own model — there is no second endpoint to reach for,
/// and a judge that needs credentials the session does not have is a judge
/// that silently degrades into an approval card.
fn default_judge_model(kind: ProviderKind, session_model: &str) -> String {
    match kind {
        ProviderKind::SomeIm => SOMEIM_SECURITY_GUARD_MODEL.to_owned(),
        _ => session_model.to_owned(),
    }
}

/// Append one approval decision to `~/.willdeep/approvals.jsonl`. This is the
/// audit trail for "why did that command run without asking me" — and the
/// raw material for tuning the static rules. Best-effort: a logging failure
/// must never block a tool call.
pub(crate) fn record_approval_trace(
    path: &Path,
    session_id: Option<uuid::Uuid>,
    trace: &ApprovalTrace,
) {
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    let source = trace.source.as_str();
    let entry = serde_json::json!({
        "at": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or_default(),
        // rc22 起带会话 id：`willdeep audit export` 按它把放行记录归到会话名下。
        // 更早的记录没有这个字段，只能按会话时间窗归入，报告里会标明。
        "session_id": session_id,
        "source": source,
        "detail": trace.detail,
        // The command is stored redacted: this file outlives the session.
        "command": willdeep_core::judge::redact_credentials(&trace.command),
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(path) {
        let _ = writeln!(file, "{entry}");
    }
}

pub(crate) enum HarnessFrontend {
    Terminal {
        json: bool,
        quiet: bool,
    },
    Tui {
        tx: crate::tui::TuiSender,
    },
    Runtime {
        independent_execution: bool,
        connection: Box<RuntimeConnection>,
        sink: Arc<dyn EventSink>,
        workspace_access: Option<crate::daemon::WorkspaceAccess>,
        approval_handle: Option<willdeep_core::SharedApprovalMode>,
        /// 用户插话的收件箱，任务管理器持有另一半；`turn.steer` 送进来的话在
        /// 下一次调模型前注入。
        instruction_inbox: Option<Arc<willdeep_core::AgentInstructionInbox>>,
        allowed_skills: Vec<String>,
        allowed_mcp_servers: Vec<String>,
        /// Runtime-session override captured when the turn was claimed.
        reasoning_level: Option<String>,
        /// 这一轮在用量账本上的出处：在哪执行、谁提交、哪个回合。
        usage_origin: UsageOrigin,
    },
}

/// 不起 Agent 的零散模型调用（Web 端下一句预测、插件页的 AI 请求）的记账
/// 句柄：进程内执行，前端记 `unknown`，全部按辅助请求记。
pub(crate) fn standalone_usage_ledger(
    home: &Path,
    session_id: Option<uuid::Uuid>,
    workspace: Option<&Path>,
) -> willdeep_core::usage_ledger::UsageLedgerScope {
    use willdeep_core::usage_ledger::{
        ClientKind, Execution, UsageLedgerContext, UsageLedgerScope, ledger_dir, shared_sink,
    };
    let mut context = UsageLedgerContext::new(ClientKind::Unknown, Execution::InProcess);
    context.session_id = session_id;
    context.workspace = workspace.map(Path::to_path_buf);
    UsageLedgerScope::new(shared_sink(&ledger_dir(home)), context)
}

/// Runtime 前端在用量账本上的出处。
#[derive(Clone, Debug)]
pub(crate) struct UsageOrigin {
    pub execution: willdeep_core::usage_ledger::Execution,
    /// `tui:<uuid>` / `cli:<uuid>` / `mobile:<uuid>`，见 `RuntimeTask::origin_client`。
    pub origin_client: Option<String>,
    pub turn_id: Option<uuid::Uuid>,
}

/// 这次 harness 的记账上下文。进程内前端按界面定 `client`，Runtime 前端按
/// 提交方的 `origin_client`。
/// 把会话里持久化的目标接回来（long-horizon.v1 RA4 的持久化部分）。
///
/// - 会话记着目标、状态也是同一句且还在进行：原样恢复，预算与清单接着算。
///   daemon 每轮都重建 harness，没有这一步每轮都会从满格预算重新开始。
/// - 会话记着目标、但状态是别的目标或已收尾：按这句目标从零开始。
/// - 会话没记目标（Web 的目标只随每轮的 `<goal>` 信封到达）：只停放，等本轮
///   信封带着同一句目标来再接回；没带就不激活，关掉的目标不会「复活」。
pub(crate) fn restore_goal(
    continuation: &willdeep_core::GoalContinuation,
    session: &willdeep_core::Session,
) {
    let budget = continuation.default_budget();
    let saved = session
        .goal_state
        .clone()
        .filter(|state| state.status == willdeep_core::GoalStatus::Active);
    let goal = session
        .goal
        .as_deref()
        .map(str::trim)
        .filter(|goal| !goal.is_empty());
    match (goal, saved) {
        (Some(goal), Some(state)) if state.statement == goal => continuation.restore(state, budget),
        (Some(goal), _) => continuation.activate(goal, budget),
        (None, Some(state)) => continuation.park(state),
        (None, None) => {}
    }
}

/// 反馈账本的记录者（`[feedback]`，docs/FEEDBACK_LEDGER.md）。出处取自用量
/// 账本的同一份上下文：同一个会话、同一个 Runtime 轮次、同一个前端。
pub(crate) fn feedback_recorder(
    home: &Path,
    settings: &crate::config::FeedbackSettings,
    context: &willdeep_core::usage_ledger::UsageLedgerContext,
) -> willdeep_core::feedback::FeedbackRecorder {
    use willdeep_core::feedback::{FeedbackRecorder, feedback_dir, shared_sink};
    if !settings.enabled {
        return FeedbackRecorder::disabled();
    }
    let client = serde_json::to_value(context.client)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());
    FeedbackRecorder::new(
        shared_sink(&feedback_dir(home)),
        client,
        settings.store_text,
    )
    .with_session(context.session_id)
    .with_turn(context.turn_id.clone())
    .with_prompt_bundle(willdeep_core::prompt_bundle::main_bundle())
}

fn usage_ledger_context(
    frontend: &HarnessFrontend,
    session_id: uuid::Uuid,
    family_session_id: uuid::Uuid,
    workspace: &Path,
) -> willdeep_core::usage_ledger::UsageLedgerContext {
    use willdeep_core::usage_ledger::{ClientKind, Execution, UsageLedgerContext};
    let mut context = match frontend {
        HarnessFrontend::Terminal { .. } => {
            UsageLedgerContext::new(ClientKind::Cli, Execution::InProcess)
        }
        HarnessFrontend::Tui { .. } => {
            UsageLedgerContext::new(ClientKind::Tui, Execution::InProcess)
        }
        HarnessFrontend::Runtime {
            connection,
            usage_origin,
            ..
        } => {
            let (client, instance) =
                ClientKind::parse_origin(usage_origin.origin_client.as_deref());
            let mut context = UsageLedgerContext::new(client, usage_origin.execution);
            context.client_instance = instance;
            context.turn_id = usage_origin.turn_id.map(|id| id.to_string());
            context.task_id = Some(connection.task_id().to_string());
            context
        }
    };
    context.session_id = Some(session_id);
    context.family_session_id = Some(family_session_id);
    context.workspace = Some(workspace.to_path_buf());
    context
}

/// 因为有待处理的运行时事件而开的那一轮，用它当提示词。
///
/// 短，且不带任何事实：真正的内容由内核在 turn 顶部注入，正文经过净化并标明
/// 了来源。把事件正文塞进提示词等于让它绕过那一层。
pub(crate) const KERNEL_WAKE_PROMPT: &str =
    "Runtime events arrived while you were away. Review them and continue.";
const INSTRUCTION_WAKE_PROMPT: &str = "Additional instructions arrived while background work is still running. Read them and respond now; do not wait for background work to finish.";

/// 无头运行等后台结果时的轮询间隔。脱离作业要读磁盘，比进程内查询贵。
const HEADLESS_BACKGROUND_POLL: std::time::Duration = std::time::Duration::from_millis(250);

pub(crate) struct BuiltHarness {
    host_input: bool,
    event_sink: Arc<dyn EventSink>,
    pub subagents: Arc<SubagentCatalog>,
    pub workflows: Arc<workflows::WorkflowService>,
    _workflow_driver: Option<workflows::WorkflowDriverGuard>,
    pub goal: Arc<willdeep_core::GoalContinuation>,
    pub session_id: uuid::Uuid,
    pub agent: Arc<Agent>,
    pub workspace: PathBuf,
    pub skills: Arc<willdeep_core::SkillCatalog>,
    pub background_tasks: Arc<BackgroundTaskRegistry>,
    /// `run_in_background` 命令的落盘作业，已绑定本会话。
    pub detached_jobs: Arc<willdeep_core::DetachedJobStore>,
    /// 宿主事件内核。前端把后台任务、入站通知交给它，主 Agent 在 turn 边界
    /// 收走——两条路只能留一条，否则同一个结果会向模型讲两遍。
    pub kernel: willdeep_core::EventKernel,
    /// 事件日志。前端在状态变化后刷盘，进程重启时由这里恢复。
    pub kernel_store: willdeep_core::kernel_store::KernelStore,
    pub context_window: u64,
    pub provider_config: ProviderConfig,
    pub provider_profile: Option<String>,
    /// Built here so every frontend — TUI, headless run, runtime — shares one
    /// dispatcher and one set of `[notifications]` switches.
    pub notifier: crate::notify::Notifier,
    _command_watcher: Option<AgentCommandWatcher>,
}

pub(crate) struct HarnessOutcome {
    pub stop_reason: willdeep_core::AgentStopReason,
    pub final_text: String,
    pub turns: usize,
    pub compressed: bool,
}

pub(crate) struct RuntimeHarnessOutcome {
    pub stop_reason: willdeep_core::AgentStopReason,
    pub final_text: String,
    pub turns: usize,
    pub session_id: uuid::Uuid,
    pub message_end: usize,
    pub message_generation: u64,
}

#[derive(Default)]
pub(crate) struct ExecutionOptions {
    pub allow_compress_command: bool,
    pub replay_existing_user_message: bool,
    pub ownership: Option<willdeep_core::checkpoint::ClaimedSessionCheckpointSink>,
}

pub(crate) async fn execute_runtime(
    home: &Path,
    request: crate::daemon::SubmitTask,
    connection: RuntimeConnection,
    sink: Arc<dyn EventSink>,
    prepare_execution: impl FnOnce(&willdeep_core::Session) -> Result<()>,
) -> Result<RuntimeHarnessOutcome> {
    let mut preparation = willdeep_core::preparation::PreparationTrace::new("runtime_execution");
    let loaded = LoadedConfig::load(request.config.as_deref())?;
    let language = Language::parse(loaded.file.agent.language.as_deref())?;
    let cli = Cli {
        command: None,
        prompt: Vec::new(),
        config: request.config.clone(),
        profile: request.profile.clone(),
        api_base: None,
        api_key: None,
        model: request.model.clone(),
        provider: None,
        api: None,
        workspace: Some(request.workspace.clone()),
        web_workspaces: Vec::new(),
        full_auto: false,
        max_turns: None,
        max_output_tokens: None,
        json: true,
        resume: request
            .execution_session_id
            .or(request.session_id)
            .map(|id| id.to_string()),
        new_session: false,
        session_startup: None,
        list_sessions: false,
        list_projects: false,
        project: None,
        list_approvals: false,
        clear_approvals: false,
        no_tui: true,
        onboarding: false,
        web: false,
        listen: None,
        language: None,
        web_input_json: false,
    };
    let execution_session_id = request.execution_session_id.or(request.session_id);
    let store = willdeep_core::SessionStore::new(&if request.execution_session_id.is_some() {
        home.join("independent")
    } else {
        home.to_path_buf()
    });
    let ownership = execution_session_id
        .map(|session_id| {
            willdeep_core::checkpoint::SessionCheckpointSink {
                store: store.clone(),
                session_id,
            }
            .claim()
        })
        .transpose()?;
    // 带基线加载：这份会话接下来会被改消息再写回，基线必须记着改动前的执行状态。
    let resumed = execution_session_id
        .map(|id| store.load_baselined(id))
        .transpose()?;
    if let Some(session) = &resumed {
        prepare_execution(session)?;
    }
    preparation.mark("session_load_and_claim");
    let built = build(
        &cli,
        &loaded,
        home,
        language,
        resumed.as_ref(),
        HarnessFrontend::Runtime {
            independent_execution: request.execution_session_id.is_some(),
            connection: Box::new(connection),
            sink,
            workspace_access: request.workspace_access,
            approval_handle: request.approval_handle.clone(),
            instruction_inbox: request.instruction_inbox.clone(),
            allowed_skills: request.workspace_skills.unwrap_or_default(),
            allowed_mcp_servers: request.workspace_mcp_servers.unwrap_or_default(),
            reasoning_level: request.reasoning_level.clone(),
            usage_origin: UsageOrigin {
                execution: willdeep_core::usage_ledger::Execution::Daemon,
                origin_client: request.origin_client.clone(),
                turn_id: request.turn_id,
            },
        },
    )
    .await?;
    preparation.mark("build_harness");
    let mut session = resumed.unwrap_or_else(|| {
        let mut session = willdeep_core::Session::new(
            built.workspace.clone(),
            request.profile.clone(),
            &request.prompt,
        );
        session.id = built.session_id;
        session.profile = built.provider_profile.clone();
        session.model = Some(built.provider_config.model.clone());
        session
    });
    // Persist the resolved model for new or explicitly reconfigured sessions.
    // Old unbound sessions continue through the disk-only resolver above.
    if session.profile.is_some() || request.profile.is_some() {
        session.profile = built.provider_profile.clone();
        session.model = Some(built.provider_config.model.clone());
    }
    session.magpie_integration = built.provider_config.magpie_integration;
    if session.config.is_none() {
        session.config = request.config.clone();
    }
    if let Some(control) = request.team_control {
        if session.delegation_read_only {
            bail!("delegated read-only sessions cannot execute team control");
        }
        let _ownership = ownership;
        return agent_team::execute(&built, &store, &mut session, control, language).await;
    }
    preparation.mark("session_binding");
    drop(preparation);
    let outcome = execute_noninteractive(
        &built,
        &store,
        &mut session,
        request.prompt,
        request.attachments,
        language,
        ExecutionOptions {
            allow_compress_command: true,
            replay_existing_user_message: request.replay_existing_user_message,
            ownership,
        },
    )
    .await?;
    Ok(RuntimeHarnessOutcome {
        stop_reason: outcome.stop_reason,
        final_text: outcome.final_text,
        turns: outcome.turns,
        session_id: session.id,
        message_end: session.messages.len(),
        message_generation: session.compression_generation,
    })
}

pub(crate) async fn execute_noninteractive(
    built: &BuiltHarness,
    store: &willdeep_core::SessionStore,
    session: &mut willdeep_core::Session,
    prompt: String,
    attachments: Vec<willdeep_core::MessageAttachment>,
    language: Language,
    options: ExecutionOptions,
) -> Result<HarnessOutcome> {
    let mut preparation = willdeep_core::preparation::PreparationTrace::new("turn_persistence");
    let checkpointer = match options.ownership {
        Some(ownership) => ownership,
        None => willdeep_core::checkpoint::SessionCheckpointSink {
            store: store.clone(),
            session_id: session.id,
        }
        .claim()?,
    };
    store.refresh_execution(session)?;
    preparation.mark("claim_and_refresh");
    if options.allow_compress_command && prompt.trim() == "/compress" {
        let result = built
            .agent
            .compress_history_recorded(session.messages.clone(), &mut |usage| {
                store
                    .record_compression_usage(session.id, usage)
                    .map_err(|error| willdeep_core::AgentError::Checkpoint(error.to_string()))
            })
            .await;
        store.refresh_execution(session)?;
        let messages = result?;
        let changed = session.replace_with_compressed_messages(messages);
        store.save(session)?;
        return Ok(HarnessOutcome {
            stop_reason: willdeep_core::AgentStopReason::Finished,
            final_text: language
                .text(
                    if changed {
                        "上下文已压缩"
                    } else {
                        "当前上下文较短，无需压缩"
                    },
                    if changed {
                        "Context compressed"
                    } else {
                        "Context is too short to compress"
                    },
                    if changed {
                        "コンテキストを圧縮しました"
                    } else {
                        "コンテキストが短いため圧縮は不要です"
                    },
                )
                .to_owned(),
            turns: 0,
            compressed: true,
        });
    }
    // L1：提交那一刻就把占位标题换掉。历史列表要在轮次跑完之前就可读——
    // 一条正在跑的会话恰恰是人最可能去列表里找的那条。
    if crate::titling::apply_derived_title(session, &prompt, !attachments.is_empty()) {
        store.save(session)?;
    }
    preparation.mark("derived_title");
    let (history, user_message) = if options.replay_existing_user_message {
        let user_message = session
            .messages
            .last()
            .cloned()
            .context("recovered Turn is missing its persisted user message")?;
        (
            session.messages[..session.messages.len() - 1].to_vec(),
            user_message,
        )
    } else {
        let mut history = session.messages.clone();
        if let Some(notice) = session
            .execution_checkpoint
            .as_ref()
            .and_then(willdeep_core::checkpoint::CheckpointMetadata::recovery_notice)
        {
            history.push(notice);
        }
        let user_message = if built.host_input {
            willdeep_core::Message::host_instruction(&prompt)
        } else {
            willdeep_core::Message::user_with_attachments(&prompt, attachments)
        };
        session.messages.push(user_message.clone());
        store.save(session)?;
        (history, user_message)
    };
    // 一次 AI 回合的遥测：供应商类型、模型、耗时、token、结构化错误码。
    // 提示词与回复正文一个字都不上报（协议里也没有能装下它们的字段）。
    let turn_telemetry = crate::telemetry::TurnTelemetry::start(
        built.provider_config.kind,
        &built.provider_config.base_url,
        &built.provider_config.model,
        !built.provider_config.api_key.is_empty(),
    );
    preparation.mark("user_message");
    drop(preparation);
    let run_result = built
        .agent
        .run_checkpointed_with_summary(
            history,
            user_message,
            Some(&checkpointer),
            session.context_summary.clone(),
        )
        .await;
    // Synchronize even on error: files may already have changed during this run.
    *session = store.load_baselined(session.id)?;
    match &run_result {
        Ok(outcome) => turn_telemetry.finish(
            crate::telemetry::global(),
            Ok((outcome.input_tokens, outcome.output_tokens)),
        ),
        Err(error) => turn_telemetry.finish(crate::telemetry::global(), Err(error)),
    }
    let mut outcome = run_result?;
    present_partial_outcome(&mut outcome, language);
    session.messages = outcome.messages.clone();
    // 摘要随消息一起落盘：下一轮直接复用它，不再把整段历史按块重摘。
    session.context_summary = outcome.context_summary.clone();
    store.save(session)?;
    let mut waiting_background = false;
    loop {
        // 「还在跑」必须在取结果**之前**看：任务先改状态、再投结果（监视器先发
        // 结束事件、再改状态），反过来看会在两步之间读到「没在跑、也没结果」，
        // 于是提前退出，最后那条结论永远到不了模型。
        let running = built.background_tasks.has_pending_tasks()
            || built.agent.workflows_pending()
            || crate::detached_delivery::has_running_jobs(&built.detached_jobs, session.id);
        let events = built.background_tasks.drain_pending();
        // run_in_background 的命令是脱离作业，不在进程内注册表里：两边都要看，
        // 否则无头运行会在命令还没跑完时就退出，模型永远见不到结论。
        let finished_jobs = crate::detached_delivery::publish_finished_jobs(
            &built.kernel,
            &built.detached_jobs,
            session.id,
        );
        if built.agent.reconcile_plan_jobs(&mut session.current_plan) {
            store.save(session)?;
            if let Some(plan) = &session.current_plan {
                built
                    .event_sink
                    .emit(willdeep_core::AgentEvent::PlanUpdated(plan.clone()))
                    .await;
            }
        }
        // 监视器的事件已经在内核里了。被节流的那些只入队，不算唤醒理由；它们
        // 随下一次唤醒（最迟是结束事件）一起交给模型。
        let monitor_wake = built.background_tasks.take_monitor_signals() > 0
            && built.kernel.pending_wake_authority(session.id).is_some();
        // S/turn.steer must also wake a parent whose model loop has already
        // returned. Leave the inbox intact until run_checkpointed consumes it;
        // task cancellation must still be able to hand undelivered text back.
        let instruction_wake = built
            .agent
            .instruction_inbox()
            .is_some_and(|inbox| inbox.has_pending());
        let workflow_wake = built.agent.workflow_reports_pending()?;
        if events.is_empty()
            && finished_jobs == 0
            && !monitor_wake
            && !instruction_wake
            && !workflow_wake
        {
            if !running {
                break;
            }
            if !waiting_background {
                built
                    .event_sink
                    .emit(willdeep_core::AgentEvent::BackgroundWait { waiting: true })
                    .await;
                waiting_background = true;
            }
            tokio::time::sleep(HEADLESS_BACKGROUND_POLL).await;
            continue;
        }
        // 后台结果走事件内核，不再一条通知起一轮。
        //
        // 两个好处：同时结束的几个任务合成一轮交给模型，而不是逼它把同一段
        // 上下文重读三遍；正文按来源净化过，Worker 的自述不会冒充系统消息。
        // 这里**不再**把 notice 当提示词——那样等于同一个结果讲两遍。
        for event in events {
            built.kernel.publish(
                willdeep_core::kernel::background_task_event(
                    session.id,
                    &event.snapshot,
                    event.notice,
                ),
                willdeep_core::DedupPolicy::Once,
            );
        }
        built
            .event_sink
            .emit(willdeep_core::AgentEvent::BackgroundWait { waiting: false })
            .await;
        waiting_background = false;
        outcome = built
            .agent
            .run_checkpointed_with_summary(
                session.messages.clone(),
                willdeep_core::Message::host_instruction(if instruction_wake {
                    INSTRUCTION_WAKE_PROMPT
                } else {
                    KERNEL_WAKE_PROMPT
                }),
                Some(&checkpointer),
                session.context_summary.clone(),
            )
            .await?;
        *session = store.load_baselined(session.id)?;
        session.messages = outcome.messages.clone();
        session.context_summary = outcome.context_summary.clone();
        store.save(session)?;
        willdeep_core::kernel_store::flush(&built.kernel, &built.kernel_store);
    }
    // L2：第一轮问答落地后跑一次摘要。放在通知之前，好让 webhook 带上的是
    // 整理过的标题而不是提示词前缀。
    if crate::titling::apply_summarized_title(&built.agent, session).await {
        store.save(session)?;
    }
    // A headless run is exactly the case where nobody is watching the
    // terminal, so this is the ping that matters most — and the one that has to
    // be awaited: this function returns straight into process teardown, and a
    // detached delivery is dropped along with the runtime.
    built
        .notifier
        .set_session(&session.id.to_string(), Some(session.title.as_str()));
    built.notifier.task_stopped(&outcome);
    built.notifier.flush().await;
    if let Some(error) = built.notifier.take_error() {
        // There is no TUI notice line on this path, so stderr is the only way
        // the failure is not swallowed. stdout stays clean for --output json.
        eprintln!(
            "{}: {error}",
            language.text(
                "通知投递失败",
                "Notification delivery failed",
                "通知の送信に失敗しました"
            )
        );
    }
    Ok(HarnessOutcome {
        stop_reason: outcome.stop_reason,
        final_text: outcome.final_text,
        turns: outcome.turns,
        compressed: false,
    })
}

pub(crate) fn present_partial_outcome(
    outcome: &mut willdeep_core::AgentOutcome,
    language: Language,
) {
    use willdeep_core::AgentStopReason;
    let notice = match outcome.stop_reason {
        AgentStopReason::Finished | AgentStopReason::GoalComplete => return,
        AgentStopReason::MaxTurns => language.text(
            "⚠ 轮次上限 {n} 已用尽，任务未收敛。改动已落盘但可能不完整；继续对话可接着做。",
            "⚠ The {n}-turn limit was reached. The task is incomplete; saved changes can be continued.",
            "⚠ ターン上限 {n} に達しました。タスクは未完了です。保存済みの変更から続行できます。",
        ),
        AgentStopReason::Incomplete => language.text(
            "⚠ 模型输出未完整结束。以下为部分结果，不能视为任务完成；未完整返回的工具调用没有执行。",
            "⚠ The provider response is incomplete. This is a partial result, not task completion. Incomplete tool calls were not executed.",
            "⚠ モデルの応答が未完了です。以下は部分的な結果です。未完了のツール呼び出しは実行していません。",
        ),
        AgentStopReason::Unverified => language.text(
            "⚠ 当前改动缺少有效的通过验证记录（检查未通过，或检查运行期间工作区被改动），任务仅部分完成。",
            "⚠ Current changes lack valid passing verification (checks failed, or the workspace changed while they ran); the task is only partially complete.",
            "⚠ 現在の変更には有効な検証成功記録がありません（チェック失敗、または実行中にワークスペースが変更）。タスクは部分完了です。",
        ),
        AgentStopReason::BudgetLimited => language.text(
            "⚠ 执行预算已用尽，目标尚未完成。以下为交接信息。",
            "⚠ The execution budget was exhausted; the goal remains incomplete. Handover follows.",
            "⚠ 実行予算に達しました。目標は未完了です。以下は引き継ぎ情報です。",
        ),
    }.replace("{n}", &outcome.turns.to_string());
    outcome
        .messages
        .push(willdeep_core::Message::assistant(&notice, Vec::new()));
    outcome.final_text = if outcome.final_text.trim().is_empty() {
        notice
    } else {
        format!("{notice}\n\n{}", outcome.final_text)
    };
}

pub(crate) async fn build(
    cli: &Cli,
    loaded: &LoadedConfig,
    home: &Path,
    language: Language,
    resumed: Option<&willdeep_core::Session>,
    frontend: HarnessFrontend,
) -> Result<BuiltHarness> {
    let web_child_proposals = matches!(&frontend, HarnessFrontend::Runtime { usage_origin, .. }
        if usage_origin.origin_client.as_deref().is_some_and(|origin| origin == "web" || origin.starts_with("web:")));
    let mut preparation = willdeep_core::preparation::PreparationTrace::new("harness");
    // Resolve before gateway/provider requests. Invalid explicit profiles never
    // silently run with legacy defaults.
    let mut effective = LoadedConfig {
        file: loaded.file.clone(),
    };
    if let Some(parameters) = willdeep_core::runtime_parameters::RuntimeParameters::load(home)
        .map_err(anyhow::Error::msg)?
    {
        let settings = &mut effective.file.agent;
        settings.max_turns = Some(parameters.max_turns);
        settings.token_budget = parameters.token_budget;
        settings.goal_token_budget = parameters.goal_token_budget;
        settings.goal_wall_clock_minutes = Some(parameters.goal_wall_clock_minutes);
        settings.goal_max_continuations = Some(parameters.goal_max_continuations);
        settings.input_suggestions = Some(parameters.input_suggestions);
        settings.small_model_routing = Some(parameters.small_model_routing);
        settings.auto_dispatch_read_only = Some(parameters.auto_dispatch_read_only);
        settings.max_deep_calls_per_harness = Some(parameters.max_deep_calls_per_harness);
    }
    if let HarnessFrontend::Runtime { usage_origin, .. } = &frontend
        && let Some(policy) =
            crate::daemon::schedule_policy(home, usage_origin.origin_client.as_deref())?
    {
        if let Some(max_turns) = policy.max_turns {
            effective.file.agent.max_turns = Some(max_turns as usize);
        }
        if let Some(token_budget) = policy.token_budget {
            effective.file.agent.token_budget = Some(token_budget);
        }
    }
    let loaded = &effective;
    let max_turns = cli
        .max_turns
        .or(loaded.file.agent.max_turns)
        .unwrap_or(crate::config::DEFAULT_MAX_TURNS);
    let parameters = loaded.file.agent.runtime_parameters(max_turns);
    parameters.validate().map_err(anyhow::Error::msg)?;
    let session_id = resumed
        .map(|session| session.id)
        .unwrap_or_else(uuid::Uuid::new_v4);
    preparation.mark("runtime_parameters");
    let selection = crate::magpie_integration::resolve(cli, loaded, resumed).await?;
    preparation.mark("provider_selection");
    let provider_profile = selection.name;
    let selected_profile = selection.profile;
    let profile = selected_profile.as_ref();
    let mut provider_config = selection.config;
    let configured_dialect = provider_config.dialect;
    // 网关能力表（some.im / tokenhub 的 /v1/model-capabilities）：取不到时一切照旧。
    let gateway = gateway_adjustments(
        willdeep_core::provider::capabilities::load_model_capability(home, &provider_config)
            .await
            .as_ref(),
        configured_dialect,
        api_pinned(cli, profile),
    );
    preparation.mark("gateway_capabilities");
    if let Some(dialect) = gateway.dialect {
        provider_config.dialect = dialect;
    }
    let kind = provider_config.kind;
    let model = provider_config.model.clone();
    // 普通 OpenAI 兼容端点没有能力表扩展，但 vLLM / OpenRouter 会在
    // `/v1/models` 里报每个模型的真实窗口。用户没显式钉死窗口时用它兜底，
    // 免得启动时按 128K 估算、第一次请求就吃服务端的 400。
    let server_window = if kind == ProviderKind::OpenAiCompatible
        && provider_config.context_window.is_none()
        && gateway.context_window.is_none()
        && resumed
            .and_then(|session| session.model_context_window)
            .is_none()
    {
        willdeep_core::provider::model_metadata::load_server_context_window(home, &provider_config)
            .await
    } else {
        None
    };
    preparation.mark("server_models");
    let context_window = resumed
        .and_then(|session| session.model_context_window)
        .unwrap_or_else(|| {
            session_context_window(
                provider_config.context_window,
                gateway.context_window,
                &model,
            )
        });
    // 服务端自报的窗口是上限：用户没写、网关没报时以它为准；写了更大的值也
    // 不能发出去——那个 400 不重试，整轮直接失败。
    let context_window = server_window.map_or(context_window, |server| {
        context_window.min(server.clamp(
            willdeep_core::CONTEXT_WINDOW_MIN,
            willdeep_core::CONTEXT_WINDOW_MAX,
        ))
    });
    provider_config.context_window = Some(context_window);
    let mut primary_provider_config = provider_config.clone();
    if let HarnessFrontend::Runtime {
        reasoning_level: Some(level),
        ..
    } = &frontend
    {
        primary_provider_config.reasoning_level = primary_provider_config
            .reasoning
            .as_ref()
            .filter(|reasoning| reasoning.levels.contains_key(level))
            .map(|_| level.clone());
    }
    // 缺省值与上限见 config.rs：命令行、配置、缺省三级取值，同一把尺子校验。
    let workspace = resolve_workspace(cli, resumed)?;
    let delegation_read_only = resumed.is_some_and(|session| session.delegation_read_only);
    // A child may be read-only or a writable isolated worktree, but it never
    // gains the parent's full access and never creates further child sessions.
    let delegation_child = resumed.is_some_and(|session| session.parent_session_id.is_some());
    let family_root = resumed
        .and_then(|session| session.parent_session_id)
        .unwrap_or(session_id);
    let team_store = willdeep_core::subagent::AgentRunStore::new(home.join("workers"), session_id)
        .with_family(family_root);
    // A child freezes the root's already-persisted policy instead of each new
    // Session getting a fresh concurrency, queue and budget allowance.
    let team_settings = if delegation_child {
        willdeep_core::subagent::AgentRunStore::new(home.join("workers"), family_root)
            .effective_settings()
            .map_err(anyhow::Error::msg)?
    } else {
        loaded.file.agent_team.clone()
    };
    if !delegation_read_only {
        team_store
            .configure_team(team_settings.clone())
            .map_err(anyhow::Error::msg)?;
    }
    // 本机用量账本（docs/USAGE_LEDGER.md）。主回合、子 Agent、压缩由 Agent 在
    // 主循环里记；这里只把辅助用途的 Provider 包上按调用记账——被主循环用的
    // Provider 不能再包，否则同一次调用会记两行。
    let usage_ledger = willdeep_core::usage_ledger::UsageLedgerScope::new(
        willdeep_core::usage_ledger::shared_sink(&willdeep_core::usage_ledger::ledger_dir(home)),
        usage_ledger_context(&frontend, session_id, family_root, &workspace),
    );
    let usage_ledger = if delegation_read_only {
        usage_ledger
    } else {
        usage_ledger.with_agent_team(team_store.clone())
    };
    let feedback = feedback_recorder(home, &loaded.file.feedback, usage_ledger.context())
        .with_runtime_parameters(parameters);
    let web_tools = (kind == ProviderKind::SomeIm).then(|| WebToolConfig {
        some_im_base_url: provider_config.base_url.clone(),
        api_key: provider_config.api_key.clone(),
    });
    let accepts_images = gateway
        .accepts_images
        .unwrap_or_else(|| model_accepts_images(&model));
    let image_fallback = if kind == ProviderKind::SomeIm && !accepts_images {
        let vision_model = profile
            .and_then(|value| value.vision_model.clone())
            .unwrap_or_else(|| SOMEIM_VISION_FALLBACK_MODEL.to_owned());
        let mut vision_config = provider_config.clone();
        vision_config.dialect = ApiDialect::ChatCompletions;
        vision_config.model = vision_model.clone();
        Some((
            usage_ledger.auxiliary(
                build_provider(vision_config).context("initialize some.im vision fallback")?,
            ),
            vision_model,
        ))
    } else {
        None
    };
    // 协议回落只针对会话模型：子 Agent、压缩器等换了模型的派生配置沿用用户配的协议，
    // 父模型只能走 Responses 不代表别的模型也是。
    let mut parent_provider_config = provider_config.clone();
    parent_provider_config.dialect = configured_dialect;
    let provider =
        build_provider(primary_provider_config.clone()).context("initialize provider")?;
    // 主模型一格共享：凡是「定义为跟主模型一样」的兜底都拿 follower，`/model`
    // 一换全跟着换；显式配了模型的照旧是自己那一份。
    let main_model = willdeep_core::provider::MainModelHandle::new(provider.clone());
    let follow_main = main_model.follower();
    let local_auxiliary_config = local_auxiliary_provider_config(&loaded.file.local_model);
    let host_input = matches!(&frontend, HarnessFrontend::Runtime { usage_origin, .. }
        if delegation::is_host_input(usage_origin.origin_client.as_deref()));
    let turn_id = match &frontend {
        HarnessFrontend::Runtime { usage_origin, .. } => usage_origin.turn_id,
        _ => None,
    };
    let independent_execution = matches!(
        &frontend,
        HarnessFrontend::Runtime {
            independent_execution: true,
            ..
        }
    );
    let web_child_proposals = web_child_proposals
        || !independent_execution
            && turn_id
                .map(|turn| {
                    crate::daemon::session_store::child_start_confirmation_for_turn(
                        home, session_id, turn,
                    )
                })
                .transpose()?
                .unwrap_or(false);
    let (runtime_access, runtime_approval_handle, runtime_inbox) = match &frontend {
        HarnessFrontend::Runtime {
            workspace_access,
            approval_handle,
            instruction_inbox,
            ..
        } => (
            *workspace_access,
            approval_handle.clone(),
            instruction_inbox.clone(),
        ),
        _ => (None, None, None),
    };
    let approval_mode = if let Some(access) = runtime_access {
        access.approval_mode()
    } else if cli.full_auto {
        // `--full-auto` 发布时 workspace-write 与 smart 是同一套判定；档位拆开
        // 之后，保留它原来的行为比悄悄换成另一档更重要。
        ApprovalMode::Smart
    } else {
        configured_approval_mode(&loaded.file.agent)?
    };
    let (allowed_skills, allowed_mcp_servers) = match &frontend {
        HarnessFrontend::Runtime {
            allowed_skills,
            allowed_mcp_servers,
            ..
        } => (allowed_skills.clone(), allowed_mcp_servers.clone()),
        _ => (Vec::new(), Vec::new()),
    };
    preparation.mark("providers_and_team");
    let mut mcp_servers = loaded.file.mcp_servers.clone();
    if !allowed_mcp_servers.is_empty() {
        mcp_servers.retain(|name, _| allowed_mcp_servers.contains(name));
    }
    let mcp_home = home.to_path_buf();
    let mcp_future = async move {
        if delegation_read_only {
            // Connecting a stdio MCP server already executes an external program.
            Ok(willdeep_core::McpRegistry::default())
        } else {
            willdeep_core::McpRegistry::connect_in(Some(&mcp_home), &mcp_servers)
                .await
                .context("initialize MCP servers")
        }
    };
    let (approver, sink, runtime_connection): (
        Arc<dyn Approver>,
        Arc<dyn EventSink>,
        Option<RuntimeConnection>,
    ) = match frontend {
        HarnessFrontend::Terminal { json, quiet } => (
            Arc::new(TerminalApprover(language)),
            Arc::new(TerminalSink { json, quiet }),
            None,
        ),
        HarnessFrontend::Tui { tx } => (
            Arc::new(crate::tui::TuiApprover(tx.clone())),
            Arc::new(crate::tui::TuiSink { ui: tx }),
            None,
        ),
        HarnessFrontend::Runtime {
            connection, sink, ..
        } => (
            daemon::runtime_approver(Some(&connection))?.context("initialize Runtime approver")?,
            sink,
            Some(*connection),
        ),
    };
    let background_lifecycle_sink = sink.clone();
    let background_tasks = if delegation_read_only {
        Arc::new(BackgroundTaskRegistry::default())
    } else {
        Arc::new(
            BackgroundTaskRegistry::default()
                .with_agent_run_store(team_store)
                .with_family_team(family_root, team_settings)
                .map_err(anyhow::Error::msg)?
                .with_lifecycle_observer(move |snapshot| {
                    let sink = background_lifecycle_sink.clone();
                    async move {
                        if snapshot.kind != BackgroundTaskKind::Shell {
                            return;
                        }
                        if snapshot.status == BackgroundTaskStatus::Running {
                            sink.emit(willdeep_core::AgentEvent::BackgroundShellStarted {
                                id: snapshot.id,
                            })
                            .await;
                        } else {
                            sink.emit(willdeep_core::AgentEvent::BackgroundShellCompleted {
                                id: snapshot.id,
                                status: snapshot.status,
                                exit_code: snapshot.exit_code,
                                elapsed_millis: snapshot.elapsed_millis,
                                output_bytes: snapshot.output_bytes,
                            })
                            .await;
                        }
                    }
                }),
        )
    };
    let verification_home = home.to_path_buf();
    let verification_workspace = workspace.clone();
    let verification_snapshot_workspace = workspace.clone();
    // The judge answers one YES/NO per ambiguous command: on some.im that is
    // the gateway's managed `someim-security-guard` policy, elsewhere the
    // session's own model. `[agent] judge_model` overrides both.
    let safety_judge = if !delegation_read_only && loaded.file.agent.safety_judge.unwrap_or(true) {
        let configured_judge = loaded.file.agent.judge_model.clone();
        let judge_model = configured_judge
            .clone()
            .unwrap_or_else(|| default_judge_model(kind, &model));
        // 没配 judge_model 且不在 some.im 上，裁判就是「会话模型」：跟着 `/model`
        // 走。裁决日志里的模型名仍记启动时那个——它只是标签。
        let judge_provider = if configured_judge.is_none() && kind != ProviderKind::SomeIm {
            follow_main.clone()
        } else {
            let mut judge_config = parent_provider_config.clone();
            judge_config.model = judge_model.clone();
            build_provider(judge_config).context("initialize safety judge provider")?
        };
        Some(Arc::new(ProviderSafetyJudge::new(
            usage_ledger.auxiliary(judge_provider),
            judge_model,
        )) as Arc<dyn SafetyJudge>)
    } else {
        None
    };
    let approval_log = home.join("approvals.jsonl");
    // 作业记在本会话名下：结束通知只投给起它的会话。
    let detached_jobs =
        Arc::new(willdeep_core::DetachedJobStore::new(home).with_owner(session_id.to_string()));
    let hooks = if delegation_child {
        // Hooks can execute host programs; a delegated conversation does not
        // inherit that side-effect chain in either workspace mode.
        HookRegistry::default()
    } else {
        build_hooks(&loaded.file.hooks).context("read [[hooks]]")?
    };
    let kernel = willdeep_core::EventKernel::new();
    let approval_handle = runtime_approval_handle
        .unwrap_or_else(|| willdeep_core::SharedApprovalMode::new(approval_mode));
    approval_handle.set(approval_mode);
    // 子 Agent 要跟随父会话的 full-access，拿同一个句柄——切档是实时的。
    let parent_approval_handle = approval_handle.clone();
    // 把外部 MCP 服务的拉起/握手与本机技能发现、沙箱 git 目录探测、过期作业清理
    // 重叠执行，压缩交互式启动在构建阶段的串行等待。
    let local_prep_workspace = workspace.clone();
    let local_prep_skill_roots = loaded.file.skills.roots.clone();
    let local_prep_allowed_skills = allowed_skills;
    let local_prep_agent_settings = loaded.file.agent.clone();
    let local_prep_jobs = detached_jobs.clone();
    let local_prep_future = async move {
        tokio::task::spawn_blocking(move || {
            // 日志现在是完整落盘的，不清理会一直涨。只动已结束的作业，扫描成本是一次
            // 目录遍历，放在启动时做一次足够。
            local_prep_jobs.prune();
            let skills = Arc::new(
                willdeep_core::SkillCatalog::discover(
                    &local_prep_workspace,
                    &local_prep_skill_roots,
                )
                .allow_only(&local_prep_allowed_skills),
            );
            let sandbox = resolve_sandbox(
                &local_prep_agent_settings,
                approval_mode,
                &local_prep_workspace,
            );
            let workspace_sandbox = resolve_workspace_sandbox(
                &local_prep_agent_settings,
                approval_mode,
                &local_prep_workspace,
            );
            (skills, sandbox, workspace_sandbox)
        })
        .await
        .context("prepare workspace skills and sandbox")
    };
    let (mcp_result, local_prep_result) = tokio::join!(mcp_future, local_prep_future);
    let (skills, sandbox, workspace_sandbox) = local_prep_result?;
    preparation.mark("skills");
    let mut mcp_registry = mcp_result?;
    preparation.mark("mcp");
    // 已启用插件的 MCP 工具（docs/PLUGINS.md「聊天里的插件工具」）：定义读持久化
    // 目录，调用时才拉起插件，优先经 Web 进程的插件网关。显式限定了 MCP 服务
    // 白名单的 Runtime 任务不带插件工具——白名单里写的是配置里的服务名。
    if !delegation_child && allowed_mcp_servers.is_empty() {
        let config_path = cli
            .config
            .clone()
            .unwrap_or_else(|| home.join("config.toml"));
        let host_requests = Arc::new(
            crate::plugin_host_requests::HostRequests::new(
                home.to_path_buf(),
                config_path,
                Arc::new(std::sync::RwLock::new(vec![workspace.clone()])),
            )
            .with_usage_context(session_id, workspace.clone()),
        );
        mcp_registry = mcp_registry.with_lazy_tools(Arc::new(
            willdeep_core::plugin::PluginChatTools::new(home)
                .with_session(session_id, workspace.clone())
                .with_host_requests(host_requests),
        ));
    }
    let screenplay_plugins_allowed = !delegation_child && allowed_mcp_servers.is_empty();
    let mcp = Arc::new(mcp_registry);
    let mut tools = ToolRegistry::new(&workspace, approval_mode)?
        .with_shared_approval_mode(approval_handle)
        .with_output_store(&home.join("tool-outputs"))
        .with_sandbox(sandbox.clone())
        .with_workspace_sandbox(workspace_sandbox)
        .with_hooks(hooks)
        .with_approver(approver)
        .with_approval_reporter(move |trace| {
            record_approval_trace(&approval_log, Some(session_id), &trace);
        })
        .with_skills(skills.clone())
        .with_mcp(mcp.clone())
        .with_background_tasks(background_tasks.clone())
        // 显式后台命令脱离父进程：Runtime 升级或重启之后，回来取结果就行，
        // 不必把一条跑了半小时的命令再跑一遍。
        .with_detached_jobs(detached_jobs.clone())
        // monitor 的事件直接进内核；TUI 与无头运行靠注册表里的信号计数决定
        // 要不要唤醒。Runtime（daemon）前端暂时没有接唤醒，事件在下一个回合
        // 边界照常交付。
        .with_monitor_events(kernel.clone(), session_id, home.join("monitors"))
        .with_fallible_verification_snapshot(move || {
            verification::snapshot(&verification_snapshot_workspace)
        })
        .with_verification_reporter(move |verification| {
            let home = verification_home.clone();
            let workspace = verification_workspace.clone();
            let mut snapshots = Vec::new();
            if let Some(snapshot_id) = verification.snapshot_id.clone() {
                snapshots.push(snapshot_id);
            }
            // 运行期间变了快照的检查对两个快照都不是证据；结束快照那条记录让
            // 审计面看到「这条命令是在它形成期间跑的」，而不是什么都没有。
            if let Some(observed) = verification.observed_snapshot_id.clone() {
                snapshots.push(observed);
            }
            if snapshots.is_empty() {
                return;
            }
            tokio::spawn(async move {
                for snapshot_id in snapshots {
                    let _ = daemon::diff_review::remote_record_verification(
                        &home,
                        &workspace,
                        snapshot_id,
                        verification.clone(),
                    )
                    .await;
                }
            });
        })
        .with_web_tools(web_tools)
        // Only the main agent is told a failure is delegable: a subagent
        // cannot spawn anything, so the hint would be an instruction it has
        // no way to act on.
        .with_delegation_hints(true)
        .with_always_allow_store(home.join("always-allow.json"))?;
    if delegation_read_only {
        tools = tools.with_delegation_read_only();
    }
    if !delegation_child && !independent_execution {
        tools = tools.with_session_collaboration(Arc::new(crate::daemon::PeerSessionClient {
            home: home.to_path_buf(),
            session_id,
            turn_id,
        }));
    }
    if let Some(judge) = safety_judge.clone() {
        tools = tools.with_safety_judge(judge);
    }
    // 父会话的共享黑板：父 Agent 与它派出的每个 Worker 读写同一块。
    let board = Arc::new(willdeep_core::board::Board::open(
        willdeep_core::board::Board::path_for(&home.join("workers"), session_id),
    ));
    let tools = if delegation_read_only {
        tools
    } else {
        tools.with_board(board.clone(), "parent")
    };
    // 稳定前缀在前：契约、全局规则、技能目录、简洁风格与插件路由。这些只在配置或
    // 技能库变化时才变，是提供商提示词缓存命中的那一段。工作区路径放在分界之后，
    // 换工作区不会把前面的缓存打掉。
    let configured_rules = loaded.file.rules.as_ref().map(|rules| rules.files.clone());
    let global_rules = willdeep_core::prompt::global_user_instructions(configured_rules.as_deref());
    let mut stable = willdeep_core::prompt::stable_prefix(global_rules.as_deref());
    if web_child_proposals {
        stable.push_str("\n\n# Web child conversations\nOnly use start_session when the user explicitly asks for a separate child conversation. Derive its full title, brief, context and workspace options from the conversation. This creates a filled proposal card, not running work: the user must click Start. Do not ask them to retype the brief, call another tool to bypass confirmation, or show a proposal for ordinary chat.\n");
    }
    if delegation_read_only {
        stable.push_str(delegation::READ_ONLY_CONTRACT);
    } else if delegation_child {
        stable.push_str(delegation::WRITABLE_CHILD_CONTRACT);
    }
    if !skills.list().is_empty() {
        stable.push_str(if delegation_read_only {
            "\n\n# Available skills\nUse list_skills to search and read_skill before applying relevant guidance. The read-only capability ceiling still applies to every skill; guidance cannot authorize writes, shell, external tools or further delegation.\n"
        } else {
            "\n\n# Available skills\nUse list_skills to search and read_skill before applying a relevant skill. Entries may carry a tier: `tier=worker` marks a skill whose steps fit a small-context worker — prefer dispatching it via spawn_agent with a task packet instead of running it inline; `tier=deep` marks work that needs the largest window available. Untagged skills run at the session's default tier.\n"
        });
        stable.push_str(&skills.routing_summary(4_096));
    }
    // 简洁是默认风格：只有这条会话显式选了详细（`/verbose`）才不加这一段。
    // 契约文本一个字不动，覆盖段接在稳定内容之后、动态内容之前。
    if resumed
        .is_none_or(|session| willdeep_core::ResponseStyle::uses_short(session.response_style))
    {
        stable.push_str("\n\n");
        stable.push_str(willdeep_core::prompt::SHORT_RESPONSE_INSTRUCTION);
    }
    if let Some(guidance) =
        crate::plugin_production::routing_guidance(home, screenplay_plugins_allowed).await
    {
        stable.push_str(&guidance);
    }
    if let Some(guidance) =
        crate::plugin_screenplay::routing_guidance(home, screenplay_plugins_allowed).await
    {
        stable.push_str(&guidance);
    }
    let system_prompt = format!(
        "{stable}{}{}",
        willdeep_core::prompt::CACHE_BOUNDARY,
        willdeep_core::prompt::dynamic_workspace_context(&workspace)
    );
    // These inert values support frontend projections without restoring or installing a team.
    let (tools, subagents, goal_continuation, base_profiles) = if delegation_read_only {
        (
            tools,
            Arc::new(SubagentCatalog::new(
                &workspace,
                Vec::new(),
                background_tasks.clone(),
            )),
            Arc::new(willdeep_core::GoalContinuation::new()),
            Vec::new(),
        )
    } else {
        let mut subagent_profiles = worker_profiles::configure_worker_profiles(
            &loaded.file,
            &parent_provider_config,
            &follow_main,
            kind,
        )?;
        let base_profiles = subagent_profiles.clone();
        subagent_profiles.extend(crate::agent_registry::runtime_profiles(
            home,
            &subagent_profiles,
            &loaded.file,
            &parent_provider_config,
        )?);
        tools
            .require_verifications(&loaded.file.agent.verification_commands)
            .map_err(anyhow::Error::msg)?;
        let tools = if let Some(contract) = loaded.file.agent.completion_contract.clone() {
            tools
                .with_completion_contract(contract)
                .map_err(anyhow::Error::msg)?
        } else {
            tools
        };
        let goal_continuation = Arc::new(willdeep_core::GoalContinuation::with_default_budget(
            loaded.file.agent.goal_budget(),
        ));
        if let Some(session) = resumed
            && !delegation_child
        {
            restore_goal(&goal_continuation, session);
        }
        // Child conversations cannot own a persistent Goal; an inherited one
        // would keep executing after the assignment terminal.
        let catalog_goal = if delegation_child {
            Arc::new(willdeep_core::GoalContinuation::new())
        } else {
            goal_continuation.clone()
        };
        let worktree_root = if delegation_child {
            home.join("worktrees")
                .join("child-subagents")
                .join(session_id.to_string())
        } else {
            home.join("worktrees").join("subagents")
        };
        let mut catalog =
            SubagentCatalog::new(&workspace, subagent_profiles, background_tasks.clone())
                .with_sandbox(sandbox)
                // 父会话在 full-access 时 Worker 跟着免审（rocky 2026-09-21 决定）。
                .with_parent_approval_mode(parent_approval_handle)
                .with_worktree_root(worktree_root)
                // Task packets may name a skill; the runtime inlines its body so the
                // worker never spends turns fetching its own instructions.
                .with_skills(skills.clone())
                // Worker 没有自己的项目规则，继承用户配置的全局规则。
                .with_global_rules(configured_rules.unwrap_or_default())
                // 兜底工种能查已连接的 MCP 服务。窄工种拿不到：它们的价值就是范围窄。
                .with_mcp(mcp)
                // Worker 与父会话共享已批准的精确动作。没有这条，后台 Worker 会在人
                // 刚刚批过的同一条命令上再卡一次，而它自己没有审批 UI。
                .with_state_home(home.join("workers"))
                .with_parent_session(session_id)
                .with_always_allow_store(home.join("always-allow.json"))
                .with_event_sink(sink.clone())
                .with_usage_ledger(usage_ledger.clone())
                .with_feedback(feedback.clone())
                .with_board(board)
                // Worker 的用量同样计入目标的 token 预算。
                .with_goal(catalog_goal);
        // 档位兑现成哪个模型。准入在 agent 层，这里只负责兑现。
        for (tier, binding) in resolve_tier_bindings(
            &loaded.file,
            &parent_provider_config,
            &follow_main,
            kind,
            context_window,
        )? {
            catalog = catalog.with_tier_binding(tier, binding);
        }
        // Verifier commands run unattended, with no approval card to fall back
        // on. They go through the same judge the main agent's shell does.
        if let Some(judge) = safety_judge {
            catalog = catalog.with_safety_judge(judge);
        }
        let binding_home = home.to_path_buf();
        let binding_file = loaded.file.clone();
        let binding_parent = parent_provider_config.clone();
        catalog = catalog.with_definition_bindings(move |binding, fallback| {
            crate::agent_registry::resolve_binding_provider(
                &binding_home,
                binding,
                &binding_file,
                &binding_parent,
                fallback,
            )
            .map_err(|error| {
                willdeep_core::AgentError::SubagentBlocked(format!(
                    "restore specialist model binding: {error:#}"
                ))
            })
        });
        (tools, Arc::new(catalog), goal_continuation, base_profiles)
    };
    preparation.mark("tools_and_workers");
    let workflows = workflows::WorkflowService::new(
        home,
        session_id,
        subagents.clone(),
        background_tasks.clone(),
        sink.clone(),
    );
    let tools = if delegation_child {
        tools
    } else {
        tools
            .with_agent_definition_host(Arc::new(agent_library::LibraryHost::new(
                home,
                subagents.clone(),
                base_profiles,
                loaded.file.clone(),
                parent_provider_config.clone(),
            )?))
            .with_workflow_host(workflows.clone())
    };
    let tools = if screenplay_plugins_allowed {
        let config_path = cli
            .config
            .clone()
            .map(Ok)
            .unwrap_or_else(crate::config::default_config_path)?;
        tools.with_configuration_host(Arc::new(
            crate::configuration_host::ChatConfigurationHost::new(
                home.to_path_buf(),
                config_path,
                parent_provider_config.clone(),
                follow_main.clone(),
            )?,
        ))
    } else {
        tools
    };
    // 构建界面只读取团队状态；恢复由取得会话执行权后的执行入口负责。
    let command_watcher = if delegation_child {
        None
    } else {
        daemon::start_agent_command_watcher(
            runtime_connection.as_ref(),
            background_tasks.clone(),
            subagents.clone(),
        )?
    };
    // 事件内核先于 Agent 建起来，并立刻把上一次运行没投递完的事件读回来。
    // 读不出来不挡启动：事件是通知不是账本，为一份坏日志让整个 Runtime 起不
    // 来才是真的坏。（内核本身在工具注册表之前就建好了：monitor 要往里发事件。）
    preparation.mark("workflow_and_watchers");
    let kernel_store = willdeep_core::kernel_store::KernelStore::new(home);
    let restored = willdeep_core::kernel_store::restore_into(&kernel, &kernel_store);
    preparation.mark("kernel_restore");
    for (path, reason) in &restored.quarantined {
        eprintln!(
            "willdeep: quarantined a damaged event log ({reason}); kept at {}",
            path.display()
        );
    }
    let instance_home = home.to_path_buf();
    let mut agent = Agent::new(
        provider.clone(),
        tools,
        AgentConfig {
            max_turns,
            system_prompt,
            context_window,
            // 轮次上限之外唯一的自动闸门；不配就不限，与此前行为一致。
            token_budget: loaded.file.agent.token_budget,
        },
    )
    .with_instance_name_source(move || {
        crate::identity::load(&instance_home)
            .map(|identity| identity.name)
            .map_err(|error| format!("{error:#}"))
    })
    .with_main_model(main_model)
    .with_event_sink(sink.clone())
    .with_background_tasks(background_tasks.clone())
    .with_event_kernel(kernel.clone())
    .with_usage_ledger(usage_ledger.clone())
    .with_feedback(feedback);
    let goal_continuation = if delegation_child {
        Arc::new(willdeep_core::GoalContinuation::new())
    } else {
        goal_continuation
    };
    if !delegation_read_only {
        agent = agent
            .with_subagents(subagents.clone())
            .with_goal_continuation(goal_continuation.clone());
    }
    if let Some(seconds) = loaded.file.agent.time_budget_seconds {
        if seconds == 0 {
            anyhow::bail!("agent.time_budget_seconds must be greater than zero");
        }
        agent = agent.with_time_budget(std::time::Duration::from_secs(seconds));
    }
    // Runtime 任务的收件箱由任务管理器持有另一半；进程内轮次用 Agent 自带的那个。
    if let Some(inbox) = runtime_inbox {
        agent = agent.with_instruction_inbox(inbox);
    }
    let routing_policy = RoutingPolicy {
        auto_dispatch_read_only: loaded.file.agent.auto_dispatch_read_only.unwrap_or(true),
        max_deep_calls: loaded.file.agent.max_deep_calls_per_harness.unwrap_or(1),
    };
    if !delegation_child && loaded.file.agent.small_model_routing.unwrap_or(true) {
        let mut routing = RoutingGuard::new(routing_policy);
        if loaded.file.local_model.enabled
            && loaded.file.local_model.prefer_for_worker_routing
            && let Some(local_config) = local_auxiliary_config.clone()
        {
            routing = routing.with_classifiers(vec![
                usage_ledger.auxiliary(
                    build_provider(local_config).context("initialize local routing model")?,
                ),
                usage_ledger.auxiliary(follow_main.clone()),
            ]);
        }
        agent = agent.with_routing_guard(Arc::new(routing));
    } else {
        // 路由关了，专家档的票据与次数上限照样生效——那道闸与路由无关。
        agent = agent.with_admission_guard(Arc::new(RoutingGuard::new(routing_policy)));
    }
    if let Some((vision_provider, vision_model)) = image_fallback {
        agent = agent.with_image_fallback(vision_provider, format!("some.im / {vision_model}"));
    }
    agent = agent.with_compression_settings(build_compression_settings(
        loaded,
        &parent_provider_config,
        follow_main.clone(),
    )?);

    // 标题同样本地优先、会话 Provider 兜底；请求仍只带一问一答各 800 字。
    // 轮次结束后的「下一句预测」用同一档模型候选，但开关各管各的：关掉自动标题
    // 不该顺手把预测也关了，反过来也一样。
    let auto_title = loaded.file.agent.auto_title.unwrap_or(true);
    let input_suggestions = loaded.file.agent.input_suggestions.unwrap_or(true);
    if auto_title || input_suggestions {
        let auxiliaries = auxiliary_providers(loaded, &parent_provider_config, Some(&follow_main))?
            .into_iter()
            .map(|provider| usage_ledger.auxiliary(provider))
            .collect::<Vec<_>>();
        if auto_title {
            agent = agent.with_titlers(auxiliaries.clone());
        }
        if input_suggestions {
            agent = agent.with_input_suggesters(auxiliaries);
        }
    }
    let notifier = crate::notify::Notifier::new(&loaded.file.notifications, language);
    let agent = Arc::new(agent);
    let workflow_driver = if delegation_child {
        None
    } else {
        workflows.bind_agent(&agent);
        Some(workflows::WorkflowDriverGuard::new(workflows.clone()))
    };
    preparation.mark("agent_and_auxiliaries");
    drop(preparation);
    Ok(BuiltHarness {
        workflows,
        _workflow_driver: workflow_driver,
        host_input,
        event_sink: sink,
        subagents,
        goal: goal_continuation,
        session_id,
        agent,
        workspace,
        skills,
        background_tasks,
        detached_jobs,
        kernel,
        kernel_store,
        context_window,
        provider_config: primary_provider_config,
        provider_profile,
        notifier,
        _command_watcher: command_watcher,
    })
}

/// 会话上下文窗口：档案里显式写的 `context_window` > 网关能力表报告的值 >
/// 按模型名的缺省（`someim-*` 256K，其余 128K，见 `model_defaults`）。
fn session_context_window(configured: Option<u64>, gateway: Option<u64>, model: &str) -> u64 {
    configured
        .or(gateway)
        .unwrap_or_else(|| crate::model_defaults::default_context_window(model))
}

/// 网关能力表对本次会话的调整；每一项为 `None` 都表示沿用原有行为。
#[derive(Debug, Default, PartialEq, Eq)]
struct GatewayAdjustments {
    /// 用户没指定协议、而网关没有线路能以缺省协议服务这个模型时，改用网关能服务的协议。
    dialect: Option<ApiDialect>,
    /// 网关给出的看图结论（只信经探测验证的声明或目录明确列出的 image）。
    accepts_images: Option<bool>,
    /// 网关报告的上下文窗口，已夹在配置允许的范围内；配置文件里写了的仍以配置为准。
    context_window: Option<u64>,
}

fn gateway_adjustments(
    capability: Option<&willdeep_core::provider::capabilities::GatewayModelCapability>,
    dialect: ApiDialect,
    api_pinned: bool,
) -> GatewayAdjustments {
    let Some(capability) = capability else {
        return GatewayAdjustments::default();
    };
    let switched = (!api_pinned)
        .then(|| capability.fallback_dialect(dialect))
        .flatten();
    let effective = switched.unwrap_or(dialect);
    GatewayAdjustments {
        dialect: switched,
        accepts_images: capability.accepts_images(effective),
        context_window: capability.context_window().map(|window| {
            window.clamp(
                willdeep_core::CONTEXT_WINDOW_MIN,
                willdeep_core::CONTEXT_WINDOW_MAX,
            )
        }),
    }
}

/// 用户是否显式指定了协议（命令行或档案里的 `api`，`auto` 不算）。
fn api_pinned(cli: &Cli, profile: Option<&ProviderProfile>) -> bool {
    cli.api.is_some_and(|api| api != ApiArg::Auto)
        || profile
            .and_then(|provider| provider.api.as_deref())
            .is_some_and(|api| !api.trim().is_empty() && !api.trim().eq_ignore_ascii_case("auto"))
}

/// 会话主 Provider 的配置：命令行 > 所选档案 > 环境里的钥匙 > 缺省。
///
/// Web 的下一句预测也从这里取，免得两处各算一遍、在某个档案上悄悄分叉。
pub(crate) fn resolve_parent_provider_config(
    cli: &Cli,
    loaded: &LoadedConfig,
    resumed_profile: Option<&str>,
) -> Result<ProviderConfig> {
    let requested = cli.profile.as_deref().or(resumed_profile);
    if requested == Some(crate::magpie_integration::PROFILE_ID) {
        let mut config = crate::magpie_integration::EffectiveProviders::snapshot(&loaded.file)
            .config(crate::magpie_integration::PROFILE_ID)?;
        if let Some(model) = &cli.model {
            config.model = model.clone();
        }
        config.validate()?;
        return Ok(config);
    }
    let profile = loaded.select_provider(requested)?;
    crate::provider_settings::ProviderEnvironment::read().resolve(profile, Some(cli))
}

/// 启动与 TUI 手动刷新共用：全部候选成功构建后才交给 Agent 一次替换，
/// `None` 窗口也必须保留，才能清除上一次刷新设置的限制。
pub(crate) fn build_compression_settings(
    loaded: &LoadedConfig,
    parent: &ProviderConfig,
    follow_main: Arc<dyn willdeep_core::provider::Provider>,
) -> Result<willdeep_core::agent::CompressionSettings> {
    let mut candidates = Vec::new();
    let window = compressor_context_window(&loaded.file);
    for (mut config, hosted_prompt) in compressor_candidate_configs(loaded, parent, parent.kind)? {
        // 显式的压缩器窗口优先；否则沿用档案/父 Provider 已知的窗口，
        // 好让请求里的 max_tokens 与实际收得下的原料对齐。
        config.context_window = window.or(config.context_window);
        let context = if hosted_prompt {
            "initialize hosted context compressor"
        } else {
            "initialize context compressor provider"
        };
        candidates.push((build_provider(config).context(context)?, hosted_prompt));
    }
    // 主模型句柄保持原样；兜底继续跟随 `/model`。
    candidates.push((follow_main, false));
    Ok(willdeep_core::agent::CompressionSettings {
        candidates,
        context_window: compressor_context_window(&loaded.file),
    })
}

/// 上下文压缩在会话模型兜底之前的候选。
///
/// 顺序：显式 `[agent] compressor_profile`（整套端点、协议、密钥与模型都取
/// `[providers.<id>]`）→ 显式 `[agent] compressor_model`（沿用会话 Provider，
/// 只换模型名）→ some.im 会话的托管压缩器（仅在没有显式配置时）。`bool` 为真
/// 表示压缩指令存在服务端并以 replace 模式注入，客户端这时只发裸转录。调用方
/// 在末尾追加会话 Provider，所以这条链永远至少有一个候选。
///
/// 档案这条路是为了让主模型留在云端、压缩跑本机：压缩候选此前只是会话
/// Provider 的克隆，主模型换端点，压缩就跟着换，没有别的出口。
///
/// 本地辅助模型（`[local_model]`）刻意不在这条链上：压缩是拿一段摘要替换整段
/// 旧历史，弱模型做坏一次就把长期上下文丢干净了，而且这是不可逆的。旧配置里的
/// `[local_model] prefer_for_context_summaries` 仍然能解析，但不再改变这里的顺序。
fn compressor_candidate_configs(
    loaded: &LoadedConfig,
    parent: &ProviderConfig,
    kind: ProviderKind,
) -> Result<Vec<(ProviderConfig, bool)>> {
    if let Some(profile_name) = loaded
        .file
        .agent
        .compressor_profile
        .as_deref()
        .map(str::trim)
    {
        let mut profile = loaded
            .file
            .providers
            .get(profile_name)
            .cloned()
            .with_context(|| {
                format!(
                    "agent.compressor_profile names a provider profile that is not configured: \
                 {profile_name}"
                )
            })?;
        // `compressor_model` 既覆盖档案里的模型名，也可以在档案没写模型时补上：
        // 只配一份端点（地址、密钥）再单独指模型，是很自然的写法。
        if profile.model.is_none() {
            profile.model = loaded.file.agent.compressor_model.clone();
        }
        let mut compressor_config = crate::provider_settings::ProviderEnvironment::read()
            .resolve(Some(&profile), None)
            .with_context(|| format!("resolve compressor provider profile {profile_name}"))?;
        if let Some(compressor_model) = loaded.file.agent.compressor_model.clone() {
            compressor_config.model = compressor_model;
        }
        return Ok(vec![(compressor_config, false)]);
    }
    if let Some(compressor_model) = loaded.file.agent.compressor_model.clone() {
        let mut compressor_config = parent.clone();
        compressor_config.model = compressor_model;
        Ok(vec![(compressor_config, false)])
    } else if kind == ProviderKind::SomeIm {
        let mut compressor_config = parent.clone();
        compressor_config.model = SOMEIM_CONTEXT_COMPRESSOR_MODEL.to_owned();
        Ok(vec![(compressor_config, true)])
    } else {
        Ok(Vec::new())
    }
}

/// 压缩摘要器自己的窗口：显式 `[agent] compressor_context_window` 优先，其次
/// 压缩档案里的 `context_window`。都没有就不绑，沿用会话窗口。
fn compressor_context_window(file: &crate::config::ConfigFile) -> Option<u64> {
    file.agent.compressor_context_window.or_else(|| {
        file.agent
            .compressor_profile
            .as_deref()
            .map(str::trim)
            .and_then(|name| file.providers.get(name))
            .and_then(|profile| profile.context_window)
    })
}

/// 标题与下一句预测共用的小模型候选：本地模型（`prefer_for_titles`）优先，
/// 会话 Provider 换成 `title_model` 兜底。按顺序试，前一家请求失败才问下一家。
///
/// 没配 `title_model` 时兜底就是会话模型：有 `follow_main` 就用它，跟着 `/model`
/// 走；没有（Web 端不起 Agent 的一次性预测）才按 `parent` 现建一份。
pub(crate) fn auxiliary_providers(
    loaded: &LoadedConfig,
    parent: &ProviderConfig,
    follow_main: Option<&Arc<dyn willdeep_core::provider::Provider>>,
) -> Result<Vec<Arc<dyn willdeep_core::provider::Provider>>> {
    let mut auxiliaries = Vec::new();
    if loaded.file.local_model.prefer_for_titles
        && let Some(local_config) = local_auxiliary_provider_config(&loaded.file.local_model)
    {
        auxiliaries
            .push(build_provider(local_config).context("initialize local session title model")?);
    }
    match (loaded.file.agent.title_model.clone(), follow_main) {
        (None, Some(follow_main)) => auxiliaries.push(follow_main.clone()),
        (title_model, _) => {
            let mut title_config = parent.clone();
            if let Some(title_model) = title_model {
                title_config.model = title_model;
            }
            auxiliaries
                .push(build_provider(title_config).context("initialize session title provider")?);
        }
    }
    Ok(auxiliaries)
}

/// Web 端的下一句预测用：不起 Agent，只按配置与会话当时的档案、模型装出
/// 与 TUI 同一组候选 Provider。命令行参数一律取缺省——Web 没有命令行。
pub(crate) fn input_suggestion_providers(
    loaded: &LoadedConfig,
    profile: Option<&str>,
    model: Option<String>,
) -> Result<Vec<Arc<dyn willdeep_core::provider::Provider>>> {
    let mut cli = <Cli as clap::Parser>::try_parse_from(["willdeep"])
        .context("build default CLI settings")?;
    cli.model = model;
    let parent = resolve_parent_provider_config(&cli, loaded, profile)?;
    auxiliary_providers(loaded, &parent, None)
}

fn local_auxiliary_provider_config(
    settings: &crate::config::LocalModelSettings,
) -> Option<ProviderConfig> {
    if !settings.enabled {
        return None;
    }
    let mut config = ProviderConfig::new(
        ProviderKind::OpenAiCompatible,
        ApiDialect::ChatCompletions,
        settings.base_url.trim(),
        "",
        settings.summary_model.trim(),
    );
    // “本地”描述的是用户自建的辅助算力，不限定部署在当前进程所在机器。
    // 家庭局域网、内网域名和回环地址都可以明确选择免 Token；普通 Provider
    // Profile 没有这个标记，缺 Key 时仍然拒绝启动。
    config.allow_unauthenticated = true;
    // Swift App caps Worker routing at eight seconds. Reusing the same local
    // client timeout keeps every auxiliary fallback bounded as well.
    config.request_timeout_secs = 8;
    Some(config)
}

pub(crate) fn resolve_workspace(
    cli: &Cli,
    resumed: Option<&willdeep_core::Session>,
) -> Result<PathBuf> {
    let project_workspace = cli.project.as_deref().map(projects::resolve).transpose()?;
    let requested_workspace = cli
        .workspace
        .clone()
        .or(project_workspace)
        .or_else(|| resumed.map(|session| session.workspace.clone()))
        .unwrap_or_else(|| PathBuf::from("."));
    requested_workspace
        .canonicalize()
        .with_context(|| format!("invalid workspace: {}", requested_workspace.display()))
}

/// 把配置里的 `[[hooks]]` 翻成注册表。
///
/// 配置错误在这里**直接失败**，不静默跳过：一条拼错事件名的门禁 hook 如果被
/// 悄悄忽略，用户会以为门禁在生效，而它一次都不会触发——这比没配还危险。
fn build_hooks(settings: &[crate::config::HookSettings]) -> Result<HookRegistry> {
    use willdeep_core::hooks::{Hook, HookEvent, HookFailure};

    let mut hooks = Vec::with_capacity(settings.len());
    for entry in settings {
        let event = match entry.event.as_str() {
            "pre_tool" => HookEvent::PreTool,
            "post_tool" => HookEvent::PostTool,
            "approval_resolved" => HookEvent::ApprovalResolved,
            other => bail!(
                "hooks.event 只能是 `pre_tool`、`post_tool` 或 `approval_resolved`，收到 `{other}`"
            ),
        };
        let on_error = match entry.on_error.as_deref() {
            None | Some("deny") => HookFailure::Deny,
            Some("ignore") => HookFailure::Ignore,
            Some(other) => bail!("hooks.on_error 只能是 `deny` 或 `ignore`，收到 `{other}`"),
        };
        if entry.command.trim().is_empty() {
            bail!("hooks.command 不能为空");
        }
        if entry.blocking && !event.can_block() {
            bail!(
                "`{}` 事件发生在动作之后，blocking = true 拦不住任何东西；\
                 想拦请改用 `pre_tool`",
                entry.event
            );
        }
        hooks.push(Hook {
            name: entry.name.clone().unwrap_or_else(|| entry.event.clone()),
            event,
            command: entry.command.clone(),
            blocking: entry.blocking,
            timeout: std::time::Duration::from_secs(entry.timeout_seconds.unwrap_or(10)),
            on_error,
        });
    }
    Ok(HookRegistry::new(hooks))
}

/// 围栏开着时默认放行的工具链缓存。它们都在工作区外，但不放行的话 `cargo fetch`、
/// `npm install`、`pip install`、`go build` 第一天就撞墙，然后所有人把围栏关掉——
/// 一个被关掉的围栏防不住任何东西。不存在的目录会在建 spec 时被丢掉，不必逐台机器裁剪。
/// `agent.sandbox_toolchain_caches = false` 可以整组去掉。
pub(crate) const DEFAULT_SANDBOX_TOOLCHAIN_ROOTS: &[&str] = &[
    "~/.cargo/registry",
    "~/.cargo/git",
    "~/.npm",
    "~/.yarn",
    "~/.cache",
    "~/Library/Caches",
    "~/Library/pnpm",
    "~/.local/share/pnpm",
    "~/go/pkg/mod",
];

/// `~` 与 `~/…` 展开成家目录；别的路径原样返回。配置里写 `~/.cargo/registry`
/// 是人的自然写法，以前不展开就被静默丢掉，等于文档教了一条不生效的配置。
pub(crate) fn expand_home(path: &std::path::Path) -> std::path::PathBuf {
    let text = path.to_string_lossy();
    let Some(rest) = text.strip_prefix('~') else {
        return path.to_path_buf();
    };
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')) {
        return path.to_path_buf();
    }
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return path.to_path_buf();
    };
    let mut expanded = std::path::PathBuf::from(home);
    let rest = rest.trim_start_matches(['/', '\\']);
    if !rest.is_empty() {
        expanded.push(rest);
    }
    expanded
}

fn sandbox_roots(
    agent: &crate::config::AgentSettings,
    workspace: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let mut roots = vec![workspace.to_path_buf(), std::env::temp_dir()];
    if agent.sandbox_toolchain_caches.unwrap_or(true) {
        roots.extend(
            DEFAULT_SANDBOX_TOOLCHAIN_ROOTS
                .iter()
                .map(|root| expand_home(std::path::Path::new(root))),
        );
    }
    roots.extend(external_git_dirs(workspace));
    roots.extend(
        agent
            .sandbox_writable_roots
            .iter()
            .map(|root| expand_home(root)),
    );
    roots
}

/// 工作区所属仓库落在工作区外的 git 目录。
///
/// 子模块的 `.git` 是个文件，指向父仓库的 `.git/modules/<name>`；`git worktree`
/// 指向主仓库的 `.git/worktrees/<name>`，对象库还在 common dir；在仓库子目录里
/// 开会话时 `.git` 在上层。这几种情况下 commit、fetch、改 ref 全写在工作区外，
/// 不放行的话围栏开着就提交不了代码。放行的只是「这个工作区自己的仓库元数据」，
/// 普通仓库的 `.git` 本来就在工作区里可写，不多出新的风险面。
/// 不在仓库里、没装 git、git 报错时返回空——这是锦上添花，不该挡住启动。
fn external_git_dirs(workspace: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(output) = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["rev-parse", "--git-dir", "--git-common-dir"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // 两个输出都可能是相对 `-C` 目录的路径。
        let Ok(dir) = workspace.join(line).canonicalize() else {
            continue;
        };
        if !dir.starts_with(&workspace) && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// 各档位的网络围栏。`read-only` 永远断；`workspace-write` 与 `smart` 一样允许
/// 网络，由外部命令的 AI 审核决定是否执行；`strict` 仍由逐次人工确认控制。
/// `agent.sandbox_network` 可以整体收紧或放开。
fn sandbox_network(
    agent: &crate::config::AgentSettings,
    approval_mode: ApprovalMode,
) -> willdeep_core::sandbox::NetworkPolicy {
    use willdeep_core::sandbox::NetworkPolicy;

    match agent.sandbox_network {
        Some(crate::config::SandboxNetwork::Allow) => NetworkPolicy::Allow,
        Some(crate::config::SandboxNetwork::Deny) => NetworkPolicy::Deny,
        None => match approval_mode {
            ApprovalMode::ReadOnly => NetworkPolicy::Deny,
            _ => NetworkPolicy::Allow,
        },
    }
}

/// 围栏开不开：`agent.sandbox` 没写就看这台机器有没有后端——有就开。
/// 显式 `true` 在没后端的机器上会让命令拒绝启动（fail closed），那是用户的选择。
pub(crate) fn sandbox_enabled(agent: &crate::config::AgentSettings) -> bool {
    agent
        .sandbox
        .unwrap_or_else(willdeep_core::sandbox::available)
}

/// 把审批档位翻译成 OS 侧的围栏档位，并算出可写根与网络策略。
///
/// 不新造一个轴：围栏档位是工作区策略的投影，用户已经选过一次的东西不该再选
/// 第二次。临时目录默认可写——不给的话 `cargo`、`rustc`、`git` 全都写不了中间
/// 文件，围栏第一天就会被关掉。
fn resolve_sandbox(
    agent: &crate::config::AgentSettings,
    approval_mode: ApprovalMode,
    workspace: &std::path::Path,
) -> willdeep_core::sandbox::SandboxSpec {
    use willdeep_core::sandbox::{SandboxPolicy, SandboxSpec};

    if !sandbox_enabled(agent) {
        return SandboxSpec::new(SandboxPolicy::Off, []);
    }
    let policy = match approval_mode {
        ApprovalMode::ReadOnly => SandboxPolicy::ReadOnly,
        _ => SandboxPolicy::WorkspaceWrite,
    };
    SandboxSpec::new(policy, sandbox_roots(agent, workspace))
        .with_network(sandbox_network(agent, approval_mode))
}

/// `workspace-write` 档的围栏。
///
/// 工作区内操作靠围栏免审；工作区外操作走 AI 审核，所以没配
/// `agent.sandbox` 时它也默认套上围栏；用户显式写了 `sandbox = false`，或
/// 这台机器没有围栏实现时返回 `None`，该档对工作区内未分类写命令改为问人。
/// 会话中途可以切到这一档，所以不论启动档位是什么都要算出来备用。
fn resolve_workspace_sandbox(
    agent: &crate::config::AgentSettings,
    approval_mode: ApprovalMode,
    workspace: &std::path::Path,
) -> Option<willdeep_core::sandbox::SandboxSpec> {
    use willdeep_core::sandbox::{SandboxPolicy, SandboxSpec};

    if approval_mode == ApprovalMode::ReadOnly
        || agent.sandbox == Some(false)
        || !willdeep_core::sandbox::available()
    {
        return None;
    }
    Some(
        SandboxSpec::new(
            SandboxPolicy::WorkspaceWrite,
            sandbox_roots(agent, workspace),
        )
        .with_network(sandbox_network(agent, ApprovalMode::WorkspaceAccess)),
    )
}

/// 配置里的 `agent.approval`，未配置时为 `smart`。`read-only` 是工作区策略，
/// 不是会话默认档，这里不接受。
pub(crate) fn configured_approval_mode(
    agent: &crate::config::AgentSettings,
) -> Result<ApprovalMode> {
    let Some(value) = agent.approval.as_deref() else {
        return Ok(ApprovalMode::Smart);
    };
    match ApprovalMode::parse(value) {
        Some(mode) if mode != ApprovalMode::ReadOnly => Ok(mode),
        _ => bail!(
            "agent.approval must be `strict`, `smart`, `workspace-write`, or `full-access`, got `{value}`"
        ),
    }
}

#[cfg(test)]
mod tests;
