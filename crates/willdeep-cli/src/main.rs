use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use base64::Engine;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use willdeep_core::provider::{ApiDialect, ProviderConfig, ProviderKind};
use willdeep_core::{
    AgentEvent, ApprovalDecision, Approver, ContinuationRung, EventSink, SoftStopReason,
    SubagentLifecycleStatus, UserQuestion,
};

mod agent_metrics;
mod agent_registry;
mod agent_rsi;
mod audit_cmd;
mod builtin_plugins;
mod cli_defaults;
mod composer_references;
mod config;
mod config_plugin;
mod configuration_host;
mod daemon;
mod detached_delivery;
mod doctor;
mod editor;
mod event_cmd;
mod favorites_plugin;
mod federation;
mod feedback_cmd;
mod feedback_review_cmd;
mod gallery;
mod handoff_cmd;
mod harness;
mod i18n;
mod identity;
mod identity_cmd;
mod integrations;
mod job_cmd;
mod local_provider_discovery;
mod magpie_integration;
mod mcp_cmd;
mod mobile;
mod model_defaults;
mod model_routing;
mod notify;
mod onboarding;
mod plugin_ai_media;
mod plugin_capabilities;
mod plugin_cmd;
mod plugin_gateway;
mod plugin_host_requests;
mod plugin_production;
mod plugin_roundtable;
mod plugin_screenplay;
mod plugin_web;
mod projects;
mod prompt_cmd;
mod provider_settings;
mod qterm;
mod schedule_cmd;
mod screenplay_cmd;
mod session_startup;
mod shares;
mod telemetry;
mod titling;
mod tui;
mod usage_cmd;
mod video_studio_plugin;
mod web;
mod workspace_approval_startup;

use config::{LoadedConfig, ProviderProfile, willdeep_home};

#[derive(Parser, Debug)]
#[command(
    name = "willdeep",
    version,
    about = "Cross-platform WillDeep coding agent"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,

    /// Task for the agent. Reads stdin when omitted.
    #[arg(value_name = "PROMPT", num_args = 0.., trailing_var_arg = true)]
    prompt: Vec<String>,

    /// TOML configuration path. Defaults to $WILLDEEP_HOME/config.toml or ~/.willdeep/config.toml.
    #[arg(short = 'c', long, env = "WILLDEEP_CONFIG", global = true)]
    config: Option<PathBuf>,

    /// Provider profile name from the TOML configuration.
    #[arg(short = 'p', long, global = true)]
    profile: Option<String>,

    /// Provider API base, for example https://some.im/v1.
    #[arg(long, env = "WILLDEEP_API_BASE", global = true)]
    api_base: Option<String>,

    /// Provider API key. Prefer the environment variable to shell history.
    #[arg(long, env = "WILLDEEP_API_KEY", hide_env_values = true, global = true)]
    api_key: Option<String>,

    /// Model identifier sent to the provider.
    #[arg(short = 'm', long, env = "WILLDEEP_MODEL", global = true)]
    model: Option<String>,

    /// Provider identity controls authentication and some.im context headers.
    #[arg(long, value_enum, global = true)]
    provider: Option<ProviderArg>,

    /// Wire API dialect. Auto selects Anthropic Messages only for api.anthropic.com; otherwise Chat Completions.
    #[arg(long = "api", value_enum, global = true)]
    api: Option<ApiArg>,

    /// Workspace root available to tools.
    #[arg(short = 'w', long, global = true)]
    workspace: Option<PathBuf>,

    /// Additional workspace allowed in Web mode. May be repeated.
    #[arg(long = "web-workspace")]
    web_workspaces: Vec<PathBuf>,

    /// Allow create/edit inside the workspace without approval. Shell and MCP still ask.
    #[arg(long, global = true)]
    full_auto: bool,

    /// Maximum model/tool rounds.
    #[arg(long, global = true)]
    max_turns: Option<usize>,

    /// Maximum output tokens for Anthropic Messages.
    #[arg(long, global = true)]
    max_output_tokens: Option<u32>,

    /// Emit newline-delimited JSON events on stdout.
    #[arg(long)]
    json: bool,

    /// Resume a saved session. Omit the value to load the most recently updated session.
    #[arg(
        short = 'r',
        visible_short_alias = 's',
        long,
        visible_alias = "session",
        value_name = "ID|latest",
        num_args = 0..=1,
        default_missing_value = "latest"
    )]
    resume: Option<String>,

    /// Open a new session, bypassing the project startup chooser.
    #[arg(long, conflicts_with = "resume")]
    new_session: bool,

    /// Interactive startup: ask (5-second countdown), latest project session, or new.
    #[arg(long, value_enum)]
    session_startup: Option<session_startup::StartupMode>,

    /// List saved sessions and exit.
    #[arg(long)]
    list_sessions: bool,

    /// List projects shared with the Swift app and exit (macOS).
    #[arg(long)]
    list_projects: bool,

    /// Select a shared Swift project by name or UUID.
    #[arg(long)]
    project: Option<String>,

    /// List persisted exact-command and MCP Always Allow rules, then exit.
    #[arg(long)]
    list_approvals: bool,

    /// Clear all persisted Always Allow rules, then exit.
    #[arg(long)]
    clear_approvals: bool,

    /// Disable the interactive TUI when no prompt argument is supplied.
    #[arg(long)]
    no_tui: bool,

    /// Run the interactive first-use provider setup again.
    #[arg(long)]
    onboarding: bool,

    /// Start the embedded browser UI and JSON API instead of the TUI.
    #[arg(long)]
    web: bool,

    /// Web listen address (`IP:PORT`). When omitted: enabled webapp-service.json,
    /// then `[web] listen`, then `127.0.0.1:9847`. Auth belongs at the reverse proxy.
    #[arg(long)]
    listen: Option<std::net::SocketAddr>,

    /// UI language: zh-CN, en, or ja. Overrides agent.language.
    #[arg(long, env = "WILLDEEP_LANGUAGE", global = true)]
    language: Option<String>,

    /// Read a trusted Web bridge prompt and attachments as JSON from stdin.
    #[arg(long, hide = true)]
    web_input_json: bool,
}

#[derive(Clone, Debug, Subcommand)]
enum CliCommand {
    /// Organize an autonomous three-episode screenplay through the Runtime.
    Screenplay {
        #[command(subcommand)]
        action: screenplay_cmd::ScreenplayCommand,
    },
    /// Define, install and synchronize versioned specialist agents.
    Agents {
        #[command(subcommand)]
        action: agent_registry::AgentAction,
    },
    /// Manage Runtime-native scheduled tasks and retained run history.
    Schedule {
        #[command(subcommand)]
        action: schedule_cmd::ScheduleCommand,
    },
    /// Run one non-interactive coding-agent turn.
    Run(RunArgs),
    /// Generate a completion script from the current command tree.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Render the current command tree as a roff man page.
    Man,
    /// Inspect or stop persistent Runtime Sessions.
    Session {
        #[command(subcommand)]
        action: daemon::SessionAction,
    },
    /// Create, validate, or inspect the TOML configuration.
    Config {
        #[command(subcommand)]
        action: config::ConfigAction,
    },
    /// Inspect anonymous product analytics, or delete what was reported.
    Telemetry {
        #[command(subcommand)]
        action: telemetry::TelemetryAction,
    },
    /// Manage the persistent local Runtime Daemon.
    Daemon {
        #[command(subcommand)]
        action: daemon::DaemonAction,
    },
    /// Invoke one stable Runtime operation with a JSON request envelope.
    Api {
        /// Namespaced operation such as session.list or agent.get.
        operation: String,
        /// JSON object file, or - for stdin. Omit for an empty object.
        #[arg(long, value_name = "PATH|-")]
        params_file: Option<PathBuf>,
        /// Stable client-generated request ID. A UUID is generated when omitted.
        #[arg(long)]
        request_id: Option<uuid::Uuid>,
        /// Emit one compact JSON object suitable for NDJSON pipelines.
        #[arg(long)]
        ndjson: bool,
    },
    /// Attach to the persistent Runtime event stream.
    Attach {
        /// Resume after this event sequence number.
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
    /// Confirm that this client can disconnect without stopping the Runtime.
    Detach,
    /// Inspect and manage optional external integrations.
    Integrations {
        #[command(subcommand)]
        action: integrations::IntegrationAction,
    },
    /// Inspect detached background jobs that outlive the Runtime.
    Job {
        #[command(subcommand)]
        action: job_cmd::JobAction,
    },
    /// Receive sessions handed off from WillDeep for macOS through git branches.
    Handoff {
        #[command(subcommand)]
        action: handoff_cmd::HandoffAction,
    },
    /// Inspect the runtime event kernel: what arrived, what still needs you.
    Event {
        #[command(subcommand)]
        action: event_cmd::EventAction,
    },
    /// Install, approve and enable plugins shared with the macOS app.
    Plugin {
        #[command(subcommand)]
        action: plugin_cmd::PluginAction,
    },
    /// Inspect configured MCP servers, log in to remote ones with OAuth, list their tools.
    Mcp {
        #[command(subcommand)]
        action: mcp_cmd::McpAction,
    },
    /// Export an auditable report: approvals, human decisions, hook denials, verification evidence and change attribution.
    Audit {
        #[command(subcommand)]
        action: audit_cmd::AuditAction,
    },
    /// Inspect prompt sections, check candidate prompt variants and draft one from a feedback candidate.
    Prompt {
        #[command(subcommand)]
        action: prompt_cmd::PromptAction,
    },
    /// Summarize the local feedback ledger ($WILLDEEP_HOME/feedback) and list prompt bundle ids.
    Feedback {
        #[command(subcommand)]
        action: feedback_cmd::FeedbackAction,
    },
    /// Maintain the local per-model-call usage ledger ($WILLDEEP_HOME/usage).
    Usage {
        #[command(subcommand)]
        action: usage_cmd::UsageAction,
    },
    /// Inspect or rename this instance's identity (name plus permanent UUID).
    Identity {
        #[command(subcommand)]
        action: Option<identity_cmd::IdentityAction>,
    },
    /// Chat with another willdeep instance over the public intake listener.
    Fed {
        #[command(subcommand)]
        action: Option<identity_cmd::FedAction>,
    },
    /// Start the Runtime-hosted Web App if needed and open it in a browser.
    Web,
    /// Enable the mobile relay and print its pairing QR code.
    Qr,
    /// Diagnose local configuration and runtime readiness without contacting a Provider.
    Doctor {
        /// Emit one stable JSON report.
        #[arg(long)]
        json: bool,
        /// Write a private, shareable ZIP diagnostic bundle without logs or local paths.
        #[arg(long, value_name = "PATH")]
        bundle: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, Args)]
struct RunArgs {
    /// Task for the agent. Reads stdin when omitted.
    #[arg(value_name = "PROMPT", num_args = 0.., trailing_var_arg = true)]
    prompt: Vec<String>,

