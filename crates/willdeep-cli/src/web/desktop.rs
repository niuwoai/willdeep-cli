use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path as FilePath, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, ensure};
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::daemon::diff_review::{self, DiffArea, DiffAttributionRecord, DiffSnapshot};

use super::{WebConfig, WebError, WebState};

const BOOTSTRAP_PREFIX: &str = "/__willdeep_desktop/";

pub(crate) async fn serve_desktop(config_path: Option<&FilePath>) -> Result<()> {
    ensure!(
        !std::io::stdin().is_terminal(),
        "desktop host requires a private input pipe"
    );
    ensure!(
        super::embedded_assets_available(),
        "desktop Web assets are missing"
    );

    let home = crate::config::willdeep_home()?;
    let _desktop_presence =
        crate::daemon::desktop_presence::acquire(&home).context("track the open desktop client")?;
    let capabilities = crate::daemon::desktop_capabilities(&home, true).await?;
    ensure!(
        capabilities.protocol_version == willdeep_runtime_protocol::PROTOCOL_VERSION
            && capabilities.min_client_protocol_version
                == willdeep_runtime_protocol::MIN_CLIENT_PROTOCOL_VERSION,
        "the active Runtime protocol is incompatible with this desktop release"
    );
    let config_path = std::path::absolute(
        config_path
            .map(FilePath::to_path_buf)
            .unwrap_or(crate::config::default_config_path()?),
    )?;
    // A broken config must not stop the window from opening. The notification
    // text falls back to the default language; the settings page reports the
    // parse error itself.
    let language = crate::config::LoadedConfig::load(Some(&config_path))
        .ok()
        .map(|loaded| crate::i18n::Language::parse(loaded.file.agent.language.as_deref()))
        .transpose()?
        .unwrap_or_default();
    let config = WebConfig {
        listen: (Ipv4Addr::LOCALHOST, 0).into(),
        integration_default: config_path == crate::config::default_config_path()?,
        config_path,
        profile: None,
        workspaces: Vec::new(),
        home,
        language,
        open_setup: false,
        setup_started: None,
        intake: None,
    };

    // The window only learns about a finished turn through this. It dies with
    // the process, so there is nothing to cancel.
    super::desktop_notify::spawn(config.home.clone(), config.language);

    let mut input = tokio::io::stdin();
    let mut close_signal = [0];
    tokio::select! {
        result = super::serve_internal(config, None, true) => result,
        // Dropping the Web listener on app exit leaves the separate daemon running.
        result = input.read(&mut close_signal) => {
            result.context("watch desktop host lifetime")?;
            Ok(())
        }
    }
}

struct DesktopAccess {
    host: String,
    origin: String,
    bootstrap: String,
    claimed: AtomicBool,
    cookie_name: String,
    cookie_secret: String,
}

pub(super) fn protect(app: Router, address: SocketAddr) -> Result<Router> {
    let access = Arc::new(DesktopAccess {
        host: address.to_string(),
        origin: format!("http://{address}"),
        bootstrap: format!("{BOOTSTRAP_PREFIX}{}", Uuid::new_v4().simple()),
        claimed: AtomicBool::new(false),
        cookie_name: format!("willdeep_desktop_{}", Uuid::new_v4().simple()),
        cookie_secret: Uuid::new_v4().simple().to_string(),
    });
    // The parent reads this from the child's private stdout pipe. Keep the
    // launch URL out of process arguments, ordinary logs and error messages.
    let mut output = std::io::stdout().lock();
    writeln!(
        output,
        "{}",
        serde_json::json!({"willdeep_desktop_url": format!("{}{}", access.origin, access.bootstrap)})
    )?;
    output.flush()?;
    Ok(app.layer(middleware::from_fn_with_state(access, authorize)))
}

