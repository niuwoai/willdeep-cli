//! Reuses the permissions popup both before TUI startup and inside its event loop.
use super::*;
use crossterm::event;
use willdeep_core::ApprovalMode;

pub(super) enum GuideNext {
    Startup,
    Workspace(uuid::Uuid),
    Session { id: String, related: bool },
}

pub(super) struct WorkspaceApprovalGuide {
    pub(super) workspace: crate::daemon::RuntimeWorkspace,
    pub(super) next: GuideNext,
}

impl App {
    pub(super) fn open_workspace_approval(
        &mut self,
        workspace: crate::daemon::RuntimeWorkspace,
        next: GuideNext,
    ) {
        self.open_permission_picker(false);
        self.workspace_picker = None;
        self.permission_picker = Some(PermissionPickerState {
            selected: 1, // Strict / Smart / Full access; Smart always starts focused.
            confirming: false,
            guide: Some(WorkspaceApprovalGuide { workspace, next }),
        });
    }

    pub(super) fn set_effective_approval(&mut self, mode: ApprovalMode) {
        if let Some(handle) = &self.approval_handle {
            handle.set(mode);
        }
        self.approval_mode = mode;
        // The first Runtime submission must carry this exact local choice.
        // Restoring the UI alone is not proof that a session override was saved.
        self.approval_synced_session = None;
    }
}

struct StartupScreen;
impl Drop for StartupScreen {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            terminal::LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = terminal::disable_raw_mode();
    }
}

/// Only called before the main TUI exists. In-TUI navigation uses its normal loop.
pub(crate) fn select_workspace_approval(
    workspace: &crate::daemon::RuntimeWorkspace,
    language: Language,
) -> Result<Option<ApprovalMode>> {
    terminal::enable_raw_mode()?;
    let _screen = StartupScreen;
    execute!(
        io::stdout(),
        terminal::EnterAlternateScreen,
        EnableBracketedPaste
    )?;
    let mut term = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut app = App::new(Vec::new(), language);
    app.open_workspace_approval(workspace.clone(), GuideNext::Startup);
    loop {
        term.draw(|frame| render_permission_picker(frame, &mut app))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(None);
            }
            match app.handle_permission_picker_key(key) {
                PermissionPickerAction::None => {}
                PermissionPickerAction::Close => return Ok(None),
                PermissionPickerAction::Apply(mode) => return Ok(Some(mode)),
            }
        }
    }
}

pub(super) fn configured_mode(
    config: Option<&std::path::Path>,
) -> Result<Option<crate::daemon::WorkspaceAccess>> {
    let loaded = crate::config::LoadedConfig::load(config.filter(|path| path.exists()))?;
    loaded
        .file
        .agent
        .approval
        .as_ref()
        .map(|_| {
            crate::harness::configured_approval_mode(&loaded.file.agent)
                .map(crate::daemon::WorkspaceAccess::from_approval_mode)
        })
        .transpose()
}

/// The session's explicit choice wins; agent.approval is the next fallback.
pub(super) fn explicit_mode(
    session_override: Option<crate::daemon::WorkspaceAccess>,
    configured_mode: Option<crate::daemon::WorkspaceAccess>,
) -> Option<crate::daemon::WorkspaceAccess> {
    session_override.or(configured_mode)
}

/// Workspace registration is the fallback. ReadOnly remains a hard ceiling.
pub(super) fn effective_mode(
    workspace: &crate::daemon::RuntimeWorkspace,
    explicit_mode: Option<crate::daemon::WorkspaceAccess>,
) -> ApprovalMode {
    workspace
        .access
        .with_session_override(explicit_mode)
        .approval_mode()
}

