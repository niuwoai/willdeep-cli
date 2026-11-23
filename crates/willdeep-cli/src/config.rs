use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::Deserialize;
use willdeep_core::McpServerConfig;

pub const CONFIG_VERSION: u32 = 1;

pub(crate) mod browser;
#[cfg(test)]
mod init_tests;
mod mcp_files;
mod mcp_migration;
mod redaction;
pub(crate) mod search;
#[cfg(test)]
mod subagent_tests;
mod ui_settings;
use redaction::redact_inline_secrets;
#[cfg(test)]
pub use ui_settings::InputSuggestionColor;
pub use ui_settings::MAX_LINE_SPACING;
pub use ui_settings::{UiMode, UiSettings};
pub use ui_settings::{load_line_spacing, save_line_spacing};
pub use ui_settings::{load_input_breathing, save_input_breathing};
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
    /// 全局规则。`None` 表示配置里从没写过 `[rules]`，按旧行为读
    /// `~/.willdeep/CLAUDE.md`；`Some` 则严格用这份列表，哪怕它是空的。
    #[serde(default)]
    pub rules: Option<RuleSettings>,
    /// 匿名产品遥测；缺省即开启。
    #[serde(default)]
    pub telemetry: TelemetrySettings,
    /// 本机反馈账本（RSI 强反馈数据）。只写本机、不上传。
    #[serde(default)]
    pub feedback: FeedbackSettings,
    /// Runtime Daemon 持久化状态（事件流归档等）。
    #[serde(default)]
    pub runtime: RuntimeSettings,
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
    /// Web 监听地址。显式 `--listen` 和已启用的服务记录优先于它。
    #[serde(default)]
    pub web: WebSettings,
    #[serde(default)]
    pub browser: browser::BrowserSettings,
    #[serde(default)]
    pub search: search::SearchConfig,
    /// 实例联邦：两台公网可达的 willdeep 互发消息。挂在入口监听上。
    #[serde(default)]
    pub federation: FederationSettings,
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

pub(crate) const DEFAULT_WEB_LISTEN: &str = "127.0.0.1:9847";

/// Web 监听地址。缺省 `127.0.0.1:9847`：不写这一段就听本机。
///
/// 只接受 IP:PORT。主机名（`localhost`）不行，否则同一份配置在 IPv4 与 IPv6
/// 环境下会解析成不同的地址。
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WebSettings {
    pub listen: String,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            listen: DEFAULT_WEB_LISTEN.to_owned(),
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
    /// 访客最终访问的地址，含协议，比如 `https://intake.example.com`。
    ///
    /// 分享、站点和表单链接都以它为准；前面的反向代理把域名转到这里。
    /// 留空时按实际监听地址拼——只适合本机自己打开的场景。
    pub public_url: String,
    pub sites: Vec<IntakeSite>,
    pub forms: Vec<IntakeForm>,
    pub webhooks: Vec<IntakeWebhook>,
}

impl Default for IntakeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "127.0.0.1:9857".to_owned(),
            public_url: String::new(),
            sites: Vec::new(),
            forms: Vec::new(),
            webhooks: Vec::new(),
        }
    }
}

/// 实例联邦。挂在公网入口那个监听上，所以开联邦必须同时开入口。
///
/// 联系人、共享密钥和每段对话对应的会话都在 `$WILLDEEP_HOME/federation.json`，
/// 不写进配置：配置会被脚本整段重写，密钥不该跟着注释一起消失。
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FederationSettings {
    pub enabled: bool,
    /// 联邦对话落在哪个工作区。必须是绝对路径；空串表示还没配。
    pub workspace: String,
    /// 自动回复策略：`always` 每条都回，`mention` 只在被 `@名字` 点到时回，
    /// `never` 从不自动回（手动 `/fed say` 仍然可用）。
    pub reply: String,
    /// 一段对话里连续自动回复的上限。到顶后只收人工消息，防止两台 Agent 互相刷屏。
    pub max_auto_turns: u32,
}