async fn authorize(
    State(access): State<Arc<DesktopAccess>>,
    request: Request,
    next: Next,
) -> Response {
    if !access.allowed_origin(request.headers()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let path = request.uri().path();
    let mut response = if path.starts_with(BOOTSTRAP_PREFIX) {
        access.claim(request.method(), path)
    } else if access.authenticated(request.headers()) {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    };
    let headers = response.headers_mut();
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

impl DesktopAccess {
    fn allowed_origin(&self, headers: &HeaderMap) -> bool {
        headers
            .get(header::HOST)
            .is_some_and(|host| host == self.host.as_str())
            && headers
                .get(header::ORIGIN)
                .is_none_or(|origin| origin == self.origin.as_str())
            && headers
                .get("sec-fetch-site")
                .is_none_or(|site| site != "cross-site")
    }

    fn claim(&self, method: &Method, path: &str) -> Response {
        if method != Method::GET
            || path != self.bootstrap
            || self.claimed.swap(true, Ordering::AcqRel)
        {
            return StatusCode::NOT_FOUND.into_response();
        }
        let cookie = format!(
            "{}={}; Path=/; HttpOnly; SameSite=Strict",
            self.cookie_name, self.cookie_secret
        );
        // A redirect from the native bootstrap can remain cross-site in WebKit,
        // withholding the Strict cookie. Commit a loopback document first.
        let mut response = Html(concat!(
            "<!doctype html><meta charset=\"utf-8\">",
            "<meta http-equiv=\"refresh\" content=\"0; url=/?desktop=1\">",
        ))
        .into_response();
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            response.headers_mut().insert(header::SET_COOKIE, value);
        }
        response
    }

    fn authenticated(&self, headers: &HeaderMap) -> bool {
        headers.get_all(header::COOKIE).iter().any(|header| {
            header.to_str().is_ok_and(|cookies| {
                cookies.split(';').any(|cookie| {
                    cookie.trim().split_once('=').is_some_and(|(name, value)| {
                        name == self.cookie_name && value == self.cookie_secret
                    })
                })
            })
        })
    }
}

pub(super) fn workspace_changes_routes() -> Router<Arc<WebState>> {
    Router::new()
        .route("/api/workspaces/changes", get(changes))
        .route("/api/workspaces/changes/file", get(change_file))
}

/// MCP 服务管理。单独成 router 是为了测试能只挂这一段，不必把整个桌面服务拉起来。
pub(super) fn mcp_routes() -> Router<Arc<WebState>> {
    Router::new()
        .route("/api/desktop/mcp", get(mcp_servers).post(mcp_add))
        .route("/api/desktop/mcp/{name}", axum::routing::delete(mcp_remove))
}

pub(super) fn routes(state: Arc<WebState>) -> Router {
    Router::new()
        .route("/api/desktop/runtime", get(runtime_status))
        .route("/api/desktop/cli", get(cli_install_plan))
        .route("/api/desktop/cli/install", post(cli_install))
        .merge(mcp_routes())
        .route("/api/desktop/selection", get(selection).put(save_selection))
        .route("/api/desktop/changes", get(changes))
        .route("/api/desktop/changes/file", get(change_file))
        .with_state(state)
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DesktopSelection {
    workspace: Option<String>,
    session: Option<Uuid>,
}

const SELECTION_FILE: &str = "desktop-selection.json";

async fn selection(State(state): State<Arc<WebState>>) -> Result<Json<DesktopSelection>, WebError> {
    match tokio::fs::read(state.home.join(SELECTION_FILE)).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Json)
            .map_err(|error| WebError::internal(format!("invalid desktop selection: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(Json(DesktopSelection::default()))
        }
        Err(error) => Err(WebError::internal(format!(
            "cannot read desktop selection: {error}"
        ))),
    }
}

async fn save_selection(
    State(state): State<Arc<WebState>>,
    Json(selection): Json<DesktopSelection>,
) -> Result<StatusCode, WebError> {
    let workspace = selection
        .workspace
        .as_deref()
        .ok_or_else(|| WebError::bad_request("workspace is required"))?;
    changes_workspace(&state, workspace, selection.session).await?;
    let path = state.home.join(SELECTION_FILE);
    let bytes =
        serde_json::to_vec(&selection).map_err(|error| WebError::internal(error.to_string()))?;
    let _guard = state.settings_write_lock.lock().await;
    tokio::task::spawn_blocking(move || crate::model_routing::write_private_atomic(&path, &bytes))
        .await
        .map_err(|error| WebError::internal(error.to_string()))?
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn runtime_status(
    State(state): State<Arc<WebState>>,
) -> Result<Json<serde_json::Value>, WebError> {
    let capabilities = crate::daemon::desktop_capabilities(&state.home, false)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(Json(serde_json::json!({
        "desktop_version": env!("CARGO_PKG_VERSION"),
        "runtime_version": capabilities.server_version,
        "protocol_version": capabilities.protocol_version,
    })))
}

/// 桌面壳里安装命令行的方案。命令在服务端拼好，页面只负责展示和选择。
///
/// macOS 有 Homebrew 就用它，没有才落到 npm 一类；Linux 只有 npm；Windows
/// 不代为执行，只把命令交回去让用户自己跑。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CliInstallPlan {
    os: &'static str,
    /// 页面可以代为执行的选项。Windows 上为空。
    managers: Vec<CliManager>,
    /// 只展示、不执行的命令。macOS 和 Linux 上为空。
    manual: Vec<ManualCommand>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CliManager {
    id: &'static str,
    command: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct ManualCommand {
    label: &'static str,
    command: String,
}

const BREW_FORMULA: &str = "willdeep-cli";
const NPM_PACKAGE: &str = "@niuwoai/willdeep";

fn npm_command(manager: &str) -> String {
    format!("{manager} install -g {NPM_PACKAGE}")
}

fn brew_command() -> String {
    format!("brew install {BREW_FORMULA}")
}

/// 这条 PATH 是桌面进程的，通常比登录 shell 瘦。把两个 Homebrew 前缀补上，
/// 否则 `brew` 明明装了也探测不到。
fn command_search_path() -> std::ffi::OsString {
    let mut parts = Vec::new();
    for prefix in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if FilePath::new(prefix).is_dir() {
            parts.push(prefix.to_owned());
        }
    }
    if let Some(existing) = std::env::var_os("PATH") {
        parts.push(existing.to_string_lossy().into_owned());
    }
    std::ffi::OsString::from(parts.join(":"))
}

fn command_exists(name: &str) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {name} >/dev/null 2>&1"))
            .env("PATH", command_search_path())
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(not(unix))]
    {
        let _ = name;
        false
    }
}

fn cli_install_plan_for(os: &str) -> CliInstallPlan {
    match os {
        "macos" => {
            let mut managers = Vec::new();
            if command_exists("brew") {
                managers.push(CliManager {
                    id: "brew",
                    command: brew_command(),
                });
            }
            for manager in ["npm", "cnpm", "pnpm"] {
                if command_exists(manager) {
                    managers.push(CliManager {
                        id: manager,
                        command: npm_command(manager),
                    });
                }
            }
            CliInstallPlan {
                os: "macos",
                managers,
                manual: Vec::new(),
            }
        }
        "linux" => CliInstallPlan {
            os: "linux",
            managers: vec![CliManager {
                id: "npm",
                command: npm_command("npm"),
            }],
            manual: Vec::new(),
        },
        _ => CliInstallPlan {
            os: "windows",
            managers: Vec::new(),
            manual: vec![ManualCommand {
                label: "npm",
                command: npm_command("npm"),
            }],
        },
    }
}

async fn cli_install_plan() -> Json<CliInstallPlan> {
    Json(cli_install_plan_for(std::env::consts::OS))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CliInstallRequest {
    manager: String,
}

#[derive(Serialize)]
struct CliInstallOutcome {
    command: String,
    exit_code: i32,
    output: String,
}

/// 代为执行页面选定的安装命令。命令行由方案表决定，请求里的 `manager` 只是
/// 在这张表里点名，拼不出表外的命令。Windows 没有可执行项，直接拒绝。
async fn cli_install(
    Json(request): Json<CliInstallRequest>,
) -> Result<Json<CliInstallOutcome>, WebError> {
    let plan = cli_install_plan_for(std::env::consts::OS);
    let Some(chosen) = plan
        .managers
        .iter()
        .find(|manager| manager.id == request.manager)
    else {
        return Err(WebError::bad_request(format!(
            "{} is not an install option on {}",
            request.manager, plan.os
        )));
    };
    let command = chosen.command.clone();
    let search_path = command_search_path();
    let output = tokio::task::spawn_blocking(move || run_install(&command, &search_path))
        .await
        .map_err(|error| WebError::internal(error.to_string()))?
        .map_err(WebError::from_anyhow)?;
    Ok(Json(output))
}

/// 配置页里的 MCP 服务名单。只在回环监听上开放：服务文件里可以有明文 token，
/// 对外暴露的实例连列都不列。
fn ensure_mcp_writable(state: &WebState) -> Result<(), WebError> {
    if state.settings_writable {
        Ok(())
    } else {
        Err(WebError::forbidden(
            "MCP servers can only be managed from a loopback Web server",
        ))
    }
}

/// 交回页面的一个服务。`token` 只说写了没有，值不离开这台机器。
#[derive(Serialize)]
struct McpServerView {
    name: String,
    transport: &'static str,
    command: Option<String>,
    args: Vec<String>,
    env: Vec<String>,
    url: Option<String>,
    headers: Vec<String>,
    auth: &'static str,
    token_set: bool,
    bearer_token_env: Option<String>,
    oauth_client_id: Option<String>,
    oauth_client_secret_env: Option<String>,
    oauth_scopes: Vec<String>,
    startup_timeout_seconds: u64,
    enabled: bool,
}

fn mcp_server_view(name: String, server: &willdeep_core::McpServerConfig) -> McpServerView {
    let transport = if server
        .url
        .as_deref()
        .is_some_and(|url| !url.trim().is_empty())
    {
        "http"
    } else {
        "stdio"
    };
    let oauth = server.oauth.clone().unwrap_or_default();
    let auth = if server
        .token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty())
    {
        "token"
    } else if server.bearer_token_env.is_some() {
        "bearer_token_env"
    } else if server.oauth.is_some() {
        "oauth"
    } else {
        "none"
    };
    McpServerView {
        name,
        transport,
        command: server.command.clone(),
        args: server.args.clone(),
        env: server.env.keys().cloned().collect(),
        url: server.url.clone(),
        headers: server.headers.keys().cloned().collect(),
        auth,
        token_set: auth == "token",
        bearer_token_env: server.bearer_token_env.clone(),
        oauth_client_id: oauth.client_id,
        oauth_client_secret_env: oauth.client_secret_env,
        oauth_scopes: oauth.scopes,
        startup_timeout_seconds: server.startup_timeout_seconds,
        enabled: server.enabled,
    }
}

#[derive(Serialize)]
struct McpServerList {
    servers: Vec<McpServerView>,
    skipped: Vec<String>,
}

async fn mcp_servers(State(state): State<Arc<WebState>>) -> Result<Json<McpServerList>, WebError> {
    ensure_mcp_writable(&state)?;
    let config_path = state.config_path.clone();
    let (servers, skipped) =
        tokio::task::spawn_blocking(move || crate::config::list_mcp_servers(Some(&config_path)))
            .await
            .map_err(|error| WebError::internal(error.to_string()))?
            .map_err(WebError::from_anyhow)?;
    Ok(Json(McpServerList {
        servers: servers
            .into_iter()
            .map(|(name, server)| mcp_server_view(name, &server))
            .collect(),
        skipped,
    }))
}

/// 页面提交的一个新服务。传输二选一，认证三选一，都交给 `McpServerConfig::validate`
/// 判，这里不另起一套规则。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpAddRequest {
    name: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    bearer_token_env: Option<String>,
    #[serde(default)]
    oauth_client_id: Option<String>,
    #[serde(default)]
    oauth_client_secret_env: Option<String>,
    #[serde(default)]
    oauth_scopes: Vec<String>,
    #[serde(default = "default_mcp_timeout")]
    startup_timeout_seconds: u64,
    #[serde(default = "default_mcp_enabled")]
    enabled: bool,
}

