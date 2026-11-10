use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ReasoningCommand {
    Show,
    Default,
    Set(String),
    Usage,
}

pub(super) fn parse(prompt: &str) -> Option<ReasoningCommand> {
    let value = prompt.trim();
    let arguments = value.strip_prefix("/reasoning")?;
    if !arguments.is_empty() && !arguments.starts_with(char::is_whitespace) {
        return None;
    }
    let mut words = arguments.split_whitespace();
    let Some(action) = words.next() else {
        return Some(ReasoningCommand::Show);
    };
    if action.eq_ignore_ascii_case("set") {
        return Some(match (words.next(), words.next()) {
            (Some(level), None) => ReasoningCommand::Set(level.to_owned()),
            _ => ReasoningCommand::Usage,
        });
    }
    if words.next().is_some() {
        return Some(ReasoningCommand::Usage);
    }
    Some(match action.to_ascii_lowercase().as_str() {
        "show" => ReasoningCommand::Show,
        "default" | "auto" | "reset" => ReasoningCommand::Default,
        _ => ReasoningCommand::Set(action.to_owned()),
    })
}

pub(super) async fn show(
    app: &mut App,
    session: &Session,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
) -> Result<String> {
    let states = crate::daemon::remote_session_states(&runtime.home).await?;
    sync_from_states(session, runtime, agent, &states)?;
    let profile = runtime.provider_config.reasoning.as_ref();
    let current = runtime
        .reasoning_level
        .as_deref()
        .unwrap_or_else(|| app.language.text("默认", "default", "既定"));
    let Some(profile) = profile else {
        return Ok(format!(
            "{}: {current} · {}",
            app.language
                .text("思考等级", "Reasoning level", "思考レベル"),
            app.language.text(
                "当前 Provider 未配置等级映射",
                "No reasoning levels are configured for this Provider",
                "この Provider に思考レベルの設定がありません",
            )
        ));
    };
    let default = profile
        .default_level
        .as_deref()
        .unwrap_or_else(|| app.language.text("未设置", "unset", "未設定"));
    let levels = profile
        .levels
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    Ok(format!(
        "{}: {current} · {}: {default} · {}: {}",
        app.language
            .text("思考等级", "Reasoning level", "思考レベル"),
        app.language
            .text("Provider 默认", "Provider default", "Provider の既定"),
        app.language.text("可选", "available", "選択可能"),
        if levels.is_empty() {
            app.language.text("无", "none", "なし").to_owned()
        } else {
            levels
        }
    ))
}

pub(super) async fn set(
    level: Option<String>,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
) -> Result<String> {
    let home = runtime.home.clone();
    let profile = session
        .profile
        .clone()
        .or_else(|| runtime.runtime_submit.profile.clone());
    let model = session
        .model
        .clone()
        .or_else(|| runtime.runtime_submit.model.clone());
    let persisted_level = level.clone();
    apply_with_persistence(level, app, runtime, agent, async {
        super::runtime_ui::persist_missing_core_session(session, store)?;
        crate::daemon::ensure_runtime_session(
            &home,
            session.id,
            &session.workspace,
            profile,
            model,
        )
        .await?;
        crate::daemon::update_remote_session_reasoning_level(&home, session.id, persisted_level)
            .await
    })
    .await
}

