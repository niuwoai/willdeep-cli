use std::collections::HashMap;
use std::convert::Infallible;
use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::i18n::Language;
use anyhow::{Context, Result};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router, middleware};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Semaphore, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use willdeep_core::{MessageAttachment, Role, Session, SessionStore};

mod agent_library;
mod composer;
mod configuration;
mod mcp_tokens;
mod peer_sessions;
mod runtime_chat;
mod schedules;
mod setup;
mod turn_queue;

const MAX_PROMPT_CHARS: usize = 100_000;
const MAX_AGENT_LABEL_CHARS: usize = 128;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAssets;

pub(crate) fn embedded_assets_available() -> bool {
    WebAssets::get("index.html").is_some()
}

pub struct WebConfig {
    pub listen: SocketAddr,
    pub config_path: PathBuf,
    pub profile: Option<String>,
    pub workspaces: Vec<PathBuf>,
    pub home: PathBuf,
    pub language: Language,
    pub open_setup: bool,
    pub setup_started: Option<tokio::sync::oneshot::Sender<SocketAddr>>,
}

pub(crate) struct WebState {
    pub(crate) config_path: PathBuf,
    profile: Option<String>,
    /// 浏览器端能看到的工作区白名单。Web 模式没有应用层鉴权，所以这份名单
    /// 就是边界本身——不在名单里的目录，前端连列都列不出来。回环监听时允许
    /// 从界面往里加（与模型路由设置同一条既有语义），因此需要可变。
    /// 用 `Arc` 是因为插件宿主要读同一份名单：`window.willdeep.fs.*` 与
    /// `process.run` 的边界必须与聊天看到的边界**是同一个对象**，不能是
    /// 启动那一刻的副本——否则从界面新加的工作区，插件侧永远看不见，
    /// 而删掉的那个插件侧永远还在。
    pub(crate) workspaces: Arc<std::sync::RwLock<Vec<PathBuf>>>,
    pub(crate) home: PathBuf,
    language: Language,
    harness_slots: Arc<Semaphore>,
    settings_writable: bool,
    settings_write_lock: Mutex<()>,
    config_tools: tokio::sync::OnceCell<Arc<willdeep_core::McpRegistry>>,
    /// 会话归属校验的缓存：会话 id →（会话文件指纹, 工作区）。
    ///
    /// `/api/runtime/events` 这类轮询每两秒打一次，而校验本来要整读会话文件
    /// （长会话有几 MB）。归属只看工作区，文件没变就不必重读。包在 `Arc` 里是
    /// 因为未命中时的整读要挪到 blocking 线程上，缓存得跟着走。
    session_workspaces: Arc<std::sync::Mutex<HashMap<uuid::Uuid, (FileStamp, PathBuf)>>>,
}

/// 会话文件的指纹：修改时间（纳秒）+ 字节数。任一变化都重新读。
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified_nanos: u64,
    len: u64,
}

impl FileStamp {
    fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            modified_nanos: metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|value| value.as_nanos() as u64)
                .unwrap_or_default(),
            len: metadata.len(),
        }
    }
}

impl WebState {
    /// MCP 端点要在 daemon 模块里读家目录，字段本身仍是私有的。
    pub(crate) fn home(&self) -> &std::path::Path {
        &self.home
    }
}

#[derive(Clone, Deserialize)]
struct ChatRequest {
    #[serde(default)]
    request_id: Option<uuid::Uuid>,
    prompt: String,
    session_id: Option<String>,
    workspace: Option<String>,
    language: Option<String>,
    #[serde(default)]
    attachments: Vec<MessageAttachment>,
    /// 先入队、以后再发。只有队列端点认它；聊天流忽略它，避免一条普通消息被暂缓。
    #[serde(default)]
    defer_until_released: bool,
}

