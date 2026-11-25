use std::collections::BTreeSet;
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
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

use crate::daemon::diff_review::{self, DiffArea, DiffSnapshot};

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
    let config = WebConfig {
        listen: (Ipv4Addr::LOCALHOST, 0).into(),
        integration_default: config_path == crate::config::default_config_path()?,
        config_path,
        profile: None,
        workspaces: Vec::new(),
        home,
        language: crate::i18n::Language::parse(None)?,
        open_setup: false,
        setup_started: None,
        intake: None,
    };

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

pub(super) fn routes(state: Arc<WebState>) -> Router {
    Router::new()
        .route("/api/desktop/runtime", get(runtime_status))
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
    let Some(branch) = git_branch(&workspace).await? else {
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
    let session_paths = attributed_paths(&state, &workspace, &snapshot, query.session).await;
    Ok(Json(WorkspaceChanges {
        git: true,
        branch,
        snapshot: Some(snapshot),
        session_paths,
    }))
}

async fn git_branch(workspace: &FilePath) -> Result<Option<Option<String>>, WebError> {
    let result = tokio::process::Command::new("git")
        .args(["branch", "--show-current"])
        .current_dir(workspace)
        .output()
        .await
        .map_err(|error| WebError::internal(format!("cannot inspect Git worktree: {error}")))?;
    Ok(result.status.success().then(|| {
        let branch = String::from_utf8_lossy(&result.stdout).trim().to_owned();
        (!branch.is_empty()).then_some(branch)
    }))
}

async fn attributed_paths(
    state: &WebState,
    workspace: &FilePath,
    snapshot: &DiffSnapshot,
    session: Option<Uuid>,
) -> Option<BTreeSet<String>> {
    let session = session?;
    match diff_review::remote_attributions(&state.home, workspace, &snapshot.id).await {
        Ok(records) => Some(
            records
                .into_iter()
                .filter(|record| record.session_id == Some(session))
                .flat_map(|record| record.paths)
                .collect(),
        ),
        Err(_) => None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeFileQuery {
    workspace: String,
    session: Option<Uuid>,
    snapshot_id: String,
    path: String,
}

async fn change_file(
    State(state): State<Arc<WebState>>,
    Query(query): Query<ChangeFileQuery>,
) -> Result<Json<serde_json::Value>, WebError> {
    let workspace = changes_workspace(&state, &query.workspace, query.session).await?;
    let content = diff_review::remote_content(
        &state.home,
        &workspace,
        &query.snapshot_id,
        &query.path,
        DiffArea::Combined,
    )
    .await
    .map_err(WebError::from_anyhow)?;
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
mod tests {
    use super::*;
    use axum::body::to_bytes;

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
}
