use super::*;

pub(super) fn dispatch_prompt(
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &TuiRuntime,
    agent: &Arc<Agent>,
    tx: &mpsc::UnboundedSender<UiMessage>,
    prompt: String,
) -> Result<()> {
    let checkpoint = willdeep_core::checkpoint::SessionCheckpointSink {
        store: store.clone(),
        session_id: session.id,
    }
    .claim()?;
    store.refresh_execution(session)?;
    // `/goal` 是长程续推的开关：目标在场时，宿主会拒绝模型的隐式收口（long-horizon.v1 RA1）。
    // 预算读 `[agent] goal_*`，与 daemon、无头运行同一份配置；读不到时退回默认预算。
    if let Some(continuation) = agent.goal_continuation() {
        match app.goal.as_deref() {
            Some(goal) => {
                continuation.activate(goal, goal_budget(runtime));
            }
            None => continuation.clear(),
        }
    }
    app.tools.reset();
    app.begin_turn(
        false,
        app.language
            .text(
                "正在思考 · 理解你的请求",
                "Thinking · understanding your request",
                "思考中 · リクエストを理解しています",
            )
            .to_owned(),
    );
    let mut history = session.messages.clone();
    if let Some(notice) = session
        .execution_checkpoint
        .as_ref()
        .and_then(willdeep_core::checkpoint::CheckpointMetadata::recovery_notice)
    {
        history.push(notice);
    }
    let attachments = std::mem::take(&mut app.attachments)
        .into_iter()
        .map(|value| value.message)
        .collect::<Vec<_>>();
    let enriched = app.enrich_prompt(&prompt, &runtime.skills);
    // L1 用**原始**提示词，不用 enrich 之后的：enrich 会把技能清单之类的
    // 系统素材拼进去，那些东西当标题毫无意义。
    crate::titling::apply_derived_title(session, &prompt, !attachments.is_empty());
    let user = Message::user_with_attachments(enriched, attachments);
    session.messages.push(user.clone());
    store.save(session)?;
    let agent = agent.clone();
    let tx = tx.clone();
    // 句柄留着，Esc 中断本地轮次时要靠它把在途的 Harness 掐掉。
    let summary = session.context_summary.clone();
    app.local_turn = Some(tokio::spawn(async move {
        let _ = tx.send(UiMessage::Finished(
            agent
                .run_checkpointed_with_summary(history, user, Some(&checkpoint), summary)
                .await,
            checkpoint,
        ));
    }));
    Ok(())
}

pub(super) fn dispatch_compress(
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    agent: &Arc<Agent>,
    tx: &mpsc::UnboundedSender<UiMessage>,
    config_path: Option<&std::path::Path>,
    parent: &willdeep_core::provider::ProviderConfig,
) -> Result<()> {
    anyhow::ensure!(
        !app.running,
        "cannot refresh compression during an active turn"
    );
    // TOML and provider errors may quote credential source lines or URLs.
    // Never attach their error chain to the user-visible TUI transcript.
    let config_error = || {
        anyhow::anyhow!(
            "{}{}",
            app.language.compression_configuration_error(),
            config_path
                .map(|path| format!(" ({})", path.display()))
                .unwrap_or_default()
        )
    };
    let loaded = crate::config::LoadedConfig::load(config_path).map_err(|_| config_error())?;
    let settings =
        crate::harness::build_compression_settings(&loaded, parent, agent.main_model().follower())
            .map_err(|_| config_error())?;
    let ownership = willdeep_core::checkpoint::SessionCheckpointSink {
        store: store.clone(),
        session_id: session.id,
    }
    .claim()?;
    store.refresh_execution(session)?;
    agent.set_compression_settings(settings)?;
    app.begin_turn(
        false,
        app.language
            .text(
                "正在压缩上下文",
                "Compressing context",
                "コンテキストを圧縮中",
            )
            .to_owned(),
    );
    let history = session.messages.clone();
    let agent = agent.clone();
    let tx = tx.clone();
    let store = store.clone();
    let session_id = session.id;
    app.local_turn = Some(tokio::spawn(async move {
        let _ = tx.send(UiMessage::Compressed(
            agent
                .compress_history_recorded(history, &mut |usage| {
                    store
                        .record_compression_usage(session_id, usage)
                        .map_err(|error| willdeep_core::AgentError::Checkpoint(error.to_string()))
                })
                .await,
            ownership,
        ));
    }));
    Ok(())
}

