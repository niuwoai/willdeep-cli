use super::*;

const MODEL_LOOKUP_DEBOUNCE: Duration = Duration::from_millis(120);

pub(super) fn receive_models(
    app: &mut App,
    request_id: uuid::Uuid,
    result: std::result::Result<Vec<String>, String>,
) {
    if app
        .model_picker
        .as_ref()
        .is_some_and(|picker| picker.request_id == request_id)
    {
        app.set_model_picker_result(result);
    }
}

pub(super) fn discover_local(runtime: &TuiRuntime) {
    let config = runtime.runtime_submit.config.clone();
    let tx = runtime.tx.clone();
    tokio::spawn(async move {
        let result = crate::local_provider_discovery::discover(config.as_deref())
            .await
            .map_err(|error| error.to_string());
        let _ = tx.send(UiMessage::LocalProvidersDiscovered(result));
    });
}

pub(super) fn receive_local(
    app: &mut App,
    runtime: &TuiRuntime,
    result: std::result::Result<usize, String>,
) {
    match result {
        Ok(0) => {}
        Ok(_) => {
            if let Some(picker) = app.model_picker.as_mut()
                && let Ok(loaded) =
                    crate::config::LoadedConfig::load(runtime.runtime_submit.config.as_deref())
            {
                picker.profiles = loaded.file.providers.keys().cloned().collect();
                app.refresh_provider_matches();
            }
        }
        Err(error) => app.notice = Some(format!("Local Provider discovery: {error}")),
    }
}

pub(super) async fn handle(
    action: ModelPickerAction,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
) {
    let result = match action {
        ModelPickerAction::None => return,
        ModelPickerAction::Close => {
            app.model_picker = None;
            return;
        }
        ModelPickerAction::Provider(profile) => {
            if profile.is_some()
                && let Some(picker) = app.model_picker.as_mut()
            {
                picker.choosing_provider = false;
            }
            if let Err(error) = browse(app, runtime, profile) {
                app.set_model_picker_result(Err(error.to_string()));
            }
            return;
        }
        ModelPickerAction::Preview(profile) => {
            if let Err(error) = browse(app, runtime, Some(profile)) {
                app.set_model_picker_result(Err(error.to_string()));
            }
            return;
        }
        ModelPickerAction::Refresh => {
            discover_local(runtime);
            let profile = app
                .model_picker
                .as_ref()
                .and_then(|picker| picker.profile.clone());
            let result = (|| -> Result<_> {
                if profile == current_profile(runtime)? {
                    return Ok(runtime.provider_config.clone());
                }
                let loaded =
                    crate::config::LoadedConfig::load(runtime.runtime_submit.config.as_deref())?;
                crate::provider_config_from_profile(
                    &loaded.file,
                    profile.as_deref().context("Provider is not selected")?,
                )
            })();
            match result {
                Ok(config) => load_models(app, config, &runtime.tx),
                Err(error) => app.set_model_picker_result(Err(error.to_string())),
            }
            return;
        }
        ModelPickerAction::Select(model) => {
            select(&model, app, session, store, runtime, agent).await
        }
    };
    match result {
        Ok(message) => {
            app.model_picker = None;
            app.append_transcript(format!("System: {message}"));
        }
        Err(error) => {
            app.notice = Some(format!(
                "{}: {error:#}",
                app.language.text("切换失败", "Switch failed", "切替失敗")
            ))
        }
    }
}

pub(super) fn current_profile(runtime: &TuiRuntime) -> Result<Option<String>> {
    let loaded = crate::config::LoadedConfig::load(runtime.runtime_submit.config.as_deref())?;
    Ok(runtime
        .runtime_submit
        .profile
        .clone()
        .or(loaded.file.default_provider)
        .or_else(|| {
            (loaded.file.providers.len() == 1)
                .then(|| loaded.file.providers.keys().next().cloned())
                .flatten()
        }))
}

pub(super) fn initialize(app: &mut App, runtime: &TuiRuntime) -> Result<()> {
    let loaded = crate::config::LoadedConfig::load(runtime.runtime_submit.config.as_deref())?;
    let profile = current_profile(runtime)?;
    let picker = app
        .model_picker
        .as_mut()
        .context("model picker is closed")?;
    picker.active_profile = profile.clone();
    picker.provider_label =
        willdeep_core::provider::ProviderIdentity::from_config(&runtime.provider_config).provider;
    picker.profile = profile;
    picker.profiles = loaded.file.providers.keys().cloned().collect();
    picker.profiles.sort();
    app.refresh_provider_matches();
    load_models(app, runtime.provider_config.clone(), &runtime.tx);
    Ok(())
}