    /// Read the task from a UTF-8 file, or - for stdin.
    #[arg(long, value_name = "PATH|-", conflicts_with = "prompt")]
    input: Option<PathBuf>,

    /// Attach a text or PNG/JPEG/WebP/GIF file. May be repeated.
    #[arg(long, value_name = "PATH")]
    attachment: Vec<PathBuf>,

    /// Resume a saved Session by UUID or `latest`.
    #[arg(long, value_name = "ID|latest")]
    session: Option<String>,

    /// Output the final result as text, one JSON object, or NDJSON events.
    #[arg(long, value_enum, default_value = "text")]
    output: RunOutput,

    /// Suppress successful output. Errors still use stderr and a non-zero exit code.
    #[arg(long)]
    quiet: bool,

    /// Use the legacy in-process Harness instead of the persistent Runtime.
    #[arg(long)]
    local: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum RunOutput {
    Text,
    Json,
    Ndjson,
}

#[derive(Deserialize)]
struct WebInput {
    prompt: String,
    #[serde(default)]
    attachments: Vec<willdeep_core::MessageAttachment>,
    #[serde(default)]
    runtime: Option<daemon::RuntimeConnection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ProviderArg {
    Auto,
    #[value(name = "openai-compatible")]
    OpenAiCompatible,
    #[value(name = "some-im")]
    SomeIm,
    Anthropic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ApiArg {
    Auto,
    #[value(name = "chat-completions")]
    ChatCompletions,
    Responses,
    #[value(name = "anthropic-messages")]
    AnthropicMessages,
}

#[tokio::main]
async fn main() {
    let result = run().await;
    // 遥测在唯一的出口 flush：run() 有几十个 return 点，逐个挂 flush 只会
    // 漏掉其中几个。没装过句柄（或被关掉）时这里是空操作。
    telemetry::flush_before_exit().await;
    // 同理：用量账本的写线程在这里排空，进程内回合的最后几行不丢。
    willdeep_core::usage_ledger::flush_all(willdeep_core::usage_ledger::EXIT_FLUSH_TIMEOUT);
    willdeep_core::feedback::flush_all(willdeep_core::usage_ledger::EXIT_FLUSH_TIMEOUT);
    if let Err(error) = result {
        eprintln!("error: {error:#}");
        std::process::exit(stable_exit_code(&error));
    }
}

#[derive(Debug)]
struct RunInputError(String);

#[derive(Debug)]
struct HeadlessRuntimeExecutionError(daemon::HeadlessRuntimeStatus);

impl std::fmt::Display for HeadlessRuntimeExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self.0 {
            daemon::HeadlessRuntimeStatus::WaitingApproval => {
                "Runtime Turn is waiting for approval; resolve it with `willdeep daemon pending`"
            }
            daemon::HeadlessRuntimeStatus::WaitingAnswer => {
                "Runtime Turn is waiting for an answer; resolve it with `willdeep daemon pending`"
            }
            daemon::HeadlessRuntimeStatus::Failed(_) => "Runtime Turn failed",
            daemon::HeadlessRuntimeStatus::Cancelled => "Runtime Turn was cancelled",
            daemon::HeadlessRuntimeStatus::Interrupted => "Runtime Turn was interrupted",
            daemon::HeadlessRuntimeStatus::Partial => "Runtime Turn is only partially complete",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for HeadlessRuntimeExecutionError {}

impl std::fmt::Display for RunInputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RunInputError {}

/// Global flags `handoff watch --accept` hands to each child `handoff accept`,
/// so an unattended run uses the same profile, model and policy the operator
/// started the watcher with. Secrets stay in the environment, never in argv.
fn handoff_forwarded_args(cli: &Cli) -> Vec<std::ffi::OsString> {
    let mut args = Vec::new();
    if let Some(config) = &cli.config {
        args.push("--config".into());
        args.push(config.clone().into_os_string());
    }
    if let Some(profile) = &cli.profile {
        args.push("--profile".into());
        args.push(profile.into());
    }
    if let Some(model) = &cli.model {
        args.push("--model".into());
        args.push(model.into());
    }
    if cli.full_auto {
        args.push("--full-auto".into());
    }
    if let Some(max_turns) = cli.max_turns {
        args.push("--max-turns".into());
        args.push(max_turns.to_string().into());
    }
    args
}

/// 管理类子命令的界面语言：命令行给了用命令行的，否则取配置里的 `agent.language`，
/// 配置读不了就用缺省——这些命令不该因为「没有配置」被拦进首次设置。
fn administrative_language(cli: &Cli) -> anyhow::Result<i18n::Language> {
    let configured = LoadedConfig::load(cli.config.as_deref())
        .ok()
        .and_then(|loaded| loaded.file.agent.language);
    i18n::Language::parse(cli.language.as_deref().or(configured.as_deref()))
}

fn invalid_run_input(message: impl Into<String>) -> anyhow::Error {
    RunInputError(message.into()).into()
}

fn stable_exit_code(error: &anyhow::Error) -> i32 {
    if error.downcast_ref::<RunInputError>().is_some() {
        return 2;
    }
    if error
        .downcast_ref::<willdeep_core::provider::ProviderError>()
        .is_some()
    {
        return 3;
    }
    if let Some(error) = error.downcast_ref::<willdeep_core::AgentError>() {
        return match error {
            willdeep_core::AgentError::Provider(_) => 3,
            willdeep_core::AgentError::Tool(willdeep_core::tools::ToolError::ApprovalDenied(_))
            | willdeep_core::AgentError::Tool(willdeep_core::tools::ToolError::ReadOnlyPolicy(_))
            | willdeep_core::AgentError::Tool(willdeep_core::tools::ToolError::OutsideWorkspace(
                _,
            )) => 4,
            _ => 5,
        };
    }
    if let Some(error) = error.downcast_ref::<willdeep_core::tools::ToolError>() {
        return match error {
            willdeep_core::tools::ToolError::ApprovalDenied(_)
            | willdeep_core::tools::ToolError::ReadOnlyPolicy(_)
            | willdeep_core::tools::ToolError::OutsideWorkspace(_) => 4,
            _ => 5,
        };
    }
    if let Some(error) = error.downcast_ref::<HeadlessRuntimeExecutionError>() {
        return match error.0 {
            daemon::HeadlessRuntimeStatus::WaitingApproval
            | daemon::HeadlessRuntimeStatus::WaitingAnswer
            | daemon::HeadlessRuntimeStatus::Failed(Some(
                willdeep_runtime_protocol::FailureDomain::Policy,
            )) => 4,
            daemon::HeadlessRuntimeStatus::Failed(Some(
                willdeep_runtime_protocol::FailureDomain::Provider,
            )) => 3,
            _ => 5,
        };
    }
    1
}

fn runtime_failure_domain(error: &anyhow::Error) -> willdeep_runtime_protocol::FailureDomain {
    use willdeep_runtime_protocol::FailureDomain;

    if error
        .downcast_ref::<willdeep_core::provider::ProviderError>()
        .is_some()
    {
        return FailureDomain::Provider;
    }
    if let Some(error) = error.downcast_ref::<willdeep_core::AgentError>() {
        return match error {
            willdeep_core::AgentError::Provider(_) => FailureDomain::Provider,
            willdeep_core::AgentError::Tool(willdeep_core::tools::ToolError::ApprovalDenied(_))
            | willdeep_core::AgentError::Tool(willdeep_core::tools::ToolError::ReadOnlyPolicy(_))
            | willdeep_core::AgentError::Tool(willdeep_core::tools::ToolError::OutsideWorkspace(
                _,
            )) => FailureDomain::Policy,
            willdeep_core::AgentError::Tool(_) => FailureDomain::Tool,
            _ => FailureDomain::Harness,
        };
    }
    if let Some(error) = error.downcast_ref::<willdeep_core::tools::ToolError>() {
        return match error {
            willdeep_core::tools::ToolError::ApprovalDenied(_)
            | willdeep_core::tools::ToolError::ReadOnlyPolicy(_)
            | willdeep_core::tools::ToolError::OutsideWorkspace(_) => FailureDomain::Policy,
            _ => FailureDomain::Tool,
        };
    }
    FailureDomain::Internal
}

async fn open_web_app(cli: &Cli) -> Result<()> {
    use willdeep_runtime_protocol::web_app::WebAppStartParams;

    let home = willdeep_home()?;
    let language = administrative_language(cli)?;
    let mut status = daemon::web_app::remote_status(&home).await?;
    if !status.running {
        let workspace = cli.workspace.clone().unwrap_or(std::env::current_dir()?);
        let workspace = workspace
            .canonicalize()
            .with_context(|| format!("invalid Web workspace: {}", workspace.display()))?;
        let config =
            std::path::absolute(cli.config.clone().unwrap_or(config::default_config_path()?))?;
        let loaded = LoadedConfig::load(cli.config.as_deref())?;
        let listen = config::resolve_web_listen(
            cli.listen,
            daemon::web_app::enabled_listen(&home)?,
            &loaded.file.web.listen,
        )?;
        if !listen.ip().is_loopback() {
            bail!("Runtime Web App only accepts a loopback address");
        }
        status = daemon::web_app::remote_start(
            &home,
            &WebAppStartParams {
                listen,
                workspace,
                config: Some(config),
                profile: cli.profile.clone(),
                language: language.text("zh-CN", "en", "ja").to_owned(),
            },
        )
        .await?;
    }
    let address = status
        .listen
        .context("Web App did not report a listening address")?;
    let url = format!("http://{address}");
    if !onboarding::open_browser(&url) {
        eprintln!("Could not open a browser automatically.");
    }
    println!("{url}");
    Ok(())
}

async fn print_pairing_qr(home: &std::path::Path) -> Result<()> {
    let enabled = daemon::mobile_enable(home).await?;
    println!("{}", mobile::render_qr(&enabled.pairing_url)?);
    eprintln!(
        "Scan with your own WillDeep Mobile device. The QR contains a relay token; do not share screenshots."
    );
    Ok(())
}

async fn run() -> Result<()> {
    // Branch before Clap constructs the global credential-aware parser.
    if let Some(result) = agent_rsi::run_free_review_standalone().await {
        return result;
    }
    let mut cli = Cli::parse();
    let run_args = match cli.command.take() {
        Some(CliCommand::Run(args)) => {
            if args.quiet && args.output != RunOutput::Text {
                return Err(invalid_run_input(
                    "--quiet cannot be combined with --output json or ndjson",
                ));
            }
            cli.no_tui = true;
            cli.resume = args.session.clone().or(cli.resume);
            cli.json = args.output == RunOutput::Ndjson;
            Some(args)
        }
        // Accepting a handoff imports the session, then continues it exactly
        // like `run --session <id> --input <brief>` so the daemon/local choice,
        // approvals and exit codes stay the ones `run` already has.
        Some(CliCommand::Handoff {
            action: handoff_cmd::HandoffAction::Accept(accept),
        }) if !accept.no_run => {
            let Some(prepared) =
                handoff_cmd::accept(&accept, &willdeep_home()?, cli.workspace.as_deref())?
            else {
                return Ok(());
            };
            cli.no_tui = true;
            cli.resume = Some(prepared.session_id.to_string());
            cli.workspace = Some(prepared.workspace);
            Some(RunArgs {
                prompt: Vec::new(),
                input: Some(prepared.brief_path),
                attachment: Vec::new(),
                session: Some(prepared.session_id.to_string()),
                output: RunOutput::Text,
                quiet: false,
                local: false,
            })
        }
        command => {
            cli.command = command;
            None
        }
    };
    enforce_prompt_variant_scope(cli.command.as_ref(), run_args.as_ref())?;
    if let Some(command) = cli.command.clone() {
        return match command {
            CliCommand::Run(_) => unreachable!("run command is normalized above"),
            CliCommand::Screenplay { action } => {
                screenplay_cmd::run(action, &willdeep_home()?).await
            }
            CliCommand::Completions { shell } => generate_completions(shell),
            CliCommand::Man => generate_man_page(),
            CliCommand::Agents { action } => agent_registry::run(action, &willdeep_home()?).await,
            CliCommand::Session { action } => daemon::handle_session(action).await,
            CliCommand::Schedule { action } => {
                println!("{}", schedule_cmd::run(action, &willdeep_home()?).await?);
                Ok(())
            }
            CliCommand::Config { action } => config::handle(action, cli.config.as_deref()),
            CliCommand::Telemetry { action } => {
                let loaded = LoadedConfig::load(cli.config.as_deref())?;
                telemetry::handle(action, &willdeep_home()?, loaded.file.telemetry.enabled).await
            }
            CliCommand::Daemon { action } => daemon::handle(action).await,
            CliCommand::Api {
                operation,
                params_file,
                request_id,
                ndjson,
            } => daemon::api(operation, params_file, request_id, ndjson).await,
            CliCommand::Attach { after } => daemon::attach(after).await,
            CliCommand::Detach => daemon::detach().await,
            CliCommand::Integrations { action } => integrations::handle(action).await,
            CliCommand::Job { action } => job_cmd::run(action, &willdeep_home()?),
            CliCommand::Event { action } => event_cmd::run(action, &willdeep_home()?),
            CliCommand::Handoff { action } => {
                handoff_cmd::run(
                    action,
                    &willdeep_home()?,
                    cli.workspace.as_deref(),
                    handoff_forwarded_args(&cli),
                )
                .await
            }
            CliCommand::Plugin { action } => plugin_cmd::run(action, &willdeep_home()?).await,
            CliCommand::Audit { action } => {
                let language = administrative_language(&cli)?;
                audit_cmd::run(action, &willdeep_home()?, language)
            }
            CliCommand::Usage { action } => usage_cmd::run(action, &willdeep_home()?),
            CliCommand::Web => open_web_app(&cli).await,
            CliCommand::Identity { action } => {
                identity_cmd::identity_command(action, &willdeep_home()?)
            }
            CliCommand::Fed { action } => {
                identity_cmd::federation_command(action, &willdeep_home()?, cli.config.as_deref())
                    .await
            }
            CliCommand::Qr => print_pairing_qr(&willdeep_home()?).await,
            CliCommand::Feedback { action } => feedback_review_cmd::run(action, &cli).await,
            CliCommand::Prompt { action } => prompt_cmd::run(action, &cli).await,
            CliCommand::Mcp { action } => {
                let language = administrative_language(&cli)?;
                mcp_cmd::run(action, &willdeep_home()?, cli.config.as_deref(), language).await
            }
            CliCommand::Doctor { json, bundle } => {
                doctor::run(doctor::DoctorOptions {
                    config_path: cli.config.clone(),
                    profile: cli.profile.clone(),
                    workspace: cli.workspace.clone(),
                    home: willdeep_home()?,
                    api_base_present: cli
                        .api_base
                        .as_deref()
                        .is_some_and(|value| !value.is_empty()),
                    api_key_present: cli
                        .api_key
                        .as_deref()
                        .is_some_and(|value| !value.is_empty()),
                    model_present: cli.model.as_deref().is_some_and(|value| !value.is_empty()),
                    json,
                    bundle,
                })
                .await
            }
        };
    }
    let administrative =
        cli.list_projects || cli.list_sessions || cli.list_approvals || cli.clear_approvals;
    if !administrative
        && !cli.onboarding
        && !magpie_integration::claims_configuration(&cli)
        && let Err(error) = local_provider_discovery::discover(cli.config.as_deref()).await
    {
        eprintln!("Local Provider discovery: {error}");
    }
    // 没有配置文件不等于要先设置：环境里有认得出的钥匙（或命令行给全了）就直接跑。
    let browser_setup = if cli.onboarding {
        onboarding::run(cli.config.as_deref()).await?
    } else {
        !cli.web && !administrative && onboarding::needs_web_setup(&cli)
    };
    let _setup_server = if browser_setup && !cli.web {
        Some(onboarding::configure_in_browser(&cli).await?)
    } else {
        None
    };
    let loaded = if cli.web && onboarding::needs_web_setup(&cli) {
        LoadedConfig {
            file: config::ConfigFile::default(),
        }
    } else {
        LoadedConfig::load(cli.config.as_deref())?
    };
    let language = i18n::Language::parse(
        cli.language
            .as_deref()
            .or(loaded.file.agent.language.as_deref()),
    )?;
    let home = willdeep_home()?;
    // 内置插件只服务交互式 TUI 的插件面板：`--web` 由 PluginWebState 惰性安装，
    // `run` 等 headless 一次性调用不经过插件面板，预装只会把首轮请求推后数秒。
    let interactively_starting_tui = !cli.web
        && !cli.no_tui
        && !cli.web_input_json
        && cli.prompt.is_empty()
        && std::io::stdin().is_terminal();
    if !administrative && interactively_starting_tui {
        favorites_plugin::ensure_installed(&home).await?;
        video_studio_plugin::ensure_installed(&home).await?;
    }
    // 匿名产品遥测。配置默认开启，WILLDEEP_TELEMETRY_DISABLED=1 可整体关掉。
    // 只写本地队列，实际发送在进程退出前一次性做（见 main）。
    telemetry::install(telemetry::Telemetry::new(
        loaded.file.telemetry.enabled,
        &home,
    ));
    telemetry::global().record({
        let mut event = telemetry::global().event(telemetry::EventName::AppSessionStarted);
        event.locale = Some(language.telemetry_tag().to_owned());
        event
    });
    if cli.web {
        let mut candidates = Vec::new();
        if let Some(project) = cli.project.as_deref() {
            candidates.extend(projects::resolve_folders(project)?);
        }
        if let Some(workspace) = cli.workspace.clone() {
            candidates.insert(0, workspace);
        }
        candidates.extend(cli.web_workspaces.clone());
        if candidates.is_empty() {
            candidates.push(std::env::current_dir()?);
        }
        let mut workspaces = Vec::new();
        for candidate in candidates {
            let canonical = candidate
                .canonicalize()
                .with_context(|| format!("invalid Web workspace: {}", candidate.display()))?;
            if !workspaces.contains(&canonical) {
                workspaces.push(canonical);
            }
        }
        let listen = config::resolve_web_listen(cli.listen, None, &loaded.file.web.listen)?;
        return web::serve(web::WebConfig {
            listen,
            config_path: std::path::absolute(
                cli.config.clone().unwrap_or(config::default_config_path()?),
            )?,
            integration_default: cli.config.is_none(),
            profile: cli.profile.clone(),
            workspaces,
            home,
            language,
            open_setup: cli.onboarding || onboarding::needs_web_setup(&cli),
            setup_started: None,
            intake: None,
        })
        .await;
    }
    let approval_store = home.join("always-allow.json");
    if cli.list_approvals {
        for rule in load_approval_rules(&approval_store)? {
            println!("{rule}");
        }
        return Ok(());
    }
    if cli.clear_approvals {
        save_approval_rules(&approval_store, &[])?;
        println!("Cleared Always Allow rules.");
        return Ok(());
    }
    if cli.list_projects {
        for project in projects::load() {
            println!(
                "{}\t{}\t{}",
                project.id,
                project.display_name,
                project
                    .folder_paths
                    .first()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            );
        }
        return Ok(());
    }
    let store = willdeep_core::SessionStore::new(&home);
    if cli.list_sessions {
        for session in store.digests() {
            println!(
                "{}\t{}\t{}\t{}",
                session.id,
                session.updated_at,
                session.workspace.display(),
                session.title
            );
        }
        return Ok(());
    }
    let mut resumed = load_session(&store, cli.resume.as_deref()).map_err(|error| {
        if run_args.is_some() {
            invalid_run_input(format!("load run Session: {error:#}"))
        } else {
            error
        }
    })?;
    let web_input = if cli.web_input_json {
        let mut value = String::new();
        std::io::stdin().read_to_string(&mut value)?;
        Some(serde_json::from_str::<WebInput>(&value).context("parse Web bridge input")?)
    } else {
        None
    };
    let prompt = if let Some(input) = &web_input {
        Some(input.prompt.clone())
    } else if let Some(args) = &run_args {
        Some(read_run_prompt(args)?)
    } else {
        read_prompt(&cli.prompt, cli.no_tui)?
    };
    let run_attachments = run_args
        .as_ref()
        .map(|args| load_run_attachments(&args.attachment))
        .transpose()?
        .unwrap_or_default();

    let interactive_tui = prompt.is_none() && std::io::stdin().is_terminal() && !cli.no_tui;
    let (tui_tx, tui_rx) = tui::channel();
    // MuseTerm 把图库帧和按键写在同一条输入流里；过滤层必须在任何读取之前装好。
    let _gallery_input = crate::gallery::install_hosted(interactive_tui, tui_tx.clone());
    let directory_approval = session_startup::prepare(
        &cli,
        &loaded,
        &home,
        &store,
        interactive_tui && std::io::stdout().is_terminal(),
        run_args.is_some(),
        language,
        &mut resumed,
    )
    .await?;
    if should_use_headless_runtime(
        &cli,
        run_args.as_ref(),
        interactive_tui,
        web_input.is_some(),
    ) {
        let prompt = prompt.context("provide a prompt argument or pipe one on stdin")?;
        return run_with_runtime(
            &cli,
            &home,
            &store,
            resumed,
            prompt,
            run_attachments,
            run_args.as_ref(),
        )
        .await;
    }
    let frontend =
        if let Some(connection) = web_input.as_ref().and_then(|input| input.runtime.clone()) {
            harness::HarnessFrontend::Runtime {
                independent_execution: false,
                connection: Box::new(connection),
                sink: Arc::new(TerminalSink {
                    json: cli.json,
                    quiet: run_args
                        .as_ref()
                        .is_some_and(|args| args.quiet || args.output == RunOutput::Json),
                }),
                workspace_access: Some(daemon::WorkspaceAccess::ReadOnly),
                approval_handle: None,
                instruction_inbox: None,
                allowed_skills: Vec::new(),
                allowed_mcp_servers: Vec::new(),
                reasoning_level: None,
                // Web 桥接子进程：自己调模型，不经过 daemon 的执行器。
                usage_origin: harness::UsageOrigin {
                    execution: willdeep_core::usage_ledger::Execution::InProcess,
                    origin_client: None,
                    turn_id: None,
                },
            }
        } else if interactive_tui {
            harness::HarnessFrontend::Tui { tx: tui_tx.clone() }
        } else {
            harness::HarnessFrontend::Terminal {
                json: cli.json,
                quiet: run_args
                    .as_ref()
                    .is_some_and(|args| args.quiet || args.output == RunOutput::Json),
            }
        };
    let built = harness::build(&cli, &loaded, &home, language, resumed.as_ref(), frontend).await?;
    let agent = built.agent.clone();
    if let Some(mode) = directory_approval {
        agent.approval_mode_handle().set(mode);
    }
    let workspace = built.workspace.clone();
    let skills = built.skills.clone();
    let background_tasks = built.background_tasks.clone();
    let context_window = built.context_window;

    let new_session = resumed.is_none();
    let mut session = resumed.unwrap_or_else(|| {
        // 没有提示词就没有标题可派生；占位符由 `Session::new` 自己给，
        // 交互式 TUI 的标题在第一条提示词提交时才成形。
        let mut session = willdeep_core::Session::new(
            workspace.clone(),
            cli.profile.clone(),
            prompt.as_deref().unwrap_or_default(),
        );
        session.profile = built.provider_profile.clone();
        session.provider_selection_explicit = cli.profile.is_some()
            || cli.model.is_some()
            || cli.config.is_some()
            || cli.provider.is_some()
            || cli.api.is_some()
            || cli.api_base.is_some()
            || cli.api_key.is_some()
            || provider_settings::ProviderEnvironment::read().has_overrides();
        session.model = Some(built.provider_config.model.clone());
        session.id = built.session_id;
        session
    });
    if cli.profile.is_some() {
        session.profile = built.provider_profile.clone();
    }
    if cli.profile.is_some() || cli.model.is_some() {
        session.model = Some(built.provider_config.model.clone());
        session.provider_selection_explicit = true;
    }
    session.magpie_integration = built.provider_config.magpie_integration;
    if session.config.is_none() {
        session.config = Some(cli.config.clone().unwrap_or(config::default_config_path()?));
    }
    // Execution ownership requires a durable session. Only initialize newly
    // allocated IDs here; resumed sessions must be refreshed after claiming.
    if new_session && !interactive_tui {
        store.save(&mut session)?;
    }
    if interactive_tui {
        let runtime_profile = session.profile.clone().or_else(|| cli.profile.clone());
        let runtime_model = session.model.clone();
        return tui::run(
            agent,
            session,
            store,
            home,
            skills,
            built.kernel.clone(),
            built.kernel_store.clone(),
            (
                tui_tx,
                tui_rx,
                context_window,
                background_tasks,
                daemon::RuntimeSubmitOptions {
                    workspace,
                    profile: runtime_profile,
                    model: runtime_model,
                    config: Some(cli.config.clone().unwrap_or(config::default_config_path()?)),
                },
                built.provider_config.clone(),
                language,
                built.notifier.clone(),
            ),
        )
        .await;
    }
    let prompt = prompt.context("provide a prompt argument or pipe one on stdin")?;
    let allow_compress_command = web_input.is_some();
    let attachments = if let Some(input) = web_input {
        input.attachments
    } else {
        run_attachments
    };
    let outcome = harness::execute_noninteractive(
        &built,
        &store,
        &mut session,
        prompt,
        attachments,
        language,
        harness::ExecutionOptions {
            allow_compress_command,
            replay_existing_user_message: false,
            ownership: None,
        },
    )
    .await?;
    if let Some(args) = run_args {
        match args.output {
            RunOutput::Text if !args.quiet && !outcome.compressed => {
                println!("{}", outcome.final_text);
            }
            RunOutput::Json => println!("{}", completion_json(&outcome, session.id)),
            RunOutput::Ndjson => println!("{}", completion_json(&outcome, session.id)),
            RunOutput::Text => {}
        }
    } else if cli.json {
        println!("{}", completion_json(&outcome, session.id));
    } else if !outcome.compressed {
        println!("{}", outcome.final_text);
    }
    if !outcome.stop_reason.is_complete() {
        return Err(HeadlessRuntimeExecutionError(daemon::HeadlessRuntimeStatus::Partial).into());
    }
    Ok(())
}

/// 候选提示词（`WILLDEEP_PROMPT_VARIANT`）只给离线评测用：只有进程内执行的
/// `willdeep run --local` 会套用它，而且变体不合法时直接报错，不退回默认提示词。
/// 别的入口（TUI、Web、daemon、经 daemon 的 `run`）一律拒绝：daemon 是常驻
/// 进程，它的提示词不跟着某一次调用的环境变量走，套了也套不上。
fn enforce_prompt_variant_scope(
    command: Option<&CliCommand>,
    run_args: Option<&RunArgs>,
) -> Result<()> {
    use willdeep_core::prompt_sections::{VARIANT_ENV, init_from_env};
    if std::env::var_os(VARIANT_ENV).is_none_or(|value| value.is_empty()) {
        return Ok(());
    }
    if matches!(
        command,
        Some(CliCommand::Prompt { .. } | CliCommand::Feedback { .. })
    ) {
        return Ok(());
    }
    if !run_args.is_some_and(|args| args.local) {
        return Err(invalid_run_input(format!(
            "{VARIANT_ENV} is only honored by `willdeep run --local`; unset it for other commands"
        )));
    }
    init_from_env().map_err(invalid_run_input)?;
    Ok(())
}

fn should_use_headless_runtime(
    cli: &Cli,
    run_args: Option<&RunArgs>,
    interactive_tui: bool,
    web_bridge: bool,
) -> bool {
    if interactive_tui || web_bridge || run_args.is_some_and(|args| args.local) {
        return false;
    }
    // These process-local CLI overrides are intentionally not serialized into
    // the Runtime task store. Provider-specific environment aliases remain
    // available to TOML profiles inherited by the Runtime process.
    cli.api_base.is_none()
        && cli.api_key.is_none()
        && cli.provider.is_none()
        && cli.api.is_none()
        && !cli.full_auto
        && cli.max_turns.is_none()
        && cli.max_output_tokens.is_none()
        && cli.language.is_none()
}

async fn run_with_runtime(
    cli: &Cli,
    home: &std::path::Path,
    store: &willdeep_core::SessionStore,
    resumed: Option<willdeep_core::Session>,
    prompt: String,
    attachments: Vec<willdeep_core::MessageAttachment>,
    run_args: Option<&RunArgs>,
) -> Result<()> {
    let workspace = harness::resolve_workspace(cli, resumed.as_ref())?;
    let loaded = LoadedConfig::load(cli.config.as_deref())?;
    let selection = magpie_integration::resolve(cli, &loaded, resumed.as_ref()).await?;
    let mut session = resumed.unwrap_or_else(|| {
        let mut session =
            willdeep_core::Session::new(workspace.clone(), selection.name.clone(), &prompt);
        session.model = Some(selection.config.model.clone());
        session
    });
    if cli.profile.is_some() {
        session.profile = selection.name;
    }
    if cli.profile.is_some() || cli.model.is_some() {
        session.model = Some(selection.config.model);
        session.provider_selection_explicit = true;
    }
    session.magpie_integration = selection.config.magpie_integration;
    if session.config.is_none() {
        session.config = Some(cli.config.clone().unwrap_or(config::default_config_path()?));
    }
    session.runtime_managed = true;
    store.save(&mut session)?;
    if cli.profile.is_some() || cli.model.is_some() {
        daemon::ensure_runtime_session(
            home,
            session.id,
            &workspace,
            session.profile.clone(),
            session.model.clone(),
        )
        .await?;
        if let Some(model) = session.model.as_deref() {
            if let Some(profile) = session.profile.as_deref() {
                daemon::tui_bridge::update_remote_session_provider(
                    home, session.id, profile, model,
                )
                .await?;
            } else {
                daemon::update_remote_session_model(home, session.id, model.to_owned()).await?;
            }
        }
    }

    let quiet = run_args.is_some_and(|args| args.quiet || args.output == RunOutput::Json);
    let machine_events = cli.json || run_args.is_some_and(|args| args.output == RunOutput::Ndjson);
    let result = daemon::execute_headless_turn(
        home,
        daemon::HeadlessRuntimeRequest {
            session_id: session.id,
            workspace,
            profile: session.profile.clone().or_else(|| cli.profile.clone()),
            model: session.model.clone(),
            prompt,
            attachments,
        },
        |event| emit_headless_runtime_event(event, machine_events, quiet),
    )
    .await?;
    let outcome =
        result.map_err(|status| anyhow::Error::new(HeadlessRuntimeExecutionError(status)))?;
    if let Some(args) = run_args {
        match args.output {
            RunOutput::Text if !args.quiet => println!("{}", outcome.final_text),
            RunOutput::Json | RunOutput::Ndjson => println!(
                "{}",
                completion_json_values(
                    &outcome.final_text,
                    outcome.turns,
                    outcome.session_id,
                    &outcome.stop_reason
                )
            ),
            RunOutput::Text => {}
        }
    } else if cli.json {
        println!(
            "{}",
            completion_json_values(
                &outcome.final_text,
                outcome.turns,
                outcome.session_id,
                &outcome.stop_reason
            )
        );
    } else {
        println!("{}", outcome.final_text);
    }
    if !matches!(outcome.stop_reason.as_str(), "finished" | "goal_complete") {
        return Err(HeadlessRuntimeExecutionError(daemon::HeadlessRuntimeStatus::Partial).into());
    }
    Ok(())
}

fn emit_headless_runtime_event(event: serde_json::Value, machine: bool, quiet: bool) {
    if quiet {
        return;
    }
    if machine {
        println!("{event}");
        return;
    }
    let kind = event
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("runtime");
    match kind {
        "turn_started" => {
            if let Some(turn) = event.get("turn").and_then(serde_json::Value::as_u64) {
                eprintln!("[turn {turn}]");
            }
        }
        "tool_requested" => eprintln!(
            "[tool] {}",
            event
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
        ),
        "tool_completed" => eprintln!(
            "[tool:{}] {}",
            if event
                .get("is_error")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                "error"
            } else {
                "done"
            },
            event
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown")
        ),
        "compression_started" => eprintln!("[context] compressing"),
        "compression_completed" => eprintln!("[context] compressed"),
        "subagent_started" => eprintln!("[subagent] started"),
        "subagent_completed" => eprintln!("[subagent] finished"),
        _ => {}
    }
}

fn generate_completions(shell: clap_complete::Shell) -> Result<()> {
    let mut command = Cli::command();
    let name = command.get_name().to_owned();
    clap_complete::generate(shell, &mut command, name, &mut std::io::stdout());
    Ok(())
}

fn generate_man_page() -> Result<()> {
    clap_mangen::Man::new(Cli::command())
        .render(&mut std::io::stdout())
        .context("render willdeep man page")
}

fn completion_json(outcome: &harness::HarnessOutcome, session_id: uuid::Uuid) -> serde_json::Value {
    completion_json_values(
        &outcome.final_text,
        outcome.turns,
        session_id,
        outcome.stop_reason.as_str(),
    )
}

fn completion_json_values(
    final_text: &str,
    turns: usize,
    session_id: uuid::Uuid,
    stop_reason: &str,
) -> serde_json::Value {
    serde_json::json!({
        "type": if matches!(stop_reason, "finished" | "goal_complete") { "completed" } else { "partial" },
        "stop_reason": stop_reason,
        "turns": turns,
        "text": final_text,
        "session_id": session_id,
        // 只有 `run --local` 会套候选提示词；其余入口恒为 null。
        "prompt_variant": willdeep_core::prompt_sections::active_variant_report()
    })
}

fn model_accepts_images(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    if lower.contains("deepseek") && !lower.contains("vl") {
        return false;
    }
    !lower.starts_with("someim-auto") && !lower.starts_with("someim-coding")
}

/// 没有配置文件、也没有 `--provider` 时，按环境里有哪把钥匙推断 Provider：
/// 标准地址变量会选择对应协议；保留 SOMEIM_API_KEY / ANTHROPIC_API_KEY
/// 的零配置回退。OpenAI-compatible 仍要求用户提供地址与模型。
pub(crate) fn provider_from_env() -> Option<ProviderArg> {
    provider_settings::ProviderEnvironment::read().inferred_provider()
}

fn provider_from_keys(
    some_im: bool,
    anthropic: bool,
    openai_with_base: bool,
) -> Option<ProviderArg> {
    if some_im {
        Some(ProviderArg::SomeIm)
    } else if anthropic {
        Some(ProviderArg::Anthropic)
    } else if openai_with_base {
        Some(ProviderArg::OpenAiCompatible)
    } else {
        None
    }
}

pub(crate) fn provider_arg_name(provider: ProviderArg) -> &'static str {
    match provider {
        ProviderArg::Auto => "auto",
        ProviderArg::OpenAiCompatible => "openai-compatible",
        ProviderArg::SomeIm => "some-im",
        ProviderArg::Anthropic => "anthropic",
    }
}

/// 没写模型名时各 Provider 的缺省模型；OpenAI-compatible 端点五花八门，不猜。
pub(crate) fn default_model(kind: ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::SomeIm => Some(model_defaults::SOMEIM_DEFAULT_MODEL),
        ProviderKind::Anthropic => Some(model_defaults::ANTHROPIC_DEFAULT_MODEL),
        ProviderKind::OpenAiCompatible => None,
    }
}

/// 没有配置文件也能开工的条件：Provider 推得出、API Base 定得下、钥匙拿得到、
/// 模型名有着落。满足就跳过首次设置直接跑，不逼用户先手写 TOML。
fn zero_config_ready(cli: &Cli) -> bool {
    let present = |value: &Option<String>| {
        value
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    };
    let Some(provider) = cli
        .provider
        .or_else(provider_from_env)
        .or_else(|| present(&cli.api_base).then_some(ProviderArg::Auto))
    else {
        return false;
    };
    let Ok(base) = resolve_base(cli, None, Some(provider)) else {
        return false;
    };
    let kind = resolve_provider(provider, &base);
    resolve_api_key(cli, None, kind).is_ok()
        && (present(&cli.model) || default_model(kind).is_some())
}

fn resolve_base(
    cli: &Cli,
    profile: Option<&ProviderProfile>,
    selected_provider: Option<ProviderArg>,
) -> Result<String> {
    provider_settings::ProviderEnvironment::read().base(
        cli.api_base.as_deref(),
        profile,
        selected_provider,
        true,
    )
}

pub(crate) fn provider_config_from_profile(
    file: &config::ConfigFile,
    name: &str,
) -> Result<ProviderConfig> {
    if name == magpie_integration::PROFILE_ID {
        return magpie_integration::EffectiveProviders::snapshot(file).config(name);
    }
    let profile = file
        .providers
        .get(name)
        .with_context(|| format!("provider profile not found: {name}"))?;
    provider_settings::ProviderEnvironment::read()
        .resolve(Some(profile), None)
        .with_context(|| format!("resolve provider profile {name}"))
}

fn resolve_provider(provider: ProviderArg, base: &str) -> ProviderKind {
    match provider {
        ProviderArg::Auto => ProviderKind::infer(base),
        ProviderArg::OpenAiCompatible => ProviderKind::OpenAiCompatible,
        ProviderArg::SomeIm => ProviderKind::SomeIm,
        ProviderArg::Anthropic => ProviderKind::Anthropic,
    }
}

fn resolve_dialect(api: ApiArg, provider: ProviderKind) -> ApiDialect {
    match api {
        ApiArg::Auto if provider == ProviderKind::Anthropic => ApiDialect::AnthropicMessages,
        ApiArg::Auto => ApiDialect::ChatCompletions,
        ApiArg::ChatCompletions => ApiDialect::ChatCompletions,
        ApiArg::Responses => ApiDialect::Responses,
        ApiArg::AnthropicMessages => ApiDialect::AnthropicMessages,
    }
}

fn resolve_api_key(
    cli: &Cli,
    profile: Option<&ProviderProfile>,
    provider: ProviderKind,
) -> Result<String> {
    provider_settings::ProviderEnvironment::read().key(
        cli.api_key.as_deref(),
        profile,
        provider,
        true,
    )
}

fn parse_provider(value: &str) -> Result<ProviderArg> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(ProviderArg::Auto),
        "openai-compatible" => Ok(ProviderArg::OpenAiCompatible),
        "some-im" | "some.im" => Ok(ProviderArg::SomeIm),
        "anthropic" => Ok(ProviderArg::Anthropic),
        _ => bail!("invalid provider value in config: {value}"),
    }
}