impl Default for FederationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            workspace: String::new(),
            reply: "always".to_owned(),
            max_auto_turns: crate::federation::DEFAULT_MAX_AUTO_TURNS,
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

pub(crate) const INTAKE_FORM_TOKEN_FIELD: &str = "__willdeep_form_token";

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
    /// `text`、`textarea`、`email`、`tel`、`url`、`date`、`number`、`choice`。
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    /// 只对 `choice` 有意义。
    #[serde(default)]
    pub options: Vec<String>,
    /// 只对 `number` 有意义：是否接受小数，为假时只收整数。
    #[serde(default)]
    pub allow_decimal: bool,
    /// 只对 `number` 有意义：允许的范围，含端点。缺省表示这一端不限。
    #[serde(default, deserialize_with = "deserialize_optional_number")]
    pub min: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_number")]
    pub max: Option<f64>,
}

/// 表单字段可以用的类型。与入口页渲染的控件、提交时的校验一一对应。
pub(crate) const INTAKE_FIELD_KINDS: &[&str] = &[
    "text", "textarea", "email", "tel", "url", "date", "number", "choice",
];

/// TOML 里 `min = 1` 和 `min = 1.5` 都得认。serde 直接反序列化 `Option<f64>`
/// 遇到整数会报类型错误，所以两种数值都收下来再统一成 f64。
fn deserialize_optional_number<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number {
        Int(i64),
        Float(f64),
    }
    Ok(
        Option::<Number>::deserialize(deserializer)?.map(|value| match value {
            Number::Int(value) => value as f64,
            Number::Float(value) => value,
        }),
    )
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
    /// `before_agent` / `agent_end` / `pre_tool` / `post_tool` / `approval_resolved`。
    pub event: String,
    pub command: String,
    /// 非零退出是否拦截。仅 `pre_tool` / `before_agent` 有效；其余事件发生在事后，
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
    /// 压缩专用的 Provider 档案名（`[providers.<id>]`）。设了之后压缩走该档案的
    /// 端点、协议、密钥、模型与窗口，主模型仍留在会话 Provider 上——云端主模型
    /// 配本机压缩器就是这一项。`compressor_model` 仍可覆盖档案里的模型名。
    pub compressor_profile: Option<String>,
    /// 压缩摘要器能吃的原料上限所用的窗口。不写时取压缩档案里的
    /// `context_window`；两处都没有才回落到会话窗口。本地模型窗口远小于云端
    /// 会话时必须写，否则一份按会话窗口切出来的原料会被本地端点拒收。
    pub compressor_context_window: Option<u64>,
    /// 单个候选一次摘要请求最多等这么多秒。不写是 60 秒，按托管小压缩器定的；
    /// 慢推理模型（如 gpt-6.1-sol、grok-4.7）做兜底时要调大。
    pub compressor_candidate_timeout_seconds: Option<u64>,
    /// 一次压缩的总预算（全部分块、归并、二次压缩共用）。不写时按实际切出来的
    /// 块数加归并和二次压缩各一次来算，每段都给足候选等待。写了也不能短于候选等待。
    pub compressor_timeout_seconds: Option<u64>,
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

/// 全局规则列表。每一项是一个文件或一个目录；目录里的 Markdown 按文件名拼接。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSettings {
    #[serde(default)]
    pub files: Vec<String>,
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

/// `[runtime]`：本地 Runtime Daemon 的维护策略。
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSettings {
    #[serde(default)]
    pub events: EventLogSettings,
}

/// `runtime.events`：`events.ndjson` 全机事件总线的归档与重新开始。
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventLogSettings {
    /// 活跃日志超过此大小时，在 daemon 启动时整文件移到 backups 并新建空日志；0 表示关闭。
    #[serde(default = "default_event_rotate_bytes")]
    pub rotate_bytes: u64,
    /// 在 backups 里最多保留几份 `runtime-events-*.ndjson` 归档。
    #[serde(default = "default_event_retain_archives")]
    pub retain_archives: u32,
}