fn load_models(app: &mut App, mut config: willdeep_core::provider::ProviderConfig, tx: &TuiSender) {
    let picker = app.model_picker.as_mut().unwrap();
    drop(picker.load_task.take());
    picker.request_id = uuid::Uuid::new_v4();
    let request_id = picker.request_id;
    picker.loading = true;
    picker.error = None;
    picker.models = picker
        .cache
        .get(&picker.profile)
        .cloned()
        .unwrap_or_default();
    picker.filtered.clear();
    picker.selected = 0;
    config.request_timeout_secs = 20;
    let tx = tx.clone();
    picker.load_task = Some(model_commands::ModelLookupTask(tokio::spawn(async move {
        // Rapid keyboard navigation only requests the last highlighted Provider.
        tokio::time::sleep(MODEL_LOOKUP_DEBOUNCE).await;
        let result = willdeep_core::provider::list_models(&config)
            .await
            .map_err(|error| error.to_string());
        let _ = tx.send(UiMessage::ModelsLoaded { request_id, result });
    })));
    app.refresh_model_picker_matches();
}

pub(super) fn browse(app: &mut App, runtime: &TuiRuntime, profile: Option<String>) -> Result<()> {
    if let Some(profile) = profile {
        if app
            .model_picker
            .as_ref()
            .is_some_and(|picker| picker.profile.as_deref() == Some(&profile))
        {
            return Ok(());
        }
        let picker = app
            .model_picker
            .as_mut()
            .context("model picker is closed")?;
        drop(picker.load_task.take());
        picker.request_id = uuid::Uuid::new_v4();
        picker.profile = Some(profile.clone());
        picker.models.clear();
        picker.filtered.clear();
        let loaded = crate::config::LoadedConfig::load(runtime.runtime_submit.config.as_deref())?;
        let config = if current_profile(runtime)?.as_deref() == Some(profile.as_str()) {
            runtime.provider_config.clone()
        } else {
            crate::provider_config_from_profile(&loaded.file, &profile)?
        };
        let picker = app
            .model_picker
            .as_mut()
            .context("model picker is closed")?;
        picker.profile = Some(profile);
        app.refresh_provider_matches();
        load_models(app, config, &runtime.tx);
    } else {
        let picker = app
            .model_picker
            .as_mut()
            .context("model picker is closed")?;
        picker.choosing_provider = true;
        app.refresh_provider_matches();
    }
    Ok(())
}

pub(super) async fn select(
    model: &str,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
) -> Result<String> {
    let profile = app
        .model_picker
        .as_ref()
        .and_then(|picker| picker.profile.clone());
    let current = current_profile(runtime)?;
    if let Some(profile) = profile.filter(|profile| current.as_ref() != Some(profile)) {
        switch(&profile, model, app, session, store, runtime, agent).await
    } else {
        switch_or_defer_model(model, app, session, store, runtime, agent).await
    }
}

