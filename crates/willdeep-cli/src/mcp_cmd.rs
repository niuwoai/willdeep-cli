//! `willdeep mcp`：看配置里的 MCP 服务、给远程服务做 OAuth 登录、连上去列工具。
//!
//! 登录是 OAuth 2.1 授权码 + PKCE：本机开一个只监听 127.0.0.1 的随机端口收回调，
//! token 落在 `$WILLDEEP_HOME/mcp-oauth/<name>.json`（0600）。之后 Harness 连这个
//! 服务时自动带 token、到期自动刷新；刷新不了就再登一次。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use willdeep_core::mcp::{McpOAuthConfig, McpRegistry, McpServerConfig, McpTransportKind, oauth};

use crate::LoadedConfig;
use crate::config;
use crate::i18n::Language;

#[derive(Clone, Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum McpAction {
    /// List configured MCP servers with their transport and authentication state.
    List {
        /// Emit one JSON array instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Log in to a remote MCP server with OAuth 2.1 (authorization code + PKCE, loopback redirect).
    Login {
        /// Server name, as in $WILLDEEP_HOME/.mcp/<name>.toml.
        name: String,
        /// Seconds to wait for the browser to come back.
        #[arg(long, default_value_t = 300)]
        timeout: u64,
        /// Print the authorization URL only; do not try to open a browser.
        #[arg(long)]
        no_browser: bool,
    },
    /// Forget the stored OAuth tokens of a server.
    Logout {
        /// Server name, as in $WILLDEEP_HOME/.mcp/<name>.toml.
        name: String,
    },
    /// Connect to one server and list the tools it exposes.
    Tools {
        /// Server name, as in $WILLDEEP_HOME/.mcp/<name>.toml.
        name: String,
        /// Emit one JSON array with full input schemas.
        #[arg(long)]
        json: bool,
    },
    /// Write one MCP server to $WILLDEEP_HOME/.mcp/<name>.toml.
    ///
    /// stdio takes the command and its arguments after `--`; http takes exactly one URL.
    /// A name that already has a file is refused rather than overwritten.
    Add {
        /// Server name: 1 to 64 characters of ASCII letters, numbers, '_' or '-'.
        name: String,
        /// stdio (the default) or http. `sse` is not a transport here; a remote
        /// server is Streamable HTTP and stores a `url`.
        #[arg(long, short = 't', value_name = "stdio|http")]
        transport: Option<String>,
        /// stdio only. Repeatable `KEY=VALUE`, stored as the server's environment.
        /// The value is literal, not a `${env:NAME}` placeholder.
        #[arg(long, short = 'e', value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// http only. Repeatable `Name: value`, sent with every request. A header
        /// that looks like a credential must be written as `${env:NAME}`.
        #[arg(long, short = 'H', value_name = "Name: value")]
        header: Vec<String>,
        /// http only. The bearer token, written in plaintext into this server's own
        /// file (mode 0600). It lands in the shell history, so prefer `--bearer-env`
        /// when the value should stay out of the file. Exclusive with `--bearer-env`
        /// and the `--oauth-*` flags.
        #[arg(long, value_name = "TOKEN")]
        token: Option<String>,
        /// http only. Name of the environment variable holding a static bearer
        /// token. The token itself never goes in the file or on this command line.
        #[arg(long, value_name = "ENV")]
        bearer_env: Option<String>,
        /// http only. Pre-registered OAuth client id. Omit it to register dynamically
        /// on `willdeep mcp login`.
        #[arg(long, value_name = "ID")]
        oauth_client_id: Option<String>,
        /// http only. Environment variable holding the OAuth client secret.
        #[arg(long, value_name = "ENV")]
        oauth_client_secret_env: Option<String>,
        /// http only. Repeatable OAuth scope to request. Exclusive with `--bearer-env`.
        #[arg(long, value_name = "SCOPE")]
        oauth_scope: Vec<String>,
        /// Seconds to wait for the server's first response, 1 to 300.
        #[arg(long, default_value_t = 30)]
        startup_timeout_seconds: u64,
        /// Write the server disabled. It stays on disk but is not connected.
        #[arg(long)]
        disabled: bool,
        /// stdio: the command, then its arguments. http: exactly one URL.
        /// Trailing so that `mcp add <name> -- npx -y some-server` keeps its flags.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command_or_url: Vec<String>,
    },
    /// Delete $WILLDEEP_HOME/.mcp/<name>.toml. Stored OAuth tokens are left in place;
    /// `willdeep mcp logout <name>` forgets those.
    Remove {
        /// Server name, as in $WILLDEEP_HOME/.mcp/<name>.toml.
        name: String,
    },
}