impl Default for EventLogSettings {
    fn default() -> Self {
        Self {
            rotate_bytes: default_event_rotate_bytes(),
            retain_archives: default_event_retain_archives(),
        }
    }
}

fn default_event_rotate_bytes() -> u64 {
    32 * 1024 * 1024
}

fn default_event_retain_archives() -> u32 {
    8
}

pub fn event_log_settings_from_config(
    loaded: &std::result::Result<LoadedConfig, anyhow::Error>,
) -> EventLogSettings {
    loaded
        .as_ref()
        .map(|value| value.file.runtime.events.clone())
        .unwrap_or_default()
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
    #[serde(default)]
    pub reasoning: Option<willdeep_core::provider::ReasoningProfile>,
}

/// `[agent] max_turns` 的缺省值与可配置上限。64 轮曾被线上一个 46 次工具调用、
/// 九分多钟的任务用完，只能靠用户说「继续」再开一轮。触顶不判失败、交出部分
/// 结果，所以放得开；跑偏靠 Esc 中止和审批档位兜底。
pub(crate) const DEFAULT_MAX_TURNS: usize = 200;
pub(crate) const MAX_TURNS_CEILING: usize = 1000;
/// `[agent] token_budget` 的合法区间，与子 Agent 的预算区间一致。
pub(crate) const TOKEN_BUDGET_MIN: u64 = 1_000;
pub(crate) const TOKEN_BUDGET_MAX: u64 = 10_000_000;
/// `[agent] compressor_candidate_timeout_seconds` 的缺省与区间。60 秒按托管小
/// 压缩器定；上限 180 秒，再慢的模型不该占着这一段不放。
pub(crate) const COMPRESSION_CANDIDATE_TIMEOUT_DEFAULT_SECONDS: u64 = 60;
pub(crate) const COMPRESSION_CANDIDATE_TIMEOUT_MIN_SECONDS: u64 = 15;
pub(crate) const COMPRESSION_CANDIDATE_TIMEOUT_MAX_SECONDS: u64 = 180;
/// `[agent] compressor_timeout_seconds` 的上限。一次卡住的压缩不能把整个回合
/// 冻住十几分钟；600 秒是普通请求原来的上限。
pub(crate) const COMPRESSION_TIMEOUT_MAX_SECONDS: u64 = 600;

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

static RUNTIME_CONFIG_CACHE: LazyLock<Mutex<HashMap<PathBuf, LoadedConfig>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn resolve_config_path(explicit_path: Option<&Path>) -> Result<(PathBuf, bool)> {
    match explicit_path {
        Some(path) => Ok((path.to_path_buf(), true)),
        None => default_config_path().map(|path| (path, false)),
    }
}

#[derive(Clone)]
pub struct LoadedConfig {
    pub file: ConfigFile,
    /// 读到了但没采用的 MCP 文件。主配置解析不受影响；启动时另行列出。
    pub mcp_skipped: Vec<String>,
}

impl LoadedConfig {
    pub fn load(explicit_path: Option<&Path>) -> Result<Self> {
        let (path, required) = resolve_config_path(explicit_path)?;
        if !path.exists() {
            if required {
                bail!("configuration file does not exist: {}", path.display());
            }
            // 第一次启动：规则目录还不存在时，把内置规则落进去。配置里没有
            // [rules] 时它不会自动生效，由配置页提示用户启用。目录已存在说明
            // 用户有自己的规则，什么都不做。失败不挡住启动。
            let _ = willdeep_core::rules::seed_bundled_rules();
            let mut file = ConfigFile::default();
            let none = mcp_migration::LegacyMcp::Absent;
            let skipped = mcp_migration::attach(&mut file, &path, "", none)?;
            return Ok(Self {
                file,
                mcp_skipped: skipped,
            });
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("read configuration file: {}", path.display()))?;
        // MCP 段先按宽松文档拆出来。它的字段不合法不能再让整份模型配置
        // 解析失败：拆不出来的段只告警，主配置照常读。
        let legacy = mcp_migration::take_legacy(&contents);
        let file = parse_config_text(mcp_migration::parseable(&contents, &legacy))
            .with_context(|| format!("parse TOML configuration: {}", path.display()))?;
        validate(&file, &path)?;
        let mut file = file;
        let skipped = mcp_migration::attach(&mut file, &path, &contents, legacy)?;
        Ok(Self {
            file,
            mcp_skipped: skipped,
        })
    }

