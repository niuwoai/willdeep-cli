use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::Deserialize;
use willdeep_core::McpServerConfig;

pub const CONFIG_VERSION: u32 = 1;

mod ui_settings;
#[cfg(test)]
pub use ui_settings::InputSuggestionColor;
pub use ui_settings::{UiMode, UiSettings};
pub use ui_settings::{load_thinking_visibility, save_thinking_visibility};

#[derive(Clone, Debug, Subcommand)]
pub enum ConfigAction {
    /// Show or save the interactive session startup preference: ask, latest, new.
    Startup {
        #[arg(value_enum)]
        mode: Option<crate::session_startup::StartupMode>,
    },
    /// Show the shared credential-free runtime profile, or create its initial file.
    Runtime {
        #[arg(long)]
        init: bool,
        #[arg(long)]
        max_turns: Option<usize>,
    },
    /// Create a private starter config without overwriting an existing file.
    Init,
    /// Parse and validate the effective config.
    Check,
    /// Print the validated config with inline secrets redacted.
    Show,
}

pub fn handle(action: ConfigAction, explicit_path: Option<&Path>) -> Result<()> {
    let path = explicit_path
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(default_config_path)?;
    match action {
        ConfigAction::Startup { mode } => {
            if let Some(mode) = mode {
                crate::model_routing::save_session_startup(&path, mode)?;
            }
            let loaded = LoadedConfig::load(explicit_path)?;
            println!("{}", loaded.file.agent.session_startup.as_str());
            Ok(())
        }
        ConfigAction::Runtime { init, max_turns } => {
            runtime_profile(init, max_turns, explicit_path)
        }
        ConfigAction::Init => init(&path),
        ConfigAction::Check => {
            let loaded = load_required(&path)?;
            println!("valid\t{}\tversion={CONFIG_VERSION}", path.display());
            for note in deprecations(&loaded.file) {
                println!("legacy\t{}", note.summary());
            }
            Ok(())
        }
        ConfigAction::Show => show(&path),
    }
}

fn runtime_profile(init: bool, max_turns: Option<usize>, config_path: Option<&Path>) -> Result<()> {
    use willdeep_core::runtime_parameters::{FILE_NAME, RuntimeParameters};
    let home = willdeep_home()?;
    if init {
        std::fs::create_dir_all(&home)?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        options
            .open(home.join(FILE_NAME))
            .context("create shared runtime profile without overwriting")?
            .write_all(RuntimeParameters::default().canonical().as_bytes())?;
    }
    let mut parameters = match RuntimeParameters::load(&home).map_err(anyhow::Error::msg)? {
        Some(parameters) => parameters,
        None => {
            let loaded = LoadedConfig::load(config_path)?;
            loaded
                .file
                .agent
                .runtime_parameters(loaded.file.agent.max_turns.unwrap_or(DEFAULT_MAX_TURNS))
        }
    };
    if let Some(max_turns) = max_turns {
        parameters.max_turns = max_turns;
    }
    parameters.validate().map_err(anyhow::Error::msg)?;
    println!("{}", parameters.canonical());
    Ok(())
}

fn init(path: &Path) -> Result<()> {
    if path.exists() {
        bail!("configuration file already exists: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create configuration directory: {}", parent.display()))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options
        .open(path)
        .with_context(|| format!("create configuration file: {}", path.display()))?
        .write_all(include_bytes!("../../../config.example.toml"))
        .with_context(|| format!("write configuration file: {}", path.display()))?;
    println!("created\t{}", path.display());
    Ok(())
}

fn show(path: &Path) -> Result<()> {
    load_required(path)?;
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("read configuration file: {}", path.display()))?;
    let mut value: toml::Value = toml::from_str(&contents)
        .with_context(|| format!("parse TOML configuration: {}", path.display()))?;
    redact_inline_secrets(&mut value);
    println!("{}", toml::to_string_pretty(&value)?);
    Ok(())
}

fn load_required(path: &Path) -> Result<LoadedConfig> {
    if !path.exists() {
        bail!("configuration file does not exist: {}", path.display());
    }
    LoadedConfig::load(Some(path))
}

fn redact_inline_secrets(value: &mut toml::Value) {
    if let Some(providers) = value
        .get_mut("providers")
        .and_then(toml::Value::as_table_mut)
    {
        for (_, provider) in providers.iter_mut() {
            redact_field(provider, "api_key");
        }
    }
    let Some(channels) = value
        .get_mut("notifications")
        .and_then(toml::Value::as_table_mut)
        .and_then(|notifications| notifications.get_mut("channels"))
        .and_then(toml::Value::as_array_mut)
    else {
        return;
    };
    for channel in channels {
        redact_field(channel, "secret");
        let Some(table) = channel.as_table_mut() else {
            continue;
        };
        if let Some(smtp) = table.get_mut("smtp") {
            redact_field(smtp, "password");
        }
        if let Some(resend) = table.get_mut("resend") {
            redact_field(resend, "api_key");
        }
    }
}