/// 这一轮目标用的预算：读 `[agent] goal_*`，与 daemon、无头运行同一份配置。
/// 配置读不到时退回默认预算，不让发一条消息失败。
fn goal_budget(runtime: &TuiRuntime) -> willdeep_core::GoalBudget {
    configured_goal_budget(runtime.runtime_submit.config.as_deref())
}

fn configured_goal_budget(config_path: Option<&std::path::Path>) -> willdeep_core::GoalBudget {
    crate::config::LoadedConfig::load(config_path)
        .map(|loaded| loaded.file.agent.goal_budget())
        .unwrap_or_default()
}

/// 队列里有待投递事件时，开一轮把它们交给模型。
///
/// **额度在这里花，不在别处。** 所有会启动一轮的路径都经过这个函数，漏掉任何
/// 一条限流就形同虚设。而且只在真的要开轮次时才问内核——`admit_wake` 一旦
/// 放行就已经记了这一笔，问了不用等于白扣一次。
///
/// 事件正文不进这个提示词：真正的内容由内核在 turn 顶部注入，那条路上有净化
/// 和来源标注。
pub(super) fn wake_for_kernel_events(
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    agent: &Arc<Agent>,
    runtime: &super::TuiRuntime,
) -> Result<()> {
    if app.running
        || app
            .permission_picker
            .as_ref()
            .is_some_and(|picker| picker.guide.is_some())
    {
        return Ok(());
    }
    let Some(authority) = runtime.kernel.pending_wake_authority(session.id) else {
        return Ok(());
    };
    if !runtime
        .kernel
        .admit_wake(session.id, authority)
        .is_allowed()
    {
        // 排队等着就是了：额度用完不是丢事件的理由，下一次用户发言或下一轮
        // 结束时它照样会被投递。
        return Ok(());
    }
    app.append_transcript("System: runtime events queued for the main agent".to_owned());
    dispatch_notification(
        app,
        session,
        store,
        agent,
        &runtime.tx,
        crate::harness::KERNEL_WAKE_PROMPT.to_owned(),
    )
}

/// Workflow nodes do not broadcast ordinary background completion events. Poll
/// their durable pending/report state so the gap between phases also wakes us.
pub(super) fn wake_for_workflows(
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    agent: &Arc<Agent>,
    runtime: &super::TuiRuntime,
) -> Result<()> {
    if app.running || session.runtime_managed {
        return Ok(());
    }
    if !agent.workflows_pending() && !agent.workflow_reports_pending()? {
        return Ok(());
    }
    match dispatch_notification(
        app,
        session,
        store,
        agent,
        &runtime.tx,
        "Collect pending workflow results and continue the workflow if necessary.".to_owned(),
    ) {
        // Another surface owns this session; its harness will collect results.
        Err(error)
            if matches!(
                error.downcast_ref::<willdeep_core::session::SessionError>(),
                Some(willdeep_core::session::SessionError::ConcurrentUpdate(
                    "active execution"
                ))
            ) =>
        {
            Ok(())
        }
        result => result,
    }
}

pub(super) fn dispatch_notification(
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    agent: &Arc<Agent>,
    tx: &mpsc::UnboundedSender<UiMessage>,
    notice: String,
) -> Result<()> {
    let checkpoint = willdeep_core::checkpoint::SessionCheckpointSink {
        store: store.clone(),
        session_id: session.id,
    }
    .claim()?;
    app.begin_turn(false, "Handling background result".to_owned());
    store.refresh_execution(session)?;
    let history = session.messages.clone();
    let message = Message::host_instruction(notice);
    session.messages.push(message.clone());
    store.save(session)?;
    let agent = agent.clone();
    let tx = tx.clone();
    let summary = session.context_summary.clone();
    app.local_turn = Some(tokio::spawn(async move {
        let _ = tx.send(UiMessage::Finished(
            agent
                .run_checkpointed_with_summary(history, message, Some(&checkpoint), summary)
                .await,
            checkpoint,
        ));
    }));
    Ok(())
}

