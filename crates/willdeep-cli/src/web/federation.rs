//! 实例联邦的 Web 接口。
//!
//! 和公网入口管理面一样只在回环开放：联系人表里有每对实例的共享密钥，
//! 远程连上来的浏览器既不该看见它，也不该替这台机器发消息。

use super::*;

pub(super) fn routes() -> Router<Arc<WebState>> {
    Router::new()
        .route("/api/federation", get(list))
        .route("/api/federation/invite", post(invite))
        .route("/api/federation/join", post(join))
        .route("/api/federation/{id}/message", post(say))
        .route("/api/federation/{id}", axum::routing::delete(remove))
        .route("/api/federation/groups", post(create_group))
        .route("/api/federation/groups/{id}/message", post(group_say))
        .route("/api/federation/groups/{id}/pause", post(group_pause))
}

#[derive(Serialize)]
struct ContactView {
    id: String,
    short_id: String,
    name: String,
    url: String,
    session_id: Option<String>,
    auto_turns: u32,
    added_at: u64,
    last_seen: Option<u64>,
}

#[derive(Serialize)]
struct FederationView {
    /// 本机身份，界面上用来标「我」。
    me: IdentityView,
    enabled: bool,
    workspace: String,
    reply: String,
    max_auto_turns: u32,
    /// 邀请链接的前缀；空串表示 [intake] public_url 还没配。
    invite_base: String,
    contacts: Vec<ContactView>,
    groups: Vec<GroupView>,
}

#[derive(Serialize)]
struct GroupView {
    id: String,
    name: String,
    /// 含建群的一方。
    members: usize,
    session_id: Option<String>,
    auto_turns: u32,
    paused: bool,
}

#[derive(Serialize)]
struct IdentityView {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinRequest {
    url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SayRequest {
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupRequest {
    name: String,
    members: Vec<String>,
}

fn settings(state: &WebState) -> crate::config::FederationSettings {
    crate::config::LoadedConfig::load(Some(&state.config_path))
        .map(|loaded| loaded.file.federation)
        .unwrap_or_default()
}

fn invite_base(state: &WebState) -> String {
    crate::config::LoadedConfig::load(Some(&state.config_path))
        .map(|loaded| {
            loaded
                .file
                .intake
                .public_url
                .trim()
                .trim_end_matches('/')
                .to_owned()
        })
        .unwrap_or_default()
}

async fn list(State(state): State<Arc<WebState>>) -> Result<Json<FederationView>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "federation is available only from a loopback Web server",
        ));
    }
    let identity = crate::identity::load(&state.home).map_err(WebError::from_anyhow)?;
    let store = crate::federation::load(&state.home).map_err(WebError::from_anyhow)?;
    let settings = settings(&state);
    Ok(Json(FederationView {
        me: IdentityView {
            id: identity.id.to_string(),
            name: identity.name,
        },
        enabled: settings.enabled,
        workspace: settings.workspace,
        reply: settings.reply,
        max_auto_turns: settings.max_auto_turns,
        invite_base: invite_base(&state),
        groups: store
            .groups
            .iter()
            .map(|group| GroupView {
                id: group.id.to_string(),
                name: group.name.clone(),
                members: group.members.len() + 1,
                session_id: group.session_id.map(|id| id.to_string()),
                auto_turns: group.auto_turns,
                paused: group.paused,
            })
            .collect(),
        contacts: store
            .contacts
            .into_iter()
            .map(|contact| ContactView {
                id: contact.id.to_string(),
                short_id: crate::identity::short_id(contact.id),
                name: contact.name,
                url: contact.url,
                session_id: contact.session_id.map(|id| id.to_string()),
                auto_turns: contact.auto_turns,
                added_at: contact.added_at,
                last_seen: contact.last_seen,
            })
            .collect(),
    }))
}

async fn invite(State(state): State<Arc<WebState>>) -> Result<Json<serde_json::Value>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "federation is available only from a loopback Web server",
        ));
    }
    let base = invite_base(&state);
    if base.is_empty() {
        return Err(WebError::bad_request(
            "[intake] public_url is empty: peers could not reach this machine",
        ));
    }
    let created = crate::federation::create_invite(&state.home).map_err(WebError::from_anyhow)?;
    Ok(Json(serde_json::json!({
        "url": crate::federation::invite_url(&base, &created.token),
        "expires_at": created.expires_at,
    })))
}

async fn join(
    State(state): State<Arc<WebState>>,
    Json(request): Json<JoinRequest>,
) -> Result<Json<FederationView>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "federation is available only from a loopback Web server",
        ));
    }
    let base = invite_base(&state);
    if base.is_empty() {
        return Err(WebError::bad_request(
            "[intake] public_url is empty: the peer could not reach this machine back",
        ));
    }
    let identity = crate::identity::load(&state.home).map_err(WebError::from_anyhow)?;
    let contact = crate::federation::join(&request.url, &identity, &base)
        .await
        .map_err(WebError::from_anyhow)?;
    crate::federation::upsert_contact(&state.home, contact).map_err(WebError::from_anyhow)?;
    list(State(state)).await
}

async fn say(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
    Json(request): Json<SayRequest>,
) -> Result<StatusCode, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "federation is available only from a loopback Web server",
        ));
    }
    let store = crate::federation::load(&state.home).map_err(WebError::from_anyhow)?;
    let contact = crate::federation::find(&store, &id)
        .map_err(|error| WebError::bad_request(error.to_string()))?
        .clone();
    let identity = crate::identity::load(&state.home).map_err(WebError::from_anyhow)?;
    crate::federation::send_message_as(identity.id, &contact, &request.text)
        .await
        .map_err(WebError::from_anyhow)?;
    // 人接手了：自动回复计数清零，循环风险归零。
    crate::federation::note_manual_message(&state.home, contact.id)
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::ACCEPTED)
}

async fn create_group(
    State(state): State<Arc<WebState>>,
    Json(request): Json<GroupRequest>,
) -> Result<Json<FederationView>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "federation is available only from a loopback Web server",
        ));
    }
    crate::federation::create_group(&state.home, &request.name, &request.members)
        .map_err(WebError::from_anyhow)?;
    list(State(state)).await
}

async fn group_say(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
    Json(request): Json<SayRequest>,
) -> Result<StatusCode, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "federation is available only from a loopback Web server",
        ));
    }
    crate::daemon::say_to_group(&state.home, &id, &request.text, Some(&state.config_path))
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::ACCEPTED)
}

async fn group_pause(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> Result<Json<FederationView>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "federation is available only from a loopback Web server",
        ));
    }
    crate::daemon::pause_host_group(&state.home, &id)
        .await
        .map_err(WebError::from_anyhow)?;
    list(State(state)).await
}

async fn remove(
    State(state): State<Arc<WebState>>,
    Path(id): Path<String>,
) -> Result<Json<FederationView>, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "federation is available only from a loopback Web server",
        ));
    }
    crate::federation::remove_contact(&state.home, &id).map_err(WebError::from_anyhow)?;
    list(State(state)).await
}