    /// 长驻 Runtime（daemon / 进行中的会话）用：重载失败时沿用该路径上一次成功解析的快照。
    pub fn load_runtime(explicit_path: Option<&Path>) -> Result<Self> {
        let (path, _) = resolve_config_path(explicit_path)?;
        match Self::load(explicit_path) {
            Ok(loaded) => {
                if let Ok(mut cache) = RUNTIME_CONFIG_CACHE.lock() {
                    cache.insert(path, loaded.clone());
                }
                Ok(loaded)
            }
            Err(error) => {
                if let Ok(cache) = RUNTIME_CONFIG_CACHE.lock()
                    && let Some(cached) = cache.get(&path)
                {
                    eprintln!(
                        "warning: configuration reload failed ({}): {error:#}; continuing with last successfully loaded configuration",
                        path.display()
                    );
                    return Ok(cached.clone());
                }
                Err(error)
            }
        }
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

/// 解析用户的主配置。
///
/// MCP 定义已经挪到 `$WILLDEEP_HOME/.mcp/<name>.toml`。主配置里残留的
/// `[mcp_servers.*]` 先按行摘掉再解析：一段写坏（多一个不认识的字段、
/// `transport = "sse"` 这种）不能再让模型配置读不出来。摘掉只影响这次
/// 解析，不改磁盘；真正迁走发生在 [`LoadedConfig::load`]。
///
/// 主配置自己的语法错误原样抛出，不拿 MCP 当挡箭牌。
pub(crate) fn parse_config_text(contents: &str) -> Result<ConfigFile, toml::de::Error> {
    let value: toml::Value = toml::from_str(contents)?;
    let has_mcp = value
        .as_table()
        .and_then(|table| table.get("mcp_servers"))
        .is_some_and(|servers| !servers.as_table().is_some_and(toml::map::Map::is_empty));
    if !has_mcp {
        return value.try_into();
    }
    toml::from_str(&crate::model_routing::strip_mcp_sections(contents))
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
    /// 命令、联网都问；推荐 `smart`。
    LegacyApproval { spelled: String },
}

impl Deprecation {
    pub fn summary(&self) -> String {
        match self {
            Self::LegacyApproval { spelled } => format!(
                "agent.approval = \"{spelled}\" is a legacy spelling of \"strict\" (every write, command and network call asks); the recommended default is \"smart\" — run `/permissions default smart` in the TUI or edit the file"
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

/// 实例联邦的语义校验。联邦路由挂在入口监听上，所以开联邦必须同时开入口；
/// 落点工作区必须是绝对路径，否则 `canonicalize` 会按 daemon 的当前目录去猜。
pub(crate) fn validate_federation(
    settings: &FederationSettings,
    intake: &IntakeSettings,
) -> Result<()> {
    if !settings.enabled {
        return Ok(());
    }
    if !intake.enabled {
        bail!(
            "[federation] enabled requires [intake] enabled: peers are reached through the intake listener"
        );
    }
    if intake.public_url.trim().is_empty() {
        bail!("[federation] needs [intake] public_url so the peer can reach this machine");
    }
    check_intake_public_url(&intake.public_url)?;
    let workspace = PathBuf::from(settings.workspace.trim());
    if !workspace.is_absolute() {
        bail!("[federation] workspace must be an absolute path");
    }
    if !matches!(settings.reply.as_str(), "always" | "mention" | "never") {
        bail!("[federation] reply must be always, mention or never");
    }
    if !(1..=50).contains(&settings.max_auto_turns) {
        bail!("[federation] max_auto_turns must be between 1 and 50");
    }
    Ok(())
}

/// 监听地址只认 IP:PORT。主机名交给调用方自己解析，不在这里做 DNS。
pub(crate) fn parse_listen_addr(field: &str, value: &str) -> Result<std::net::SocketAddr> {
    value
        .parse()
        .with_context(|| format!("{field} must be IP:PORT"))
}

/// Web 监听地址的语义校验。`config check` 走这里，启动时再校一遍。
pub(crate) fn validate_web(settings: &WebSettings) -> Result<()> {
    parse_listen_addr("web.listen", &settings.listen)?;
    Ok(())
}

/// Web 监听地址的三级优先级：显式指定，然后是已启用服务记下的地址，
/// 最后才是配置里的 `web.listen`。
///
/// 已停用的服务不该在这里赢：调用方只在服务启用时才传入它的地址。
pub(crate) fn resolve_web_listen(
    explicit: Option<std::net::SocketAddr>,
    enabled_service_listen: Option<std::net::SocketAddr>,
    config_listen: &str,
) -> Result<std::net::SocketAddr> {
    if let Some(listen) = explicit {
        return Ok(listen);
    }
    if let Some(listen) = enabled_service_listen {
        return Ok(listen);
    }
    parse_listen_addr("web.listen", config_listen)
}

/// 公网入口的语义校验。`config check` 和入口启动都走这里，规则只写一份。
pub(crate) fn validate_intake(settings: &IntakeSettings) -> Result<()> {
    parse_listen_addr("intake.listen", &settings.listen)?;
    if !settings.public_url.trim().is_empty() {
        check_intake_public_url(&settings.public_url)?;
    }
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
            if field.name == INTAKE_FORM_TOKEN_FIELD {
                bail!("intake form {} uses a reserved field name", form.slug);
            }
            if field.name.trim().is_empty() || !names.insert(field.name.clone()) {
                bail!(
                    "intake form {} has an empty or repeated field name",
                    form.slug
                );
            }
            if !INTAKE_FIELD_KINDS.contains(&field.kind.as_str()) {
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
            if field.kind != "number"
                && (field.allow_decimal || field.min.is_some() || field.max.is_some())
            {
                bail!(
                    "intake form {} field {} sets number options on a {} field",
                    form.slug,
                    field.name,
                    field.kind
                );
            }
            for (bound, value) in [("min", field.min), ("max", field.max)] {
                if let Some(value) = value
                    && !value.is_finite()
                {
                    bail!(
                        "intake form {} field {} has a non-finite {bound}",
                        form.slug,
                        field.name
                    );
                }
            }
            if let (Some(min), Some(max)) = (field.min, field.max)
                && min > max
            {
                bail!(
                    "intake form {} field {} has min greater than max",
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

/// `public_url` 是拼给访客的完整前缀，只能是一个 origin：带协议和主机，
/// 不带路径、查询、片段或凭据。带路径会让 `/share/...` 的拼接结果和入口
/// 实际挂载的路径对不上。
fn check_intake_public_url(value: &str) -> Result<()> {
    let trimmed = value.trim();
    let parsed = reqwest::Url::parse(trimmed)
        .with_context(|| format!("intake.public_url must be a valid URL: {trimmed}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("intake.public_url must use http or https and include a host");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("intake.public_url must not carry credentials");
    }
    // 只接受 scheme + authority 的形状：协议后面不能再出现路径、查询、片段或
    // 反斜杠。判断看原串而不是归一化后的 URL——`https://x\`、`https://x?`
    // 会被解析器改写成别的形状，原样拼上 `/share/...` 就是一条打不开的链接。
    // 用结构判断而不是和规范化 origin 比字符串，是为了放行大小写、显式默认端口
    // 和国际化域名这些浏览器能正确处理的写法。
    let authority = trimmed
        .find("://")
        .map(|index| &trimmed[index + 3..])
        .ok_or_else(|| anyhow::anyhow!("intake.public_url must start with http:// or https://"))?;
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.is_empty()
        || authority.chars().any(|char| {
            matches!(char, '/' | '\\' | '?' | '#') || char.is_whitespace() || char.is_control()
        })
    {
        bail!("intake.public_url must be an origin without a path, query or fragment");
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
    if file
        .agent
        .compressor_profile
        .as_deref()
        .is_some_and(|profile| profile.trim().is_empty())
    {
        bail!("agent.compressor_profile must not be empty");
    }
    if let Some(profile) = file.agent.compressor_profile.as_deref().map(str::trim)
        && !file.providers.contains_key(profile)
    {
        bail!(
            "agent.compressor_profile names a provider profile that is not configured: {profile}"
        );
    }
    if let Some(window) = file.agent.compressor_context_window
        && !(willdeep_core::CONTEXT_WINDOW_MIN..=willdeep_core::CONTEXT_WINDOW_MAX)
            .contains(&window)
    {
        bail!(
            "agent.compressor_context_window must be between {} and {}",
            willdeep_core::CONTEXT_WINDOW_MIN,
            willdeep_core::CONTEXT_WINDOW_MAX
        );
    }
    let candidate_timeout = file
        .agent
        .compressor_candidate_timeout_seconds
        .unwrap_or(COMPRESSION_CANDIDATE_TIMEOUT_DEFAULT_SECONDS);
    if !(COMPRESSION_CANDIDATE_TIMEOUT_MIN_SECONDS..=COMPRESSION_CANDIDATE_TIMEOUT_MAX_SECONDS)
        .contains(&candidate_timeout)
    {
        bail!(
            "agent.compressor_candidate_timeout_seconds must be between {COMPRESSION_CANDIDATE_TIMEOUT_MIN_SECONDS} and {COMPRESSION_CANDIDATE_TIMEOUT_MAX_SECONDS}"
        );
    }
    let timeout = file
        .agent
        .compressor_timeout_seconds
        .unwrap_or(candidate_timeout);
    if timeout > COMPRESSION_TIMEOUT_MAX_SECONDS {
        bail!("agent.compressor_timeout_seconds must be at most {COMPRESSION_TIMEOUT_MAX_SECONDS}");
    }
    if timeout < candidate_timeout {
        bail!(
            "agent.compressor_timeout_seconds ({timeout}) must be at least the candidate timeout ({candidate_timeout})"
        );
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
        if let Some(reasoning) = &provider.reasoning {
            reasoning
                .validate()
                .map_err(|error| anyhow::anyhow!("providers.{name}.reasoning: {error}"))?;
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
            let language = crate::i18n::Language::parse(file.agent.language.as_deref())?;
            bail!(
                "unknown subagent profile: {name} ({}; [subagents.{name}])\n{}",
                path.display(),
                language.subagent_configuration_hint()
            );
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
        // 与 `[agent] max_turns` 同一个上限。早先卡在内置最大值 32，那是
        // 为了让 ops_runner 的默认值能写进示例，不是运行时的能力边界：
        // 跑超上限只是交出部分结果，不判失败。两边不一致的代价是配置页
        // 能存下 128，下一次任务加载时才报错。
        if subagent
            .max_turns
            .is_some_and(|value| !(1..=MAX_TURNS_CEILING).contains(&value))
        {
            bail!("subagents.{name}.max_turns must be between 1 and {MAX_TURNS_CEILING}");
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
    validate_web(&file.web)?;
    file.browser.validate().map_err(anyhow::Error::msg)?;
    file.search.validate()?;
    validate_federation(&file.federation, &file.intake)?;
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
mod tests;
