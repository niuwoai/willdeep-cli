//! Keep terminal mouse ownership and the visible interaction mode in sync.
use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FrameAction {
    Draw,
    Frozen,
    Restore,
}

/// TUI startup and the external editor's resume path both enable capture.
pub(super) struct TerminalInput {
    mouse_captured: bool,
}

impl Default for TerminalInput {
    fn default() -> Self {
        Self {
            mouse_captured: true,
        }
    }
}

impl TerminalInput {
    /// Called before drawing, including after asynchronous gates arrive. A
    /// native selection gets one frame, then background events change state
    /// without touching the terminal until selection ends or a modal opens.
    pub(super) fn prepare_frame(
        &mut self,
        output: &mut impl io::Write,
        app: &mut App,
    ) -> io::Result<FrameAction> {
        if !native_selection_allowed(app) {
            if app.selection_mode || app.native_selection_mode || app.chat_selection.is_some() {
                app.exit_selection_mode();
            }
            app.input.clear_selection();
            app.pending_chat_click = None;
        }
        let captured = !app.native_selection_mode;
        if captured != self.mouse_captured {
            if captured {
                execute!(output, EnableMouseCapture)?;
            } else {
                execute!(output, DisableMouseCapture)?;
            }
            self.mouse_captured = captured;
            return Ok(if captured {
                FrameAction::Restore
            } else {
                FrameAction::Draw
            });
        }
        Ok(if captured {
            FrameAction::Draw
        } else {
            FrameAction::Frozen
        })
    }

    pub(super) fn redraw<B: ratatui::backend::Backend + io::Write>(
        &mut self,
        term: &mut Terminal<B>,
        app: &mut App,
        skills: &SkillCatalog,
        session: &Session,
    ) -> Result<()> {
        match self.prepare_frame(term.backend_mut(), app)? {
            FrameAction::Frozen => return Ok(()),
            FrameAction::Restore => term.clear()?,
            FrameAction::Draw => {}
        }
        redraw::redraw(term, app, skills, session)
    }

    pub(super) fn external_editor_resumed(&mut self, app: &mut App) {
        app.exit_selection_mode();
        self.mouse_captured = true;
    }
}

pub(super) fn native_selection_allowed(app: &App) -> bool {
    !app.has_open_overlay() && app.permission_picker.is_none() && app.prompt_history.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(app: &mut App) -> oneshot::Receiver<Option<String>> {
        let (sender, receiver) = oneshot::channel();
        app.enqueue_question(AskDialog {
            request: UserQuestion {
                question: "Choose a fixture option".to_owned(),
                options: vec!["First".to_owned(), "Second".to_owned()],
                multi_select: false,
            },
            selected: 0,
            checked: vec![false; 2],
            answer: PromptEditor::default(),
            sender,
        });
        receiver
    }

    #[test]
    fn native_selection_draws_once_then_freezes_until_restored() {
        let mut app = App::new(Vec::new(), Language::En);
        let mut input = TerminalInput::default();
        let mut output = Vec::new();
        assert_eq!(
            input.prepare_frame(&mut output, &mut app).unwrap(),
            FrameAction::Draw
        );
        assert!(output.is_empty());
        app.enter_native_selection_mode();
        assert_eq!(
            input.prepare_frame(&mut output, &mut app).unwrap(),
            FrameAction::Draw
        );
        let disabled = output.clone();
        assert!(String::from_utf8_lossy(&disabled).contains("\x1b[?1006l"));
        app.append_transcript("Background output continues to arrive".to_owned());
        assert_eq!(
            input.prepare_frame(&mut output, &mut app).unwrap(),
            FrameAction::Frozen
        );
        assert_eq!(output, disabled);
        app.exit_selection_mode();
        assert_eq!(
            input.prepare_frame(&mut output, &mut app).unwrap(),
            FrameAction::Restore
        );
        assert!(String::from_utf8_lossy(&output).contains("\x1b[?1006h"));
        let restored = output.clone();
        assert_eq!(
            input.prepare_frame(&mut output, &mut app).unwrap(),
            FrameAction::Draw
        );
        assert_eq!(output, restored);
    }

    #[test]
    fn async_question_restores_mouse_capture_before_its_first_frame() {
        let mut app = App::new(Vec::new(), Language::En);
        let mut input = TerminalInput::default();
        let mut output = Vec::new();
        app.enter_native_selection_mode();
        input.prepare_frame(&mut output, &mut app).unwrap();
        output.clear();
        let _receiver = question(&mut app);
        assert_eq!(
            input.prepare_frame(&mut output, &mut app).unwrap(),
            FrameAction::Restore
        );
        assert!(!app.selection_mode && !app.native_selection_mode);
        assert!(app.question.is_some());
        assert!(String::from_utf8_lossy(&output).contains("\x1b[?1006h"));
        assert!(!native_selection_allowed(&app));
    }

    #[test]
    fn async_approval_restores_capture_without_a_keyboard_event() {
        let mut app = App::new(Vec::new(), Language::En);
        let mut input = TerminalInput::default();
        let mut output = Vec::new();
        app.enter_native_selection_mode();
        input.prepare_frame(&mut output, &mut app).unwrap();
        let (sender, _receiver) = oneshot::channel();
        app.enqueue_approval(("Fixture approval".to_owned(), false, sender));
        output.clear();
        assert_eq!(
            input.prepare_frame(&mut output, &mut app).unwrap(),
            FrameAction::Restore
        );
        assert!(String::from_utf8_lossy(&output).contains("\x1b[?1006h"));
        assert!(!app.native_selection_mode);
    }

    #[test]
    fn gates_clear_chat_and_draft_selection_without_losing_the_draft() {
        let mut app = App::new(Vec::new(), Language::En);
        app.input.insert("Draft 中文 remains");
        app.input.begin_selection_visual(0, 0, 40);
        app.selection_mode = true;
        app.pending_chat_click = Some((2, None));
        let _receiver = question(&mut app);
        let mut input = TerminalInput::default();
        input.prepare_frame(&mut Vec::new(), &mut app).unwrap();
        assert!(!app.selection_mode && !app.input.selection_active());
        assert!(app.pending_chat_click.is_none());
        assert_eq!(app.input.text(), "Draft 中文 remains");
    }

    #[test]
    fn regular_overlay_ends_native_selection_and_editor_resume_is_consistent() {
        let mut app = App::new(Vec::new(), Language::En);
        let mut input = TerminalInput::default();
        let mut output = Vec::new();
        app.enter_native_selection_mode();
        input.prepare_frame(&mut output, &mut app).unwrap();
        app.help_visible = true;
        assert_eq!(
            input.prepare_frame(&mut output, &mut app).unwrap(),
            FrameAction::Restore
        );
        assert!(!app.native_selection_mode);
        app.help_visible = false;
        app.enter_native_selection_mode();
        input.prepare_frame(&mut output, &mut app).unwrap();
        input.external_editor_resumed(&mut app);
        output.clear();
        assert_eq!(
            input.prepare_frame(&mut output, &mut app).unwrap(),
            FrameAction::Draw
        );
        assert!(output.is_empty());
        assert!(!app.native_selection_mode);
    }
}