pub(crate) async fn run(
    action: McpAction,
    home: &Path,
    config_path: Option<&Path>,
    language: Language,
) -> Result<()> {
    let loaded = LoadedConfig::load(config_path)?;
    let servers = &loaded.file.mcp_servers;
    match action {
        McpAction::List { json } => list(home, servers, json, language),
        McpAction::Login {
            name,
            timeout,
            no_browser,
        } => login(home, servers, &name, timeout, no_browser, language).await,
        McpAction::Logout { name } => logout(home, &name, language),
        McpAction::Tools { name, json } => tools(home, servers, &name, json, language).await,
        McpAction::Add {
            name,
            transport,
            env,
            header,
            token,
            bearer_env,
            oauth_client_id,
            oauth_client_secret_env,
            oauth_scope,
            startup_timeout_seconds,
            disabled,
            command_or_url,
        } => add(
            config_path,
            &name,
            transport.as_deref(),
            &env,
            &header,
            token.as_deref(),
            bearer_env.as_deref(),
            oauth_client_id.as_deref(),
            oauth_client_secret_env.as_deref(),
            &oauth_scope,
            startup_timeout_seconds,
            disabled,
            &command_or_url,
            language,
        ),
        McpAction::Remove { name } => remove(config_path, &name, language),
    }
}

fn server<'a>(
    servers: &'a BTreeMap<String, McpServerConfig>,
    name: &str,
) -> Result<&'a McpServerConfig> {
    servers.get(name).with_context(|| {
        format!(
            "no MCP server {name} in $WILLDEEP_HOME/.mcp (known: {})",
            servers.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })
}

fn auth_label(home: &Path, name: &str, config: &McpServerConfig, language: Language) -> String {
    if config
        .token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty())
    {
        // 只说「写了明文」，token 本身不进列表、也不进 --json。
        return language
            .text(
                "bearer 明文（已配置）",
                "bearer inline (set)",
                "bearer 平文（設定済み）",
            )
            .to_owned();
    }
    if let Some(variable) = &config.bearer_token_env {
        let present = std::env::var(variable).is_ok_and(|value| !value.trim().is_empty());
        return format!(
            "bearer ${variable} ({})",
            if present {
                language.text("已设置", "set", "設定済み")
            } else {
                language.text("未设置", "unset", "未設定")
            }
        );
    }
    if config.oauth.is_some() {
        return match oauth::load_tokens(home, name) {
            Ok(Some(tokens)) => format!(
                "oauth {}{}",
                language.text("已登录", "signed in", "ログイン済み"),
                match tokens.expires_at {
                    Some(at) if tokens.expired() => format!(
                        "，{} {}",
                        language.text("已过期于", "expired at", "期限切れ"),
                        willdeep_core::session::format_iso8601(at)
                    ),
                    Some(at) => format!(
                        "，{} {}",
                        language.text("有效期至", "valid until", "有効期限"),
                        willdeep_core::session::format_iso8601(at)
                    ),
                    None => String::new(),
                }
            ),
            Ok(None) => format!(
                "oauth {}",
                language.text("未登录", "not signed in", "未ログイン")
            ),
            Err(error) => format!("oauth ({error})"),
        };
    }
    language.text("无", "none", "なし").to_owned()
}