fn parse_api(value: &str) -> Result<ApiArg> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(ApiArg::Auto),
        "chat-completions" => Ok(ApiArg::ChatCompletions),
        "responses" => Ok(ApiArg::Responses),
        "anthropic-messages" => Ok(ApiArg::AnthropicMessages),
        _ => bail!("invalid api value in config: {value}"),
    }
}

fn read_prompt(arguments: &[String], no_tui: bool) -> Result<Option<String>> {
    if !arguments.is_empty() {
        return Ok(Some(arguments.join(" ")));
    }
    if std::io::stdin().is_terminal() {
        if no_tui {
            bail!("provide a prompt argument or pipe one on stdin");
        }
        return Ok(None);
    }
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt)?;
    if prompt.trim().is_empty() {
        bail!("prompt is empty");
    }
    Ok(Some(prompt))
}

fn read_run_prompt(args: &RunArgs) -> Result<String> {
    if !args.prompt.is_empty() {
        return Ok(args.prompt.join(" "));
    }
    let mut prompt = String::new();
    match args.input.as_deref() {
        Some(path) if path == std::path::Path::new("-") => {
            std::io::stdin().read_to_string(&mut prompt)?;
        }
        Some(path) => {
            prompt = std::fs::read_to_string(path).map_err(|error| {
                invalid_run_input(format!("read run input {}: {error}", path.display()))
            })?;
        }
        None if std::io::stdin().is_terminal() => {
            return Err(invalid_run_input(
                "provide a prompt, --input PATH, or pipe a prompt on stdin",
            ));
        }
        None => {
            std::io::stdin().read_to_string(&mut prompt)?;
        }
    }
    if prompt.trim().is_empty() {
        return Err(invalid_run_input("prompt is empty"));
    }
    Ok(prompt)
}