async fn apply_with_persistence(
    level: Option<String>,
    app: &mut App,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
    persist: impl std::future::Future<Output = Result<()>>,
) -> Result<String> {
    if let Some(level) = &level {
        let mapped = runtime
            .provider_config
            .reasoning
            .as_ref()
            .is_some_and(|profile| profile.levels.contains_key(level));
        anyhow::ensure!(
            mapped,
            "reasoning level `{level}` is not configured for the current Provider"
        );
    }
    let mut config = runtime.provider_config.clone();
    config.reasoning_level = level.clone();
    let provider = willdeep_core::provider::build_provider(config.clone())?;
    // Validate before polling the persistence future; rejected commands must not
    // replace a previously accepted choice. Runtime snapshots the level at claim.
    persist.await?;
    runtime.reasoning_level = level.clone();
    if app.running {
        // The durable choice is already live for the next claim. Only the local
        // main-model handle waits for this turn, so its in-flight calls stay put.
        app.pending_reasoning_level = Some(level.clone());
        return Ok(match level {
            Some(level) => app.language.text(
                "思考等级已设为 {level}，下一任务生效；当前任务保持不变",
                "Reasoning level set to {level} for the next task; the current task is unchanged",
                "思考レベルを {level} に設定しました。次のタスクから適用し、現在のタスクは変更しません",
            ).replace("{level}", &level),
            None => app.language.text(
                "已恢复 Provider 默认思考等级，下一任务生效；当前任务保持不变",
                "Provider default restored for the next task; the current task is unchanged",
                "Provider の既定思考レベルに戻しました。次のタスクから適用し、現在のタスクは変更しません",
            ).to_owned(),
        });
    }
    agent.main_model().set_provider(provider)?;
    runtime.provider_config = config;
    Ok(applied_message(level.as_deref(), app.language))
}

fn applied_message(level: Option<&str>, language: Language) -> String {
    match level {
        Some(level) => format!(
            "{}: {level}",
            language.text(
                "思考等级已切换",
                "Reasoning level switched",
                "思考レベルを切り替えました"
            )
        ),
        None => language
            .text(
                "已恢复 Provider 默认思考等级",
                "Restored the Provider's default reasoning level",
                "Provider の既定思考レベルに戻しました",
            )
            .to_owned(),
    }
}

pub(super) async fn sync_pending(
    session: &Session,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
    language: Language,
) -> Result<String> {
    // Read the latest durable value; another client may have changed it since
    // this TUI accepted the command. Never persist the old pending value again.
    sync_session(session, runtime, agent).await?;
    Ok(applied_message(
        runtime.reasoning_level.as_deref(),
        language,
    ))
}

// A reasoning choice creates Runtime metadata before the first task is submitted.
// `runtime_managed` only marks task ownership, so it cannot gate restoring preferences.
fn restored_level(
    session: &Session,
    states: &[crate::daemon::tui_bridge::RemoteSessionState],
) -> Result<Option<String>> {
    match states.iter().find(|state| state.id == session.id) {
        Some(state) => Ok(state.reasoning_level.clone()),
        None if !session.runtime_managed => Ok(None),
        None => anyhow::bail!("Runtime Session not found"),
    }
}

pub(super) fn should_sync_on_startup(session: &Session, store: &SessionStore) -> bool {
    session.runtime_managed || !session.messages.is_empty() || store.load(session.id).is_ok()
}

pub(super) async fn sync_session(
    session: &Session,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
) -> Result<()> {
    let states = crate::daemon::remote_session_states(&runtime.home).await?;
    sync_from_states(session, runtime, agent, &states)
}

