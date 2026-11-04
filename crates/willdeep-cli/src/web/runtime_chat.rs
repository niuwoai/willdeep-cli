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
    // 第一条消息之前选的模型与档案从这里带进来；会话已经存在时它们不生效，
    // 以 `/model` 那条路为准——那里改完会立刻告诉 Runtime。
    let requested_model = input.model.clone();
    let requested_profile = input
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let mut session = if let Some(raw_id) = input.session_id.as_deref() {
        let id = uuid::Uuid::parse_str(raw_id)
            .map_err(|_| WebError::bad_request("session_id must be a UUID"))?;
        // 带基线加载：领养会改写这份快照再保存，基线必须记着改动前的执行状态。
        store
            .load_baselined(id)
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
            let session_profile = requested_profile.clone().or(profile.clone());
            let mut session = Session::new(
                workspace.clone(),
                session_profile.clone(),
                input.prompt.trim(),
            );
            if let Some(id) = id {
                session.id = id;
            }
            let loaded = crate::config::LoadedConfig::load(Some(&state.config_path))
                .or_else(|error| {
                    if state.config_path.exists() {
                        Err(error)
                    } else {
                        Ok(crate::config::LoadedConfig {
                            file: Default::default(),
                        })
                    }
                })
                .map_err(WebError::from_anyhow)?;
            let mut cli = <crate::Cli as clap::Parser>::try_parse_from(["willdeep"])
                .map_err(|error| WebError::internal(error.to_string()))?;
            cli.config = (!state.integration_default).then(|| state.config_path.clone());
            cli.profile = session_profile;
            cli.model = requested_model.clone();
            let selection = crate::magpie_integration::resolve(&cli, &loaded, None)
                .await
                .map_err(WebError::from_anyhow)?;
            session.profile = selection.name;
            session.magpie_integration = selection.config.magpie_integration;
            session.model = Some(selection.config.model);
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
    let session_model = session.model.clone();
    let requested_access = input.approval_mode;
    let approval_writable = state.settings_writable;
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
            // 会话上写着哪个模型就按哪个跑。空着交给配置默认只适用于旧会话，
            // 新会话在上面的创建分支里已经把默认模型写下来了。
            let session_model = session_model.clone();
            let prompt = prompt.clone();
            let attachments = input.attachments.clone();

            async move {
                let remote_session = crate::daemon::ensure_runtime_session(
                    &home,
                    session_id,
                    &workspace,
                    session_profile.clone(),
                    session_model.clone(),
                )
                .await?;
                // `ensure` 对已经存在的 Runtime 会话只回原样，不看这次带的内容。
                // 会话文件里有模型而 Runtime 那边还空着（先改模型后领养、或者重试
                // 打到了上一次建好的会话）时，得显式补一次，否则这一轮又按默认模型跑。
                // 这里不判断「是不是本来就一样」：写一次是幂等的，比多读一份状态稳。
                if let Some(model) = session_model.as_deref() {
                    match session_profile.as_deref() {
                        Some(profile) => {
                            crate::daemon::update_remote_session_provider(
                                &home,
                                remote_session.id,
                                profile,
                                model,
                            )
                            .await?;
                        }
                        None => {
                            crate::daemon::update_remote_session_model(
                                &home,
                                remote_session.id,
                                model.to_owned(),
                            )
                            .await?;
                        }
                    }
                }
                // 档位要在提交之前落地：这一轮的第一个工具调用就按它判。
                if let Some(access) = requested_access
                    && approval_writable
                {
                    crate::daemon::update_remote_session_approval_mode(
                        &home,
                        session_id,
                        access.approval_mode(),
                    )
                    .await?;
                    // 与 `/api/sessions/{id}/approval-mode` 同口径留一条审计。
                    crate::harness::record_approval_trace(
                        &home.join("approvals.jsonl"),
                        Some(session_id),
                        &willdeep_core::tools::ApprovalTrace {
                            command: access.wire_name().to_owned(),
                            source: willdeep_core::tools::ApprovalSource::ModeChange,
                            detail: format!(
                                "operator chose approval mode {} for a new Web session",
                                access.wire_name()
                            ),
                        },
                    );
                }
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
                serde_json::json!({"type":"error", "message":format!("turn.{status:?}").to_ascii_lowercase(), "session_id":remote_session.id,"turn_id":turn.id,"task_id":turn.active_task_id})
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