fn load_run_attachments(paths: &[PathBuf]) -> Result<Vec<willdeep_core::MessageAttachment>> {
    const MAX_ATTACHMENTS: usize = 12;
    const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
    const MAX_TEXT_CHARS: usize = 200_000;
    const MAX_IMAGE_PIXELS: u64 = 100_000_000;

    if paths.len() > MAX_ATTACHMENTS {
        return Err(invalid_run_input(format!(
            "at most {MAX_ATTACHMENTS} attachments are allowed"
        )));
    }
    let mut attachments = Vec::with_capacity(paths.len());
    let mut total_bytes = 0usize;
    for path in paths {
        let bytes = std::fs::read(path).map_err(|error| {
            invalid_run_input(format!("read attachment {}: {error}", path.display()))
        })?;
        total_bytes = total_bytes.saturating_add(bytes.len());
        if bytes.is_empty() || total_bytes > MAX_ATTACHMENT_BYTES {
            return Err(invalid_run_input(
                "attachments must be non-empty and total at most 10 MiB",
            ));
        }
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| invalid_run_input("attachment file name must be valid UTF-8"))?
            .to_owned();
        if let Ok(format) = image::guess_format(&bytes) {
            let media_type = match format {
                image::ImageFormat::Png => "image/png",
                image::ImageFormat::Jpeg => "image/jpeg",
                image::ImageFormat::WebP => "image/webp",
                image::ImageFormat::Gif => "image/gif",
                _ => {
                    return Err(invalid_run_input(format!(
                        "unsupported image attachment format: {}",
                        path.display()
                    )));
                }
            };
            let (width, height) = image::ImageReader::new(std::io::Cursor::new(&bytes))
                .with_guessed_format()
                .map_err(|error| {
                    invalid_run_input(format!("detect attachment image format: {error}"))
                })?
                .into_dimensions()
                .map_err(|error| {
                    invalid_run_input(format!("read image dimensions {}: {error}", path.display()))
                })?;
            if width == 0
                || height == 0
                || u64::from(width).saturating_mul(u64::from(height)) > MAX_IMAGE_PIXELS
            {
                return Err(invalid_run_input(
                    "image attachment dimensions are invalid or too large",
                ));
            }
            attachments.push(willdeep_core::MessageAttachment::Image {
                name,
                media_type: media_type.to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
                width,
                height,
            });
        } else {
            let content = String::from_utf8(bytes).map_err(|_| {
                invalid_run_input(format!(
                    "attachment is neither supported image nor UTF-8 text: {}",
                    path.display()
                ))
            })?;
            if content.chars().count() > MAX_TEXT_CHARS {
                return Err(invalid_run_input(format!(
                    "text attachment exceeds {MAX_TEXT_CHARS} characters"
                )));
            }
            attachments.push(willdeep_core::MessageAttachment::Text { name, content });
        }
    }
    Ok(attachments)
}