fn default_mcp_timeout() -> u64 {
    30
}

fn default_mcp_enabled() -> bool {
    true
}

async fn mcp_add(
    State(state): State<Arc<WebState>>,
    Json(request): Json<McpAddRequest>,
) -> Result<Json<McpServerView>, WebError> {
    ensure_mcp_writable(&state)?;
    if !crate::config::mcp_name_is_valid(&request.name) {
        return Err(WebError::bad_request(
            "MCP server name must be 1 to 64 characters of ASCII letters, numbers, '_' or '-'",
        ));
    }
    let oauth = if request.oauth_client_id.is_some()
        || request.oauth_client_secret_env.is_some()
        || !request.oauth_scopes.is_empty()
    {
        Some(willdeep_core::mcp::McpOAuthConfig {
            client_id: request.oauth_client_id.clone(),
            client_secret_env: request.oauth_client_secret_env.clone(),
            scopes: request.oauth_scopes.clone(),
        })
    } else {
        None
    };
    let server = willdeep_core::McpServerConfig {
        command: request.command.clone(),
        args: request.args.clone(),
        env: request.env.clone(),
        url: request.url.clone(),
        headers: request.headers.clone(),
        token: request.token.clone(),
        bearer_token_env: request.bearer_token_env.clone(),
        oauth,
        startup_timeout_seconds: request.startup_timeout_seconds,
        enabled: request.enabled,
    };
    let config_path = state.config_path.clone();
    let name = request.name.clone();
    let written = server.clone();
    tokio::task::spawn_blocking(move || {
        crate::config::add_mcp_server(Some(&config_path), &name, &written)
    })
    .await
    .map_err(|error| WebError::internal(error.to_string()))?
    .map_err(|error| {
        let text = error.to_string();
        if text.contains("already exists") {
            WebError::conflict(text)
        } else {
            WebError::bad_request(text)
        }
    })?;
    Ok(Json(mcp_server_view(request.name, &server)))
}