fn sync_from_states(
    session: &Session,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
    states: &[crate::daemon::tui_bridge::RemoteSessionState],
) -> Result<()> {
    let level = restored_level(session, states)?;
    let mut config = runtime.provider_config.clone();
    let configured_level = level.as_ref().filter(|level| {
        config
            .reasoning
            .as_ref()
            .is_some_and(|profile| profile.levels.contains_key(*level))
    });
    config.reasoning_level = configured_level.cloned();
    let provider = willdeep_core::provider::build_provider(config.clone())?;
    agent.main_model().set_provider(provider)?;
    runtime.provider_config = config;
    runtime.reasoning_level = level;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(id: uuid::Uuid, level: Option<&str>) -> crate::daemon::tui_bridge::RemoteSessionState {
        crate::daemon::tui_bridge::RemoteSessionState {
            id,
            archived: false,
            active: false,
            active_turn_id: None,
            profile: Some("preview".to_owned()),
            model: None,
            reasoning_level: level.map(str::to_owned),
            approval_mode: None,
        }
    }

    #[test]
    fn resume_restores_override_before_first_task_and_respects_clear() {
        let session = Session::new(std::env::temp_dir(), Some("preview".to_owned()), "");
        assert!(!session.runtime_managed);
        assert!(session.messages.is_empty());
        for level in [Some("deep"), None] {
            assert_eq!(
                restored_level(&session, &[state(session.id, level)]).unwrap(),
                level.map(str::to_owned),
            );
        }
    }

    #[test]
    fn fresh_session_does_not_inherit_another_sessions_override() {
        let mut session = Session::new(std::env::temp_dir(), None, "");
        let states = [state(uuid::Uuid::new_v4(), Some("deep"))];
        assert_eq!(restored_level(&session, &states).unwrap(), None);
        session.runtime_managed = true;
        assert!(restored_level(&session, &states).is_err());
    }

    #[test]
    fn brand_new_unpersisted_session_skips_startup_sync_until_saved() {
        let root = std::env::temp_dir().join(format!("reasoning-startup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionStore::new(&root);
        let mut session = Session::new(root.clone(), None, "");
        assert!(!should_sync_on_startup(&session, &store));
        store.save(&mut session).unwrap();
        assert!(should_sync_on_startup(&session, &store));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn explicit_set_supports_names_that_are_also_commands() {
        for (input, expected) in [
            ("/reasoning", ReasoningCommand::Show),
            ("/reasoning show", ReasoningCommand::Show),
            ("/reasoning deep", ReasoningCommand::Set("deep".into())),
            ("/reasoning default", ReasoningCommand::Default),
            ("/reasoning AUTO", ReasoningCommand::Default),
            ("/reasoning reset", ReasoningCommand::Default),
            ("/reasoning set auto", ReasoningCommand::Set("auto".into())),
            (
                "/reasoning set default",
                ReasoningCommand::Set("default".into()),
            ),
            (
                "/reasoning set reset",
                ReasoningCommand::Set("reset".into()),
            ),
            ("/reasoning set show", ReasoningCommand::Set("show".into())),
            ("/reasoning set Auto", ReasoningCommand::Set("Auto".into())),
            ("/reasoning set", ReasoningCommand::Usage),
            ("/reasoning set deep extra", ReasoningCommand::Usage),
            ("/reasoning deep extra", ReasoningCommand::Usage),
        ] {
            assert_eq!(parse(input), Some(expected), "{input}");
        }
        assert_eq!(parse("/reasonings"), None);
    }

    struct Fixture {
        root: std::path::PathBuf,
        runtime: TuiRuntime,
        agent: Arc<Agent>,
        sessions: crate::daemon::reasoning_test_support::ReasoningSessionFixture,
        session: Session,
    }

    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("reasoning-command-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let sessions =
                crate::daemon::reasoning_test_support::ReasoningSessionFixture::new(&root).unwrap();
            sessions.set(Some("balanced")).unwrap();
            let session = SessionStore::new(&root).load(sessions.id).unwrap();
            let mut config = willdeep_core::provider::ProviderConfig::new(
                willdeep_core::provider::ProviderKind::OpenAiCompatible,
                willdeep_core::provider::ApiDialect::ChatCompletions,
                "https://provider.example/v1",
                "test-only",
                "test-model",
            );
            config.reasoning = Some(willdeep_core::provider::ReasoningProfile {
                default_level: Some("balanced".into()),
                parameter: None,
                levels: [
                    ("balanced".into(), serde_json::json!("medium")),
                    ("deep".into(), serde_json::json!("high")),
                ]
                .into(),
            });
            config.reasoning_level = Some("balanced".into());
            let agent = Arc::new(Agent::new(
                willdeep_core::provider::build_provider(config.clone()).unwrap(),
                willdeep_core::ToolRegistry::new(&root, willdeep_core::ApprovalMode::Strict)
                    .unwrap(),
                willdeep_core::AgentConfig {
                    max_turns: 2,
                    system_prompt: String::new(),
                    context_window: 128_000,
                    token_budget: None,
                },
            ));
            let (tx, rx) = mpsc::unbounded_channel();
            let runtime = TuiRuntime {
                home: root.clone(),
                notifier: crate::notify::Notifier::disabled(),
                skills: Arc::new(SkillCatalog::default()),
                kernel: willdeep_core::EventKernel::new(),
                kernel_store: willdeep_core::kernel_store::KernelStore::new(&root),
                detached_jobs: willdeep_core::DetachedJobStore::new(&root),
                context_window: 128_000,
                background_tasks: Arc::new(BackgroundTaskRegistry::default()),
                runtime_submit: crate::daemon::RuntimeSubmitOptions {
                    workspace: root.clone(),
                    profile: Some("preview".into()),
                    model: Some("test-model".into()),
                    config: None,
                },
                provider_config: config,
                reasoning_level: Some("balanced".into()),
                local_workspace: root.clone(),
                tx,
                rx,
            };
            Self {
                root,
                runtime,
                agent,
                sessions,
                session,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[tokio::test]
    async fn running_change_is_persisted_before_next_claim_without_changing_current_task() {
        let mut fixture = Fixture::new();
        let first = fixture.sessions.enqueue("first").unwrap();
        let second = fixture.sessions.enqueue("second").unwrap();
        let in_flight = fixture.sessions.claim().unwrap();
        let current_provider = fixture.agent.main_model().current().unwrap();
        let mut app = App::new(Vec::new(), Language::En);
        app.running = true;
        let sessions = &fixture.sessions;
        let id = fixture.session.id;
        apply_with_persistence(
            Some("deep".into()),
            &mut app,
            &mut fixture.runtime,
            &fixture.agent,
            async {
                sessions.set(Some("deep"))?;
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(sessions.level().unwrap().as_deref(), Some("deep"));
        assert_eq!(in_flight.1.as_deref(), Some("balanced"));
        assert!(Arc::ptr_eq(
            &current_provider,
            &fixture.agent.main_model().current().unwrap()
        ));
        assert_eq!(
            fixture.runtime.provider_config.reasoning_level.as_deref(),
            Some("balanced")
        );
        assert_eq!(app.pending_reasoning_level, Some(Some("deep".into())));

        // The daemon claims B before the TUI has consumed A's terminal event.
        sessions.cancel(first).unwrap();
        let claimed = sessions.claim().unwrap();
        assert_eq!(claimed.0, second);
        assert_eq!(claimed.1.as_deref(), Some("deep"));
        assert!(app.running);

        // A later Web choice wins when the TUI finally refreshes its local handle.
        sessions.set(None).unwrap();
        sync_from_states(
            &fixture.session,
            &mut fixture.runtime,
            &fixture.agent,
            &[state(id, None)],
        )
        .unwrap();
        assert_eq!(fixture.runtime.reasoning_level, None);
        assert_eq!(fixture.runtime.provider_config.reasoning_level, None);
        assert_eq!(sessions.level().unwrap(), None);
        assert_eq!(claimed.1.as_deref(), Some("deep"));
    }

    #[tokio::test]
    async fn invalid_running_choice_keeps_the_previous_accepted_choice() {
        let mut fixture = Fixture::new();
        let mut app = App::new(Vec::new(), Language::En);
        app.running = true;
        app.pending_reasoning_level = Some(Some("deep".into()));
        fixture.runtime.reasoning_level = Some("deep".into());
        fixture.sessions.set(Some("deep")).unwrap();
        let result = apply_with_persistence(
            Some("depe".into()),
            &mut app,
            &mut fixture.runtime,
            &fixture.agent,
            async { panic!("invalid choices must be rejected before persistence") },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(app.pending_reasoning_level, Some(Some("deep".into())));
        assert_eq!(fixture.runtime.reasoning_level.as_deref(), Some("deep"));
        assert_eq!(fixture.sessions.level().unwrap().as_deref(), Some("deep"));
    }

    #[tokio::test]
    async fn failed_persistence_does_not_acknowledge_or_queue_a_change() {
        let mut fixture = Fixture::new();
        let mut app = App::new(Vec::new(), Language::En);
        app.running = true;
        let result = apply_with_persistence(
            Some("deep".into()),
            &mut app,
            &mut fixture.runtime,
            &fixture.agent,
            async { anyhow::bail!("persistence failed") },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(app.pending_reasoning_level, None);
        assert_eq!(fixture.runtime.reasoning_level.as_deref(), Some("balanced"));
        assert_eq!(
            fixture.sessions.level().unwrap().as_deref(),
            Some("balanced")
        );
    }
}