fn load_session(
    store: &willdeep_core::SessionStore,
    resume: Option<&str>,
) -> Result<Option<willdeep_core::Session>> {
    match resume {
        Some("latest") => Ok(Some(store.latest()?.context("no saved sessions found")?)),
        // 带基线加载：续上的会话会被追加消息再写回，基线必须记着改动前的执行状态。
        Some(value) => Ok(Some(store.load_baselined(
            uuid::Uuid::parse_str(value).context("--resume must be a session UUID or `latest`")?,
        )?)),
        None => Ok(None),
    }
}

fn load_approval_rules(path: &std::path::Path) -> Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&std::fs::read_to_string(path)?).context("parse Always Allow rules")
}

fn save_approval_rules(path: &std::path::Path, rules: &[String]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&serde_json::to_vec_pretty(rules)?)?;
    file.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

struct TerminalApprover(i18n::Language);

#[async_trait]
impl Approver for TerminalApprover {
    async fn approve(&self, description: &str, always_allow_available: bool) -> ApprovalDecision {
        if !std::io::stdin().is_terminal() {
            return ApprovalDecision::Deny;
        }
        let description = description.to_owned();
        let language = self.0;
        tokio::task::spawn_blocking(move || {
            eprint!(
                "\n{}: {description}\n{} ",
                language.text("需要确认", "Approval required", "承認が必要"),
                if always_allow_available {
                    language.text(
                        "[y] 允许一次 / [a] 始终允许 / [N] 拒绝",
                        "[y] Allow once / [a] Always allow / [N] Disallow",
                        "[y] 一度だけ許可 / [a] 常に許可 / [N] 拒否",
                    )
                } else {
                    language.text(
                        "[y] 允许一次 / [N] 拒绝",
                        "[y] Allow once / [N] Disallow",
                        "[y] 一度だけ許可 / [N] 拒否",
                    )
                }
            );
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            if std::io::stdin().read_line(&mut answer).is_err() {
                return ApprovalDecision::Deny;
            }
            match answer.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => ApprovalDecision::AllowOnce,
                "a" | "always" if always_allow_available => ApprovalDecision::AlwaysAllow,
                _ => ApprovalDecision::Deny,
            }
        })
        .await
        .unwrap_or(ApprovalDecision::Deny)
    }
    async fn ask_user(&self, request: UserQuestion) -> Option<String> {
        if !std::io::stdin().is_terminal() {
            return None;
        }
        tokio::task::spawn_blocking(move || {
            eprintln!("\n{}", request.question);
            for (index, option) in request.options.iter().enumerate() {
                eprintln!("  {}. {}", index + 1, option);
            }
            eprint!(
                "{}: ",
                if request.multi_select {
                    "Choose numbers separated by commas, or type another answer"
                } else {
                    "Choose a number, or type another answer"
                }
            );
            let _ = std::io::stderr().flush();
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer).ok()?;
            let answer = answer.trim();
            let selected = answer
                .split(',')
                .map(str::trim)
                .map(str::parse::<usize>)
                .collect::<Result<Vec<_>, _>>()
                .ok()
                .filter(|values| request.multi_select || values.len() == 1)
                .map(|values| {
                    values
                        .into_iter()
                        .filter_map(|index| request.options.get(index.saturating_sub(1)).cloned())
                        .collect::<Vec<_>>()
                })
                .filter(|values| !values.is_empty());
            Some(
                selected
                    .map(|values| values.join(", "))
                    .unwrap_or_else(|| answer.to_owned()),
            )
        })
        .await
        .unwrap_or(None)
    }
}