/// `config show` 的输出经常被贴进 issue；内联密钥一律换成占位符，环境变量名
/// 不是秘密，原样保留。
fn redact_field(value: &mut toml::Value, field: &str) {
    if let Some(table) = value.as_table_mut()
        && table.contains_key(field)
    {
        table.insert(
            field.to_owned(),
            toml::Value::String("[REDACTED]".to_owned()),
        );
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    #[serde(default)]
    pub ui: UiSettings,
    #[serde(default)]
    pub agent_team: willdeep_core::subagent::AgentTeamSettings,
    pub version: Option<u32>,
    pub default_provider: Option<String>,
    #[serde(default)]
    pub agent: AgentSettings,
    /// Credential-free loopback model used for lightweight auxiliary work.
    /// Kept separate from Provider profiles so enabling it never changes the
    /// session's root model or donates a cloud credential to a local server.
    #[serde(default)]
    pub local_model: LocalModelSettings,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderProfile>,
    #[serde(default)]
    pub subagents: BTreeMap<String, SubagentProfileSettings>,
    /// 每个 Worker 档位兑现成哪个模型。与 `[subagents.*]` 是两根轴：那边配
    /// 「这个职责平时用什么」，这边配「派工时说要贵一档，贵成什么样」。
    ///
    /// 段名只认 `standard` / `advanced` / `expert` 三个正名。`deep` 之类的
    /// 别名在 `worker_tier` 参数上仍然可用，但不能做段名——两个段落映射到同
    /// 一档时谁赢是说不清楚的。
    #[serde(default)]
    pub worker_tiers: BTreeMap<String, WorkerTierSettings>,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    #[serde(default)]
    pub skills: SkillSettings,
    /// 匿名产品遥测；缺省即开启。
    #[serde(default)]
    pub telemetry: TelemetrySettings,
    /// 本机反馈账本（RSI 强反馈数据）。只写本机、不上传。
    #[serde(default)]
    pub feedback: FeedbackSettings,
    /// Cross-client attention delivery settings shared with WillDeep.app.
    /// The CLI accepts the full section even when only webhook fields are
    /// relevant on a headless machine; desktop-only sound fields remain
    /// harmless metadata.
    #[serde(default)]
    pub notifications: NotificationSettings,
    /// 公网入口：站点、表单与接收 webhook。与 `[notifications]` 那条向外发的
    /// webhook 是反方向的两件事。
    #[serde(default)]
    pub intake: IntakeSettings,
    /// 生命周期挂钩：审计留痕与门禁拦截。
    ///
    /// 与 `[notifications]` 那条 webhook 分开是有意的：webhook 是事后的礼貌
    /// 通知，跑在关键路径外、可以丢；hook 是事中的裁决，跑在关键路径上、
    /// 非零退出会真的拦下动作。把审计需求接到 webhook 上会丢事件。
    #[serde(default)]
    pub hooks: Vec<HookSettings>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LocalModelSettings {
    pub enabled: bool,
    pub base_url: String,
    pub summary_model: String,
    pub prefer_for_titles: bool,
    /// 已废弃：上下文压缩不再支持本地辅助模型，这个键的值不再影响任何行为。
    ///
    /// 字段留着只为兼容旧的 `config.toml`——本段是 `deny_unknown_fields`，
    /// 直接删掉字段会让写过这一行的用户一启动就报「unknown field」。改名
    /// 加 `serde(rename)` 是有意的：键照旧接受，代码里再有人读它就很显眼。
    #[serde(rename = "prefer_for_context_summaries")]
    pub deprecated_prefer_for_context_summaries: bool,
    pub prefer_for_worker_routing: bool,
}

impl Default for LocalModelSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            summary_model: "gemma4:e4b-it-qat".to_owned(),
            prefer_for_titles: true,
            deprecated_prefer_for_context_summaries: false,
            prefer_for_worker_routing: true,
        }
    }
}

/// 公网入口。缺省关闭：不写这一段就什么都不监听。
///
/// 只新增一个监听，站点、表单和 webhook 用路径区分。前面是谁做反向代理
/// （sslcat、nginx-proxy 或其他）不影响这里：willdeep 只认这个端口。
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IntakeSettings {
    pub enabled: bool,
    pub listen: String,
    pub sites: Vec<IntakeSite>,
    pub forms: Vec<IntakeForm>,
    pub webhooks: Vec<IntakeWebhook>,
}