#[derive(Serialize)]
struct SessionSummary {
    id: String,
    title: String,
    /// 标题还是占位符时，列表拿它来区分会话。标题正常时不下发，
    /// 免得给前端两个都能显示的字段、让它自己去猜该用哪个。
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<String>,
    workspace: String,
    /// 创建时间。谱系行按它算「多久前分凉出来的」，不用 `updated_at`。
    created_at: u64,
    updated_at: u64,
    pinned_at: Option<u64>,
    /// 直接分凉来源的会话 id。没有来源时不下发，前端按普通会话画。
    #[serde(skip_serializing_if = "Option::is_none")]
    started_by: Option<String>,
    archived: bool,
    active: bool,
    active_turn_id: Option<uuid::Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResumeStreamQuery {
    #[serde(default)]
    after: Option<u64>,
    language: Option<String>,
}

#[derive(Deserialize)]
struct RenameSessionRequest {
    title: String,
}

#[derive(Default, Deserialize)]
struct ForkSessionRequest {
    title: Option<String>,
    #[serde(default)]
    through_turn_id: Option<uuid::Uuid>,
    #[serde(default)]
    provider_profile: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
struct DeleteSessionRequest {
    confirmation: uuid::Uuid,
}

#[derive(Serialize)]
struct ForkSessionResponse {
    id: String,
}

#[derive(Serialize)]
struct SessionDetail {
    id: String,
    workspace: String,
    messages: Vec<willdeep_core::conversation::ConversationItem>,
    /// `/short` 与 `/verbose` 的会话级开关。刷新页面后前端靠它恢复，不靠自己的内存。
    #[serde(skip_serializing_if = "Option::is_none")]
    response_style: Option<willdeep_core::ResponseStyle>,
}

#[derive(Serialize)]
struct WorkspaceSummary {
    id: String,
    path: String,
    name: String,
    active: bool,
    access: &'static str,
}

#[derive(Deserialize)]
struct RuntimeActivityQuery {
    workspace: String,
    session: Option<uuid::Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeEventsQuery {
    /// 只看这个会话的事件。Web 一次只展示当前会话——跨会话列表在浏览器上没有
    /// 归属可校验，那正是白名单要挡住的东西。
    session: uuid::Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IgnoreRuntimeEventBody {
    session: uuid::Uuid,
}

/// 浏览器能看到的事件字段。
///
/// **正文不在里面**，标题也只是打码截断过的摘要。这是独立于
/// `PublicKernelEvent` 的第二层白名单：协议那边加字段不会自动流到浏览器。
#[derive(Serialize)]
struct WebKernelEvent {
    id: uuid::Uuid,
    source: String,
    kind: String,
    priority: String,
    title_excerpt: Option<String>,
    requires_user_action: bool,
    merge_count: u32,
    created_at: String,
    delivery_state: String,
}

/// 文件指纹没变时直接返回缓存的工作区。
fn cached_session_workspace(
    cache: &std::sync::Mutex<HashMap<uuid::Uuid, (FileStamp, PathBuf)>>,
    session_id: uuid::Uuid,
    stamp: FileStamp,
) -> Option<PathBuf> {
    cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&session_id)
        .filter(|(cached, _)| *cached == stamp)
        .map(|(_, workspace)| workspace.clone())
}

/// 指纹对不上（或还没有）时读会话、记下新的归属。`load` 失败返回 `None`，
/// 不写缓存——下一次请求再试，而不是把一次瞬时的读失败记成永久拒绝。
fn remember_session_workspace(
    cache: &std::sync::Mutex<HashMap<uuid::Uuid, (FileStamp, PathBuf)>>,
    session_id: uuid::Uuid,
    stamp: FileStamp,
    load: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    let workspace = load()?;
    cache
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(session_id, (stamp, workspace.clone()));
    Some(workspace)
}

fn web_kernel_event(event: &willdeep_runtime_protocol::KernelEvent) -> WebKernelEvent {
    let public = event.to_public(willdeep_core::judge::redact_credentials);
    WebKernelEvent {
        id: public.id,
        source: format!("{:?}", public.source).to_lowercase(),
        kind: public.kind,
        priority: format!("{:?}", public.priority).to_lowercase(),
        title_excerpt: public.title_excerpt,
        requires_user_action: public.requires_user_action,
        merge_count: public.merge_count,
        created_at: public.created_at,
        delivery_state: format!("{:?}", public.delivery_state).to_lowercase(),
    }
}

/// 会话必须属于白名单里的工作区，否则一律拒绝。
///
/// 与会话详情同一条门：浏览器端没有应用层鉴权，能看到什么完全由这份白名单
/// 决定，事件不该是例外。
///
/// 这条路径被前端每两秒轮询一次。归属只取决于会话的工作区，而工作区写进
/// 会话文件后基本不变，所以按文件指纹缓存：文件没变就不再整读会话。
async fn authorized_event_session(
    state: &Arc<WebState>,
    session_id: uuid::Uuid,
) -> Result<(), WebError> {
    let not_found = || WebError::bad_request("session was not found");
    let path = SessionStore::new(&state.home).path(session_id);
    let metadata = std::fs::metadata(&path).map_err(|_| not_found())?;
    let stamp = FileStamp::of(&metadata);
    let cached = cached_session_workspace(&state.session_workspaces, session_id, stamp);
    let workspace = match cached {
        Some(workspace) => workspace,
        None => {
            // 整读会话是贵的（长会话几 MB），别堵在 tokio worker 上。
            let home = state.home.clone();
            let cache = Arc::clone(&state.session_workspaces);
            tokio::task::spawn_blocking(move || {
                remember_session_workspace(&cache, session_id, stamp, || {
                    SessionStore::new(&home)
                        .load(session_id)
                        .map(|session| session.workspace)
                        .ok()
                })
            })
            .await
            .map_err(|error| WebError::internal(error.to_string()))?
            .ok_or_else(not_found)?
        }
    };
    if !workspace_allowed(state, &workspace).await? {
        return Err(WebError::bad_request(
            "session workspace is not in the server allowlist",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebWorkspaceAction {
    workspace: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebAgentRetryAction {
    workspace: String,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebAgentPromptAction {
    workspace: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebAgentSpawnAction {
    workspace: String,
    session_id: uuid::Uuid,
    profile: String,
    prompt: String,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WebApprovalDecision {
    AllowOnce,
    Deny,
    AlwaysAllow,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebApprovalAction {
    workspace: String,
    session: Option<uuid::Uuid>,
    decision: WebApprovalDecision,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebQuestionAction {
    workspace: String,
    session: Option<uuid::Uuid>,
    answer: Option<String>,
}

#[derive(Serialize)]
struct RuntimeActivitySummary {
    tools: Vec<willdeep_runtime_protocol::RuntimeTool>,
    artifacts: Vec<willdeep_runtime_protocol::RuntimeArtifact>,
    agents: Vec<WebRuntimeAgent>,
    tasks: Vec<WebRuntimeTask>,
    gates: Vec<WebRuntimeGate>,
    attention_count: usize,
    teams: Vec<willdeep_runtime_protocol::agent_team::AgentTeamSummary>,
    team_statistics_unavailable: bool,
}

#[derive(Serialize)]
struct WebRuntimeAgent {
    id: uuid::Uuid,
    parent_id: Option<uuid::Uuid>,
    label: Option<String>,
    background: bool,
    profile: Option<String>,
    model: Option<String>,
    status: &'static str,
    current_turn: u64,
    current_tool: Option<String>,
    retry_wait: Option<willdeep_runtime_protocol::AgentRetryWait>,
    total_tokens: Option<u64>,
    elapsed_seconds: u64,
    finished_seconds_ago: Option<u64>,
    worktree_branch: Option<String>,
    dedicated_worktree: bool,
}

#[derive(Serialize)]
struct WebRuntimeTask {
    id: uuid::Uuid,
    session_id: Option<uuid::Uuid>,
    turn_id: Option<uuid::Uuid>,
    agent_id: Option<uuid::Uuid>,
    status: &'static str,
    profile: Option<String>,
    elapsed_seconds: u64,
    exit_code: Option<i32>,
    failure_domain: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WebRuntimeGate {
    Approval {
        parked: bool,
        id: uuid::Uuid,
        task_id: uuid::Uuid,
        description: String,
        always_allow_available: bool,
    },
    Question {
        parked: bool,
        id: uuid::Uuid,
        task_id: uuid::Uuid,
        question: String,
        options: Vec<String>,
        multi_select: bool,
    },
}

pub async fn serve(config: WebConfig) -> Result<()> {
    serve_internal(config, None).await
}

pub(crate) async fn serve_managed(
    config: WebConfig,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    serve_internal(config, Some(shutdown)).await
}

async fn serve_internal(
    config: WebConfig,
    shutdown: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<()> {
    crate::favorites_plugin::ensure_installed(&config.home).await?;
    crate::video_studio_plugin::ensure_installed(&config.home).await?;
    if config.listen.ip().is_loopback() {
        if let Some(parent) = config.config_path.parent() {
            std::fs::create_dir_all(parent).context("create Web configuration directory")?;
        }
        crate::config_plugin::prepare_builtin_runtime(&config.home)?;
    }
    if shutdown.is_none() {
        for workspace in &config.workspaces {
            crate::daemon::ensure_remote_workspace(&config.home, workspace).await?;
        }
    }
    let state = Arc::new(WebState {
        config_path: config.config_path,
        profile: config.profile,
        workspaces: Arc::new(std::sync::RwLock::new(config.workspaces.clone())),
        home: config.home,
        language: config.language,
        harness_slots: Arc::new(Semaphore::new(2)),
        settings_writable: config.listen.ip().is_loopback(),
        settings_write_lock: Mutex::new(()),
        config_tools: tokio::sync::OnceCell::new(),
        session_workspaces: Arc::new(std::sync::Mutex::new(HashMap::new())),
    });
    let app = Router::new()
        .merge(schedules::routes())
        .route("/health", get(health))
        .route("/mcp", post(crate::daemon::mcp_server::handle))
        .route(
            "/api/mcp/tokens",
            get(mcp_tokens::list).post(mcp_tokens::create),
        )
        .route(
            "/api/mcp/tokens/{id}",
            axum::routing::delete(mcp_tokens::revoke),
        )
        .route("/api/setup", get(setup::setup_status))
        .route(
            "/api/settings/providers",
            get(configuration::list).put(configuration::update),
        )
        .route("/api/settings/config-page", get(configuration::page))
        .route(
            "/api/settings/config-command",
            post(configuration::execute_config_tool),
        )
        .route("/api/chat/stream", post(chat_stream))
        .route("/api/sessions", get(sessions))
        .route("/api/sessions/{id}/peers", get(peer_sessions::peers))
        .route(
            "/api/sessions/{id}/peer-messages",
            get(peer_sessions::inbox).post(peer_sessions::send),
        )
        .route(
            "/api/sessions/{id}/peer-messages/{note}/read",
            post(peer_sessions::read),
        )
        .route(
            "/api/sessions/{id}",
            get(session_detail).delete(delete_session),
        )
        .route("/api/sessions/{id}/stream", get(resume_session_stream))
        .route("/api/sessions/{id}/rename", post(rename_session))
        .route(
            "/api/sessions/{id}/response-style",
            post(set_response_style),
        )
        .route("/api/sessions/{id}/steer", post(steer_session))
        .route("/api/sessions/{id}/fork", post(fork_session))
        .route("/api/sessions/{id}/rewind-points", get(rewind_points))
        .route("/api/sessions/{id}/rewind", post(rewind_session))
        .route(
            "/api/sessions/{id}/input-suggestion",
            post(input_suggestion),
        )
        .route(
            "/api/sessions/{id}/input-suggestion/feedback",
            post(input_suggestion_feedback),
        )
        .route("/api/sessions/{id}/archive", post(archive_session))
        .route("/api/sessions/{id}/unarchive", post(unarchive_session))
        .route("/api/sessions/{id}/pin", post(pin_session))
        .route("/api/sessions/{id}/unpin", post(unpin_session))
        .route("/api/sessions/{id}/export", get(export_session))
        .route("/api/turns/{id}/stop", post(stop_turn))
        .route(
            "/api/sessions/{id}/queue",
            get(turn_queue::list)
                .post(turn_queue::enqueue)
                .patch(turn_queue::update),
        )
        .route(
            "/api/sessions/{id}/independent",
            get(turn_queue::independent),
        )
        .route("/api/workspaces", get(workspaces).post(add_workspace))
        .route("/api/runtime/activity", get(runtime_activity))
        .route("/api/runtime/events", get(runtime_events))
        .route(
            "/api/runtime/events/{id}/ignore",
            post(ignore_runtime_event),
        )
        .route(
            "/api/runtime/approvals/{id}/resolve",
            post(resolve_runtime_approval),
        )
        .route(
            "/api/runtime/questions/{id}/answer",
            post(answer_runtime_question),
        )
        .route("/api/runtime/agents/{id}/stop", post(stop_runtime_agent))
        .route("/api/runtime/agents/{id}/runs", get(agent_run_history))
        .route("/api/runtime/agents/{id}/retry", post(retry_runtime_agent))
        .route("/api/runtime/agents/spawn", post(spawn_runtime_agent))
        .route(
            "/api/runtime/agents/{id}/prompt",
            post(prompt_runtime_agent),
        )
        .route(
            "/api/runtime/agents/{id}/continue",
            post(continue_runtime_agent),
        )
        .route(
            "/api/settings/model-routing",
            get(model_routing_settings).put(update_model_routing_settings),
        )
        .route("/api/composer", get(composer::get))
        .merge(agent_library::routes())
        .route("/", get(index))
        // 单页应用的 catch-all。插件的 /plugin-host/... 比它更具体，
        // 路由树按具体度匹配，所以不会被这一条吞掉。
        .route("/{*path}", get(asset))
        .with_state(state.clone());
    // 插件宿主自带一套路由与状态。发现失败不该让整个 Web 起不来——
    // 一个装坏的插件包不能变成"聊天也用不了"。
    let app = match willdeep_core::plugin::PluginHost::discover(&state.home) {
        Ok(host) => {
            let host = Arc::new(host);
            host.set_config_path(state.config_path.clone());
            // 插件 MCP 进程可以反向请求出图、问模型；凭据留在宿主。
            host.set_host_requests(Arc::new(crate::plugin_host_requests::HostRequests::new(
                state.home.clone(),
                state.config_path.clone(),
                state.workspaces.clone(),
            )));
            // 插件 MCP 网关：只绑 127.0.0.1，端口与 token 持久化在发现文件里。
            // 起不来不影响 Web 本身。
            let gateway = match crate::plugin_gateway::PluginGateway::start(
                &state.home,
                host.clone(),
            )
            .await
            {
                Ok(gateway) => {
                    println!(
                        "Plugin MCP gateway: {} ({})",
                        gateway.url(),
                        willdeep_core::plugin::gateway::discovery_path(&state.home).display()
                    );
                    Some(Arc::new(gateway))
                }
                Err(error) => {
                    eprintln!("warning: plugin MCP gateway unavailable: {error}");
                    None
                }
            };
            app.merge(crate::plugin_web::router(Arc::new(
                crate::plugin_web::PluginWebState::new(
                    host,
                    state.config_path.clone(),
                    state.home.clone(),
                    state.workspaces.clone(),
                )
                .with_gateway(gateway),
            )))
        }
        Err(error) => {
            eprintln!("warning: plugin host unavailable: {error}");
            app
        }
    };
    let app = app
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn(server_version_header));
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("bind Web server at {}", config.listen))?;
    let address = listener.local_addr()?;
    println!("WillDeep Web: http://{address}");
    if config.open_setup && address.ip().is_loopback() {
        let url = format!("http://{address}/?setup=1#configuration");
        println!("Provider setup: {url}");
        if config.setup_started.is_none()
            && std::io::stdin().is_terminal()
            && !crate::onboarding::open_browser(&url)
        {
            eprintln!("Could not open the browser; open the Provider setup URL above manually.");
        }
    }
    for workspace in &config.workspaces {
        println!("Workspace: {}", workspace.display());
    }
    if !config.listen.ip().is_loopback() {
        eprintln!(
            "warning: Web mode has no application authentication; place it behind nginx/VPN and HTTPS"
        );
    }
    if let Some(started) = config.setup_started {
        let _ = started.send(address);
    }
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            match shutdown {
                Some(receiver) => crate::daemon::wait_for_shutdown(receiver).await,
                None => std::future::pending::<()>().await,
            }
        })
        .await
        .context("run Web server")?;
    Ok(())
}

pub(crate) fn setup_ready(path: &std::path::Path, profile: Option<&str>) -> bool {
    setup::setup_ready(path, profile)
}

async fn model_routing_settings(
    State(state): State<Arc<WebState>>,
) -> Result<Json<crate::model_routing::ModelRoutingSettings>, WebError> {
    crate::model_routing::load(&state.config_path, state.profile.as_deref())
        .map(Json)
        .map_err(WebError::from_anyhow)
}

async fn update_model_routing_settings(
    State(state): State<Arc<WebState>>,
    Json(update): Json<crate::model_routing::ModelRoutingUpdate>,
) -> Result<Json<crate::model_routing::ModelRoutingSettings>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "model routing settings can only be changed from a loopback Web server",
        ));
    }
    let _guard = state.settings_write_lock.lock().await;
    crate::model_routing::save(&state.config_path, state.profile.as_deref(), &update)
        .map(Json)
        .map_err(|error| {
            if error
                .to_string()
                .contains(crate::model_routing::STALE_CONFIG_MESSAGE)
            {
                WebError::conflict(error.to_string())
            } else {
                WebError::bad_request(error.to_string())
            }
        })
}

fn safe_skill_description(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if [
        "password", "passwd", "api_key", "api-key", "token=", "secret",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return "[sensitive description hidden]".to_owned();
    }
    truncate(value, 320)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"status":"ok","version":willdeep_core::VERSION}))
}

async fn server_version_header(request: Request, next: middleware::Next) -> Response {
    let mut response = next.run(request).await;
    if let Ok(value) = header::HeaderValue::from_str(willdeep_core::VERSION) {
        response.headers_mut().insert("x-app-version", value);
    }
    response
}

async fn workspaces(
    State(state): State<Arc<WebState>>,
) -> Result<Json<Vec<WorkspaceSummary>>, WebError> {
    let values = registered_web_workspaces(&state)
        .await?
        .into_iter()
        .map(|workspace| WorkspaceSummary {
            id: workspace.id.to_string(),
            path: workspace.root.display().to_string(),
            name: workspace.name,
            active: workspace.active,
            access: workspace.access.wire_name(),
        })
        .collect();
    Ok(Json(values))
}

#[derive(Deserialize)]
struct AddWorkspaceRequest {
    path: String,
}

/// 往浏览器端的工作区白名单里加一个目录。
///
/// 只在回环监听时开放，与模型路由设置同一条既有语义：Web 模式没有应用层鉴权，
/// 能连到端口的人就是「这个用户」。对外暴露的实例上，能加工作区就等于能让
/// Agent 去读写机器上任意目录——那条边界必须留在启动命令里（`--web-workspace`）。
async fn add_workspace(
    State(state): State<Arc<WebState>>,
    Json(request): Json<AddWorkspaceRequest>,
) -> Result<Json<WorkspaceSummary>, WebError> {
    if !state.settings_writable {
        return Err(WebError::bad_request(
            "adding a workspace from the browser is only available when the Web server \
             listens on a loopback address; start it with --web-workspace <path> instead",
        ));
    }
    let requested = PathBuf::from(shellexpand_home(request.path.trim()));
    if requested.as_os_str().is_empty() {
        return Err(WebError::bad_request("workspace path cannot be empty"));
    }
    // 先规范化再注册：符号链接与 `..` 在这里解掉，之后 allowlist 比对的是
    // 真实路径，不是用户打进来的那串字。
    let root = requested.canonicalize().map_err(|error| {
        WebError::bad_request(format!("cannot resolve {}: {error}", requested.display()))
    })?;
    if !root.is_dir() {
        return Err(WebError::bad_request(format!(
            "{} is not a directory",
            root.display()
        )));
    }
    let workspace = crate::daemon::ensure_remote_workspace(&state.home, &root)
        .await
        .map_err(WebError::from_anyhow)?;
    {
        let mut allowed = state
            .workspaces
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if !allowed.contains(&workspace.root) {
            allowed.push(workspace.root.clone());
        }
    }
    Ok(Json(WorkspaceSummary {
        id: workspace.id.to_string(),
        path: workspace.root.display().to_string(),
        name: workspace.name,
        active: workspace.active,
        access: workspace.access.wire_name(),
    }))
}

/// 只展开开头的 `~`。不做通配符与变量展开：这是一个路径，不是一条 shell 命令。
fn shellexpand_home(value: &str) -> String {
    let Some(rest) = value.strip_prefix('~') else {
        return value.to_owned();
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return value.to_owned();
    }
    match std::env::var_os("HOME") {
        Some(home) => format!("{}{rest}", home.to_string_lossy()),
        None => value.to_owned(),
    }
}

/// 当前会话的运行时事件，最新在前。
///
/// 读事件日志而不是某个进程的内存：跑 Agent 的可能是 daemon，也可能是别的
/// 前端，日志是它们之间唯一的共享事实。代价是最多落后一次刷盘。
async fn runtime_events(
    State(state): State<Arc<WebState>>,
    Query(query): Query<RuntimeEventsQuery>,
) -> Result<Json<Vec<WebKernelEvent>>, WebError> {
    authorized_event_session(&state, query.session).await?;
    let store = willdeep_core::kernel_store::KernelStore::new(&state.home);
    let mut events = store
        .load_session(query.session)
        .map_err(|error| WebError::from_anyhow(anyhow::Error::msg(error)))?;
    events.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    events.truncate(50);
    Ok(Json(events.iter().map(web_kernel_event).collect()))
}

/// 把一条事件的**用户侧**标记掉。
///
/// 它不批准任何操作：审批仍然要在原来的审批卡片上回答。这里只是把「还等着
/// 人」这个标记摘掉，事件本身留在日志里。
async fn ignore_runtime_event(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<IgnoreRuntimeEventBody>,
) -> Result<StatusCode, WebError> {
    authorized_event_session(&state, body.session).await?;
    let store = willdeep_core::kernel_store::KernelStore::new(&state.home);
    let mut events = store
        .load_session(body.session)
        .map_err(|error| WebError::from_anyhow(anyhow::Error::msg(error)))?;
    let Some(event) = events.iter_mut().find(|event| event.event_id == id) else {
        return Err(WebError::not_found(
            "Runtime event not found in this session",
        ));
    };
    event.delivery.state = willdeep_runtime_protocol::DeliveryState::Ignored;
    event.requires_user_action = false;
    store
        .save_session(body.session, &events)
        .map_err(|error| WebError::from_anyhow(anyhow::Error::new(error)))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn runtime_activity(
    State(state): State<Arc<WebState>>,
    Query(query): Query<RuntimeActivityQuery>,
) -> Result<Json<RuntimeActivitySummary>, WebError> {
    let workspace = select_workspace(&state, Some(&query.workspace)).await?;
    let snapshot = crate::daemon::runtime_snapshot(
        &state.home,
        &workspace.root,
        query.session,
        crate::Surface::Web,
    )
    .await
    .map_err(WebError::from_anyhow)?;
    let mut teams = Vec::new();
    let mut team_statistics_unavailable = false;
    for agent in snapshot
        .agents
        .iter()
        .filter(|agent| agent.parent_id.is_none())
    {
        match crate::daemon::tui_bridge::remote_agent_team(&state.home, agent.id).await {
            Ok(team) => teams.push(team),
            Err(_) => team_statistics_unavailable = true,
        }
    }
    let mut agents: Vec<_> = snapshot.agents.into_iter().map(web_runtime_agent).collect();
    for worker in teams.iter().flat_map(|team| &team.workers) {
        if let Some(agent) = agents.iter_mut().find(|agent| agent.id == worker.agent_id) {
            let run = &worker.latest_run;
            let end = run
                .finished_at_ms
                .unwrap_or_else(|| now_seconds().saturating_mul(1000));
            agent.elapsed_seconds = run
                .started_at_ms
                .map(|start| end.saturating_sub(start) / 1000)
                .unwrap_or(0);
            agent.finished_seconds_ago = run
                .finished_at_ms
                .map(|finished| now_seconds().saturating_sub(finished / 1000));
            agent.total_tokens = run.usage.total_tokens;
        }
    }
    Ok(Json(RuntimeActivitySummary {
        teams,
        team_statistics_unavailable,
        tools: snapshot.tools,
        artifacts: snapshot.artifacts,
        agents,
        tasks: snapshot.tasks.into_iter().map(web_runtime_task).collect(),
        gates: snapshot
            .gates
            .into_iter()
            .map(|gate| match gate {
                crate::daemon::RemoteGate::Approval {
                    parked,
                    id,
                    task_id,
                    description,
                    always_allow_available,
                } => WebRuntimeGate::Approval {
                    parked,
                    id,
                    task_id,
                    description,
                    always_allow_available,
                },
                crate::daemon::RemoteGate::Question {
                    parked,
                    id,
                    task_id,
                    question,
                    options,
                    multi_select,
                } => WebRuntimeGate::Question {
                    parked,
                    id,
                    task_id,
                    question,
                    options,
                    multi_select,
                },
            })
            .collect(),
        attention_count: snapshot.attention.len(),
    }))
}

fn web_runtime_agent(agent: crate::daemon::tui_bridge::RemoteAgent) -> WebRuntimeAgent {
    let elapsed_seconds = agent_elapsed_seconds(&agent);
    let finished_seconds_ago = agent
        .completed_at
        .map(|completed_at| now_seconds().saturating_sub(completed_at));
    WebRuntimeAgent {
        id: agent.id,
        parent_id: agent.parent_id,
        label: agent.label,
        background: agent.background,
        profile: agent.profile,
        model: agent.model,
        status: runtime_status_name(agent.status),
        current_turn: agent.current_turn,
        current_tool: agent.current_tool,
        retry_wait: agent.retry_wait.clone(),
        total_tokens: agent.total_tokens,
        elapsed_seconds,
        finished_seconds_ago,
        worktree_branch: agent.worktree_branch,
        dedicated_worktree: agent.dedicated_worktree,
    }
}

fn web_runtime_task(task: crate::daemon::tui_bridge::RemoteTask) -> WebRuntimeTask {
    let started_at = task.started_at.unwrap_or(task.created_at);
    let completed_at = task.completed_at.unwrap_or_else(now_seconds);
    WebRuntimeTask {
        id: task.id,
        session_id: task.session_id,
        turn_id: task.turn_id,
        agent_id: task.agent_id,
        status: task_status_name(task.status),
        profile: task.profile,
        elapsed_seconds: completed_at.saturating_sub(started_at),
        exit_code: task.exit_code,
        failure_domain: task.failure_domain.map(failure_domain_name),
    }
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn agent_elapsed_seconds(agent: &crate::daemon::tui_bridge::RemoteAgent) -> u64 {
    agent.elapsed_seconds_at(now_seconds())
}

fn task_status_name(status: willdeep_runtime_protocol::TaskStatus) -> &'static str {
    match status {
        willdeep_runtime_protocol::TaskStatus::Queued => "queued",
        willdeep_runtime_protocol::TaskStatus::Running => "running",
        willdeep_runtime_protocol::TaskStatus::Cancelling => "cancelling",
        willdeep_runtime_protocol::TaskStatus::WaitingApproval => "waiting_approval",
        willdeep_runtime_protocol::TaskStatus::WaitingAnswer => "waiting_answer",
        willdeep_runtime_protocol::TaskStatus::Completed => "completed",
        willdeep_runtime_protocol::TaskStatus::Failed => "failed",
        willdeep_runtime_protocol::TaskStatus::Cancelled => "cancelled",
        willdeep_runtime_protocol::TaskStatus::Interrupted => "interrupted",
        willdeep_runtime_protocol::TaskStatus::Partial => "partial",
    }
}

fn failure_domain_name(domain: willdeep_runtime_protocol::FailureDomain) -> &'static str {
    match domain {
        willdeep_runtime_protocol::FailureDomain::Provider => "provider",
        willdeep_runtime_protocol::FailureDomain::Policy => "policy",
        willdeep_runtime_protocol::FailureDomain::Tool => "tool",
        willdeep_runtime_protocol::FailureDomain::Harness => "harness",
        willdeep_runtime_protocol::FailureDomain::Internal => "internal",
        willdeep_runtime_protocol::FailureDomain::Unknown => "unknown",
    }
}

fn runtime_status_name(status: willdeep_core::RuntimeStatus) -> &'static str {
    match status {
        willdeep_core::RuntimeStatus::Idle => "idle",
        willdeep_core::RuntimeStatus::Working => "working",
        willdeep_core::RuntimeStatus::Queued => "queued",
        willdeep_core::RuntimeStatus::Interrupted => "interrupted",
        willdeep_core::RuntimeStatus::Blocked => "blocked",
        willdeep_core::RuntimeStatus::WaitingApproval => "waiting_approval",
        willdeep_core::RuntimeStatus::WaitingAnswer => "waiting_answer",
        willdeep_core::RuntimeStatus::Failed => "failed",
        willdeep_core::RuntimeStatus::Done => "done",
        willdeep_core::RuntimeStatus::Partial => "partial",
        willdeep_core::RuntimeStatus::Cancelled => "cancelled",
        willdeep_core::RuntimeStatus::Unknown => "unknown",
    }
}

async fn resolve_runtime_approval(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(action): Json<WebApprovalAction>,
) -> Result<StatusCode, WebError> {
    let snapshot = authorized_gate_snapshot(&state, &action.workspace, action.session).await?;
    let allowed = snapshot_has_approval(&snapshot, id);
    if !allowed {
        return Err(WebError::not_found(
            "Runtime approval not found in this workspace",
        ));
    }
    let decision = match action.decision {
        WebApprovalDecision::AllowOnce => willdeep_core::ApprovalDecision::AllowOnce,
        WebApprovalDecision::Deny => willdeep_core::ApprovalDecision::Deny,
        WebApprovalDecision::AlwaysAllow => willdeep_core::ApprovalDecision::AlwaysAllow,
    };
    crate::daemon::resolve_remote_approval(&state.home, id, decision)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn answer_runtime_question(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(action): Json<WebQuestionAction>,
) -> Result<StatusCode, WebError> {
    let snapshot = authorized_gate_snapshot(&state, &action.workspace, action.session).await?;
    let allowed = snapshot_has_question(&snapshot, id);
    if !allowed {
        return Err(WebError::not_found(
            "Runtime question not found in this workspace",
        ));
    }
    crate::daemon::answer_remote_question(&state.home, id, action.answer)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_runtime_agent(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(action): Json<WebWorkspaceAction>,
) -> Result<StatusCode, WebError> {
    authorize_runtime_agent(&state, &action.workspace, id).await?;
    crate::daemon::stop_remote_agent(&state.home, id)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn retry_runtime_agent(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(action): Json<WebAgentRetryAction>,
) -> Result<StatusCode, WebError> {
    authorize_runtime_agent(&state, &action.workspace, id).await?;
    let model = action.model.map(|model| model.trim().to_owned());
    if model
        .as_deref()
        .is_some_and(|model| model.is_empty() || model.len() > 256)
    {
        return Err(WebError::bad_request(
            "model must contain 1 to 256 bytes when provided",
        ));
    }
    crate::daemon::retry_remote_agent_with_model(&state.home, id, model)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn prompt_runtime_agent(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(action): Json<WebAgentPromptAction>,
) -> Result<StatusCode, WebError> {
    authorize_runtime_agent(&state, &action.workspace, id).await?;
    crate::daemon::instruct_remote_agent(&state.home, id, action.message)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn continue_runtime_agent(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(action): Json<WebAgentPromptAction>,
) -> Result<StatusCode, WebError> {
    authorize_runtime_agent(&state, &action.workspace, id).await?;
    crate::daemon::continue_remote_agent(&state.home, id, action.message)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::ACCEPTED)
}

#[derive(Deserialize)]
struct AgentRunHistoryQuery {
    workspace: String,
    after: Option<u64>,
}

async fn agent_run_history(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Query(query): Query<AgentRunHistoryQuery>,
) -> Result<Json<willdeep_runtime_protocol::AgentRunsPage>, WebError> {
    authorize_runtime_agent(&state, &query.workspace, id).await?;
    crate::daemon::tui_bridge::remote_agent_runs(&state.home, id, query.after)
        .await
        .map(Json)
        .map_err(WebError::from_anyhow)
}

async fn spawn_runtime_agent(
    State(state): State<Arc<WebState>>,
    Json(action): Json<WebAgentSpawnAction>,
) -> Result<(StatusCode, Json<WebRuntimeAgent>), WebError> {
    let workspace = select_workspace(&state, Some(&action.workspace)).await?;
    let session = SessionStore::new(&state.home)
        .load(action.session_id)
        .map_err(|_| WebError::not_found("Session not found"))?;
    if session.workspace != workspace.root {
        return Err(WebError::not_found(
            "Session not found in the selected workspace",
        ));
    }
    let active = crate::daemon::remote_session_states(&state.home)
        .await
        .map_err(WebError::from_anyhow)?
        .into_iter()
        .any(|candidate| candidate.id == session.id && candidate.active && !candidate.archived);
    if !active {
        return Err(WebError::conflict(
            "A read-only child Agent requires an active parent session",
        ));
    }
    let (profile, prompt, label) = validate_agent_spawn(action)?;
    let agent = crate::daemon::spawn_remote_agent(&state.home, session.id, prompt, profile, label)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok((StatusCode::ACCEPTED, Json(web_runtime_agent(agent))))
}

fn validate_agent_spawn(
    action: WebAgentSpawnAction,
) -> Result<(String, String, Option<String>), WebError> {
    let profile = action.profile.trim().to_owned();
    // 只读的两个公开职责。旧名一并接受：别人保存的流程与脚本不该因为改名断掉。
    if !matches!(
        profile.as_str(),
        "generalist" | "reviewer" | "reader" | "judge"
    ) {
        return Err(WebError::bad_request(
            "external spawn permits the public read-only generalist or reviewer trade; command and writing trades require the parent Agent safety chain",
        ));
    }
    let prompt = action.prompt.trim().to_owned();
    let prompt_chars = prompt.chars().count();
    if prompt_chars == 0 || prompt_chars > MAX_PROMPT_CHARS {
        return Err(WebError::bad_request(format!(
            "prompt must contain 1 to {MAX_PROMPT_CHARS} characters"
        )));
    }
    let label = action
        .label
        .map(|label| label.trim().to_owned())
        .filter(|label| !label.is_empty());
    if label
        .as_deref()
        .is_some_and(|label| label.chars().count() > MAX_AGENT_LABEL_CHARS)
    {
        return Err(WebError::bad_request(format!(
            "label must contain at most {MAX_AGENT_LABEL_CHARS} characters"
        )));
    }
    Ok((profile, prompt, label))
}

async fn authorized_runtime_snapshot(
    state: &WebState,
    workspace: &str,
) -> Result<crate::daemon::RuntimeSnapshot, WebError> {
    let workspace = select_workspace(state, Some(workspace)).await?;
    crate::daemon::runtime_snapshot(&state.home, &workspace.root, None, crate::Surface::Web)
        .await
        .map_err(WebError::from_anyhow)
}

async fn authorized_gate_snapshot(
    state: &WebState,
    workspace: &str,
    session: Option<uuid::Uuid>,
) -> Result<crate::daemon::RuntimeSnapshot, WebError> {
    let workspace = select_workspace(state, Some(workspace)).await?;
    crate::daemon::runtime_snapshot(&state.home, &workspace.root, session, crate::Surface::Web)
        .await
        .map_err(WebError::from_anyhow)
}

async fn authorize_runtime_agent(
    state: &WebState,
    workspace: &str,
    id: uuid::Uuid,
) -> Result<(), WebError> {
    let snapshot = authorized_runtime_snapshot(state, workspace).await?;
    if snapshot_has_agent(&snapshot, id) {
        Ok(())
    } else {
        Err(WebError::not_found(
            "Runtime Agent not found in this workspace",
        ))
    }
}

/// Turn actions carry no Workspace in the request, so the Session that owns the
/// Turn is resolved first and its Workspace checked against the server
/// allowlist. Everything unknown or out of scope collapses into the same 404 so
/// a caller cannot probe for Turn ids in other Workspaces.
async fn authorize_runtime_turn(state: &WebState, id: uuid::Uuid) -> Result<(), WebError> {
    let not_found = || WebError::not_found("Runtime Turn not found in this workspace");
    let session_id = crate::daemon::remote_turn_session(&state.home, id)
        .await
        .map_err(WebError::from_anyhow)?
        .ok_or_else(not_found)?;
    let session = SessionStore::new(&state.home)
        .load(session_id)
        .map_err(|_| not_found())?;
    if workspace_allowed(state, &session.workspace).await? {
        Ok(())
    } else {
        Err(not_found())
    }
}

fn snapshot_has_approval(snapshot: &crate::daemon::RuntimeSnapshot, id: uuid::Uuid) -> bool {
    snapshot.gates.iter().any(
        |gate| matches!(gate, crate::daemon::RemoteGate::Approval { id: value, .. } if *value == id),
    )
}

fn snapshot_has_question(snapshot: &crate::daemon::RuntimeSnapshot, id: uuid::Uuid) -> bool {
    snapshot.gates.iter().any(
        |gate| matches!(gate, crate::daemon::RemoteGate::Question { id: value, .. } if *value == id),
    )
}

fn snapshot_has_agent(snapshot: &crate::daemon::RuntimeSnapshot, id: uuid::Uuid) -> bool {
    snapshot.agents.iter().any(|agent| agent.id == id)
}

async fn sessions(
    State(state): State<Arc<WebState>>,
) -> Result<Json<Vec<SessionSummary>>, WebError> {
    let allowed = registered_web_workspaces(&state)
        .await?
        .into_iter()
        .map(|workspace| workspace.root)
        .collect::<Vec<_>>();
    let runtime_states = crate::daemon::remote_session_states(&state.home)
        .await
        .map_err(WebError::from_anyhow)?
        .into_iter()
        .map(|session| {
            (
                session.id,
                (session.archived, session.active, session.active_turn_id),
            )
        })
        .collect::<HashMap<_, _>>();
    // 会话目录可能有上百个文件、几十 MB；解析走 blocking 线程池，别堵 tokio worker。
    let home = state.home.clone();
    let digests = tokio::task::spawn_blocking(move || SessionStore::new(&home).digests())
        .await
        .map_err(|error| WebError::internal(error.to_string()))?;
    let values = digests
        .into_iter()
        .filter(|session| allowed.contains(&session.workspace))
        .filter(|session| {
            session.has_user_input
                || runtime_states
                    .get(&session.id)
                    .is_some_and(|(_, active, active_turn_id)| *active || active_turn_id.is_some())
        })
        .map(|session| {
            let (archived, active, active_turn_id) = runtime_states
                .get(&session.id)
                .copied()
                .unwrap_or((false, false, None));
            // 标题生成没跑成的会话在列表里长得一模一样。给它一段首条用户
            // 消息，比一排 `New session` 强，也比直接把它们藏起来诚实——
            // 那些会话是有内容的。
            let preview = willdeep_core::session_title::is_placeholder(&session.title)
                .then_some(session.preview)
                .flatten();
            SessionSummary {
                id: session.id.to_string(),
                title: session.title,
                preview,
                workspace: session.workspace.display().to_string(),
                created_at: session.created_at,
                updated_at: session.updated_at,
                started_by: session.started_by.map(|id| id.to_string()),
                pinned_at: session.pinned_at,
                archived,
                active,
                active_turn_id,
            }
        })
        .collect();
    Ok(Json(values))
}

async fn session_detail(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<SessionDetail>, WebError> {
    let session = SessionStore::new(&state.home)
        .load(id)
        .map_err(|_| WebError::bad_request("session was not found"))?;
    if !workspace_allowed(&state, &session.workspace).await? {
        return Err(WebError::bad_request(
            "session workspace is not in the server allowlist",
        ));
    }
    let messages =
        willdeep_core::conversation::project(&session.messages, session.current_plan.as_ref());
    Ok(Json(SessionDetail {
        id: session.id.to_string(),
        workspace: session.workspace.display().to_string(),
        messages,
        response_style: session.response_style,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseStyleRequest {
    /// `"short"` / `"detailed"`；`null` 清掉偏好、回到默认（简洁）。别的值直接拒绝。
    response_style: Option<willdeep_core::ResponseStyle>,
}

/// `/short` 与 `/verbose` 的落点。风格只是这条会话的偏好，下一轮构建 harness 时才读，
/// 所以直接写核心会话文件就够了，不必走 Runtime 协议。
async fn set_response_style(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(request): Json<ResponseStyleRequest>,
) -> Result<StatusCode, WebError> {
    ensure_web_runtime_session(&state, id).await?;
    SessionStore::new(&state.home)
        .update(id, |session| {
            session.response_style = request.response_style;
        })
        .map_err(|_| WebError::bad_request("session was not found"))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn rename_session(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(request): Json<RenameSessionRequest>,
) -> Result<StatusCode, WebError> {
    ensure_web_runtime_session(&state, id).await?;
    crate::daemon::rename_remote_session(&state.home, id, request.title)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SteerSessionRequest {
    message: String,
}

/// 本轮进行中的用户插话：送进该会话正在跑的任务，下一次调模型前以用户身份注入，
/// 不打断手上的活。没有在途任务时 `delivered` 为假，前端提示稍后再发——Web 没有
/// 本地队列，不替用户暗中排队。
async fn steer_session(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(request): Json<SteerSessionRequest>,
) -> Result<Json<serde_json::Value>, WebError> {
    ensure_web_runtime_session(&state, id).await?;
    let message = request.message.trim();
    if message.is_empty() {
        return Err(WebError::bad_request("message must not be empty"));
    }
    let delivered = crate::daemon::steer_remote_turn(&state.home, id, message.to_owned())
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(Json(serde_json::json!({ "delivered": delivered })))
}

async fn fork_session(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(request): Json<ForkSessionRequest>,
) -> Result<(StatusCode, Json<ForkSessionResponse>), WebError> {
    ensure_web_runtime_session(&state, id).await?;
    let fork_id = crate::daemon::fork_remote_session(
        &state.home,
        id,
        request.title,
        request.through_turn_id,
        request.provider_profile,
        request.model,
    )
    .await
    .map_err(WebError::from_anyhow)?;
    Ok((
        StatusCode::CREATED,
        Json(ForkSessionResponse {
            id: fork_id.to_string(),
        }),
    ))
}

#[derive(Deserialize)]
struct RewindSessionRequest {
    #[serde(default)]
    through_turn_id: Option<uuid::Uuid>,
    #[serde(default)]
    restore_workspace: bool,
}

/// 「回到第 N 步」能选的步骤。会话在跑时也能列，真正回退时 Runtime 会拒绝。
async fn rewind_points(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Vec<crate::daemon::RewindPoint>>, WebError> {
    ensure_web_runtime_session(&state, id).await?;
    crate::daemon::remote_rewind_points(&state.home, id)
        .await
        .map(Json)
        .map_err(WebError::from_anyhow)
}

async fn rewind_session(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(request): Json<RewindSessionRequest>,
) -> Result<Json<willdeep_runtime_protocol::RewindSessionResult>, WebError> {
    ensure_web_runtime_session(&state, id).await?;
    crate::daemon::rewind_remote_session(
        &state.home,
        id,
        request.through_turn_id,
        request.restore_workspace,
    )
    .await
    .map(Json)
    .map_err(WebError::from_anyhow)
}

async fn archive_session(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<StatusCode, WebError> {
    ensure_web_runtime_session(&state, id).await?;
    crate::daemon::set_remote_session_archived(&state.home, id, true)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unarchive_session(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<StatusCode, WebError> {
    ensure_web_runtime_session(&state, id).await?;
    crate::daemon::set_remote_session_archived(&state.home, id, false)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pin_session(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<StatusCode, WebError> {
    set_session_pinned(&state, id, true).await
}

async fn unpin_session(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<StatusCode, WebError> {
    set_session_pinned(&state, id, false).await
}

// 置顶是本地会话元数据（与 Xedit 的 pinnedAt 同语义），不需要把会话拉入 Runtime。
async fn set_session_pinned(
    state: &WebState,
    id: uuid::Uuid,
    pinned: bool,
) -> Result<StatusCode, WebError> {
    let store = SessionStore::new(&state.home);
    let session = store
        .load(id)
        .map_err(|_| WebError::bad_request("session was not found"))?;
    if !workspace_allowed(state, &session.workspace).await? {
        return Err(WebError::bad_request(
            "session workspace is not in the server allowlist",
        ));
    }
    store
        .set_pinned(id, pinned)
        .map_err(|error| WebError::internal(error.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_session(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(request): Json<DeleteSessionRequest>,
) -> Result<StatusCode, WebError> {
    if request.confirmation != id {
        return Err(WebError::bad_request(
            "session deletion confirmation does not match target",
        ));
    }
    ensure_web_runtime_session(&state, id).await?;
    crate::daemon::delete_remote_session(&state.home, id)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn export_session(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<serde_json::Value>, WebError> {
    ensure_web_runtime_session(&state, id).await?;
    crate::daemon::export_remote_session(&state.home, id)
        .await
        .map(Json)
        .map_err(WebError::from_anyhow)
}

async fn ensure_web_runtime_session(state: &WebState, id: uuid::Uuid) -> Result<(), WebError> {
    let session = SessionStore::new(&state.home)
        .load(id)
        .map_err(|_| WebError::bad_request("session was not found"))?;
    if !workspace_allowed(state, &session.workspace).await? {
        return Err(WebError::bad_request(
            "session workspace is not in the server allowlist",
        ));
    }
    crate::daemon::ensure_runtime_session(
        &state.home,
        session.id,
        &session.workspace,
        session.profile,
        None,
    )
    .await
    .map_err(WebError::from_anyhow)?;
    Ok(())
}

async fn stop_turn(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
) -> Result<StatusCode, WebError> {
    authorize_runtime_turn(&state, id).await?;
    crate::daemon::stop_remote_turn(&state.home, id)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::ACCEPTED)
}

async fn chat_stream(
    State(state): State<Arc<WebState>>,
    Json(input): Json<ChatRequest>,
) -> Result<impl IntoResponse, WebError> {
    let workspace = validate_chat(&state, &input).await?;
    let profile = workspace
        .provider_profile
        .clone()
        .or_else(|| state.profile.clone());
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(run_runtime_turn(state, input, workspace.root, profile, tx));
    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("working"),
    ))
}

async fn resume_session_stream(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Query(query): Query<ResumeStreamQuery>,
) -> Result<impl IntoResponse, WebError> {
    let session = SessionStore::new(&state.home)
        .load(id)
        .map_err(|_| WebError::bad_request("session was not found"))?;
    if !workspace_allowed(&state, &session.workspace).await? {
        return Err(WebError::bad_request(
            "session workspace is not in the server allowlist",
        ));
    }
    let event_head = crate::daemon::runtime_event_head(&state.home)
        .await
        .map_err(WebError::from_anyhow)?;
    let language = query
        .language
        .as_deref()
        .map(|value| Language::parse(Some(value)))
        .transpose()
        .map_err(|error| WebError::bad_request(error.to_string()))?
        .unwrap_or_else(|| state_language(&state));
    let (tx, rx) = mpsc::channel(64);
    let stream_state = state.clone();
    if let Some(active) = crate::daemon::remote_active_turn(&state.home, id)
        .await
        .map_err(WebError::from_anyhow)?
    {
        let cursor = bounded_resume_cursor(query.after, active.replay_after, event_head);
        tokio::spawn(async move {
            send_event(
                &tx,
                serde_json::json!({
                    "type":"resumed",
                    "session_id":active.session_id,
                    "turn_id":active.turn_id,
                    "cursor":cursor,
                }),
            )
            .await;
            if relay_runtime_turn(
                &stream_state,
                &session.workspace,
                RuntimeTurnRelay {
                    cursor,
                    session_id: active.session_id,
                    turn_id: active.turn_id,
                    task_id: active.task_id,
                    language,
                },
                &tx,
            )
            .await
            .is_err()
            {
                send_event(
                    &tx,
                    serde_json::json!({"type":"error","message":"Runtime stream unavailable"}),
                )
                .await;
            }
        });
    } else {
        let turn = crate::daemon::remote_latest_turn(&state.home, id)
            .await
            .map_err(WebError::from_anyhow)?
            .ok_or_else(|| WebError::conflict("session has no Turn to resume"))?;
        let cursor = query.after.unwrap_or(event_head).min(event_head);
        let final_text = latest_assistant_text(&state.home, id).unwrap_or_default();
        tokio::spawn(async move {
            send_event_at(
                &tx,
                serde_json::json!({
                    "type":"resumed",
                    "session_id":id,
                    "turn_id":turn.id,
                    "cursor":cursor,
                }),
                cursor,
            )
            .await;
            let event = match turn.status {
                willdeep_runtime_protocol::TurnStatus::Completed
                | willdeep_runtime_protocol::TurnStatus::Partial => serde_json::json!({
                    "type":if turn.status == willdeep_runtime_protocol::TurnStatus::Partial { "partial" } else { "completed" },
                    "text":final_text,
                    "session_id":id,
                    "turn_id":turn.id,
                }),
                status => serde_json::json!({
                    "type":"error",
                    "message":format!("turn.{status:?}").to_ascii_lowercase(),
                    "session_id":id,
                    "turn_id":turn.id,
                }),
            };
            send_event_at(&tx, event, event_head).await;
        });
    }
    Ok(Sse::new(ReceiverStream::new(rx)).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("working"),
    ))
}

fn bounded_resume_cursor(requested: Option<u64>, replay_after: u64, event_head: u64) -> u64 {
    requested
        .unwrap_or(replay_after)
        .max(replay_after)
        .min(event_head.max(replay_after))
}

async fn validate_chat(
    state: &WebState,
    input: &ChatRequest,
) -> Result<crate::daemon::RuntimeWorkspace, WebError> {
    let prompt = input.prompt.trim();
    if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(WebError::bad_request(
            "prompt must contain 1 to 100000 characters",
        ));
    }
    Language::parse(input.language.as_deref())
        .map_err(|error| WebError::bad_request(error.to_string()))?;
    validate_attachments(&input.attachments)?;
    let workspace = select_workspace(state, input.workspace.as_deref()).await?;
    if let Some(raw_id) = input.session_id.as_deref() {
        let id = uuid::Uuid::parse_str(raw_id)
            .map_err(|_| WebError::bad_request("session_id must be a UUID"))?;
        let session = SessionStore::new(&state.home)
            .load(id)
            .map_err(|_| WebError::bad_request("session_id was not found"))?;
        if session.workspace != workspace.root {
            return Err(WebError::bad_request(
                "session does not belong to the selected workspace",
            ));
        }
    }
    Ok(workspace)
}

async fn run_runtime_turn(
    state: Arc<WebState>,
    input: ChatRequest,
    workspace: PathBuf,
    profile: Option<String>,
    tx: mpsc::Sender<Result<Event, Infallible>>,
) {
    let result = runtime_chat::run_runtime_turn_inner(state, input, workspace, profile, &tx).await;
    if let Err(error) = result {
        send_event(
            &tx,
            serde_json::json!({"type":"error","message":error.message}),
        )
        .await;
    }
}

struct RuntimeTurnRelay {
    cursor: u64,
    session_id: uuid::Uuid,
    turn_id: uuid::Uuid,
    task_id: Option<uuid::Uuid>,
    language: Language,
}

async fn relay_runtime_turn(
    state: &WebState,
    workspace: &std::path::Path,
    relay: RuntimeTurnRelay,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) -> Result<(), WebError> {
    let RuntimeTurnRelay {
        mut cursor,
        session_id,
        turn_id,
        mut task_id,
        language,
    } = relay;
    let mut final_text = None;
    loop {
        if tx.is_closed() {
            return Ok(());
        }
        let events = crate::daemon::runtime_events(&state.home, cursor, workspace)
            .await
            .map_err(WebError::from_anyhow)?;
        for event in events {
            cursor = cursor.max(event.sequence);
            if event.kind == "turn.started"
                && event_uuid(&event.message, "turn_id") == Some(turn_id)
            {
                task_id = event_uuid(&event.message, "task_id");
            }
            if event.kind == "task.output"
                && task_id.is_some()
                && event_uuid(&event.message, "task_id") == task_id
                && let Some(value) = runtime_output_payload(&event.message)
            {
                if matches!(
                    value.get("type").and_then(|value| value.as_str()),
                    Some("completed" | "partial")
                ) {
                    final_text = value
                        .get("text")
                        .and_then(|value| value.as_str())
                        .map(ToOwned::to_owned);
                } else if let Some(client) = client_event(value, language) {
                    send_event_at(tx, client, event.sequence).await;
                }
            }
            if matches!(
                event.kind.as_str(),
                "turn.completed"
                    | "turn.partial"
                    | "turn.cancelled"
                    | "turn.interrupted"
                    | "turn.failed"
            ) && event_uuid(&event.message, "turn_id") == Some(turn_id)
            {
                if matches!(event.kind.as_str(), "turn.completed" | "turn.partial") {
                    let text = final_text
                        .or_else(|| latest_assistant_text(&state.home, session_id))
                        .unwrap_or_default();
                    send_event_at(
                        tx,
                        serde_json::json!({
                            "type":if event.kind == "turn.partial" { "partial" } else { "completed" },
                            "text":text,
                            "session_id":session_id,
                            "turn_id":turn_id,
                        }),
                        event.sequence,
                    )
                    .await;
                } else {
                    send_event_at(
                        tx,
                        serde_json::json!({
                            "type":"error",
                            "message":event.kind,
                            "session_id":session_id,
                            "turn_id":turn_id,
                        }),
                        event.sequence,
                    )
                    .await;
                }
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn runtime_output_payload(message: &str) -> Option<serde_json::Value> {
    let (_, payload) = message.split_once(' ')?;
    serde_json::from_str(payload).ok()
}

fn event_uuid(message: &str, key: &str) -> Option<uuid::Uuid> {
    message.split_whitespace().find_map(|part| {
        let (candidate, value) = part.split_once('=')?;
        (candidate == key)
            .then(|| uuid::Uuid::parse_str(value).ok())
            .flatten()
    })
}

#[derive(Deserialize)]
struct InputSuggestionRequest {
    /// 前端发起时看到的那一轮；原样带回去，前端据此丢弃晚到的结果。
    #[serde(default)]
    turn_id: Option<uuid::Uuid>,
}

mod runtime_handoff;
mod suggestion_feedback;
use runtime_handoff::{RUNTIME_HANDOFF_RETRY, retry_through_runtime_handoff};
#[cfg(test)]
use suggestion_feedback::{SuggestionFeedbackRequest, apply_suggestion_feedback};
use suggestion_feedback::{input_suggestion_feedback, issue_suggestion};

#[derive(Serialize)]
struct InputSuggestionResponse {
    suggestion: Option<String>,
    turn_id: Option<uuid::Uuid>,
    /// 这条建议在反馈账本里的身份。前端上报展示 / 采用 / 放弃 / 发送时带回来。
    suggestion_id: Option<uuid::Uuid>,
}

/// 轮次收尾后预测用户的下一句（与 TUI 同一契约，见 `input_suggestion` 模块）。
///
/// 预测是装饰：会话不存在、不在白名单、开关关着、会话还在跑、组不出正文、所有
/// 候选 Provider 都失败——一律 200 + `null`，不许 5xx。只读会话文件，不写任何
/// 东西，也不经 Runtime 协议；刷新页面后自然没有，这是有意的。
async fn input_suggestion(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(request): Json<InputSuggestionRequest>,
) -> Json<InputSuggestionResponse> {
    let suggestion = predict_session_input(&state, id).await;
    Json(InputSuggestionResponse {
        suggestion_id: suggestion.as_deref().map(|text| issue_suggestion(id, text)),
        suggestion,
        turn_id: request.turn_id,
    })
}

async fn predict_session_input(state: &WebState, id: uuid::Uuid) -> Option<String> {
    let loaded = crate::config::LoadedConfig::load(Some(&state.config_path)).ok()?;
    let profile = willdeep_core::runtime_parameters::RuntimeParameters::load(&state.home).ok()?;
    // 开关先看：关着就连会话文件和 Runtime 都不碰。
    if !profile
        .map(|value| value.input_suggestions)
        .unwrap_or_else(|| loaded.file.agent.input_suggestions.unwrap_or(true))
    {
        return None;
    }
    let session = SessionStore::new(&state.home).load(id).ok()?;
    if !workspace_allowed(state, &session.workspace).await.ok()? {
        return None;
    }
    // 查不到 Runtime 状态时按「在跑」处理：宁可少一句预测，也不在轮次中途冒出来。
    let running = crate::daemon::remote_session_states(&state.home)
        .await
        .map(|sessions| {
            sessions
                .iter()
                .any(|entry| entry.id == id && entry.active_turn_id.is_some())
        })
        .unwrap_or(true);
    let profile = session.profile.clone().or_else(|| state.profile.clone());
    let model = session.model.clone();
    let ledger =
        crate::harness::standalone_usage_ledger(&state.home, Some(id), Some(&session.workspace));
    suggest_for_session(&session.messages, running, || {
        crate::harness::input_suggestion_providers(&loaded, profile.as_deref(), model).map(
            |providers| {
                providers
                    .into_iter()
                    .map(|provider| ledger.auxiliary(provider))
                    .collect()
            },
        )
    })
    .await
}

/// 与 I/O 无关的那一半，单测直接喂假 Provider。
async fn suggest_for_session(
    messages: &[willdeep_core::Message],
    running: bool,
    providers: impl FnOnce() -> Result<Vec<Arc<dyn willdeep_core::provider::Provider>>>,
) -> Option<String> {
    if running {
        return None;
    }
    let payload = willdeep_core::input_suggestion::payload(messages)?;
    let providers = providers().ok()?;
    willdeep_core::input_suggestion::predict_first(&providers, &payload).await
}

fn latest_assistant_text(home: &std::path::Path, session_id: uuid::Uuid) -> Option<String> {
    SessionStore::new(home)
        .load(session_id)
        .ok()?
        .messages
        .into_iter()
        .rev()
        .find(|message| message.role == Role::Assistant && !message.content.trim().is_empty())
        .map(|message| message.content)
}

fn state_language(state: &WebState) -> Language {
    state.language
}

fn validate_attachments(attachments: &[MessageAttachment]) -> Result<(), WebError> {
    if attachments.len() > 12 {
        return Err(WebError::bad_request("at most 12 attachments are allowed"));
    }
    for attachment in attachments {
        match attachment {
            MessageAttachment::Text { content, .. } if content.chars().count() > 200_000 => {
                return Err(WebError::bad_request("pasted text attachment is too large"));
            }
            MessageAttachment::Image {
                media_type,
                data,
                width,
                height,
                ..
            } => {
                if !matches!(
                    media_type.as_str(),
                    "image/png" | "image/jpeg" | "image/webp" | "image/gif"
                ) || data.len() > 20_000_000
                    || *width == 0
                    || *height == 0
                {
                    return Err(WebError::bad_request(
                        "image attachment is unsupported or too large",
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn client_event(value: serde_json::Value, language: Language) -> Option<serde_json::Value> {
    let kind = value.get("type")?.as_str()?;
    if matches!(kind, "provider_retry_started" | "subagent_retry_started") {
        return Some(
            serde_json::json!({"type":kind,"label":language.text("正在重试", "Retrying", "再試行中")}),
        );
    }
    if kind == "assistant_text_delta" {
        return Some(serde_json::json!({"type":kind,"text":value.get("text")?.as_str()?}));
    }
    // 思维链增量与正文增量一样只带文本；前端做灰色「思考中」尾巴行，正文一来就隐藏。
    if kind == "reasoning_delta" {
        return Some(serde_json::json!({"type":kind,"text":value.get("text")?.as_str()?}));
    }
    if kind == "subagent_retry_wait" {
        let id = value.get("id")?.as_str()?.parse::<uuid::Uuid>().ok()?;
        let attempt = value.get("attempt")?.as_u64()?;
        let delay = value.get("delay_ms")?.as_u64()?;
        return Some(serde_json::json!({
            "type":kind, "id":id, "attempt":attempt, "delay_ms":delay,
            "label":format!("{} {} · {} {attempt} · {}s",
                language.text("子 Agent", "Subagent", "サブエージェント"),
                &id.to_string()[..8],
                language.text("等待重试", "Waiting to retry", "再試行を待機中"),
                delay.div_ceil(1000)),
        }));
    }
    if kind == "provider_retry_wait" {
        let delay = value
            .get("delay_ms")
            .and_then(|value| value.as_u64())
            .unwrap_or_default();
        return Some(serde_json::json!({
            "type":kind,
            "label":format!("{} · {}s", language.text("等待重试", "Waiting to retry", "再試行を待機中"), delay.div_ceil(1000)),
        }));
    }
    if kind == "usage_snapshot" {
        return None;
    }
    let label = match kind {
        "turn_started" => format!(
            "{} {}",
            language.text("正在思考 · 第", "Thinking · turn", "思考中 · ターン"),
            value.get("turn").and_then(|v| v.as_u64()).unwrap_or(1)
        ),
        "tool_requested" => format!(
            "{} {}",
            language.text("正在使用", "Using", "使用中"),
            value.get("name").and_then(|v| v.as_str()).unwrap_or("tool")
        ),
        "tool_completed" => format!(
            "{} {}",
            if value
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                language.text("执行失败", "Failed", "実行失敗")
            } else {
                language.text("已完成", "Finished", "完了")
            },
            value.get("name").and_then(|v| v.as_str()).unwrap_or("tool")
        ),
        "compression_started" => language
            .text(
                "正在压缩上下文",
                "Compressing context",
                "コンテキストを圧縮中",
            )
            .to_owned(),
        "compression_completed" => language
            .text(
                "上下文压缩完成",
                "Context compressed",
                "コンテキストを圧縮しました",
            )
            .to_owned(),
        "usage" => language
            .text("正在整理结果", "Preparing result", "結果を整理中")
            .to_owned(),
        "completed" => return Some(value),
        "assistant_text" => {
            let thought = value
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            return Some(serde_json::json!({"type":"thought","text":truncate(thought,240)}));
        }
        _ => kind.to_owned(),
    };
    Some(serde_json::json!({
        "type":kind,
        "label":label,
        // 这一步具体在干什么。摘要在 `tool_detail` 那边就收敛并打码过了，这里
        // 只负责原样带出去——界面把它显示成一行淡色小字。
        "detail":value.get("detail").and_then(|item| item.as_str()),
        "id":value.get("id").and_then(|item| item.as_str()),
        "name":value.get("name").and_then(|item| item.as_str()),
        "is_error":value.get("is_error").and_then(|item| item.as_bool()),
        "plan_update":value.get("plan_update"),
    }))
}

async fn send_event(tx: &mpsc::Sender<Result<Event, Infallible>>, value: serde_json::Value) {
    if let Ok(event) = Event::default().json_data(value) {
        let _ = tx.send(Ok(event)).await;
    }
}

async fn send_event_at(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    mut value: serde_json::Value,
    cursor: u64,
) {
    if let Some(object) = value.as_object_mut() {
        object.insert("cursor".to_owned(), serde_json::Value::from(cursor));
    }
    if let Ok(event) = Event::default().id(cursor.to_string()).json_data(value) {
        let _ = tx.send(Ok(event)).await;
    }
}

/// 读一份白名单快照。锁被毒化时也要拿到里面的值——把一个中毒的锁变成
/// 「所有工作区都不可用」，比继续用那份数据更糟。
fn allowed_workspaces(state: &WebState) -> Vec<PathBuf> {
    state
        .workspaces
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

async fn registered_web_workspaces(
    state: &WebState,
) -> Result<Vec<crate::daemon::RuntimeWorkspace>, WebError> {
    let registered = crate::daemon::remote_workspaces(&state.home)
        .await
        .map_err(WebError::from_anyhow)?;
    let mut allowed = state
        .workspaces
        .write()
        .unwrap_or_else(|error| error.into_inner());
    Ok(web_workspace_allowlist(
        registered,
        &mut allowed,
        state.settings_writable,
    ))
}

/// 浏览器端能看到哪些已注册工作区。
///
/// 回环监听时，浏览器就是这台机器的用户：`+` 本来就能把任意目录加进来（见
/// [`add_workspace`]），再拿启动参数里的白名单挡住自己用过的常用工作区没有
/// 意义。此时把注册表并进白名单，选择器、会话列表和插件边界看到同一份名单；
/// 已经不存在的目录不进选择器，也不进白名单——选它只会在发送时失败。
/// 对外暴露的实例没有应用层鉴权，仍只认启动参数给出的白名单。
fn web_workspace_allowlist(
    registered: Vec<crate::daemon::RuntimeWorkspace>,
    allowed: &mut Vec<PathBuf>,
    loopback: bool,
) -> Vec<crate::daemon::RuntimeWorkspace> {
    if !loopback {
        return registered
            .into_iter()
            .filter(|workspace| allowed.contains(&workspace.root))
            .collect();
    }
    registered
        .into_iter()
        .filter(|workspace| {
            if !workspace.root.is_dir() {
                return false;
            }
            if !allowed.contains(&workspace.root) {
                allowed.push(workspace.root.clone());
            }
            true
        })
        .collect()
}

async fn workspace_allowed(
    state: &WebState,
    requested: &std::path::Path,
) -> Result<bool, WebError> {
    Ok(workspace_in_allowlist(
        &registered_web_workspaces(state).await?,
        requested,
    ))
}

fn workspace_in_allowlist(
    allowed: &[crate::daemon::RuntimeWorkspace],
    requested: &std::path::Path,
) -> bool {
    allowed.iter().any(|workspace| workspace.root == requested)
}

async fn select_workspace(
    state: &WebState,
    requested: Option<&str>,
) -> Result<crate::daemon::RuntimeWorkspace, WebError> {
    select_registered_workspace(registered_web_workspaces(state).await?, requested)
}

fn select_registered_workspace(
    allowed: Vec<crate::daemon::RuntimeWorkspace>,
    requested: Option<&str>,
) -> Result<crate::daemon::RuntimeWorkspace, WebError> {
    match requested {
        Some(value) => allowed
            .into_iter()
            .find(|workspace| workspace.root.to_string_lossy() == value)
            .ok_or_else(|| WebError::bad_request("workspace is not in the server allowlist")),
        None => allowed
            .iter()
            .find(|workspace| workspace.active)
            .cloned()
            .or_else(|| allowed.into_iter().next())
            .ok_or_else(|| WebError::internal("server has no workspace")),
    }
}

async fn index() -> Response {
    embedded("index.html")
}
async fn asset(Path(path): Path<String>) -> Response {
    embedded(&path)
}
fn embedded(path: &str) -> Response {
    let Some(file) = WebAssets::get(path).or_else(|| WebAssets::get("index.html")) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let content_type = match path.rsplit('.').next() {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    Response::builder().header(header::CONTENT_TYPE, content_type).header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:")
        .body(Body::from(file.data.into_owned())).unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

#[derive(Debug)]
struct WebError {
    status: StatusCode,
    message: String,
}
impl WebError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: message.into(),
        }
    }
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
    fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }
    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }
    fn from_anyhow(error: anyhow::Error) -> Self {
        Self::internal(error.to_string())
    }
}
impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({"error":self.message}))).into_response()
    }
}
#[cfg(test)]
mod tests;

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}