struct TerminalSink {
    json: bool,
    quiet: bool,
}

#[async_trait]
impl EventSink for TerminalSink {
    async fn emit(&self, event: AgentEvent) {
        if self.quiet {
            return;
        }
        if self.json {
            let value = agent_event_json(event);
            println!("{value}");
            return;
        }
        let event = event.into_unscoped();
        match event {
            AgentEvent::PlanUpdated(_) | AgentEvent::WorkflowUpdated(_) => {}
            AgentEvent::RouteDecided {
                tier,
                profile,
                confidence,
                auto_dispatched,
                reason,
                model,
                fallback,
            } => eprintln!(
                "[route] tier={} profile={} model={} confidence={} auto={} reason={reason}{}",
                tier.as_str(),
                profile.as_deref().unwrap_or("root"),
                model.as_deref().unwrap_or("-"),
                confidence,
                auto_dispatched,
                fallback
                    .map(|fallback| format!(" fallback={fallback}"))
                    .unwrap_or_default()
            ),
            AgentEvent::ModelFallback {
                purpose,
                skipped,
                used,
                reason,
            } => eprintln!(
                "[model fallback] {purpose}: skipped={} used={} reason={reason}",
                skipped.as_deref().unwrap_or("-"),
                used.as_deref().unwrap_or("-"),
            ),
            AgentEvent::TurnStarted { turn } if turn > 1 => eprintln!("[turn {turn}]"),
            AgentEvent::TurnPreempted { turn } => {
                eprintln!("[turn {turn}] preempted by a runtime event")
            }
            AgentEvent::ToolRequested(call) => eprintln!("[tool] {}", call.name),
            AgentEvent::ProviderProgress(event) => match event {
                willdeep_core::provider::ProviderEvent::TextDelta(text) => eprint!("{text}"),
                willdeep_core::provider::ProviderEvent::ReasoningDelta(_) => {}
                willdeep_core::provider::ProviderEvent::RetryWait { attempt, delay } => {
                    eprintln!("[retry {attempt}] waiting {} seconds", delay.as_secs_f64())
                }
                willdeep_core::provider::ProviderEvent::Usage(_) => {}
                willdeep_core::provider::ProviderEvent::RetryStarted { attempt } => {
                    eprintln!("[retry {attempt}] started")
                }
            },
            AgentEvent::ToolCompleted {
                call,
                output,
                is_error,
            } => {
                let status = if is_error { "error" } else { "done" };
                eprintln!("[tool:{status}] {}\n{}", call.name, compact_output(&output));
            }
            AgentEvent::Usage(usage) => {
                if let Some(total) = usage.total_tokens {
                    eprintln!("[usage] {total} tokens");
                }
            }
            AgentEvent::CompressionStarted { estimated_tokens } => {
                eprintln!("[context] compressing approximately {estimated_tokens} tokens");
            }
            AgentEvent::CompressionCompleted {
                estimated_tokens,
                dropped_messages,
            } => {
                if dropped_messages > 0 {
                    eprintln!(
                        "[context] compressed to approximately {estimated_tokens} tokens, dropped {dropped_messages} message(s) from the request view"
                    );
                } else {
                    eprintln!("[context] compressed to approximately {estimated_tokens} tokens");
                }
            }
            AgentEvent::BackgroundShellStarted { id } => {
                eprintln!("[background] started id={id}");
            }
            AgentEvent::BackgroundShellCompleted { id, status, .. } => {
                eprintln!("[background] finished id={id} status={status:?}");
            }
            AgentEvent::SubagentQueued { id, profile, .. } => {
                eprintln!("[subagent] queued {profile} ({id})");
            }
            AgentEvent::SubagentStarted {
                id,
                profile,
                background,
                ..
            } => eprintln!("[subagent] started id={id} profile={profile} background={background}"),
            AgentEvent::SubagentCompleted { id, status, .. } => {
                eprintln!("[subagent] finished id={id} status={status:?}")
            }
            AgentEvent::SubagentTurnStarted { id, turn } => {
                eprintln!("[subagent] id={id} turn={turn}")
            }
            AgentEvent::SubagentToolRequested { id, name } => {
                eprintln!("[subagent] id={id} tool={name}")
            }
            AgentEvent::SubagentToolCompleted { id, name, is_error } => eprintln!(
                "[subagent] id={id} tool={name} status={}",
                if is_error { "error" } else { "done" }
            ),
            AgentEvent::SubagentUsage { id, usage } => {
                if let Some(total) = usage.total_tokens {
                    eprintln!("[subagent] id={id} usage={total}");
                }
            }
            AgentEvent::SubagentRetryStarted { id, attempt } => {
                eprintln!("[subagent] id={id} retry={attempt} started")
            }
            AgentEvent::SubagentRetryWait { id, attempt, delay } => {
                eprintln!(
                    "[subagent] id={id} retry={attempt} waiting {} seconds",
                    delay.as_secs_f64()
                );
            }
            AgentEvent::SubagentVerdict {
                id,
                verifier_passed,
                attempts,
                ..
            } => eprintln!(
                "[subagent] id={id} verified={} attempts={attempts}",
                match verifier_passed {
                    Some(true) => "passed",
                    Some(false) => "failed",
                    None => "not-verified",
                }
            ),
            AgentEvent::GoalContinuationInjected { rung } => {
                eprintln!("[goal] not met · continuing · {rung:?}")
            }
            AgentEvent::GoalBudgetLimited { reason } => {
                eprintln!("[goal] budget exhausted · wrapping up · {reason:?}")
            }
            AgentEvent::AssistantText { .. }
            | AgentEvent::BackgroundWait { .. }
            | AgentEvent::TurnStarted { .. }
            | AgentEvent::RunScoped { .. } => {}
        }
    }
}