impl Default for IntakeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "127.0.0.1:9857".to_owned(),
            sites: Vec::new(),
            forms: Vec::new(),
            webhooks: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeSite {
    pub slug: String,
    /// 站点文件所在目录，必须是绝对路径。
    pub root: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeForm {
    pub slug: String,
    pub title: String,
    /// 提交落到哪个已授权的工作区，必须是绝对路径。
    pub workspace: PathBuf,
    /// `continue` 续在同一个会话，`new` 每条提交新开一个。
    pub session: String,
    /// 消息开头的固定说明。收到的字段跟在后面，当作数据而不是指令。
    pub prompt: String,
    pub fields: Vec<IntakeField>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeField {
    pub name: String,
    pub label: String,
    /// `text`、`textarea`、`email`、`choice`。
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    /// 只对 `choice` 有意义。
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntakeWebhook {
    pub slug: String,
    pub workspace: PathBuf,
    pub session: String,
    pub prompt: String,
    /// 密钥从哪个环境变量读。不把密钥写进配置文件。
    pub secret_env: String,
}

/// 一条 hook 的配置。
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookSettings {
    /// 出现在拒绝理由里的名字。不给就用事件名——但配了两条同事件的 hook 时，
    /// 拒绝理由会分不清是谁拦的，所以强烈建议起个名。
    pub name: Option<String>,
    /// `pre_tool` / `post_tool` / `approval_resolved`。
    pub event: String,
    pub command: String,
    /// 非零退出是否拦截。只有 `pre_tool` 上有意义——`post_tool` 发生在事后，
    /// 让它"拦截"只会制造一种拦得住的错觉。
    #[serde(default)]
    pub blocking: bool,
    /// 默认 10 秒。它在关键路径上，给太长等于给 agent 装了个刹车。
    pub timeout_seconds: Option<u64>,
    /// hook 自己超时或起不来时：`deny`（默认）或 `ignore`。
    /// 默认拦，是因为坏掉就自动放行的门禁恰好会在出事时失效。
    pub on_error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSettings {
    /// Yield an unanswered human gate to independent queued work after this
    /// many seconds. Default 300; zero disables automatic yielding.
    pub human_wait_yield_seconds: Option<u64>,
    /// Interactive startup only; explicit prompts and --resume bypass this preference.
    #[serde(default)]
    pub session_startup: crate::session_startup::StartupMode,
    /// Admission ceiling for schedules, including occupied interactive Runtime sessions.
    /// Defaults to 4; preparing runs remain durable while waiting for capacity.
    pub schedule_max_active: Option<usize>,
    #[serde(default)]
    pub verification_commands: Vec<String>,
    pub completion_contract: Option<willdeep_core::tools::CompletionContract>,
    pub max_turns: Option<usize>,
    /// Per-run wall clock budget, including model and tool waits. Omitted means unlimited.
    pub time_budget_seconds: Option<u64>,
    /// 主 Agent 一轮的 token 预算（输入 + 输出累计）。轮次上限放到 200 之后，
    /// 这是唯一的自动闸门；用尽时交出部分结果与交接信息，不判失败。不写不限。
    pub token_budget: Option<u64>,
    pub approval: Option<String>,
    pub language: Option<String>,
    /// Consult the AI judge for commands the static classifier cannot
    /// decide. Defaults to on; turning it off means every ambiguous command
    /// goes straight to an approval card.
    pub safety_judge: Option<bool>,
    /// Model used for the judge. Defaults to the profile's cheap model.
    pub judge_model: Option<String>,
    /// Model used for context-compression summaries. On some.im defaults to
    /// the gateway-hosted `someim-32b-compressor`; elsewhere defaults to the
    /// session model with the inline instruction.
    pub compressor_model: Option<String>,
    /// 自动整理会话标题。默认开：一屏 `New session` 的历史列表等于没有列表，
    /// 而这条链路只在会话**第一轮**花一次便宜调用，不随对话长度增长。
    /// 关掉后标题停在第一条提示词的确定性派生，仍然可读。
    pub auto_title: Option<bool>,
    /// 轮次结束后在空输入框里灰字预测用户的下一句，Tab 采用。不写按开。
    /// 用与标题摘要相同的模型候选，但开关独立于 `auto_title`。
    pub input_suggestions: Option<bool>,
    /// 一个目标（`/goal`）跨轮次、跨重启累计的主 Agent token 上限，用尽时有序
    /// 收尾交接。不写不限——wall-clock 与续推次数仍然兜底。
    pub goal_token_budget: Option<u64>,
    /// 一个目标的实际运行时长上限（分钟）。缺省 240。
    pub goal_wall_clock_minutes: Option<u64>,
    /// 一个目标的续推次数上限。缺省 64。
    pub goal_max_continuations: Option<usize>,
    /// daemon 重启打断了一个还在进行的目标时，自动排一轮续推。缺省开。
    pub goal_auto_resume: Option<bool>,
    /// 会话标题摘要模型。默认取会话模型——标题请求只发一问一答各 800 字，
    /// 成本可忽略，而另指一个端点意味着它可能缺凭据、然后静默退化。
    pub title_model: Option<String>,
    /// Deterministic worker/standard/deep routing. Defaults to enabled.
    pub small_model_routing: Option<bool>,
    /// Dispatch high-confidence read-only requests before the root model sees
    /// the repository. Defaults to enabled when routing is enabled.
    pub auto_dispatch_read_only: Option<bool>,
    /// Admission budget for the scarce deep profile in one harness.
    pub max_deep_calls_per_harness: Option<usize>,
    /// OS 级写入围栏（macOS Seatbelt / Linux bubblewrap）。
    ///
    /// 不写：这台机器有后端就开，没有就不开（并在开屏与 `doctor` 里说明）。
    /// 显式 `true`：没后端时命令拒绝启动（fail closed）。显式 `false`：关。
    /// 默认开之所以敢开，是因为常见工具链缓存默认放行（见 `sandbox_toolchain_caches`），
    /// `cargo fetch` / `npm install` 不会第一天就撞墙。
    pub sandbox: Option<bool>,
    /// 围栏开着时，除工作区、临时目录与默认放行的工具链缓存之外还允许写入的根。
    /// 支持 `~/…`。
    #[serde(default)]
    pub sandbox_writable_roots: Vec<PathBuf>,
    /// 是否默认放行常见工具链缓存（`~/.cargo/registry`、`~/.npm`、`~/.cache` …）。
    /// 不写按放行；关掉后只剩工作区、临时目录与 `sandbox_writable_roots`。
    pub sandbox_toolchain_caches: Option<bool>,
    /// 网络围栏。不写按档位：`read-only` 与 `workspace-write` 断，`strict` / `smart` 通。
    /// `"deny"` 让所有套围栏的命令都断网（需要联网的命令走 `network: true` 由人放行），
    /// `"allow"` 让 `workspace-write` 也通网。`read-only` 永远断。
    pub sandbox_network: Option<SandboxNetwork>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SandboxNetwork {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillSettings {
    #[serde(default)]
    pub roots: Vec<PathBuf>,
}

/// 匿名产品遥测。默认开启，`enabled = false` 关掉；
/// `WILLDEEP_TELEMETRY_DISABLED=1` 优先级更高（CI、容器构建不该被算成用户）。
/// 采集范围与红线见 `crates/willdeep-cli/src/telemetry.rs`。
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetrySettings {
    #[serde(default = "default_telemetry_enabled")]
    pub enabled: bool,
}

impl Default for TelemetrySettings {
    fn default() -> Self {
        Self {
            enabled: default_telemetry_enabled(),
        }
    }
}

fn default_telemetry_enabled() -> bool {
    true
}

/// `[feedback]`：本机反馈账本 `$WILLDEEP_HOME/feedback/*.jsonl`。
///
/// 记下一句建议的采用 / 放弃 / 改写、工具失败、Worker 轮次耗尽等信号，供离线
/// 改进提示词与路由（`docs/FEEDBACK_LEDGER.md`）。默认开，但只记 hash 与长度；
/// `store_text = true` 才写建议与发送的原文，含凭据特征的文本始终不写。
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackSettings {
    #[serde(default = "default_feedback_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub store_text: bool,
    /// 保留最近几个月的分片，更早的在 daemon 启动时删掉；0 表示不清理。
    #[serde(default = "default_feedback_retain_months")]
    pub retain_months: u32,
}

impl Default for FeedbackSettings {
    fn default() -> Self {
        Self {
            enabled: default_feedback_enabled(),
            store_text: false,
            retain_months: default_feedback_retain_months(),
        }
    }
}

fn default_feedback_retain_months() -> u32 {
    12
}

fn default_feedback_enabled() -> bool {
    true
}

/// Deliberately *not* `deny_unknown_fields`: this is the one section both
/// WillDeep.app and the CLI read out of the same file. Rejecting unknown keys
/// would mean the app cannot add a field without breaking every CLI that has
/// not been upgraded yet, which would weld the two release trains together.
/// Unknown keys here are ignored; the model-routing editor patches only its
/// named scalar keys, so desktop-only values survive a TUI/Web save.
#[derive(Clone, Debug, Default, Deserialize)]
#[allow(dead_code)] // Sound fields belong to the desktop client; the CLI only carries them.
pub struct NotificationSettings {
    pub sound: Option<String>,
    pub custom_sound_file: Option<String>,
    pub custom_sound_display_name: Option<String>,
    pub webhook_enabled: Option<bool>,
    pub webhook_url: Option<String>,
    pub webhook_on_task_completed: Option<bool>,
    pub webhook_on_attention_required: Option<bool>,
    /// 除老 webhook 之外的出站渠道。老的 `webhook_*` 字段一个都不动：那份配置
    /// 与桌面端共用，删掉字段会让还没升级的另一端读不懂。
    #[serde(default)]
    pub channels: Vec<NotificationChannel>,
}

/// 一条出站渠道。用内部标记（`kind = "..."`）而不是一堆可选字段拼成的大结构：
/// `kind` 拼错、某个渠道不该有的字段写进来，都在解析阶段报错，不会静默忽略。
///
/// 这里刻意没有 `deny_unknown_fields`：`[notifications]` 整段是与桌面端共用的，
/// 桌面端将来往里加字段时，老 CLI 不能因为不认识就拒绝启动。单个渠道的
/// `smtp` / `resend` 子表是 CLI 自己的，那两层收紧。
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NotificationChannel {
    /// 一次 HTTP POST，报文仍是 `willdeep.webhook.v1`，与老 webhook 同一份。
    Webhook {
        #[serde(default)]
        name: Option<String>,
        url: String,
        /// 直接写在配置里的签名密钥，与 `secret_env` 二选一。内联写法会让
        /// 整份配置文件成为敏感文件（Unix 上要求权限 0600）。
        #[serde(default)]
        secret: Option<String>,
        /// 签名密钥所在的环境变量。设了就对原始请求体做 HMAC-SHA256，带
        /// `X-Willdeep-Signature: sha256=<hex>`——与 `[intake]` 收的方向同一套。
        #[serde(default)]
        secret_env: Option<String>,
        #[serde(default = "all_notification_events")]
        events: Vec<NotificationEventKind>,
    },
    /// 飞书自定义机器人。国际版 Lark 把 `webhook_url` 换成 open.larksuite.com 即可。
    Lark {
        #[serde(default)]
        name: Option<String>,
        webhook_url: String,
        /// 直接写在配置里的签名密钥，与 `secret_env` 二选一。
        #[serde(default)]
        secret: Option<String>,
        /// 机器人开了「签名校验」时的密钥环境变量；没开就留空。
        #[serde(default)]
        secret_env: Option<String>,
        #[serde(default = "all_notification_events")]
        events: Vec<NotificationEventKind>,
    },
    /// 邮件。`smtp` 与 `resend` 二选一，运行时落到同一个 `EmailTransport`。
    Email {
        #[serde(default)]
        name: Option<String>,
        from: String,
        to: Vec<String>,
        #[serde(default = "all_notification_events")]
        events: Vec<NotificationEventKind>,
        #[serde(default)]
        smtp: Option<SmtpChannelSettings>,
        #[serde(default)]
        resend: Option<ResendChannelSettings>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum NotificationEventKind {
    TaskCompleted,
    AttentionRequired,
}

fn all_notification_events() -> Vec<NotificationEventKind> {
    vec![
        NotificationEventKind::TaskCompleted,
        NotificationEventKind::AttentionRequired,
    ]
}

/// 直连 SMTP。支持隐式 TLS（465）、STARTTLS（587）和明文（25，只该用于本机中继）。
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmtpChannelSettings {
    pub host: String,
    /// 不写就按 `tls` 取默认端口：implicit 465、starttls 587、none 25。
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub tls: SmtpTls,
    /// 不写用户名就是匿名中继，不做 AUTH。
    #[serde(default)]
    pub username: Option<String>,
    /// 密码或授权码原文，与 `password_env` 二选一。
    #[serde(default)]
    pub password: Option<String>,
    /// 密码所在的环境变量。
    #[serde(default)]
    pub password_env: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SmtpTls {
    #[default]
    Implicit,
    Starttls,
    None,
}

/// Resend 的 HTTP 发信接口。Resend 自己的 SMTP 用 `smtp` 配就行（host
/// `smtp.resend.com`、用户名 `resend`、密码填 API key）。
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResendChannelSettings {
    /// API Key 原文，与 `api_key_env` 二选一。
    #[serde(default)]
    pub api_key: Option<String>,
    /// API Key 的环境变量名，与 `api_key` 二选一。
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// 覆盖 API 基址，默认 `https://api.resend.com`；测试用它指到本机。
    #[serde(default)]
    pub api_base: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfile {
    pub provider: Option<String>,
    pub api: Option<String>,
    pub api_base: Option<String>,
    pub api_base_env: Option<String>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub model: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub context_window: Option<u64>,
    pub vision_model: Option<String>,
}

/// `[agent] max_turns` 的缺省值与可配置上限。64 轮曾被线上一个 46 次工具调用、
/// 九分多钟的任务用完，只能靠用户说「继续」再开一轮。触顶不判失败、交出部分
/// 结果，所以放得开；跑偏靠 Esc 中止和审批档位兜底。
pub(crate) const DEFAULT_MAX_TURNS: usize = 200;
pub(crate) const MAX_TURNS_CEILING: usize = 1000;
/// `[agent] token_budget` 的合法区间，与子 Agent 的预算区间一致。
pub(crate) const TOKEN_BUDGET_MIN: u64 = 1_000;
pub(crate) const TOKEN_BUDGET_MAX: u64 = 10_000_000;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubagentProfileSettings {
    pub provider_profile: Option<String>,
    pub model: Option<String>,
    pub max_turns: Option<usize>,
    pub context_window: Option<u64>,
    pub token_budget: Option<u64>,
    pub timeout_seconds: Option<u64>,
    pub max_consecutive_failures: Option<usize>,
    pub worktree: Option<String>,
    /// Byte cap on this worker's tool payloads. Keep it proportional to the
    /// window: a payload cap larger than the window is not a cap.
    pub tool_output_limit: Option<usize>,
    /// Attempts a verified run makes before it reports failure and escalates.
    pub max_attempts: Option<usize>,
}

/// 一个工种改名前的段落名，按优先级排。已经写在别人 config 里的
/// `[subagents.deep]` / `[subagents.judge]` 不该因为一次改名就失效。
///
/// 这是**唯一**一处映射：运行时（harness 组装工种）和设置面板（显示、保存）
/// 都从 [`subagent_section_id`] 取段落，两边不可能再对不上——以前运行时只认
/// 正名、面板却回落旧名，面板上显示的模型并不是实际在跑的那个。
pub(crate) fn legacy_subagent_sections(id: &str) -> &'static [&'static str] {
    match id {
        "generalist" => &["reader", "deep"],
        "reviewer" => &["judge"],
        _ => &[],
    }
}

/// 工种 `id` 在 config 里实际生效的段落名：正名优先，其次是改名前的名字；
/// 都没写就是 `None`。
pub(crate) fn subagent_section_id<'a>(file: &'a ConfigFile, id: &str) -> Option<&'a str> {
    std::iter::once(id)
        .chain(legacy_subagent_sections(id).iter().copied())
        .find_map(|name| {
            file.subagents
                .get_key_value(name)
                .map(|(key, _)| key.as_str())
        })
}

/// 工种 `id` 实际生效的那段 `[subagents.*]` 设置。
pub(crate) fn subagent_settings<'a>(
    file: &'a ConfigFile,
    id: &str,
) -> Option<&'a SubagentProfileSettings> {
    subagent_section_id(file, id).and_then(|section| file.subagents.get(section))
}

/// `[worker_tiers.<档>]`：这一档兑现成哪个模型。
///
/// 三个字段都可以只填一部分：只填 `model` 就沿用当前 provider 的端点与凭据
/// 换个模型；填了 `provider_profile` 则整套端点都换（例如专家档走
/// Anthropic，其余仍走网关）。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerTierSettings {
    pub provider_profile: Option<String>,
    pub model: Option<String>,
    pub context_window: Option<u64>,
}

pub struct LoadedConfig {
    pub file: ConfigFile,
}

impl LoadedConfig {
    pub fn load(explicit_path: Option<&Path>) -> Result<Self> {
        let (path, required) = match explicit_path {
            Some(path) => (path.to_path_buf(), true),
            None => (default_config_path()?, false),
        };
        if !path.exists() {
            if required {
                bail!("configuration file does not exist: {}", path.display());
            }
            return Ok(Self {
                file: ConfigFile::default(),
            });
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("read configuration file: {}", path.display()))?;
        let file: ConfigFile = toml::from_str(&contents)
            .with_context(|| format!("parse TOML configuration: {}", path.display()))?;
        validate(&file, &path)?;
        Ok(Self { file })
    }

    pub fn select_provider(&self, requested: Option<&str>) -> Result<Option<&ProviderProfile>> {
        let selected = requested
            .map(str::to_owned)
            .or_else(|| self.file.default_provider.clone())
            .or_else(|| {
                (self.file.providers.len() == 1)
                    .then(|| self.file.providers.keys().next().cloned())
                    .flatten()
            });
        match selected {
            Some(name) => self
                .file
                .providers
                .get(&name)
                .map(Some)
                .with_context(|| format!("provider profile not found in config: {name}")),
            None => Ok(None),
        }
    }
}

pub fn default_config_path() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("WILLDEEP_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home).join("config.toml"));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("cannot find home directory; set WILLDEEP_HOME or pass --config")?;
    Ok(PathBuf::from(home).join(".willdeep").join("config.toml"))
}

pub fn willdeep_home() -> Result<PathBuf> {
    Ok(default_config_path()?
        .parent()
        .context("configuration path has no parent")?
        .to_path_buf())
}

/// 还能解析、但语义可能与用户记忆不同的旧写法。启动时、`config check` 与 doctor
/// 都点名，不再静默映射：线上一份 `approval = "ask"` 让用户每开一个终端都得手动切档。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Deprecation {
    /// `agent.approval` 用了 `ask` / `request-every-time`：等同 `strict`，每次写入、
    /// 命令、联网都问；推荐 `workspace-write`。
    LegacyApproval { spelled: String },
}

impl Deprecation {
    pub fn summary(&self) -> String {
        match self {
            Self::LegacyApproval { spelled } => format!(
                "agent.approval = \"{spelled}\" is a legacy spelling of \"strict\" (every write, command and network call asks); the recommended default is \"workspace-write\" — run `/permissions default workspace-write` in the TUI or edit the file"
            ),
        }
    }
}

pub fn deprecations(file: &ConfigFile) -> Vec<Deprecation> {
    let mut notes = Vec::new();
    if let Some(spelled) = file.agent.approval.as_deref()
        && matches!(spelled, "ask" | "request-every-time")
    {
        notes.push(Deprecation::LegacyApproval {
            spelled: spelled.to_owned(),
        });
    }
    notes
}

/// 公网入口的语义校验。`config check` 和入口启动都走这里，规则只写一份。
pub(crate) fn validate_intake(settings: &IntakeSettings) -> Result<()> {
    settings
        .listen
        .parse::<std::net::SocketAddr>()
        .context("intake.listen must be IP:PORT")?;
    let mut slugs = HashSet::new();
    for site in &settings.sites {
        check_intake_slug("site", &site.slug, &mut slugs)?;
        if !site.root.is_absolute() {
            bail!("intake site {} root must be an absolute path", site.slug);
        }
    }
    for form in &settings.forms {
        check_intake_slug("form", &form.slug, &mut slugs)?;
        check_intake_target(&form.workspace, &form.session, &form.slug)?;
        if form.prompt.trim().is_empty() {
            bail!("intake form {} needs a prompt", form.slug);
        }
        let mut names = HashSet::new();
        for field in &form.fields {
            if field.name.trim().is_empty() || !names.insert(field.name.clone()) {
                bail!(
                    "intake form {} has an empty or repeated field name",
                    form.slug
                );
            }
            if !matches!(
                field.kind.as_str(),
                "text" | "textarea" | "email" | "choice"
            ) {
                bail!(
                    "intake form {} field {} has an unknown type",
                    form.slug,
                    field.name
                );
            }
            if field.kind == "choice" && field.options.is_empty() {
                bail!(
                    "intake form {} field {} needs options",
                    form.slug,
                    field.name
                );
            }
        }
    }
    for hook in &settings.webhooks {
        check_intake_slug("webhook", &hook.slug, &mut slugs)?;
        check_intake_target(&hook.workspace, &hook.session, &hook.slug)?;
        if hook.prompt.trim().is_empty() || hook.secret_env.trim().is_empty() {
            bail!("intake webhook {} needs a prompt and secret_env", hook.slug);
        }
    }
    Ok(())
}

fn check_intake_slug(kind: &str, slug: &str, seen: &mut HashSet<String>) -> Result<()> {
    let ok = !slug.is_empty()
        && slug.len() <= 64
        && slug
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '-');
    if !ok {
        bail!("intake {kind} slug must be letters, digits and dashes: {slug}");
    }
    if !seen.insert(format!("{kind}:{slug}")) {
        bail!("intake {kind} slug is duplicated: {slug}");
    }
    Ok(())
}

fn check_intake_target(workspace: &Path, session: &str, slug: &str) -> Result<()> {
    if !workspace.is_absolute() {
        bail!("intake {slug} workspace must be an absolute path");
    }
    if !matches!(session, "continue" | "new") {
        bail!("intake {slug} session must be continue or new");
    }
    Ok(())
}

/// 出站渠道的配置检查。这里只判「一定错」的事：字段空着、URL 解析不了、
/// 邮件 provider 二选一没选。环境变量在不在**不在这里查**——`config check`
/// 可能跑在没装密钥的机器上；缺密钥是投递时的事，记一条错误、跳过这条渠道，
/// 不能让整份配置拒绝启动。
fn validate_notification_channels(channels: &[NotificationChannel]) -> Result<()> {
    let mut names = HashSet::new();
    for (index, channel) in channels.iter().enumerate() {
        let label = notification_channel_label(channel, index);
        if let Some(name) = notification_channel_name(channel) {
            if name.trim().is_empty() {
                bail!("notifications.channels[{index}].name cannot be empty");
            }
            if !names.insert(name.trim().to_owned()) {
                bail!("notifications.channels[{index}] duplicates the name '{name}'");
            }
        }
        match channel {
            NotificationChannel::Webhook {
                url,
                secret,
                secret_env,
                events,
                ..
            } => {
                check_notification_events(&label, events)?;
                check_http_url(&label, "url", url)?;
                check_inline_or_env_secret(
                    &label,
                    "secret",
                    secret.as_deref(),
                    "secret_env",
                    secret_env.as_deref(),
                )?;
            }
            NotificationChannel::Lark {
                webhook_url,
                secret,
                secret_env,
                events,
                ..
            } => {
                check_notification_events(&label, events)?;
                check_http_url(&label, "webhook_url", webhook_url)?;
                check_inline_or_env_secret(
                    &label,
                    "secret",
                    secret.as_deref(),
                    "secret_env",
                    secret_env.as_deref(),
                )?;
            }
            NotificationChannel::Email {
                from,
                to,
                events,
                smtp,
                resend,
                ..
            } => {
                check_notification_events(&label, events)?;
                check_mailbox(&label, "from", from)?;
                if to.is_empty() {
                    bail!("{label}.to must list at least one recipient");
                }
                for (slot, address) in to.iter().enumerate() {
                    check_mailbox(&label, &format!("to[{slot}]"), address)?;
                }
                match (smtp, resend) {
                    (Some(smtp), None) => {
                        if smtp.host.trim().is_empty() {
                            bail!("{label}.smtp.host cannot be empty");
                        }
                        if smtp
                            .host
                            .chars()
                            .any(|value| value.is_control() || value.is_whitespace())
                        {
                            bail!(
                                "{label}.smtp.host must not contain whitespace or control characters"
                            );
                        }
                        if smtp.port == Some(0) {
                            bail!("{label}.smtp.port must be greater than zero");
                        }
                        check_inline_or_env_secret(
                            &label,
                            "smtp.password",
                            smtp.password.as_deref(),
                            "smtp.password_env",
                            smtp.password_env.as_deref(),
                        )?;
                        let has_secret = smtp.password.is_some() || smtp.password_env.is_some();
                        match (smtp.username.as_deref(), has_secret) {
                            (Some(user), true) => {
                                if user.trim().is_empty() {
                                    bail!("{label}.smtp.username cannot be empty");
                                }
                            }
                            (None, false) => {}
                            _ => bail!(
                                "{label}.smtp needs both username and password/password_env, or neither"
                            ),
                        }
                        // 明文连接上传密码，等于把密码交给整条链路。本机中继
                        // 例外——那是同一个进程边界内的回环连接。
                        if smtp.tls == SmtpTls::None
                            && smtp
                                .username
                                .as_deref()
                                .is_some_and(|user| !user.trim().is_empty())
                            && !is_loopback_host(&smtp.host)
                        {
                            bail!(
                                "{label}.smtp must not send credentials over a plaintext connection; use implicit or starttls"
                            );
                        }
                    }
                    (None, Some(resend)) => {
                        if resend.api_key.is_none() && resend.api_key_env.is_none() {
                            bail!("{label}.resend needs api_key or api_key_env");
                        }
                        check_inline_or_env_secret(
                            &label,
                            "resend.api_key",
                            resend.api_key.as_deref(),
                            "resend.api_key_env",
                            resend.api_key_env.as_deref(),
                        )?;
                        if let Some(base) = resend.api_base.as_deref() {
                            check_http_url(&label, "resend.api_base", base)?;
                        }
                    }
                    (Some(_), Some(_)) => bail!("{label} cannot configure both smtp and resend"),
                    (None, None) => bail!("{label} needs either smtp or resend"),
                }
            }
        }
    }
    Ok(())
}

fn notification_channel_label(channel: &NotificationChannel, index: usize) -> String {
    match notification_channel_name(channel) {
        Some(name) if !name.trim().is_empty() => {
            format!("notifications.channels[{index}] ({})", name.trim())
        }
        _ => format!("notifications.channels[{index}]"),
    }
}

fn notification_channel_name(channel: &NotificationChannel) -> Option<&str> {
    match channel {
        NotificationChannel::Webhook { name, .. }
        | NotificationChannel::Lark { name, .. }
        | NotificationChannel::Email { name, .. } => name.as_deref(),
    }
}

fn check_notification_events(label: &str, events: &[NotificationEventKind]) -> Result<()> {
    if events.is_empty() {
        bail!("{label}.events is empty; remove the channel instead");
    }
    let mut seen = HashSet::new();
    for event in events {
        if !seen.insert(*event) {
            bail!("{label}.events lists the same event twice");
        }
    }
    Ok(())
}

fn check_http_url(label: &str, field: &str, value: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(value.trim())
        .with_context(|| format!("{label}.{field} must be a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("{label}.{field} must use http or https");
    }
    Ok(())
}

/// 只挡一定错的东西：空值、控制字符、没有 `@`。真正的地址语法交给发信服务器，
/// 在这里发明一套更严的正则只会把合法的地址也挡在外面。
fn check_mailbox(label: &str, field: &str, value: &str) -> Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("{label}.{field} cannot be empty");
    }
    if trimmed.chars().any(char::is_control) {
        bail!("{label}.{field} must not contain control characters");
    }
    let address = match trimmed.rsplit_once('<') {
        Some((_, rest)) if rest.trim_end().ends_with('>') => {
            rest.trim().trim_end_matches('>').trim()
        }
        _ => trimmed,
    };
    let Some((local, domain)) = address.rsplit_once('@') else {
        bail!("{label}.{field} must be an email address");
    };
    if local.trim().is_empty()
        || domain.trim().is_empty()
        || address.chars().any(char::is_whitespace)
    {
        bail!("{label}.{field} must be an email address");
    }
    Ok(())
}

fn check_env_reference(label: &str, field: &str, value: Option<&str>) -> Result<()> {
    let Some(value) = value else { return Ok(()) };
    if value.trim().is_empty() {
        bail!("{label}.{field} cannot be empty");
    }
    if value
        .chars()
        .any(|value| !(value.is_ascii_alphanumeric() || value == '_'))
    {
        bail!("{label}.{field} must be an environment variable name");
    }
    Ok(())
}

/// 一条密钥要么内联，要么指向环境变量，不能两头都写。内联值只判空，不 trim：
/// 首尾空格也可能属于密钥本身。
fn check_inline_or_env_secret(
    label: &str,
    inline_field: &str,
    inline: Option<&str>,
    env_field: &str,
    env: Option<&str>,
) -> Result<()> {
    match (inline, env) {
        (Some(_), Some(_)) => bail!("{label} cannot define both {inline_field} and {env_field}"),
        (Some(value), None) if value.trim().is_empty() => {
            bail!("{label}.{inline_field} cannot be empty")
        }
        (Some(_), None) => Ok(()),
        (None, Some(value)) => check_env_reference(label, env_field, Some(value)),
        (None, None) => Ok(()),
    }
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

pub(crate) fn validate(file: &ConfigFile, path: &Path) -> Result<()> {
    file.agent_team.validate().map_err(anyhow::Error::msg)?;
    if let Some(version) = file.version
        && version != CONFIG_VERSION
    {
        bail!(
            "unsupported config version {version} in {}; expected {CONFIG_VERSION}",
            path.display()
        );
    }
    willdeep_core::ToolRegistry::validate_required_verifications(&file.agent.verification_commands)
        .map_err(anyhow::Error::msg)?;
    if file
        .agent
        .schedule_max_active
        .is_some_and(|limit| !(1..=64).contains(&limit))
    {
        bail!("agent.schedule_max_active must be between 1 and 64");
    }
    if let Some(max_turns) = file.agent.max_turns
        && !(1..=MAX_TURNS_CEILING).contains(&max_turns)
    {
        bail!("agent.max_turns must be between 1 and {MAX_TURNS_CEILING}");
    }
    if let Some(budget) = file.agent.token_budget
        && !(TOKEN_BUDGET_MIN..=TOKEN_BUDGET_MAX).contains(&budget)
    {
        bail!("agent.token_budget must be between {TOKEN_BUDGET_MIN} and {TOKEN_BUDGET_MAX}");
    }
    if file
        .agent
        .max_deep_calls_per_harness
        .is_some_and(|value| value > 16)
    {
        bail!("agent.max_deep_calls_per_harness must be between 0 and 16");
    }
    crate::i18n::Language::parse(file.agent.language.as_deref())?;
    if file.local_model.enabled {
        let base_url = reqwest::Url::parse(file.local_model.base_url.trim())
            .context("local_model.base_url must be a valid URL")?;
        if !matches!(base_url.scheme(), "http" | "https") || base_url.host_str().is_none() {
            bail!("local_model.base_url must be an HTTP(S) URL with a host");
        }
        if file.local_model.summary_model.trim().is_empty() {
            bail!("local_model.summary_model must not be empty");
        }
    }
    if file.notifications.webhook_enabled.unwrap_or(false) {
        let webhook_url = file
            .notifications
            .webhook_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("notifications.webhook_url is required when webhook_enabled is true")?;
        let parsed = reqwest::Url::parse(webhook_url)
            .context("notifications.webhook_url must be a valid URL")?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            bail!("notifications.webhook_url must use http or https");
        }
    }
    validate_notification_channels(&file.notifications.channels)?;
    for (name, provider) in &file.providers {
        if provider.api_base.is_some() && provider.api_base_env.is_some() {
            bail!("providers.{name} cannot define both api_base and api_base_env");
        }
        if provider
            .api_base_env
            .as_deref()
            .is_some_and(|name| name.trim().is_empty())
        {
            bail!("providers.{name}.api_base_env cannot be empty");
        }

        if provider.api_key.as_deref().is_some_and(str::is_empty) {
            bail!("providers.{name}.api_key cannot be empty");
        }
        if provider.api_key_env.as_deref().is_some_and(str::is_empty) {
            bail!("providers.{name}.api_key_env cannot be empty");
        }
        if provider.api_key.is_some() && provider.api_key_env.is_some() {
            bail!("providers.{name} cannot define both api_key and api_key_env");
        }
    }
    for (name, subagent) in &file.subagents {
        if !matches!(
            name.as_str(),
            // 五个公开职责 + 改名前的公开名 + 内部专门工种。旧名保留是为了
            // 别人已经写好的 config 不因为一次改名就报「unknown subagent」。
            "generalist"
                | "reviewer"
                | "scout"
                | "reader"
                | "deep"
                | "judge"
                | "editor"
                | "implementer"
                | "test_fixer"
                | "build_fixer"
                | "log_inspector"
                | "git_detective"
                | "tester"
                | "ops_runner"
        ) {
            bail!("unknown subagent profile: {name}");
        }
        // 4K is below any usable worker budget once the system prompt and
        // tool schemas are paid for; past the 1M ceiling nothing real is being
        // described. The ceiling is 2^20, matching the largest budget the
        // settings panel offers — a value the panel can pick must be storable.
        if subagent.context_window.is_some_and(|value| {
            !(willdeep_core::CONTEXT_WINDOW_MIN..=willdeep_core::CONTEXT_WINDOW_MAX)
                .contains(&value)
        }) {
            bail!(
                "subagents.{name}.context_window must be between {} and {}",
                willdeep_core::CONTEXT_WINDOW_MIN,
                willdeep_core::CONTEXT_WINDOW_MAX
            );
        }
        if subagent
            .max_attempts
            .is_some_and(|value| !(1..=6).contains(&value))
        {
            bail!("subagents.{name}.max_attempts must be between 1 and 6");
        }
        if subagent
            .tool_output_limit
            .is_some_and(|value| !(1_024..=131_072).contains(&value))
        {
            bail!("subagents.{name}.tool_output_limit must be between 1024 and 131072");
        }
        // 上限取内置里最能跑的那个工种（ops_runner 的 32）。卡在 24 的时候，
        // 用户连内置默认值都抄不下来——config.example.toml 自己就因此校验
        // 不过，而那份示例正是让人照抄的。
        if subagent
            .max_turns
            .is_some_and(|value| !(1..=32).contains(&value))
        {
            bail!("subagents.{name}.max_turns must be between 1 and 32");
        }
        if subagent
            .token_budget
            .is_some_and(|value| !(1_000..=10_000_000).contains(&value))
        {
            bail!("subagents.{name}.token_budget must be between 1000 and 10000000");
        }
        if subagent
            .timeout_seconds
            .is_some_and(|value| !(10..=86_400).contains(&value))
        {
            bail!("subagents.{name}.timeout_seconds must be between 10 and 86400");
        }
        if subagent
            .max_consecutive_failures
            .is_some_and(|value| !(1..=20).contains(&value))
        {
            bail!("subagents.{name}.max_consecutive_failures must be between 1 and 20");
        }
        if subagent
            .worktree
            .as_deref()
            .is_some_and(|value| !matches!(value, "shared" | "dedicated"))
        {
            bail!("subagents.{name}.worktree must be shared or dedicated");
        }
        if let Some(provider) = &subagent.provider_profile
            && !file.providers.contains_key(provider)
        {
            bail!("subagents.{name}.provider_profile not found: {provider}");
        }
    }
    for (name, tier) in &file.worker_tiers {
        // 只认正名。`deep`/`basic` 这些别名在 `worker_tier` 参数上还通，但做
        // 段名就意味着 `[worker_tiers.deep]` 与 `[worker_tiers.expert]` 可以
        // 同时存在却指向同一档——与其定一条谁赢的规则，不如不许写。
        if !willdeep_core::WorkerTier::ALL
            .iter()
            .any(|value| value.as_str() == name)
        {
            bail!("unknown worker tier: {name} (expected standard, advanced or expert)");
        }
        if tier.model.as_deref().is_some_and(|value| value.is_empty()) {
            bail!("worker_tiers.{name}.model cannot be empty");
        }
        if tier.context_window.is_some_and(|value| {
            !(willdeep_core::CONTEXT_WINDOW_MIN..=willdeep_core::CONTEXT_WINDOW_MAX)
                .contains(&value)
        }) {
            bail!(
                "worker_tiers.{name}.context_window must be between {} and {}",
                willdeep_core::CONTEXT_WINDOW_MIN,
                willdeep_core::CONTEXT_WINDOW_MAX
            );
        }
        if let Some(provider) = &tier.provider_profile
            && !file.providers.contains_key(provider)
        {
            bail!("worker_tiers.{name}.provider_profile not found: {provider}");
        }
    }
    for (name, server) in &file.mcp_servers {
        // 传输方式、url 协议、头里的凭据写法：规则只在 core 里定一份，连接时再校一遍。
        server
            .validate(name)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    }
    validate_intake(&file.intake)?;
    for root in &file.agent.sandbox_writable_roots {
        // 相对路径相对谁？工作区、家目录、当前目录都说得通，所以谁都不算——
        // 一条被猜错基准的放行等于放行了别的目录。
        if !crate::harness::expand_home(root).is_absolute() {
            bail!(
                "agent.sandbox_writable_roots entry {} must be absolute (or start with ~/)",
                root.display()
            );
        }
    }
    enforce_secret_file_permissions(file, path)
}

#[cfg(unix)]
fn enforce_secret_file_permissions(file: &ConfigFile, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let has_inline_provider_key = file
        .providers
        .values()
        .any(|provider| provider.api_key.is_some());
    if !has_inline_provider_key && !has_inline_notification_secrets(&file.notifications) {
        return Ok(());
    }
    let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        bail!(
            "{} contains an inline secret but permissions are {:o}; run `chmod 600 {}` or use the *_env fields",
            path.display(),
            mode,
            path.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn has_inline_notification_secrets(settings: &NotificationSettings) -> bool {
    settings.channels.iter().any(|channel| match channel {
        NotificationChannel::Webhook { secret, .. } | NotificationChannel::Lark { secret, .. } => {
            secret.is_some()
        }
        NotificationChannel::Email { smtp, resend, .. } => {
            smtp.as_ref().is_some_and(|smtp| smtp.password.is_some())
                || resend
                    .as_ref()
                    .is_some_and(|resend| resend.api_key.is_some())
        }
    })
}

#[cfg(not(unix))]
fn enforce_secret_file_permissions(_file: &ConfigFile, _path: &Path) -> Result<()> {
    Ok(())
}

impl AgentSettings {
    pub(crate) fn runtime_parameters(
        &self,
        max_turns: usize,
    ) -> willdeep_core::runtime_parameters::RuntimeParameters {
        willdeep_core::runtime_parameters::RuntimeParameters {
            max_turns,
            token_budget: self.token_budget,
            goal_token_budget: self.goal_token_budget,
            goal_wall_clock_minutes: self.goal_wall_clock_minutes.unwrap_or(240),
            goal_max_continuations: self.goal_max_continuations.unwrap_or(64),
            input_suggestions: self.input_suggestions.unwrap_or(true),
            small_model_routing: self.small_model_routing.unwrap_or(true),
            auto_dispatch_read_only: self.auto_dispatch_read_only.unwrap_or(true),
            max_deep_calls_per_harness: self.max_deep_calls_per_harness.unwrap_or(1),
            ..Default::default()
        }
    }
    /// `[agent] goal_*` 配置对应的目标预算。
    pub(crate) fn goal_budget(&self) -> willdeep_core::GoalBudget {
        let defaults = willdeep_core::GoalBudget::default();
        willdeep_core::GoalBudget {
            wall_clock: self
                .goal_wall_clock_minutes
                .map(|minutes| std::time::Duration::from_secs(minutes.saturating_mul(60)))
                .or(defaults.wall_clock),
            max_continuations: self
                .goal_max_continuations
                .unwrap_or(defaults.max_continuations),
            max_tokens: self.goal_token_budget.filter(|tokens| *tokens > 0),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn automatic_read_only_dispatch_defaults_on_and_preserves_an_explicit_opt_out() {
        let defaults = super::AgentSettings::default();
        assert!(defaults.runtime_parameters(128).auto_dispatch_read_only);
        let disabled = super::AgentSettings {
            auto_dispatch_read_only: Some(false),
            ..defaults
        };
        assert!(!disabled.runtime_parameters(128).auto_dispatch_read_only);
    }
    use super::*;

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
    fn parses_explicit_verification_commands_and_defaults_to_no_contract() {
        let configured: ConfigFile =
            toml::from_str("[agent]\nverification_commands = ['cargo test --workspace']\n")
                .unwrap();
        assert_eq!(
            configured.agent.verification_commands,
            vec!["cargo test --workspace"]
        );
        let defaults: ConfigFile = toml::from_str("[agent]\nmax_turns = 4\n").unwrap();
        assert!(defaults.agent.verification_commands.is_empty());
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
                toml::from_str(&format!("version = 1\n[agent]\napproval = \"{spelled}\"\n"))
                    .unwrap();
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
        let loaded = LoadedConfig { file: parsed };
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
        let parsed: ConfigFile = toml::from_str(include_str!("../../../config.example.toml"))
            .expect("parse config.example.toml");
        assert_eq!(parsed.mcp_servers.len(), 2);
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
    fn init_is_private_and_never_overwrites() {
        let root = std::env::temp_dir().join(format!("willdeep-config-{}", uuid::Uuid::new_v4()));
        let path = root.join("nested/config.toml");
        init(&path).unwrap();
        assert!(path.exists());
        assert!(init(&path).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
