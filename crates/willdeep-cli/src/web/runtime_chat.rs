use super::*;

pub(super) async fn run_runtime_turn_inner(
    state: Arc<WebState>,
    input: ChatRequest,
    workspace: PathBuf,
    profile: Option<String>,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
) -> Result<(), WebError> {
    let _slot = state
        .harness_slots
        .clone()
        .acquire_owned()
        .await
        .map_err(|error| WebError::internal(error.to_string()))?;
    let store = SessionStore::new(&state.home);
    let mut session = if let Some(raw_id) = input.session_id.as_deref() {
        let id = uuid::Uuid::parse_str(raw_id)
            .map_err(|_| WebError::bad_request("session_id must be a UUID"))?;
        store
            .load(id)
            .map_err(|_| WebError::bad_request("session_id was not found"))?
    } else {
        let id = input.request_id.map(|request| {
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                format!("web-session:{request}").as_bytes(),
            )
        });
        if let Some(saved) = id.and_then(|id| store.load(id).ok()) {
            if saved.workspace != workspace {
                return Err(WebError::not_found("Session not found in Web workspace"));
            }
            saved
        } else {
            let mut session = Session::new(workspace.clone(), profile.clone(), input.prompt.trim());
            if let Some(id) = id {
                session.id = id;
            }
            session
        }
    };
    let event_head = crate::daemon::runtime_event_head(&state.home)
        .await
        .map_err(WebError::from_anyhow)?;
    let needs_adoption_save = !session.runtime_managed;
    if session.config.is_none() {
        session.config = Some(state.config_path.clone());
    }
    session.runtime_managed = true;
    if session.runtime_event_cursor == 0 {
        session.runtime_event_cursor = event_head;
    }
    if needs_adoption_save {
        store
            .save(&mut session)
            .map_err(|error| WebError::internal(error.to_string()))?;
    }
    let language = Language::parse(input.language.as_deref()).unwrap_or(state_language(&state));
    let session_profile = session.profile.clone().or(profile);
    let prompt = input.prompt.trim().to_owned();
    // 领养会话与提交都可能撞上旧 Runtime 的排空闸门；两步一起重来，
    // 领养本身幂等，不会多建会话。
    let request_id = input.request_id.unwrap_or_else(uuid::Uuid::new_v4);
    let (remote_session, turn) =
        // Reuse the same turn identity through handoff and lost-response retries.
        retry_through_runtime_handoff(tx, language, RUNTIME_HANDOFF_RETRY, || {
            // 每次重试都拿自己的一份：借用捕获的 async 闭包过不了 tokio::spawn 的 Send 推导。
            let home = state.home.clone();
            let workspace = workspace.clone();
            let session_id = session.id;
            let session_profile = session_profile.clone();
            let prompt = prompt.clone();
            let attachments = input.attachments.clone();

            async move {
                let remote_session = crate::daemon::ensure_runtime_session(
                    &home,
                    session_id,
                    &workspace,
                    session_profile,
                    None,
                )
                .await?;
                let turn = crate::daemon::submit_runtime_turn_with_id(
                    &home,
                    remote_session.id,
                    prompt,
                    attachments,
                    crate::Surface::Web,
                    request_id,
                )
                .await?;
                Ok((remote_session, turn))
            }
        })
        .await?;
    send_event(
        tx,
        serde_json::json!({
            "type":"submitted",
            "session_id":remote_session.id,
            "turn_id":turn.id,
            "root_agent_id":remote_session.root_agent_id,
            "cursor":event_head,
        }),
    )
    .await;
    use willdeep_runtime_protocol::TurnStatus;
    if matches!(
        turn.status,
        TurnStatus::Completed
            | TurnStatus::Partial
            | TurnStatus::Failed
            | TurnStatus::Cancelled
            | TurnStatus::Interrupted
    ) {
        let event = match turn.status {
            TurnStatus::Completed | TurnStatus::Partial => serde_json::json!({
                "type": if turn.status == TurnStatus::Partial { "partial" } else { "completed" },
                "text": turn_text(&store, &turn).unwrap_or_default(),
                "session_id": remote_session.id, "turn_id": turn.id,
            }),
            status => {
                serde_json::json!({"type":"error", "message":format!("turn.{status:?}").to_ascii_lowercase(), "session_id":remote_session.id,"turn_id":turn.id})
            }
        };
        send_event(tx, event).await;
        return Ok(());
    }
    relay_runtime_turn(
        &state,
        &workspace,
        RuntimeTurnRelay {
            cursor: event_head,
            session_id: remote_session.id,
            turn_id: turn.id,
            task_id: None,
            language,
        },
        tx,
    )
    .await
}

fn turn_text(
    store: &SessionStore,
    turn: &willdeep_runtime_protocol::RuntimeTurn,
) -> Option<String> {
    let session = store.load(turn.session_id).ok()?;
    if turn.message_generation != session.compression_generation {
        return None;
    }
    let messages = session
        .messages
        .get(turn.message_start?..turn.message_end?)?;
    messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .map(|message| message.content.clone())
}