/// 失败详情写进持久事件日志，所以必须有上界：整段 stdout 或一个巨大的
/// 参数对象没有排查价值，只会把日志撑爆。头尾都留，中间掐掉——报错通常在末尾。
fn failure_excerpt(value: &str) -> String {
    const MAX_FAILURE_EXCERPT_CHARS: usize = 2_000;
    let characters = value.chars().count();
    if characters <= MAX_FAILURE_EXCERPT_CHARS {
        return value.to_owned();
    }
    let head = value
        .chars()
        .take(MAX_FAILURE_EXCERPT_CHARS / 2)
        .collect::<String>();
    let tail = value
        .chars()
        .skip(characters - MAX_FAILURE_EXCERPT_CHARS / 2)
        .collect::<String>();
    let dropped = characters - MAX_FAILURE_EXCERPT_CHARS;
    format!("{head}\n…[{dropped} chars omitted]…\n{tail}")
}

/// 提交轮次的那一端。审批与提问要弹回它，而不是弹给所有打开着同一个会话的端。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Surface {
    Tui,
    Web,
    Cli,
    /// Runtime 里的手机中继网关代手机提交的轮次。
    Mobile,
    /// 另一台 willdeep 实例经联邦入口发来的消息。
    Federation,
}

impl Surface {
    fn prefix(self) -> &'static str {
        match self {
            Self::Tui => "tui",
            Self::Web => "web",
            Self::Cli => "cli",
            Self::Mobile => "mobile",
            Self::Federation => "federation",
        }
    }
}

/// 这一端的客户端标识。
///
/// **按界面分,不是按进程分。** TUI 里用 `/webapp` 起的 Web 与 TUI 本身跑在同
/// 一个进程里,如果共用一个 ID,浏览器里发起的审批照样会弹到终端——那正是这个
/// 字段要解决的问题。
///
/// 每次启动重新生成,不落盘:进程没了,它发起的那些问题本来也没人回答得了,
/// 这时按会话回落比认一个死掉的 ID 更有用。
pub(crate) fn client_identity(surface: Surface) -> &'static str {
    static TUI: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static WEB: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static CLI: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static MOBILE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    static FEDERATION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let cell = match surface {
        Surface::Tui => &TUI,
        Surface::Web => &WEB,
        Surface::Cli => &CLI,
        Surface::Mobile => &MOBILE,
        Surface::Federation => &FEDERATION,
    };
    cell.get_or_init(|| format!("{}:{}", surface.prefix(), uuid::Uuid::new_v4()))
}

// 工具调用摘要（一句话说清这一步在干什么，路径打码、凭据脱敏、按字符截断）搬进了
// core 的 `conversation::tool_detail`：会话回放的投影也要用它。这里只再导出，事件
// JSON 与进程内 TUI 照旧调用，测试也留在这里。
pub(crate) use willdeep_core::conversation::tool_detail;