/// L2 摘要跑在后台任务里：它是一次网络往返，在事件循环里 `await` 会让界面
/// 在每轮收尾时僵住。跑完只回一个可选标题，写库仍由主循环做。
///
/// `force` 是 `/session retitle` 走的路：绕过「一个进程只试一次」，但绕不过
/// 人自己起的名字。
pub(super) fn dispatch_retitle(
    session: &Session,
    agent: &Arc<Agent>,
    tx: &mpsc::UnboundedSender<UiMessage>,
    force: bool,
) {
    if force {
        if session.title_source == willdeep_core::TitleSource::User {
            return;
        }
    } else if !crate::titling::claim_summary_attempt(session) {
        return;
    }
    let messages = session.messages.clone();
    let agent = agent.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let title = crate::titling::summarized_title(&agent, &messages).await;
        let _ = tx.send(UiMessage::Retitled {
            title,
            requested: force,
        });
    });
}

/// 轮次正常收尾后预测用户的下一句。一次网络往返，扔进后台；结果带着发起时的
/// 世代号回来，主循环复核后才落地。这里的判断只是省一次请求，真正的闸门在
/// [`App::adopt_input_suggestion`]。
pub(super) fn dispatch_input_suggestion(
    app: &App,
    session: &Session,
    agent: &Arc<Agent>,
    tx: &mpsc::UnboundedSender<UiMessage>,
) {
    if !agent.input_suggestions_enabled()
        || app.running
        || !app.input.is_empty()
        || !app.attachments.is_empty()
        || !app.queued_prompts.is_empty()
    {
        return;
    }
    let Some(payload) = willdeep_core::input_suggestion::payload(&session.messages) else {
        return;
    };
    let epoch = app.input_suggestion_epoch;
    let agent = agent.clone();
    let tx = tx.clone();
    tokio::spawn(async move {
        let suggestion = agent.suggest_next_input(&payload).await;
        let _ = tx.send(UiMessage::InputSuggested { suggestion, epoch });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UnusedProvider;
    #[async_trait::async_trait]
    impl willdeep_core::provider::Provider for UnusedProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[willdeep_core::types::ToolDefinition],
        ) -> std::result::Result<
            willdeep_core::types::Completion,
            willdeep_core::provider::ProviderError,
        > {
            panic!("short history compression must not request a provider");
        }
    }

    fn parent_config() -> willdeep_core::provider::ProviderConfig {
        willdeep_core::provider::ProviderConfig::new(
            willdeep_core::provider::ProviderKind::OpenAiCompatible,
            willdeep_core::provider::ApiDialect::ChatCompletions,
            "https://compressor.invalid/v1",
            "fixture",
            "main-model",
        )
    }

    struct OriginalCompressor;

    #[async_trait::async_trait]
    impl willdeep_core::provider::Provider for OriginalCompressor {
        async fn complete(
            &self,
            _: &[Message],
            _: &[willdeep_core::types::ToolDefinition],
        ) -> std::result::Result<
            willdeep_core::types::Completion,
            willdeep_core::provider::ProviderError,
        > {
            Ok(willdeep_core::types::Completion {
                content: "original compression settings".to_owned(),
                reasoning: None,
                tool_calls: Vec::new(),
                finish_reason: Some("stop".to_owned()),
                usage: None,
            })
        }
    }

    /// TUI 的 `/goal` 用配置里的预算，不再一律用默认预算。配置读不到时退回默认，
    /// 不让发一条消息失败。
    #[test]
    fn a_local_goal_uses_the_configured_budget() {
        let root = std::env::temp_dir().join(format!("tui-goal-budget-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("config.toml");
        std::fs::write(
            &config,
            "version = 1\n[agent]\ngoal_token_budget = 2000000\ngoal_wall_clock_minutes = 30\ngoal_max_continuations = 10\n",
        )
        .unwrap();

        let budget = configured_goal_budget(Some(&config));
        assert_eq!(budget.max_tokens, Some(2_000_000));
        assert_eq!(budget.max_continuations, 10);
        assert_eq!(
            budget.wall_clock,
            Some(std::time::Duration::from_secs(30 * 60))
        );

        let missing = root.join("missing.toml");
        assert_eq!(
            configured_goal_budget(Some(&missing)),
            willdeep_core::GoalBudget::default(),
            "an unreadable config falls back to the default budget"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn manual_compression_rejects_invalid_refresh_without_dispatching_old_settings() {
        let root =
            std::env::temp_dir().join(format!("tui-compression-refresh-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("explicit-config.toml");
        let store = SessionStore::new(&root);
        let mut session = Session::new(root.clone(), None, "refresh");
        store.save(&mut session).unwrap();
        // Enough history to force a provider request if a failed refresh were
        // ever allowed to fall back to the old Agent configuration.
        for _ in 0..10 {
            session
                .messages
                .push(Message::assistant("old evidence", Vec::new()));
        }
        store.save(&mut session).unwrap();
        let agent = Arc::new(Agent::new(
            Arc::new(UnusedProvider),
            willdeep_core::ToolRegistry::new(&root, willdeep_core::ApprovalMode::ReadOnly).unwrap(),
            willdeep_core::AgentConfig {
                max_turns: 1,
                system_prompt: String::new(),
                context_window: 32000,
                token_budget: None,
            },
        ));
        let parent = parent_config();
        let mut app = App::new(Vec::new(), Language::En);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let missing = dispatch_compress(
            &mut app,
            &mut session,
            &store,
            &agent,
            &tx,
            Some(&config),
            &parent,
        )
        .unwrap_err();
        assert!(missing.to_string().contains("explicit-config.toml"));
        assert!(!app.running);
        assert!(app.local_turn.is_none());
        for invalid in [
            "version = [",
            "version = 1\n[providers.synthetic]\napi_key = \"CREDENTIAL_CANARY\"x\n",
            "version = 1\n[agent]\ncompressor_profile = 'missing'\n",
            "version = 1\n[agent]\ncompressor_context_window = 1024\n",
            // Parses and passes LoadedConfig validation, but Provider construction rejects it.
            "version = 1\n[agent]\ncompressor_model = ''\n",
            "version = 1\n[agent]\ncompressor_profile = 'broken'\n[providers.broken]\nprovider = 'openai-compatible'\napi_base = 'not-a-url'\nmodel = 'summary'\n",
        ] {
            std::fs::write(&config, invalid).unwrap();
            let error = dispatch_compress(
                &mut app,
                &mut session,
                &store,
                &agent,
                &tx,
                Some(&config),
                &parent,
            )
            .unwrap_err();
            let displayed = format!("{error:#}");
            assert!(!displayed.contains("CREDENTIAL_CANARY"));
            assert!(!displayed.contains("api_key"));
            assert!(displayed.contains("explicit-config.toml"));
            assert!(!app.running);
            assert!(app.local_turn.is_none());
            assert!(rx.try_recv().is_err());
            assert!(
                willdeep_core::checkpoint::SessionCheckpointSink {
                    store: store.clone(),
                    session_id: session.id,
                }
                .claim()
                .is_ok()
            );
        }
        // The same TUI remains usable with a corrected explicit config path.
        std::fs::write(&config, "version = 1\n").unwrap();
        session.messages.clear();
        store.save(&mut session).unwrap();
        dispatch_compress(
            &mut app,
            &mut session,
            &store,
            &agent,
            &tx,
            Some(&config),
            &parent,
        )
        .unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, UiMessage::Compressed(Ok(_), _)));
        drop(result);
        app.local_turn.take().unwrap().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manual_compression_does_not_refresh_during_a_running_turn() {
        let root =
            std::env::temp_dir().join(format!("tui-compression-busy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionStore::new(&root);
        let mut session = Session::new(root.clone(), None, "busy");
        let agent = Arc::new(Agent::new(
            Arc::new(UnusedProvider),
            willdeep_core::ToolRegistry::new(&root, willdeep_core::ApprovalMode::ReadOnly).unwrap(),
            willdeep_core::AgentConfig {
                max_turns: 1,
                system_prompt: String::new(),
                context_window: 32000,
                token_budget: None,
            },
        ));
        let parent = willdeep_core::provider::ProviderConfig::new(
            willdeep_core::provider::ProviderKind::OpenAiCompatible,
            willdeep_core::provider::ApiDialect::ChatCompletions,
            "https://compression-fixture.invalid/v1",
            "fixture",
            "main",
        );
        let mut app = App::new(Vec::new(), Language::En);
        app.running = true;
        let (tx, _) = mpsc::unbounded_channel();
        let error = dispatch_compress(
            &mut app,
            &mut session,
            &store,
            &agent,
            &tx,
            Some(&root.join("missing.toml")),
            &parent,
        )
        .unwrap_err();
        assert!(error.to_string().contains("active turn"));
        assert!(app.running);
        assert!(app.local_turn.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn manual_compression_claims_before_dispatch_and_holds_until_result_consumed() {
        use willdeep_core::checkpoint::SessionCheckpointSink;
        let root =
            std::env::temp_dir().join(format!("tui-compression-lock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("custom-compression.toml");
        std::fs::write(
            &config,
            "version = 1\n[agent]\ncompressor_model = 'custom-summary'\n",
        )
        .unwrap();
        let parent = parent_config();
        let store = SessionStore::new(&root);
        let mut session = Session::new(root.clone(), None, "short history");
        store.save(&mut session).unwrap();
        let claimant = || SessionCheckpointSink {
            store: store.clone(),
            session_id: session.id,
        };
        let id = session.id;
        let competitor = claimant().claim().unwrap();
        let agent = Arc::new(
            Agent::new(
                Arc::new(UnusedProvider),
                willdeep_core::ToolRegistry::new(&root, willdeep_core::ApprovalMode::ReadOnly)
                    .unwrap(),
                willdeep_core::AgentConfig {
                    max_turns: 1,
                    system_prompt: String::new(),
                    context_window: 32000,
                    token_budget: None,
                },
            )
            .with_compressor(Arc::new(OriginalCompressor), false),
        );
        let mut app = App::new(Vec::new(), Language::En);
        let (tx, mut rx) = mpsc::unbounded_channel();
        assert!(
            dispatch_compress(
                &mut app,
                &mut session,
                &store,
                &agent,
                &tx,
                Some(&config),
                &parent
            )
            .is_err()
        );
        assert!(app.local_turn.is_none());
        assert!(!app.running);
        // A failed ownership claim must not apply the newly built candidates.
        let old_settings = agent
            .compress_history(
                (0..10)
                    .map(|_| Message::assistant("old evidence", Vec::new()))
                    .collect(),
            )
            .await
            .unwrap();
        assert!(
            old_settings
                .iter()
                .any(|message| message.content.contains("original compression settings"))
        );
        drop(competitor);
        // A completed Runtime turn has an execution checkpoint. Use a write
        // baseline so save persists the new execution state, like a real writer.
        let mut saved = store.load_baselined(id).unwrap();
        saved
            .messages
            .push(Message::user("persisted after TUI snapshot"));
        saved.execution_checkpoint = Some(willdeep_core::checkpoint::CheckpointMetadata {
            status: willdeep_core::checkpoint::CheckpointStatus::Completed,
            ..Default::default()
        });
        store.save(&mut saved).unwrap();
        dispatch_compress(
            &mut app,
            &mut session,
            &store,
            &agent,
            &tx,
            Some(&config),
            &parent,
        )
        .unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(
            SessionCheckpointSink {
                store: store.clone(),
                session_id: id
            }
            .claim()
            .is_err()
        );
        let UiMessage::Compressed(Ok(messages), ownership) = result else {
            panic!("expected compressed history")
        };
        assert!(
            messages
                .iter()
                .any(|message| message.content == "persisted after TUI snapshot")
        );
        session.replace_with_compressed_messages(messages);
        store.save(&mut session).unwrap();
        drop(ownership);
        assert!(
            SessionCheckpointSink {
                store: store.clone(),
                session_id: id
            }
            .claim()
            .is_ok()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