async fn mcp_remove(
    State(state): State<Arc<WebState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<StatusCode, WebError> {
    ensure_mcp_writable(&state)?;
    if !crate::config::mcp_name_is_valid(&name) {
        return Err(WebError::bad_request(
            "MCP server name must be 1 to 64 characters of ASCII letters, numbers, '_' or '-'",
        ));
    }
    let config_path = state.config_path.clone();
    let target = name.clone();
    let removed = tokio::task::spawn_blocking(move || {
        crate::config::remove_mcp_server(Some(&config_path), &target)
    })
    .await
    .map_err(|error| WebError::internal(error.to_string()))?
    .map_err(WebError::from_anyhow)?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(WebError::not_found(format!(
            "MCP server {name} was not found"
        )))
    }
}

fn run_install(command: &str, search_path: &std::ffi::OsStr) -> Result<CliInstallOutcome> {
    let result = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("PATH", search_path)
        .output()
        .with_context(|| format!("run install command: {command}"))?;
    let mut text = String::from_utf8_lossy(&result.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&result.stderr));
    const LIMIT: usize = 16 * 1024;
    if text.len() > LIMIT {
        text.truncate(LIMIT);
        text.push_str("\n…");
    }
    Ok(CliInstallOutcome {
        command: command.to_owned(),
        exit_code: result.status.code().unwrap_or(-1),
        output: text,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangesQuery {
    workspace: String,
    session: Option<Uuid>,
}

#[derive(Serialize)]
struct WorkspaceChanges {
    git: bool,
    branch: Option<String>,
    snapshot: Option<DiffSnapshot>,
    session_paths: Option<BTreeSet<String>>,
}

async fn changes(
    State(state): State<Arc<WebState>>,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<WorkspaceChanges>, WebError> {
    let workspace = changes_workspace(&state, &query.workspace, query.session).await?;
    let Some(branch) = git_worktree_branch(&workspace).await? else {
        return Ok(Json(WorkspaceChanges {
            git: false,
            branch: None,
            snapshot: None,
            session_paths: None,
        }));
    };
    let snapshot = diff_review::remote_snapshot(&state.home, &workspace)
        .await
        .map_err(WebError::from_anyhow)?;
    let session_paths = if let Some(session) = query.session {
        let path = diff_review::attribution_store_path(&state.home);
        let workspace = snapshot.workspace.clone();
        let snapshot_id = snapshot.id.clone();
        match tokio::task::spawn_blocking(move || {
            diff_review::load_attributions(&path)
                .map(|records| attributed_paths(&records, &workspace, &snapshot_id, session))
        })
        .await
        {
            Ok(Ok(mut paths)) => {
                let visible = snapshot
                    .files
                    .iter()
                    .map(|file| &file.path)
                    .collect::<BTreeSet<_>>();
                paths.retain(|path| visible.contains(path));
                Some(paths)
            }
            Ok(Err(_)) | Err(_) => {
                eprintln!(r#"{{"level":"warn","event":"workspace_attribution_read_failed"}}"#);
                None
            }
        }
    } else {
        None
    };
    Ok(Json(WorkspaceChanges {
        git: true,
        branch,
        snapshot: Some(snapshot),
        session_paths,
    }))
}

async fn is_git_worktree_root(workspace: &FilePath) -> Result<bool, WebError> {
    let output = git_worktree_command(
        workspace,
        &["rev-parse", "--show-prefix", "--is-inside-work-tree"],
    )
    .await?;
    // Git snapshots use repository-relative paths. An allowed subdirectory must not
    // implicitly authorize the rest of the repository.
    Ok(output.status.success() && output.stdout == b"\ntrue\n")
}

async fn git_worktree_branch(workspace: &FilePath) -> Result<Option<Option<String>>, WebError> {
    if !is_git_worktree_root(workspace).await? {
        return Ok(None);
    }
    let result = git_worktree_command(workspace, &["branch", "--show-current"]).await?;
    Ok(result.status.success().then(|| {
        let branch = String::from_utf8_lossy(&result.stdout).trim().to_owned();
        (!branch.is_empty()).then_some(branch)
    }))
}

async fn git_worktree_command(
    workspace: &FilePath,
    args: &[&str],
) -> Result<std::process::Output, WebError> {
    tokio::process::Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .await
        .map_err(|error| WebError::internal(format!("cannot inspect Git worktree: {error}")))
}

fn attributed_paths(
    records: &[DiffAttributionRecord],
    workspace: &FilePath,
    snapshot_id: &str,
    session: Uuid,
) -> BTreeSet<String> {
    let mut cursor = snapshot_id;
    let mut current = BTreeSet::new();
    let mut other = BTreeSet::new();
    // Follow only the continuous suffix in append order. An external edit or an
    // overlapping tool window can break the chain. Never jump across that gap.
    for record in records
        .iter()
        .rev()
        .filter(|record| record.workspace == workspace)
    {
        if record.after_snapshot_id != cursor {
            break;
        }
        cursor = &record.before_snapshot_id;
        for path in &record.paths {
            if record.session_id == Some(session) && record.confirmed_paths.contains(path) {
                current.insert(path.clone());
            } else {
                other.insert(path);
            }
        }
    }
    current.retain(|path| !other.contains(path));
    current
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeFileQuery {
    workspace: String,
    session: Option<Uuid>,
    snapshot_id: String,
    path: String,
    #[serde(default)]
    area: DiffArea,
}

async fn change_file(
    State(state): State<Arc<WebState>>,
    Query(query): Query<ChangeFileQuery>,
) -> Result<Json<serde_json::Value>, WebError> {
    let workspace = changes_workspace(&state, &query.workspace, query.session).await?;
    if !is_git_worktree_root(&workspace).await? {
        return Err(WebError::not_found("workspace is not a Git worktree root"));
    }
    let content = diff_review::remote_content(
        &state.home,
        &workspace,
        &query.snapshot_id,
        &query.path,
        query.area,
    )
    .await
    .map_err(|error| {
        use willdeep_runtime_protocol::{ApiError, ErrorCode};
        match error.downcast_ref::<ApiError>().map(|error| error.code) {
            Some(ErrorCode::StaleSnapshot) => WebError::conflict("refresh the workspace snapshot"),
            Some(ErrorCode::NotFound) => {
                WebError::not_found("file is not in the workspace snapshot")
            }
            Some(ErrorCode::Forbidden) => WebError::forbidden("workspace is unavailable"),
            _ => WebError::from_anyhow(error),
        }
    })?;
    Ok(Json(serde_json::json!({ "content": content })))
}

async fn changes_workspace(
    state: &WebState,
    workspace: &str,
    session: Option<Uuid>,
) -> Result<PathBuf, WebError> {
    let root = FilePath::new(workspace);
    if let Some(id) = session {
        let conversation = willdeep_core::SessionStore::new(&state.home)
            .load(id)
            .map_err(|_| WebError::not_found("session was not found"))?;
        if !super::session_workspace_allowed(state, &conversation).await? {
            return Err(WebError::forbidden("session workspace is unavailable"));
        }
        if conversation.workspace != root {
            let parent = conversation
                .parent_session_id
                .and_then(|id| willdeep_core::SessionStore::new(&state.home).load(id).ok());
            if parent.is_none_or(|parent| parent.workspace != root) {
                return Err(WebError::forbidden("session workspace is unavailable"));
            }
        }
        return Ok(conversation.workspace);
    } else if !super::workspace_allowed(state, root).await? {
        return Err(WebError::forbidden("workspace is unavailable"));
    }
    Ok(root.to_path_buf())
}

#[cfg(test)]
fn mcp_state(home: &FilePath, writable: bool) -> WebState {
    WebState {
        config_path: home.join("config.toml"),
        integration_default: false,
        profile: None,
        workspaces: Arc::new(std::sync::RwLock::new(Vec::new())),
        home: home.to_path_buf(),
        language: crate::i18n::Language::default(),
        harness_slots: Arc::new(tokio::sync::Semaphore::new(1)),
        settings_writable: writable,
        settings_write_lock: tokio::sync::Mutex::new(()),
        config_tools: tokio::sync::OnceCell::new(),
        session_workspaces: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        intake: None,
    }
}

#[cfg(test)]
impl CliInstallPlan {
    fn manager_ids(&self) -> Vec<&str> {
        self.managers.iter().map(|manager| manager.id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    fn scratch() -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!("desktop-mcp-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.toml"), "").unwrap();
        home
    }

    struct McpHarness {
        origin: String,
        client: reqwest::Client,
    }

    impl McpHarness {
        async fn start(state: WebState) -> Self {
            let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let app = mcp_routes().with_state(Arc::new(state));
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            Self {
                origin,
                client: reqwest::Client::builder().no_proxy().build().unwrap(),
            }
        }

        async fn call(&self, method: &str, path: &str, body: Option<&str>) -> (StatusCode, String) {
            let request = self
                .client
                .request(method.parse().unwrap(), format!("{}{path}", self.origin));
            let request = if let Some(body) = body {
                request
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(body.to_owned())
            } else {
                request
            };
            let response = request.send().await.unwrap();
            let status = response.status();
            (status, response.text().await.unwrap())
        }
    }

    #[tokio::test]
    async fn mcp_management_lists_adds_and_removes_without_echoing_the_token() {
        let home = scratch();
        let harness = McpHarness::start(mcp_state(&home, true)).await;

        let (status, body) = harness.call("GET", "/api/desktop/mcp", None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("\"servers\":[]"), "{body}");

        let (status, body) = harness.call(
            "POST",
            "/api/desktop/mcp",
            Some(
                r#"{"name":"docs","url":"https://docs.example.com/mcp","token":"super-secret-value"}"#,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"auth\":\"token\""), "{body}");
        assert!(body.contains("\"token_set\":true"), "{body}");
        assert!(
            !body.contains("super-secret-value"),
            "response echoed the token: {body}"
        );
        let stored = std::fs::read_to_string(home.join(".mcp/docs.toml")).unwrap();
        assert!(stored.contains("super-secret-value"), "{stored}");

        let (status, body) = harness
            .call(
                "POST",
                "/api/desktop/mcp",
                Some(r#"{"name":"docs","command":"echo"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");

        let (status, body) = harness
            .call(
                "POST",
                "/api/desktop/mcp",
                Some(r#"{"name":"../escape","command":"echo"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

        let remote = McpHarness::start(mcp_state(&home, false)).await;
        let (status, body) = remote
            .call(
                "POST",
                "/api/desktop/mcp",
                Some(r#"{"name":"remote","command":"echo"}"#),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(!home.join(".mcp/remote.toml").exists());

        let (status, _) = harness.call("DELETE", "/api/desktop/mcp/docs", None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(!home.join(".mcp/docs.toml").exists());
        let (status, _) = harness.call("DELETE", "/api/desktop/mcp/docs", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn workspace_attribution_uses_only_the_continuous_same_workspace_suffix() {
        use diff_review::AttributionConfidence;
        let workspace = FilePath::new("/workspace");
        let selected = Uuid::new_v4();
        let other = Uuid::new_v4();
        let record = |root: &FilePath,
                      session,
                      before: &str,
                      after: &str,
                      paths: &[&str],
                      confirmed: &[&str]| {
            DiffAttributionRecord {
                id: Uuid::new_v4(),
                workspace: root.to_path_buf(),
                session_id: Some(session),
                turn_id: None,
                task_id: Uuid::new_v4(),
                agent_id: Uuid::new_v4(),
                before_snapshot_id: before.to_owned(),
                after_snapshot_id: after.to_owned(),
                tool: "write_file".to_owned(),
                paths: paths.iter().map(|path| path.to_string()).collect(),
                confirmed_paths: confirmed.iter().map(|path| path.to_string()).collect(),
                path_states: Vec::new(),
                confidence: AttributionConfidence::ToolWindow,
                created_at: 0,
            }
        };
        let records = [
            record(workspace, selected, "before", "s1", &["too-old.txt"], &[]),
            record(
                workspace,
                selected,
                "s2",
                "s3",
                &["ours.txt", "shared.txt", "then-unknown.txt", "external.txt"],
                &["ours.txt", "shared.txt", "then-unknown.txt"],
            ),
            record(
                workspace,
                other,
                "s3",
                "s4",
                &["theirs.txt", "shared.txt"],
                &["theirs.txt", "shared.txt"],
            ),
            record(
                workspace,
                selected,
                "s4",
                "s5",
                &["then-unknown.txt", "legacy.txt"],
                &[],
            ),
            record(
                FilePath::new("/different"),
                selected,
                "s4",
                "s5",
                &["different.txt"],
                &["different.txt"],
            ),
        ];
        assert_eq!(
            attributed_paths(&records, workspace, "s5", selected),
            BTreeSet::from(["ours.txt".to_owned()])
        );
        assert_eq!(
            attributed_paths(&records, workspace, "s5", other),
            BTreeSet::from(["theirs.txt".to_owned()])
        );
        assert!(attributed_paths(&records, workspace, "edited-outside-tools", selected).is_empty());
        assert!(attributed_paths(&records, workspace, "s1", selected).is_empty());
    }

    #[tokio::test]
    async fn opaque_plugin_frame_loads_only_its_granted_static_assets() {
        use crate::plugin_web::{self, PluginWebState};
        use willdeep_core::plugin::PluginHost;

        let home = std::env::temp_dir().join(format!("desktop-plugin-test-{}", Uuid::new_v4()));
        let root = home.join("plugins/demo/1.0.0");
        for (path, body) in [
            (
                ".codex-plugin/plugin.json",
                r#"{"name":"demo","version":"1.0.0"}"#,
            ),
            (
                ".willdeep-plugin/plugin.json",
                r#"{"schemaVersion":1,"contributes":{"pages":[{"id":"main","runtime":"localWeb","entryPath":"./ui/index.html"}],"destinations":[{"id":"main","titleKey":"main","mainPage":"main","icon":"./art/nav.svg"}]}}"#,
            ),
            (".willdeep-plugin/locales/en.json", r#"{"main":"Main"}"#),
            (
                "art/nav.svg",
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16"/></svg>"#,
            ),
            (
                "ui/index.html",
                r#"<html><head><script type="module" crossorigin src="./assets/main.js"></script><link rel="stylesheet" crossorigin href="./assets/main.css"></head></html>"#,
            ),
            ("ui/assets/main.js", r#"import "./chunk.js";"#),
            ("ui/assets/chunk.js", "export const ready = true;"),
            ("ui/assets/main.css", "body { color: red; }"),
            ("ui/other.html", "undeclared page"),
            ("ui/data.json", "{}"),
            ("outside.js", "outside the entry directory"),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        crate::video_studio_plugin::ensure_installed(&home)
            .await
            .unwrap();
        crate::builtin_plugins::ensure_token_stats_installed(&home).unwrap();
        let host = Arc::new(PluginHost::discover_live(&home).unwrap());
        let other_host = PluginHost::discover(&home).unwrap();
        let studio = crate::video_studio_plugin::ID;
        let stats = "willdeep-token-stats";
        for id in [studio, stats, "demo"] {
            other_host.approve(id, 1).await.unwrap();
            other_host.set_enabled(id, true).await.unwrap().unwrap();
        }
        let state = Arc::new(
            PluginWebState::new(
                host,
                home.join("config.toml"),
                home,
                Arc::new(std::sync::RwLock::new(Vec::new())),
            )
            .with_desktop_assets(),
        );
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let origin = format!("http://{address}");
        let access = Arc::new(DesktopAccess {
            host: address.to_string(),
            origin: origin.clone(),
            bootstrap: format!("{BOOTSTRAP_PREFIX}test"),
            claimed: AtomicBool::new(true),
            cookie_name: "desktop_test".into(),
            cookie_secret: "fixture".into(),
        });
        let cookie = format!("{}={}", access.cookie_name, access.cookie_secret);
        let router = plugin_web::router(state.clone())
            .layer(middleware::from_fn_with_state(access, authorize))
            .merge(plugin_web::desktop_assets::router(state, address));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let entry = format!("{origin}/plugin-host/demo/ui/index.html");
        let cookie = cookie.as_str();
        assert_eq!(
            client.get(&entry).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let icon = format!("{origin}/plugin-host/demo/art/nav.svg");
        assert_eq!(
            client.get(&icon).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let response = client
            .get(&icon)
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert_eq!(
            response.headers()["cross-origin-resource-policy"],
            "same-origin"
        );
        assert!(response.text().await.unwrap().starts_with("<svg "));
        let plugins: serde_json::Value = client
            .get(format!("{origin}/api/plugins"))
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let demo = plugins["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|plugin| plugin["id"] == "demo")
            .unwrap();
        assert_eq!(
            demo["destinations"][0]["page_url"],
            "/plugin-host/demo/ui/index.html"
        );
        let page = client
            .get(&entry)
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let csp = page.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(csp.contains("sandbox allow-scripts;"));
        assert!(csp.contains("connect-src 'none'"));
        let html = page.text().await.unwrap();
        let base = html
            .split_once("<base href=\"")
            .unwrap()
            .1
            .split_once('"')
            .unwrap()
            .0;
        assert!(csp.contains(&format!("base-uri {origin}{base}")));
        assert!(csp.contains(&format!("{origin}/plugin-media/")));
        assert_eq!(
            client
                .get(format!("{origin}/plugin-media/demo/fixture.png"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED,
        );
        let asset_base = reqwest::Url::parse(&entry).unwrap().join(base).unwrap();
        let asset = |path: &str| asset_base.join(path).unwrap();
        let script = asset("./assets/main.js");
        let nested_chunk = script.join("./chunk.js").unwrap();
        for (url, mime) in [
            (script.clone(), "text/javascript; charset=utf-8"),
            (nested_chunk, "text/javascript; charset=utf-8"),
            (asset("./assets/main.css"), "text/css; charset=utf-8"),
        ] {
            let response = client
                .get(url.clone())
                .header(header::ORIGIN, "null")
                .header("sec-fetch-site", "cross-site")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{}", url.path());
            assert_eq!(response.headers()[header::CONTENT_TYPE], mime);
            assert_eq!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                "null"
            );
            assert!(
                !response
                    .headers()
                    .contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            );
        }
        let response = client.head(asset("assets/main.js")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.bytes().await.unwrap().is_empty());

        for path in ["index.html", "data.json", "%2e%2e%2foutside.js"] {
            let response = client
                .get(asset(path))
                .header(header::ORIGIN, "null")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        let script = script.as_str();
        let other_plugin = script.replace("/demo/", "/other/");
        assert_eq!(
            client.get(other_plugin).send().await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            client
                .get(script)
                .header(header::ORIGIN, "https://example.invalid")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client
                .get(script)
                .header(header::HOST, "example.invalid")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client.post(script).send().await.unwrap().status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        assert_eq!(
            client
                .get(format!("{origin}/api/plugins"))
                .header(header::ORIGIN, "null")
                .header(header::COOKIE, cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client
                .get(format!("{origin}/plugin-host/demo/ui/other.html"))
                .header(header::COOKIE, cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );

        let edited = root.join("ui/assets/main.css");
        let original = std::fs::read(&edited).unwrap();
        std::fs::write(&edited, "body { color: tan; }").unwrap();
        assert_eq!(
            client
                .get(asset("assets/main.css"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND,
        );
        assert_eq!(
            client
                .get(&entry)
                .header(header::COOKIE, cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT,
        );
        std::fs::write(&edited, original).unwrap();
        other_host
            .set_enabled("demo", false)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            client.get(script).send().await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        other_host.set_enabled("demo", true).await.unwrap().unwrap();
        assert_eq!(
            client.get(script).send().await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
        let page = client
            .get(&entry)
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        assert!(!page.text().await.unwrap().contains(base));

        let stats_page = client
            .get(format!("{origin}/plugin-host/{stats}/ui/index.html"))
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(stats_page.status(), StatusCode::OK);
        assert!(
            stats_page.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("sandbox allow-scripts allow-downloads;")
        );

        let studio_entry = format!("{origin}/plugin-host/{studio}/ui/dist/index.html");
        let studio_page = client
            .get(&studio_entry)
            .header(header::COOKIE, cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(studio_page.status(), StatusCode::OK);
        assert!(
            studio_page.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("sandbox allow-scripts allow-downloads")
        );
        let studio_html = studio_page.text().await.unwrap();
        let studio_base = studio_html
            .split_once("<base href=\"")
            .unwrap()
            .1
            .split_once('"')
            .unwrap()
            .0;
        let studio_base = reqwest::Url::parse(&studio_entry)
            .unwrap()
            .join(studio_base)
            .unwrap();
        for (attribute, mime) in [
            ("src", "text/javascript; charset=utf-8"),
            ("href", "text/css; charset=utf-8"),
        ] {
            let marker = format!("crossorigin {attribute}=\"");
            let relative = studio_html
                .split_once(&marker)
                .unwrap()
                .1
                .split_once('"')
                .unwrap()
                .0;
            let asset = client
                .get(studio_base.join(relative).unwrap())
                .header(header::ORIGIN, "null")
                .send()
                .await
                .unwrap();
            assert_eq!(asset.status(), StatusCode::OK);
            assert_eq!(asset.headers()[header::CONTENT_TYPE], mime);
            assert_eq!(asset.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "null");
            assert!(!asset.bytes().await.unwrap().is_empty());
        }
        server.abort();
    }

    #[tokio::test]
    async fn bootstrap_commits_a_document_before_navigating_with_the_strict_cookie() {
        let access = DesktopAccess {
            host: "127.0.0.1:12345".into(),
            origin: "http://127.0.0.1:12345".into(),
            bootstrap: format!("{BOOTSTRAP_PREFIX}test"),
            claimed: AtomicBool::new(false),
            cookie_name: "desktop_test".into(),
            cookie_secret: "fixture".into(),
        };

        let response = access.claim(&Method::GET, &access.bootstrap);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert!(!response.headers().contains_key(header::LOCATION));
        assert_eq!(
            response.headers()[header::SET_COOKIE],
            "desktop_test=fixture; Path=/; HttpOnly; SameSite=Strict"
        );
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert!(
            std::str::from_utf8(&body)
                .unwrap()
                .contains("url=/?desktop=1")
        );
        assert_eq!(
            access.claim(&Method::GET, &access.bootstrap).status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn linux_installs_only_through_npm_and_windows_never_runs_anything() {
        let linux = super::cli_install_plan_for("linux");
        assert_eq!(linux.manager_ids(), ["npm"]);
        assert_eq!(
            linux.managers[0].command,
            "npm install -g @niuwoai/willdeep"
        );
        assert!(linux.manual.is_empty());

        let windows = super::cli_install_plan_for("windows");
        assert!(
            windows.managers.is_empty(),
            "windows must not run an install"
        );
        assert_eq!(windows.manual.len(), 1);
        assert_eq!(
            windows.manual[0].command,
            "npm install -g @niuwoai/willdeep"
        );
    }

    #[test]
    fn macos_offers_brew_only_when_it_is_installed() {
        let plan = super::cli_install_plan_for("macos");
        let brew_offered = plan.manager_ids().contains(&"brew");
        assert_eq!(brew_offered, super::command_exists("brew"));
        if brew_offered {
            assert_eq!(plan.managers[0].id, "brew");
            assert_eq!(plan.managers[0].command, "brew install willdeep-cli");
        }
        for manager in plan.managers.iter().filter(|manager| manager.id != "brew") {
            assert!(
                manager
                    .command
                    .starts_with(&format!("{} install -g ", manager.id)),
                "{}",
                manager.command
            );
        }
    }
}
