use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReasoningLevelRequest {
    reasoning_level: Option<String>,
}

pub(super) async fn set_session_reasoning_level(
    State(state): State<Arc<WebState>>,
    Path(id): Path<uuid::Uuid>,
    Json(request): Json<ReasoningLevelRequest>,
) -> Result<StatusCode, WebError> {
    if !state.settings_writable {
        return Err(WebError::forbidden(
            "changing reasoning level from the browser is only available when the Web server listens on a loopback address; use `/reasoning` in the TUI instead",
        ));
    }
    ensure_web_runtime_session(&state, id).await?;
    if let Some(level) = request.reasoning_level.as_deref() {
        let core = SessionStore::new(&state.home)
            .load(id)
            .map_err(|_| WebError::bad_request("session was not found"))?;
        let runtime_session = crate::daemon::remote_session_states(&state.home)
            .await
            .map_err(WebError::from_anyhow)?
            .into_iter()
            .find(|session| session.id == id)
            .ok_or_else(|| WebError::bad_request("Runtime Session was not found"))?;
        let loaded = crate::config::LoadedConfig::load(core.config.as_deref())
            .map_err(WebError::from_anyhow)?;
        let profile = loaded
            .select_provider(runtime_session.profile.as_deref())
            .map_err(WebError::from_anyhow)?;
        let configured = profile
            .and_then(|profile| profile.reasoning.as_ref())
            .is_some_and(|reasoning| reasoning.levels.contains_key(level));
        if !configured {
            return Err(WebError::bad_request(
                "reasoning level is not configured for this Session Provider",
            ));
        }
    }
    crate::daemon::update_remote_session_reasoning_level(&state.home, id, request.reasoning_level)
        .await
        .map_err(WebError::from_anyhow)?;
    Ok(StatusCode::NO_CONTENT)
}