fn list(
    home: &Path,
    servers: &BTreeMap<String, McpServerConfig>,
    json: bool,
    language: Language,
) -> Result<()> {
    if json {
        let rows = servers
            .iter()
            .map(|(name, config)| {
                serde_json::json!({
                    "name": name,
                    "transport": transport_label(config),
                    "enabled": config.enabled,
                    "target": config.url.clone().or_else(|| config.command.clone()),
                    "auth": auth_label(home, name, config, Language::En),
                    "valid": config.validate(name).map(|_| true).unwrap_or(false),
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if servers.is_empty() {
        println!(
            "{}",
            language.text(
                "没有 MCP 服务（$WILLDEEP_HOME/.mcp/<name>.toml）。",
                "No MCP servers configured ($WILLDEEP_HOME/.mcp/<name>.toml).",
                "MCP サーバーは設定されていません（$WILLDEEP_HOME/.mcp/<name>.toml）。"
            )
        );
        return Ok(());
    }
    for (name, config) in servers {
        let state = match config.validate(name) {
            Ok(()) if config.enabled => language.text("启用", "enabled", "有効").to_owned(),
            Ok(()) => language.text("停用", "disabled", "無効").to_owned(),
            Err(error) => format!(
                "{}: {error}",
                language.text("配置无效", "invalid", "無効な設定")
            ),
        };
        println!(
            "{name:<20} {:<6} {:<8} {}\n{:>20} {}: {}",
            transport_label(config),
            state,
            config
                .url
                .clone()
                .or_else(|| config.command.clone())
                .unwrap_or_default(),
            "",
            language.text("鉴权", "auth", "認証"),
            auth_label(home, name, config, language)
        );
    }
    Ok(())
}

fn transport_label(config: &McpServerConfig) -> &'static str {
    match config.validated_transport("") {
        Ok(McpTransportKind::Stdio) => "stdio",
        Ok(McpTransportKind::StreamableHttp) => "http",
        Err(_) => "?",
    }
}

async fn login(
    home: &Path,
    servers: &BTreeMap<String, McpServerConfig>,
    name: &str,
    timeout: u64,
    no_browser: bool,
    language: Language,
) -> Result<()> {
    let config = server(servers, name)?;
    config.validate(name)?;
    let Some(url) = config.url.as_deref() else {
        bail!(
            "{}",
            language.text(
                "这是一个 stdio 服务，没有登录一说。",
                "this is a stdio server; there is nothing to log in to.",
                "stdio サーバーにはログインはありません。"
            )
        );
    };
    let Some(oauth_config) = &config.oauth else {
        bail!(
            "{}",
            language.text(
                "这个服务没有配置 oauth；静态 token 请用 token 或 bearer_token_env。",
                "this server has no oauth section; use token or bearer_token_env for a static token.",
                "このサーバーには oauth の設定がありません。静的トークンは token または bearer_token_env を使ってください。"
            )
        );
    };
    let client_secret = match &oauth_config.client_secret_env {
        Some(variable) => Some(std::env::var(variable).with_context(|| {
            format!("oauth.client_secret_env points at {variable}, which is not set")
        })?),
        None => None,
    };
    let progress = |line: &str| println!("{line}");
    let open = move |authorize_url: &str| {
        if !no_browser && !open_in_browser(authorize_url) {
            eprintln!(
                "{}",
                language.text(
                    "打不开浏览器，请手动打开上面的 URL。",
                    "could not open a browser; open the URL above by hand.",
                    "ブラウザを開けませんでした。上の URL を手動で開いてください。"
                )
            );
        }
    };
    let outcome = oauth::login(
        oauth::LoginRequest {
            home,
            server: name,
            url,
            config: oauth_config,
            client_secret,
            timeout: Duration::from_secs(timeout.max(10)),
        },
        &open,
        &progress,
    )
    .await?;
    println!(
        "{}: {} · client_id {}{} · scope {} · {}",
        language.text("已登录", "signed in", "ログイン済み"),
        outcome.authorization_server,
        outcome.client_id,
        if outcome.registered_dynamically {
            language.text("（动态注册）", " (registered dynamically)", "（動的登録）")
        } else {
            ""
        },
        outcome.scope.as_deref().unwrap_or("-"),
        match outcome.expires_at {
            Some(at) => format!(
                "{} {}",
                language.text("有效期至", "valid until", "有効期限"),
                willdeep_core::session::format_iso8601(at)
            ),
            None => language
                .text("无过期时间", "no expiry", "期限なし")
                .to_owned(),
        }
    );
    // 登完立刻连一次：token 拿到了但服务连不上，现在就该知道，而不是下一轮才发现。
    let mut only = BTreeMap::new();
    only.insert(
        name.to_owned(),
        McpServerConfig {
            enabled: true,
            ..config.clone()
        },
    );
    let registry = McpRegistry::connect_in(Some(home), &only).await?;
    if !registry.has_server(name) {
        bail!(
            "{}",
            language.text(
                "登录成功，但连接服务失败（见上方警告）。",
                "signed in, but connecting to the server failed (see the warning above).",
                "ログインは成功しましたが、サーバーへの接続に失敗しました（上の警告を参照）。"
            )
        );
    }
    println!(
        "{}: {}",
        language.text("可用工具", "tools available", "利用可能なツール"),
        registry.definitions().len()
    );
    Ok(())
}

fn logout(home: &Path, name: &str, language: Language) -> Result<()> {
    if oauth::forget_tokens(home, name)? {
        println!(
            "{}",
            language.text("已清除登录状态。", "signed out.", "ログアウトしました。")
        );
    } else {
        println!(
            "{}",
            language.text(
                "本来就没有登录状态。",
                "nothing to sign out of.",
                "ログイン状態はありませんでした。"
            )
        );
    }
    Ok(())
}

async fn tools(
    home: &Path,
    servers: &BTreeMap<String, McpServerConfig>,
    name: &str,
    json: bool,
    language: Language,
) -> Result<()> {
    let config = server(servers, name)?;
    let mut only = BTreeMap::new();
    only.insert(
        name.to_owned(),
        McpServerConfig {
            enabled: true,
            ..config.clone()
        },
    );
    let registry = McpRegistry::connect_in(Some(home), &only).await?;
    if !registry.has_server(name) {
        bail!(
            "{}",
            language.text(
                "连接失败（见上方警告）。",
                "connection failed (see the warning above).",
                "接続に失敗しました（上の警告を参照）。"
            )
        );
    }
    let definitions = registry.definitions();
    if json {
        println!("{}", serde_json::to_string_pretty(&definitions)?);
        return Ok(());
    }
    if definitions.is_empty() {
        println!(
            "{}",
            language.text(
                "连上了，但这个服务没有工具（可能只提供资源）。",
                "connected, but this server exposes no tools (it may only serve resources).",
                "接続しましたが、このサーバーにはツールがありません（リソースのみの可能性）。"
            )
        );
        return Ok(());
    }
    for definition in definitions {
        println!(
            "{:<48} {}",
            definition.name,
            definition.description.lines().next().unwrap_or_default()
        );
    }
    Ok(())
}

/// 远程服务长什么样。只有 `http://` / `https://` 开头才算 URL，其余都当命令。
fn looks_like_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

fn parse_env_pair(raw: &str) -> Result<(String, String)> {
    let (key, value) = raw.split_once('=').with_context(|| {
        format!("--env {raw:?} is not KEY=VALUE; the value may be empty but the '=' is required")
    })?;
    if key.is_empty()
        || !key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("--env name {key:?} must be ASCII letters, numbers or '_'");
    }
    Ok((key.to_owned(), value.to_owned()))
}

fn parse_header(raw: &str) -> Result<(String, String)> {
    let (name, value) = raw.split_once(':').with_context(|| {
        format!("--header {raw:?} is not 'Name: value'; separate the name and value with ':'")
    })?;
    let name = name.trim();
    if name.is_empty() {
        bail!("--header {raw:?} has an empty name");
    }
    Ok((name.to_owned(), value.trim().to_owned()))
}

fn env_name(flag: &str, value: &str) -> Result<String> {
    let name = value.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        bail!("{flag} needs an environment variable name, not the value it holds");
    }
    Ok(name.to_owned())
}

#[allow(clippy::too_many_arguments)]
fn add(
    config_path: Option<&Path>,
    name: &str,
    transport: Option<&str>,
    env_pairs: &[String],
    headers: &[String],
    token: Option<&str>,
    bearer_env: Option<&str>,
    oauth_client_id: Option<&str>,
    oauth_client_secret_env: Option<&str>,
    oauth_scopes: &[String],
    startup_timeout_seconds: u64,
    disabled: bool,
    command_or_url: &[String],
    language: Language,
) -> Result<()> {
    if !config::mcp_name_is_valid(name) {
        bail!(
            "MCP server name {name:?} must be 1 to 64 characters of ASCII letters, numbers, '_' or '-'"
        );
    }
    let kind = match transport.map(str::trim).filter(|value| !value.is_empty()) {
        None => None,
        Some("stdio") => Some(McpTransportKind::Stdio),
        Some("http") => Some(McpTransportKind::StreamableHttp),
        Some("sse") => bail!(
            "sse is not a transport here; a remote MCP server is Streamable HTTP, so use --transport http and give its URL"
        ),
        Some(other) => bail!("unknown transport {other:?}; use stdio or http"),
    };
    let inferred = match kind {
        Some(explicit) => explicit,
        None => match command_or_url {
            [only] if looks_like_url(only) => McpTransportKind::StreamableHttp,
            _ => McpTransportKind::Stdio,
        },
    };

    let mut server = McpServerConfig {
        startup_timeout_seconds,
        enabled: !disabled,
        ..McpServerConfig::default()
    };
    match inferred {
        McpTransportKind::Stdio => {
            if headers.iter().any(|header| !header.is_empty())
                || token.is_some()
                || bearer_env.is_some()
            {
                bail!(
                    "--header, --token and --bearer-env belong to an http server, not a stdio command"
                );
            }
            if oauth_client_id.is_some()
                || oauth_client_secret_env.is_some()
                || !oauth_scopes.is_empty()
            {
                bail!(
                    "--oauth-client-id, --oauth-client-secret-env and --oauth-scope belong to an http server"
                );
            }
            let Some((command, args)) = command_or_url.split_first() else {
                bail!(
                    "a stdio server needs a command: willdeep mcp add {name} -- <command> [args...]"
                );
            };
            if looks_like_url(command) {
                bail!(
                    "{command} looks like a URL, but this is a stdio server; pass --transport http to store it as one"
                );
            }
            server.command = Some(command.clone());
            server.args = args.to_vec();
            for raw in env_pairs {
                let (key, value) = parse_env_pair(raw)?;
                if server.env.insert(key.clone(), value).is_some() {
                    bail!("--env {key} is given more than once");
                }
            }
        }
        McpTransportKind::StreamableHttp => {
            if !env_pairs.is_empty() {
                bail!(
                    "--env belongs to a stdio server; an http server reads its credentials from the environment it was started in"
                );
            }
            let [url] = command_or_url else {
                bail!("an http server takes exactly one URL, not a command and arguments");
            };
            server.url = Some(url.clone());
            for raw in headers {
                let (key, value) = parse_header(raw)?;
                if server.headers.insert(key.clone(), value).is_some() {
                    bail!("--header {key} is given more than once");
                }
            }
            server.token = token
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            server.bearer_token_env = bearer_env
                .map(|value| env_name("--bearer-env", value))
                .transpose()?;
            let wants_oauth = oauth_client_id.is_some()
                || oauth_client_secret_env.is_some()
                || !oauth_scopes.is_empty();
            if wants_oauth {
                server.oauth = Some(McpOAuthConfig {
                    client_id: oauth_client_id.map(str::to_owned),
                    client_secret_env: oauth_client_secret_env
                        .map(|value| env_name("--oauth-client-secret-env", value))
                        .transpose()?,
                    scopes: oauth_scopes.to_vec(),
                });
            }
        }
    }

    let path = config::add_mcp_server(config_path, name, &server)?;
    println!(
        "{}: {} ({})",
        language.text("已添加", "added", "追加しました"),
        name,
        path.display()
    );
    println!(
        "{}",
        language.text(
            "新文件要等 runtime 下次启动才连得上；凭据走环境变量的，变量必须在启动 runtime 之前就在它的环境里。",
            "The new file is connected the next time the runtime starts. A credential read from an environment variable must already be in the environment that starts the runtime, not one exported afterwards.",
            "新しいファイルは runtime の次回起動時に接続されます。環境変数から読む資格情報は、runtime を起動する環境にあらかじめ入っている必要があります。"
        )
    );
    Ok(())
}

fn remove(config_path: Option<&Path>, name: &str, language: Language) -> Result<()> {
    if config::remove_mcp_server(config_path, name)? {
        println!(
            "{}: {name}",
            language.text("已删除", "removed", "削除しました")
        );
    } else {
        println!(
            "{}: {name}",
            language.text("本来就没有", "nothing to remove", "元からありませんでした")
        );
    }
    Ok(())
}

/// 尽力打开系统浏览器；打不开也不是错——URL 已经打印出来了。
fn open_in_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("willdeep-mcp-add-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        // 加载按配置文件是否存在决定家目录；空文件即可，MCP 不写进它。
        std::fs::write(path.join("config.toml"), b"").unwrap();
        path
    }

    fn add_ok(home: &Path, name: &str, transport: Option<&str>, extra: &[&str]) -> McpServerConfig {
        let mut command_or_url = Vec::new();
        let mut env = Vec::new();
        let mut header = Vec::new();
        let mut token = None;
        let mut bearer_env = None;
        let mut rest = extra;
        while let Some((flag, tail)) = rest.split_first() {
            match *flag {
                "--" => {
                    command_or_url.extend(tail.iter().map(|value| (*value).to_owned()));
                    break;
                }
                "-e" => {
                    env.push(tail[0].to_owned());
                    rest = &tail[1..];
                }
                "-H" => {
                    header.push(tail[0].to_owned());
                    rest = &tail[1..];
                }
                "--token" => {
                    token = Some(tail[0]);
                    rest = &tail[1..];
                }
                "--bearer-env" => {
                    bearer_env = Some(tail[0]);
                    rest = &tail[1..];
                }
                other => panic!("test flag {other} is not wired"),
            }
        }
        add(
            Some(&home.join("config.toml")),
            name,
            transport,
            &env,
            &header,
            token,
            bearer_env,
            None,
            None,
            &[],
            30,
            false,
            &command_or_url,
            Language::En,
        )
        .unwrap();
        let loaded = LoadedConfig::load(Some(&home.join("config.toml"))).unwrap();
        loaded.file.mcp_servers.get(name).unwrap().clone()
    }

    #[test]
    fn stdio_server_round_trips_with_args_and_env() {
        let home = scratch();
        let server = add_ok(
            &home,
            "filesystem",
            None,
            &["-e", "NODE_NO_WARNINGS=1", "--", "npx", "-y", "some-server"],
        );
        assert_eq!(server.command.as_deref(), Some("npx"));
        assert_eq!(server.args, ["-y", "some-server"]);
        assert_eq!(
            server.env.get("NODE_NO_WARNINGS").map(String::as_str),
            Some("1")
        );
        assert!(server.url.is_none());
        assert!(server.enabled);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn http_server_keeps_the_variable_name_not_the_token() {
        let home = scratch();
        let server = add_ok(
            &home,
            "issues",
            Some("http"),
            &[
                "--bearer-env",
                "ISSUES_TOKEN",
                "-H",
                "X-Trace: willdeep",
                "--",
                "https://mcp.example.com/mcp",
            ],
        );
        assert_eq!(server.url.as_deref(), Some("https://mcp.example.com/mcp"));
        assert_eq!(server.bearer_token_env.as_deref(), Some("ISSUES_TOKEN"));
        assert_eq!(
            server.headers.get("X-Trace").map(String::as_str),
            Some("willdeep")
        );
        let written = std::fs::read_to_string(home.join(".mcp/issues.toml")).unwrap();
        assert!(!written.contains("secret-token"), "{written}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn token_is_written_into_the_server_file_and_excludes_bearer_env() {
        let home = scratch();
        let server = add_ok(
            &home,
            "panel",
            Some("http"),
            &["--token", "paste-me", "--", "https://panel.example.com/mcp"],
        );
        assert_eq!(server.token.as_deref(), Some("paste-me"));
        assert!(server.bearer_token_env.is_none());
        let written = std::fs::read_to_string(home.join(".mcp/panel.toml")).unwrap();
        assert!(written.contains("paste-me"), "{written}");

        let both = add(
            Some(&home.join("config.toml")),
            "panel-both",
            Some("http"),
            &[],
            &[],
            Some("paste-me"),
            Some("PANEL_TOKEN"),
            None,
            None,
            &[],
            30,
            false,
            &["https://panel.example.com/mcp".to_owned()],
            Language::En,
        )
        .unwrap_err();
        assert!(both.to_string().contains("mutually exclusive"), "{both}");
        assert!(!home.join(".mcp/panel-both.toml").exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_bare_url_is_stored_as_http_without_a_transport_flag() {
        let home = scratch();
        let server = add_ok(
            &home,
            "remote",
            None,
            &["--", "https://mcp.example.com/mcp"],
        );
        assert_eq!(server.url.as_deref(), Some("https://mcp.example.com/mcp"));
        assert!(server.command.is_none());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn an_existing_server_is_refused_instead_of_overwritten() {
        let home = scratch();
        let config = Some(home.join("config.toml"));
        add(
            config.as_deref(),
            "filesystem",
            None,
            &[],
            &[],
            None,
            None,
            None,
            None,
            &[],
            30,
            false,
            &["npx".to_owned()],
            Language::En,
        )
        .unwrap();
        let error = add(
            config.as_deref(),
            "filesystem",
            None,
            &[],
            &[],
            None,
            None,
            None,
            None,
            &[],
            30,
            false,
            &["other".to_owned()],
            Language::En,
        )
        .unwrap_err();
        assert!(error.to_string().contains("already exists"), "{error}");
        let loaded = LoadedConfig::load(config.as_deref()).unwrap();
        assert_eq!(
            loaded.file.mcp_servers["filesystem"].command.as_deref(),
            Some("npx")
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_plaintext_credential_header_never_reaches_disk() {
        let home = scratch();
        let error = add(
            Some(&home.join("config.toml")),
            "issues",
            Some("http"),
            &[],
            &["X-Api-Key: literal-secret".to_owned()],
            None,
            None,
            None,
            None,
            &[],
            30,
            false,
            &["https://mcp.example.com/mcp".to_owned()],
            Language::En,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("looks like a credential"),
            "{error}"
        );
        assert!(!home.join(".mcp/issues.toml").exists());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn sse_is_rejected_and_points_at_streamable_http() {
        let home = scratch();
        let error = add(
            Some(&home.join("config.toml")),
            "legacy",
            Some("sse"),
            &[],
            &[],
            None,
            None,
            None,
            None,
            &[],
            30,
            false,
            &["https://mcp.example.com/sse".to_owned()],
            Language::En,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Streamable HTTP"), "{error}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn remove_deletes_the_file_and_is_quiet_when_absent() {
        let home = scratch();
        let config = home.join("config.toml");
        add(
            Some(&config),
            "filesystem",
            None,
            &[],
            &[],
            None,
            None,
            None,
            None,
            &[],
            30,
            false,
            &["npx".to_owned()],
            Language::En,
        )
        .unwrap();
        remove(Some(&config), "filesystem", Language::En).unwrap();
        let loaded = LoadedConfig::load(Some(&config)).unwrap();
        assert!(!loaded.file.mcp_servers.contains_key("filesystem"));
        remove(Some(&config), "filesystem", Language::En).unwrap();
        let _ = std::fs::remove_dir_all(&home);
    }
}