/// Returns no mode while a guide is open, before saving, activating or unarchiving.
pub(super) async fn prepare_session(
    app: &mut App,
    target: &Session,
    runtime: &TuiRuntime,
    current_workspace: &std::path::Path,
    related: bool,
) -> Result<Option<ApprovalMode>> {
    let session_override =
        crate::workspace_approval_startup::session_override(&runtime.home, Some(target)).await?;
    let configured_mode = configured_mode(
        target
            .config
            .as_deref()
            .or(runtime.runtime_submit.config.as_deref()),
    )?;
    let explicit_mode = explicit_mode(session_override, configured_mode);
    // Switching among local sessions in the already-open directory must not start
    // or query Runtime just to restore the current TUI approval mode.
    if !target.runtime_managed
        && target.parent_session_id.is_none()
        && target.workspace.canonicalize()? == current_workspace.canonicalize()?
    {
        return Ok(Some(
            crate::daemon::WorkspaceAccess::from_approval_mode(app.approval_mode)
                .with_session_override(explicit_mode)
                .approval_mode(),
        ));
    }
    // Delegated worktrees inherit their existing session policy; they are not new
    // user-selected directories and may not exist until prepared by Runtime.
    let workspace = if target.parent_session_id.is_none() {
        Some(crate::daemon::ensure_remote_workspace(&runtime.home, &target.workspace).await?)
    } else {
        crate::daemon::remote_workspaces(&runtime.home)
            .await?
            .into_iter()
            .find(|workspace| workspace.root == target.workspace)
    };
    if let Some(workspace) = workspace {
        if crate::workspace_approval_startup::needs_guide(
            &workspace,
            true,
            explicit_mode.map(crate::daemon::WorkspaceAccess::approval_mode),
        ) {
            app.open_workspace_approval(
                workspace,
                GuideNext::Session {
                    id: target.id.to_string(),
                    related,
                },
            );
            return Ok(None);
        }
        return Ok(Some(effective_mode(&workspace, explicit_mode)));
    }
    Ok(Some(
        crate::daemon::WorkspaceAccess::from_approval_mode(app.approval_mode)
            .with_session_override(explicit_mode)
            .approval_mode(),
    ))
}

pub(super) async fn complete(
    mode: ApprovalMode,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
) -> Result<()> {
    let guide = app
        .permission_picker
        .as_ref()
        .and_then(|picker| picker.guide.as_ref())
        .context("directory approval guide is not open")?;
    crate::daemon::set_remote_workspace_approval_mode(
        &runtime.home,
        &guide.workspace.root,
        mode,
        mode == ApprovalMode::FullAccess,
    )
    .await?;
    let guide = app
        .permission_picker
        .take()
        .and_then(|picker| picker.guide)
        .context("directory approval guide disappeared")?;
    match guide.next {
        GuideNext::Startup => {}
        GuideNext::Workspace(id) => app.pending_workspace_switch = Some(id),
        GuideNext::Session { id, related } => {
            let message = if related {
                session_commands::switch_related(app, session, store, runtime, &id).await?
            } else {
                session_commands::switch(app, session, store, runtime, &id).await?
            };
            app.notice = Some(message);
        }
    }
    Ok(())
}

pub(super) async fn handle_key(
    key: KeyEvent,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
    agent: &Arc<Agent>,
) {
    let language = app.language;
    match app.handle_permission_picker_key(key) {
        PermissionPickerAction::None => {}
        PermissionPickerAction::Close => app.permission_picker = None,
        PermissionPickerAction::Apply(mode) => {
            if app
                .permission_picker
                .as_ref()
                .is_some_and(|picker| picker.guide.is_some())
            {
                if let Err(error) = complete(mode, app, session, store, runtime).await {
                    app.notice = Some(format!(
                        "{}: {error:#}",
                        language.text(
                            "保存目录审批模式失败",
                            "Could not save directory approval mode",
                            "ディレクトリの承認モードを保存できません"
                        )
                    ));
                }
            } else {
                app.permission_picker = None;
                match permission_commands::apply(mode, app, session, runtime, agent).await {
                    Ok(message) => app.append_transcript(message),
                    Err(error) => app.append_transcript(format!(
                        "Error: {}: {error:#}",
                        language.text(
                            "切换审批模式失败",
                            "Approval mode switch failed",
                            "承認モードの切替に失敗"
                        )
                    )),
                }
            }
        }
    }
}