/// Preserve structured values and line breaks while redacting every display string.
fn public_workflow_value(workflow: &willdeep_core::workflow::WorkflowRun) -> serde_json::Value {
    fn redact(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                *text = text
                    .lines()
                    .map(willdeep_core::judge::redact_credentials)
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(redact),
            serde_json::Value::Object(fields) => fields.values_mut().for_each(redact),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(workflow).expect("workflow serializes");
    redact(&mut value);
    value
}

pub(crate) fn agent_event_json(event: AgentEvent) -> serde_json::Value {
    match event {
        AgentEvent::WorkflowUpdated(workflow) => serde_json::json!({
            "type": "workflow_updated",
            "workflow": public_workflow_value(&workflow),
        }),
        AgentEvent::PlanUpdated(mut plan) => {
            for step in &mut plan.steps {
                step.text = willdeep_core::judge::redact_credentials(&step.text);
                step.detail = step
                    .detail
                    .as_deref()
                    .map(willdeep_core::judge::redact_credentials);
            }
            serde_json::json!({"type":"plan_updated", "plan_update":willdeep_core::planning::Receipt::new(plan, false)})
        }
        AgentEvent::RunScoped {
            run_id,
            sequence,
            event,
        } => {
            let mut value = agent_event_json(*event);
            value["run_id"] = serde_json::json!(run_id);
            value["run_sequence"] = serde_json::json!(sequence);
            value
        }
        AgentEvent::RouteDecided {
            tier,
            profile,
            confidence,
            auto_dispatched,
            reason,
            model,
            fallback,
        } => serde_json::json!({
            "type": "route_decided",
            "tier": tier.as_str(),
            "profile": profile,
            "confidence": confidence,
            "auto_dispatched": auto_dispatched,
            "reason": reason,
            "model": model,
            "fallback": fallback
        }),
        AgentEvent::ModelFallback {
            purpose,
            skipped,
            used,
            reason,
        } => serde_json::json!({
            "type": "model_fallback",
            "purpose": purpose,
            "skipped": skipped,
            "used": used,
            "reason": reason
        }),
        AgentEvent::BackgroundWait { waiting } => {
            serde_json::json!({"type":"background_wait","waiting":waiting})
        }
        AgentEvent::TurnStarted { turn } => {
            serde_json::json!({"type": "turn_started", "turn": turn})
        }
        AgentEvent::TurnPreempted { turn } => {
            serde_json::json!({"type": "turn_preempted", "turn": turn})
        }
        AgentEvent::AssistantText { text, model } => {
            serde_json::json!({"type": "assistant_text", "text": text, "model": model})
        }
        AgentEvent::ProviderProgress(event) => match event {
            willdeep_core::provider::ProviderEvent::RetryStarted { attempt } => {
                serde_json::json!({"type":"provider_retry_started", "attempt":attempt})
            }
            willdeep_core::provider::ProviderEvent::TextDelta(text) => {
                serde_json::json!({"type":"assistant_text_delta","text":text})
            }
            willdeep_core::provider::ProviderEvent::ReasoningDelta(text) => {
                serde_json::json!({"type":"reasoning_delta","text":text})
            }
            willdeep_core::provider::ProviderEvent::Usage(usage) => {
                serde_json::json!({"type":"usage_snapshot","usage":usage})
            }
            willdeep_core::provider::ProviderEvent::RetryWait { attempt, delay } => {
                serde_json::json!({"type":"provider_retry_wait","attempt":attempt,"delay_ms":delay.as_millis()})
            }
        },
        AgentEvent::ToolRequested(call) => {
            let mut value = serde_json::json!({
                "type": "tool_requested",
                "id": call.id,
                "name": call.name
            });
            if let Some(object) = value.as_object_mut() {
                if let Some(detail) = tool_detail(&call.name, &call.arguments) {
                    object.insert("detail".to_owned(), serde_json::Value::String(detail));
                }
                // 点开工具行时显示的完整命令。与摘要分开：摘要要短、要省略路径。
                if let Some(expansion) =
                    willdeep_core::conversation::tool_expansion(&call.name, &call.arguments)
                {
                    object.insert("expansion".to_owned(), serde_json::Value::String(expansion));
                }
            }
            value
        }
        // 成功的工具只报名字。失败的额外带上参数和输出尾巴——「哪条命令、为什么挂」
        // 全在这两个字段里，没有它们，事后排查只剩一个 task_id。这两个字段名正是
        // `public_event` 会从公共事件流里剥掉的那两个，所以远端客户端看不到；
        // 本机通过 `task.diagnostics` 读未脱敏原文。
        AgentEvent::ToolCompleted {
            call,
            output,
            is_error,
        } => {
            let mut value = serde_json::json!({
                "type": "tool_completed",
                "id": call.id,
                "name": call.name,
                "is_error": is_error
            });
            if !is_error
                && call.name == "update_plan"
                && let Some(mut receipt) = willdeep_core::planning::receipt(&output)
            {
                for step in &mut receipt.plan.steps {
                    step.text = willdeep_core::judge::redact_credentials(&step.text);
                    step.detail = step
                        .detail
                        .as_deref()
                        .map(willdeep_core::judge::redact_credentials);
                }
                value["plan_update"] = serde_json::to_value(receipt).expect("plan serializes");
            }
            if !is_error
                && call.name == "workflow_control"
                && let Ok(receipt) = serde_json::from_str::<serde_json::Value>(&output)
                && let Some(run) = receipt.get("workflow")
                && let Ok(run) = serde_json::from_value(run.clone())
            {
                value["workflow"] = public_workflow_value(&run);
            }
            if !is_error
                && call.name == "run_command"
                && let Some(seconds) =
                    willdeep_core::tools::command_wait::notice_wait_seconds(&output)
            {
                value["background_delay_seconds"] = seconds.into();
            }
            if let Some(object) = value.as_object_mut() {
                if let Some(detail) = tool_detail(&call.name, &call.arguments) {
                    object.insert("detail".to_owned(), serde_json::Value::String(detail));
                }
                if let Some(expansion) =
                    willdeep_core::conversation::tool_expansion(&call.name, &call.arguments)
                {
                    object.insert("expansion".to_owned(), serde_json::Value::String(expansion));
                }
            }
            if is_error && let Some(object) = value.as_object_mut() {
                object.insert(
                    "arguments".to_owned(),
                    serde_json::Value::String(failure_excerpt(
                        &willdeep_core::judge::redact_credentials(&call.arguments),
                    )),
                );
                object.insert(
                    "output".to_owned(),
                    serde_json::Value::String(failure_excerpt(
                        &willdeep_core::judge::redact_credentials(&output),
                    )),
                );
            }
            value
        }
        AgentEvent::Usage(usage) => serde_json::json!({
            "type": "usage",
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.total_tokens,
            "cache_read_tokens": usage.cache_read_tokens
        }),
        AgentEvent::CompressionStarted { estimated_tokens } => serde_json::json!({
            "type": "compression_started",
            "estimated_tokens": estimated_tokens
        }),
        AgentEvent::CompressionCompleted {
            estimated_tokens,
            dropped_messages,
        } => serde_json::json!({
            "type": "compression_completed",
            "estimated_tokens": estimated_tokens,
            "dropped_messages": dropped_messages
        }),
        AgentEvent::BackgroundShellStarted { id } => serde_json::json!({
            "type": "background_shell_started",
            "id": id
        }),
        AgentEvent::BackgroundShellCompleted {
            id,
            status,
            exit_code,
            elapsed_millis,
            output_bytes,
        } => serde_json::json!({
            "type": "background_shell_completed",
            "id": id,
            "status": status,
            "exit_code": exit_code,
            "elapsed_millis": elapsed_millis,
            "output_bytes": output_bytes
        }),
        AgentEvent::SubagentQueued {
            id,
            profile,
            model,
            label,
        } => serde_json::json!({
            "type": "subagent_queued", "id": id, "profile": profile,
            "model": model, "label": label, "background": true
        }),
        AgentEvent::SubagentStarted {
            id,
            profile,
            model,
            label,
            background,
            max_turns,
            token_budget,
            timeout_seconds,
            workspace: _,
            root_workspace: _,
            worktree_branch,
            dedicated_worktree,
        } => serde_json::json!({
            "type": "subagent_started",
            "id": id,
            "profile": profile,
            "model": model,
            "label": label,
            "background": background,
            "max_turns": max_turns,
            "token_budget": token_budget,
            "timeout_seconds": timeout_seconds,
            "worktree_branch": worktree_branch,
            "dedicated_worktree": dedicated_worktree
        }),
        AgentEvent::SubagentCompleted {
            id,
            status,
            report: _,
        } => serde_json::json!({
            "type": "subagent_completed",
            "id": id,
            "status": match status {
                SubagentLifecycleStatus::Completed => "completed",
                SubagentLifecycleStatus::Partial => "partial",
                SubagentLifecycleStatus::Blocked => "blocked",
                SubagentLifecycleStatus::Cancelled => "cancelled",
                SubagentLifecycleStatus::Failed => "failed",
            }
        }),
        AgentEvent::GoalContinuationInjected { rung } => serde_json::json!({
            "type": "goal_continuation_injected",
            "rung": match rung {
                ContinuationRung::Guidance => "guidance",
                ContinuationRung::Reconcile => "reconcile",
                ContinuationRung::Backoff => "backoff",
            }
        }),
        AgentEvent::GoalBudgetLimited { reason } => serde_json::json!({
            "type": "goal_budget_limited",
            "reason": match reason {
                SoftStopReason::WallClock => "wall_clock",
                SoftStopReason::Continuations => "continuations",
                SoftStopReason::Tokens => "tokens",
            }
        }),
        AgentEvent::SubagentVerdict {
            id,
            repo_commit,
            verifier_command,
            verifier_passed,
            attempts,
            claims_checked,
            claims_unverifiable,
        } => serde_json::json!({
            "type": "subagent_verdict",
            "id": id,
            "repo_commit": repo_commit,
            "verifier_command": verifier_command,
            "verifier_passed": verifier_passed,
            "attempts": attempts,
            "claims_checked": claims_checked,
            "claims_unverifiable": claims_unverifiable
        }),
        AgentEvent::SubagentTurnStarted { id, turn } => serde_json::json!({
            "type": "subagent_turn_started",
            "id": id,
            "turn": turn
        }),
        AgentEvent::SubagentToolRequested { id, name } => serde_json::json!({
            "type": "subagent_tool_requested",
            "id": id,
            "name": name
        }),
        AgentEvent::SubagentToolCompleted { id, name, is_error } => serde_json::json!({
            "type": "subagent_tool_completed",
            "id": id,
            "name": name,
            "is_error": is_error
        }),
        AgentEvent::SubagentUsage { id, usage } => serde_json::json!({
            "type": "subagent_usage",
            "id": id,
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.total_tokens
        }),
        AgentEvent::SubagentRetryWait { id, attempt, delay } => serde_json::json!({
            "type": "subagent_retry_wait", "id": id, "attempt": attempt,
            "delay_ms": delay.as_millis()
        }),
        AgentEvent::SubagentRetryStarted { id, attempt } => {
            serde_json::json!({"type":"subagent_retry_started", "id":id, "attempt":attempt})
        }
    }
}

fn compact_output(output: &str) -> String {
    const LIMIT: usize = 2_000;
    let mut value = output.chars().take(LIMIT).collect::<String>();
    if output.chars().count() > LIMIT {
        value.push_str("\n[display truncated]");
    }
    value
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod main_tests;

#[cfg(test)]
#[path = "main_cli_tests.rs"]
mod tests;
