//! 交互式启动桥接屏：在 `session_startup` / 审批引导结束后、`harness::build` 之前
//! 先进入 Alternate Screen 画一帧蓝鲸欢迎面，并在 `tui::run` 接管时平滑移交终端，
//! 避免选择「新建会话」后先退回空白普通终端苦等初始化。

use super::*;

pub struct StartupSplash {
    term: Option<Terminal<CrosstermBackend<io::Stdout>>>,
}

impl StartupSplash {
    pub fn show(
        workspace: &std::path::Path,
        ui: crate::config::UiSettings,
        approval_mode: Option<willdeep_core::ApprovalMode>,
        language: Language,
    ) -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(
            stdout,
            terminal::EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        ) {
            let _ = terminal::disable_raw_mode();
            return Err(error.into());
        }
        let mut term = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(term) => term,
            Err(error) => {
                restore_terminal_state();
                return Err(error.into());
            }
        };
        let (mut app, session) = prepare_splash_state(workspace, ui, approval_mode, language);
        let skills = SkillCatalog::default();
        let _ = redraw::redraw(&mut term, &mut app, &skills, &session);
        Ok(Self { term: Some(term) })
    }

    pub(super) fn into_terminal(mut self) -> Option<Terminal<CrosstermBackend<io::Stdout>>> {
        self.term.take()
    }
}

fn prepare_splash_state(
    workspace: &std::path::Path,
    ui: crate::config::UiSettings,
    approval_mode: Option<willdeep_core::ApprovalMode>,
    language: Language,
) -> (App, Session) {
    let mut app = App::new(Vec::new(), language);
    app.ui = ui;
    app.workspace = Some(workspace.to_path_buf());
    if let Some(mode) = approval_mode {
        app.approval_mode = mode;
    }
    app.ensure_welcome(workspace, language);
    app.notice = Some(
        language
            .text("正在准备会话…", "Preparing Session…", "セッションを準備中…")
            .to_owned(),
    );
    let session = Session::new(workspace.to_path_buf(), None, "");
    (app, session)
}

impl Drop for StartupSplash {
    fn drop(&mut self) {
        if let Some(mut term) = self.term.take() {
            let _ = execute!(
                term.backend_mut(),
                DisableMouseCapture,
                DisableBracketedPaste,
                terminal::LeaveAlternateScreen
            );
            let _ = term.show_cursor();
            let _ = terminal::disable_raw_mode();
        }
    }
}

fn restore_terminal_state() {
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        terminal::LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = terminal::disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splash_state_preloads_welcome_banner_and_preparing_notice() {
        let workspace = std::path::Path::new("/tmp/willdeep-workspace");
        let (app, session) = prepare_splash_state(
            workspace,
            crate::config::UiSettings::default(),
            Some(willdeep_core::ApprovalMode::Smart),
            Language::ZhCn,
        );
        assert_eq!(app.workspace.as_deref(), Some(workspace));
        assert_eq!(app.approval_mode, willdeep_core::ApprovalMode::Smart);
        assert_eq!(app.notice.as_deref(), Some("正在准备会话…"));
        assert_eq!(app.transcript.len(), 1);
        assert!(app.transcript[0].contains("WILLDEEP"));
        assert!(session.messages.is_empty());
    }
}