pub(super) fn pending_message(language: Language) -> String {
    language.workspace_approval_title().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn fixture() -> crate::daemon::RuntimeWorkspace {
        crate::daemon::RuntimeWorkspace {
            schema: 1,
            id: uuid::Uuid::new_v4(),
            name: "new".into(),
            root: "/new-project".into(),
            access: crate::daemon::WorkspaceAccess::Smart,
            approval_mode_configured: false,
            provider_profile: None,
            skills: vec![],
            mcp_servers: vec![],
            created_at: 0,
            updated_at: 0,
            active: false,
        }
    }

    fn strict_configured_mode() -> crate::daemon::WorkspaceAccess {
        let path = std::env::temp_dir().join(format!(
            "willdeep-tui-approval-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "[agent]\napproval = \"strict\"\n").unwrap();
        let configured = configured_mode(Some(&path))
            .unwrap()
            .expect("agent.approval is explicit");
        std::fs::remove_file(path).unwrap();
        configured
    }

    fn press(app: &mut App, code: KeyCode) -> PermissionPickerAction {
        app.handle_permission_picker_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn workspace_approval_enter_defaults_to_smart_even_from_full_access() {
        let mut app = App::new(vec![], Language::En);
        app.approval_mode = ApprovalMode::FullAccess;
        app.open_workspace_approval(fixture(), GuideNext::Startup);
        assert!(matches!(
            press(&mut app, KeyCode::Enter),
            PermissionPickerAction::Apply(ApprovalMode::Smart)
        ));
    }

    #[test]
    fn workspace_approval_strict_maps_directly_and_fourth_digit_does_not_elevate() {
        let mut app = App::new(vec![], Language::En);
        app.open_workspace_approval(fixture(), GuideNext::Startup);
        assert!(matches!(
            press(&mut app, KeyCode::Char('4')),
            PermissionPickerAction::None
        ));
        assert!(!app.permission_picker.as_ref().unwrap().confirming);
        assert!(matches!(
            press(&mut app, KeyCode::Char('1')),
            PermissionPickerAction::Apply(ApprovalMode::Strict)
        ));
    }

    #[test]
    fn workspace_approval_full_access_requires_y_and_escape_returns_before_cancel() {
        let mut app = App::new(vec![], Language::En);
        app.open_workspace_approval(fixture(), GuideNext::Startup);
        assert!(matches!(
            press(&mut app, KeyCode::Char('3')),
            PermissionPickerAction::None
        ));
        assert!(app.permission_picker.as_ref().unwrap().confirming);
        assert!(matches!(
            press(&mut app, KeyCode::Esc),
            PermissionPickerAction::None
        ));
        assert!(!app.permission_picker.as_ref().unwrap().confirming);
        assert!(matches!(
            press(&mut app, KeyCode::Esc),
            PermissionPickerAction::Close
        ));

        app.open_workspace_approval(fixture(), GuideNext::Startup);
        assert!(matches!(
            press(&mut app, KeyCode::Char('3')),
            PermissionPickerAction::None
        ));
        assert!(matches!(
            press(&mut app, KeyCode::Enter),
            PermissionPickerAction::None
        ));
        assert!(matches!(
            press(&mut app, KeyCode::Char('y')),
            PermissionPickerAction::Apply(ApprovalMode::FullAccess)
        ));
    }

    #[test]
    fn workspace_approval_cancellation_does_not_schedule_navigation_or_change_handle() {
        let mut app = App::new(vec![], Language::En);
        let handle = willdeep_core::SharedApprovalMode::new(ApprovalMode::Strict);
        app.approval_handle = Some(handle.clone());
        app.open_workspace_approval(fixture(), GuideNext::Workspace(uuid::Uuid::new_v4()));
        assert!(matches!(
            press(&mut app, KeyCode::Esc),
            PermissionPickerAction::Close
        ));
        app.permission_picker = None;
        assert!(app.pending_workspace_switch.is_none());
        assert!(app.pending_session_switch.is_none());
        assert_eq!(handle.get(), ApprovalMode::Strict);
    }

    #[test]
    fn workspace_approval_restored_mode_updates_the_execution_handle() {
        let mut app = App::new(vec![], Language::En);
        let handle = willdeep_core::SharedApprovalMode::new(ApprovalMode::FullAccess);
        app.approval_handle = Some(handle.clone());
        app.approval_synced_session = Some(uuid::Uuid::new_v4());
        app.set_effective_approval(ApprovalMode::Strict);
        assert_eq!(handle.get(), ApprovalMode::Strict);
        assert_eq!(app.approval_mode, handle.get());
        assert_eq!(
            app.approval_synced_session, None,
            "first Runtime turn must sync this mode"
        );
    }

    #[test]
    fn new_session_without_override_uses_explicit_strict_config_over_workspace_full_access() {
        let mut workspace = fixture();
        workspace.access = crate::daemon::WorkspaceAccess::FullAccess;
        workspace.approval_mode_configured = true;

        let explicit_mode = explicit_mode(None, Some(strict_configured_mode()));

        assert_eq!(
            effective_mode(&workspace, explicit_mode),
            ApprovalMode::Strict
        );
    }

    #[test]
    fn session_switch_without_override_uses_explicit_strict_config_over_workspace_full_access() {
        let mut workspace = fixture();
        workspace.access = crate::daemon::WorkspaceAccess::FullAccess;
        workspace.approval_mode_configured = true;

        let explicit_mode = explicit_mode(None, Some(strict_configured_mode()));

        assert_eq!(
            effective_mode(&workspace, explicit_mode),
            ApprovalMode::Strict
        );
    }

    #[test]
    fn explicit_session_mode_wins_and_read_only_stays_a_hard_ceiling() {
        let session_override = Some(crate::daemon::WorkspaceAccess::FullAccess);
        let explicit_mode = explicit_mode(
            session_override,
            Some(crate::daemon::WorkspaceAccess::Strict),
        );
        let mut workspace = fixture();
        workspace.approval_mode_configured = true;
        assert_eq!(
            effective_mode(&workspace, explicit_mode),
            ApprovalMode::FullAccess
        );

        workspace.access = crate::daemon::WorkspaceAccess::ReadOnly;
        assert_eq!(
            effective_mode(&workspace, Some(crate::daemon::WorkspaceAccess::FullAccess)),
            ApprovalMode::ReadOnly
        );
    }

    #[test]
    fn workspace_mode_remains_the_fallback_without_an_explicit_agent_mode() {
        let mut workspace = fixture();
        workspace.access = crate::daemon::WorkspaceAccess::FullAccess;
        workspace.approval_mode_configured = true;

        assert_eq!(effective_mode(&workspace, None), ApprovalMode::FullAccess);
    }

    #[test]
    fn workspace_approval_popup_shows_directory_and_scenarios_in_each_language() {
        for language in [Language::ZhCn, Language::En, Language::Ja] {
            let mut app = App::new(vec![], language);
            app.open_workspace_approval(fixture(), GuideNext::Startup);
            let mut term = Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
            term.draw(|frame| render_permission_picker(frame, &mut app))
                .unwrap();
            // Wide graphemes mark their continuation cells as `skip`; those cells
            // are buffer layout, not spaces in the rendered title.
            let text = term
                .backend()
                .buffer()
                .content()
                .iter()
                .filter(|cell| !cell.skip)
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(text.contains("/new-project"));
            let compact = |value: &str| {
                value
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect::<String>()
            };
            assert!(
                compact(&text).contains(&compact(language.workspace_approval_title())),
                "language={language:?}; rendered={text:?}; expected={:?}",
                language.workspace_approval_title()
            );
            assert!(!text.contains("1-4"));
        }
    }
}