pub(super) async fn switch(
    profile: &str,
    model: &str,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
) -> Result<String> {
    let loaded = crate::config::LoadedConfig::load(runtime.runtime_submit.config.as_deref())?;
    let mut config = crate::provider_config_from_profile(&loaded.file, profile)?;
    let model = model.trim();
    anyhow::ensure!(
        !model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control),
        "Invalid model"
    );
    config.model = model.to_owned();
    let provider = willdeep_core::provider::build_provider(config.clone())?;
    if app.running {
        let message = model_commands::defer_model_switch(app, model)?;
        app.pending_provider = Some(profile.to_owned());
        return Ok(format!("Provider: {profile} · {message}"));
    }
    runtime_ui::persist_missing_core_session(session, store)?;
    agent.main_model().current()?;
    crate::daemon::ensure_runtime_session(
        &runtime.home,
        session.id,
        &session.workspace,
        session.profile.clone(),
        session.model.clone(),
    )
    .await?;
    crate::daemon::tui_bridge::update_remote_session_provider(
        &runtime.home,
        session.id,
        profile,
        model,
    )
    .await?;
    *session = store
        .load(session.id)
        .context("reload configured Session")?;
    agent.main_model().set_provider(provider)?;
    runtime.runtime_submit.profile = Some(profile.to_owned());
    runtime.runtime_submit.model = Some(model.to_owned());
    runtime.provider_config = config;
    app.clear_input_suggestion();
    Ok(format!(
        "{}: {profile} / {model}",
        app.language.text(
            "Provider 与模型已切换",
            "Provider and model switched",
            "Provider とモデルを切替"
        )
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn running_turn_defers_connection_and_rejects_invalid_profiles_without_mutation() {
        let root = std::env::temp_dir().join(format!("provider-defer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("config.toml");
        std::fs::write(&config, "version = 1\ndefault_provider = 'alpha'\n[providers.alpha]\napi_base = 'http://127.0.0.1:1/alpha/v1'\napi_key = 'test-only-alpha'\nmodel = 'old'\n[providers.beta]\napi_base = 'http://127.0.0.1:1/beta/v1'\napi_key = 'test-only-beta'\nmodel = 'new'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let loaded = crate::config::LoadedConfig::load(Some(&config)).unwrap();
        let initial = crate::provider_config_from_profile(&loaded.file, "alpha").unwrap();
        let agent = Arc::new(Agent::new(
            willdeep_core::provider::build_provider(initial.clone()).unwrap(),
            willdeep_core::ToolRegistry::new(&root, willdeep_core::ApprovalMode::Strict).unwrap(),
            willdeep_core::AgentConfig {
                max_turns: 2,
                system_prompt: String::new(),
                context_window: 128_000,
                token_budget: None,
            },
        ));
        let (tx, rx) = mpsc::unbounded_channel();
        let mut runtime = TuiRuntime {
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
                profile: Some("alpha".into()),
                model: Some("old".into()),
                config: Some(config),
            },
            provider_config: initial,
            local_workspace: root.clone(),
            tx,
            rx,
        };
        let store = SessionStore::new(&root);
        let mut session = Session::new(root.clone(), Some("alpha".into()), "existing");
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(true, "in flight".into());
        switch(
            "beta",
            "new",
            &mut app,
            &mut session,
            &store,
            &mut runtime,
            &agent,
        )
        .await
        .unwrap();
        assert_eq!(app.pending_provider.as_deref(), Some("beta"));
        assert_eq!(app.pending_model.as_deref(), Some("new"));
        assert_eq!(runtime.runtime_submit.profile.as_deref(), Some("alpha"));
        assert_eq!(session.profile.as_deref(), Some("alpha"));
        assert_eq!(agent.main_model().model().as_deref(), Some("old"));
        assert!(!root.join("runtime/daemon.json").exists());
        assert!(
            switch(
                "missing",
                "new",
                &mut app,
                &mut session,
                &store,
                &mut runtime,
                &agent
            )
            .await
            .is_err()
        );
        assert_eq!(app.pending_provider.as_deref(), Some("beta"));
        // Choosing another model on the active profile cancels the queued provider switch.
        app.open_model_picker("old".into());
        app.model_picker.as_mut().unwrap().profile = Some("alpha".into());
        select(
            "different-model",
            &mut app,
            &mut session,
            &store,
            &mut runtime,
            &agent,
        )
        .await
        .unwrap();
        assert!(app.pending_provider.is_none());
        assert_eq!(app.pending_model.as_deref(), Some("different-model"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provider_choices_work_with_keyboard_and_mouse_and_do_not_commit_a_switch() {
        let mut app = App::new(Vec::new(), Language::ZhCn);
        app.open_model_picker("old-model".into());
        let picker = app.model_picker.as_mut().unwrap();
        picker.profile = Some("old".into());
        picker.active_profile = Some("old".into());
        picker.profiles = vec!["old".into(), "magpie".into()];
        picker.models = vec!["old-model".into()];
        picker.loading = false;
        picker.choosing_provider = true;
        app.refresh_provider_matches();
        app.handle_model_picker_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(
            matches!(app.handle_model_picker_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), ModelPickerAction::Provider(Some(profile)) if profile == "magpie")
        );
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| render_model_picker(f, &mut app)).unwrap();
        let row = app
            .model_provider_hits
            .iter()
            .find(|(_, index)| *index == 1)
            .unwrap()
            .0;
        assert!(
            matches!(app.activate_model_picker_at(row.x, row.y), Some(ModelPickerAction::Preview(profile)) if profile == "magpie")
        );
        assert!(matches!(
            app.handle_model_picker_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            ModelPickerAction::Provider(None)
        ));
        assert_eq!(
            app.model_picker.as_ref().unwrap().active_profile.as_deref(),
            Some("old")
        );
    }

    #[test]
    fn late_model_lists_never_replace_a_new_provider_and_load_in_either_pane() {
        let mut app = App::new(Vec::new(), Language::En);
        app.open_model_picker("old".into());
        let old = uuid::Uuid::new_v4();
        let current = uuid::Uuid::new_v4();
        app.model_picker.as_mut().unwrap().request_id = current;
        receive_models(&mut app, old, Ok(vec!["wrong".into()]));
        assert!(app.model_picker.as_ref().unwrap().models.is_empty());
        receive_models(&mut app, current, Ok(vec!["right".into()]));
        assert_eq!(app.model_picker.as_ref().unwrap().models, ["right"]);
        app.model_picker.as_mut().unwrap().choosing_provider = true;
        receive_models(&mut app, current, Ok(vec!["refreshed".into()]));
        assert_eq!(app.model_picker.as_ref().unwrap().models, ["refreshed"]);
    }

    #[test]
    fn model_lookup_failure_still_allows_a_manual_model_name() {
        let mut app = App::new(Vec::new(), Language::En);
        app.open_model_picker("old".into());
        app.set_model_picker_result(Err("no models endpoint".into()));
        app.model_picker
            .as_mut()
            .unwrap()
            .editor
            .insert("manual-model");
        assert!(
            matches!(app.handle_model_picker_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), ModelPickerAction::Select(model) if model == "manual-model")
        );
    }
}
