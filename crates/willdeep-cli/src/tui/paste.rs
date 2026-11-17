use super::*;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PasteAction {
    Done,
    RefreshSessionPicker,
}

/// Match the visible human gate before routing to any editor underneath it.
/// The event loop only handles the asynchronous session-picker refresh.
pub(super) fn route(app: &mut App, value: String) -> PasteAction {
    if let Some(dialog) = app.question.as_mut() {
        dialog.answer.insert(&value);
    } else if app.approval.is_some() {
        app.handle_approval_text(&value);
    } else if app.permission_picker.is_some() {
        // Permission selection does not accept pasted input.
    } else if app.attachment_panel.is_some() {
        app.attachment_paste(&value);
    } else if let Some(panel) = app.prompt_queue.as_mut() {
        if let Some(editor) = panel.editor.as_mut() {
            editor.insert(&value);
        }
    } else if app.routing_settings_paste(&value) {
    } else if let Some(picker) = app.model_picker.as_mut() {
        picker.editor.insert(&value);
        app.refresh_model_picker_matches();
    } else if let Some(picker) = app.workspace_picker.as_mut() {
        picker.editor.insert(&value);
        app.refresh_workspace_picker_matches();
    } else if let Some(picker) = app.session_picker.as_mut() {
        picker.editor.insert(&value);
        return PasteAction::RefreshSessionPicker;
    } else {
        app.handle_paste(value);
    }
    PasteAction::Done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(app: &mut App) -> oneshot::Receiver<Option<String>> {
        let (sender, receiver) = oneshot::channel();
        app.question = Some(AskDialog {
            request: UserQuestion {
                question: "Choose".to_owned(),
                options: vec!["A".to_owned()],
                multi_select: false,
            },
            selected: 0,
            checked: vec![false],
            answer: PromptEditor::default(),
            sender,
        });
        receiver
    }

    fn select_draft(app: &mut App) {
        app.input.insert("draft stays here");
        app.input.begin_selection_visual(0, 0, 80);
        app.input.extend_selection_visual(0, 5, 80);
        app.input.finish_selection();
        assert_eq!(app.input.selected_text(), Some("draft"));
    }

    #[tokio::test]
    async fn visible_question_consumes_yes_without_approving_a_hidden_gate() {
        let mut app = App::new(Vec::new(), Language::En);
        select_draft(&mut app);
        let receiver = question(&mut app);
        let (sender, mut approval) = oneshot::channel();
        app.approval = Some(("Run tests".to_owned(), false, sender));
        assert_eq!(route(&mut app, "yes".to_owned()), PasteAction::Done);
        assert!(approval.try_recv().is_err());
        assert!(app.approval.is_some());
        assert!(human_gate_ui::handle_key(
            &mut app,
            KeyEvent::from(KeyCode::Enter)
        ));
        assert_eq!(receiver.await.unwrap().as_deref(), Some("yes"));
        assert_eq!(app.input.text(), "draft stays here");
        assert_eq!(app.input.selected_text(), Some("draft"));
        assert!(app.attachments.is_empty());
        route(&mut app, "unrecognized approval text".to_owned());
        assert!(app.approval.is_some());
        route(&mut app, "yes".to_owned());
        assert_eq!(approval.try_recv(), Ok(ApprovalDecision::AllowOnce));
    }

    #[tokio::test]
    async fn question_preempts_permission_attachment_and_queue_editors() {
        for underlying in ["permissions", "attachments", "queue"] {
            let mut app = App::new(Vec::new(), Language::En);
            select_draft(&mut app);
            match underlying {
                "permissions" => app.open_permission_picker(false),
                "attachments" => {
                    app.open_text_attachment("note.txt".to_owned(), "keep attachment".to_owned());
                }
                "queue" => {
                    let mut panel = prompt_queue::QueuePanel::default();
                    let mut editor = PromptEditor::default();
                    editor.insert("keep queued draft");
                    panel.editor = Some(editor);
                    app.prompt_queue = Some(panel);
                }
                _ => unreachable!(),
            }
            let receiver = question(&mut app);
            let pasted = "custom answer\nsecond line\nthird\nfourth";
            assert_eq!(route(&mut app, pasted.to_owned()), PasteAction::Done);
            assert_eq!(app.question.as_ref().unwrap().answer.text(), pasted);
            assert_eq!(app.input.selected_text(), Some("draft"));
            assert_eq!(app.input.text(), "draft stays here");
            assert!(app.attachments.is_empty());
            if let Some(panel) = app.prompt_queue.as_ref() {
                assert_eq!(panel.editor.as_ref().unwrap().text(), "keep queued draft");
            }
            assert!(human_gate_ui::handle_key(
                &mut app,
                KeyEvent::from(KeyCode::Enter)
            ));
            assert_eq!(receiver.await.unwrap().as_deref(), Some(pasted));
        }
    }

    #[tokio::test]
    async fn question_keys_keep_a_hidden_full_access_confirmation_pending() {
        let mut app = App::new(Vec::new(), Language::En);
        select_draft(&mut app);
        app.open_permission_picker(true);
        let mode = app.approval_mode;
        let receiver = question(&mut app);
        route(&mut app, "answer".to_owned());
        assert!(human_gate_ui::handle_key(
            &mut app,
            KeyEvent::from(KeyCode::Char('y'))
        ));
        assert!(human_gate_ui::handle_key(
            &mut app,
            KeyEvent::from(KeyCode::Enter)
        ));
        assert_eq!(receiver.await.unwrap().as_deref(), Some("answery"));
        assert!(app.permission_picker.as_ref().unwrap().confirming);
        assert_eq!(app.approval_mode, mode);
        assert_eq!(app.input.selected_text(), Some("draft"));
    }

    #[test]
    fn question_mouse_does_not_approve_a_button_behind_it() {
        let mut app = App::new(Vec::new(), Language::En);
        let _receiver = question(&mut app);
        route(&mut app, "yes".to_owned());
        let (sender, mut approval) = oneshot::channel();
        app.approval = Some(("Run tests".to_owned(), false, sender));
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| human_gate_ui::render(frame, &mut app))
            .unwrap();
        let overlap = app
            .approval_action_hits
            .iter()
            .map(|(rect, _)| rect.intersection(app.question_rect))
            .find(|rect| rect.width > 0 && rect.height > 0)
            .expect("fixture must have a hidden approval button behind the question");
        app.handle_mouse(
            overlap.x + overlap.width / 2,
            overlap.y,
            &BackgroundTaskRegistry::default(),
            &SkillCatalog::default(),
        );
        assert!(approval.try_recv().is_err());
        assert!(app.approval.is_some());
    }

    #[test]
    fn ordinary_paste_keeps_existing_composer_and_attachment_semantics() {
        let mut app = App::new(Vec::new(), Language::En);
        select_draft(&mut app);
        assert_eq!(route(&mut app, "new".to_owned()), PasteAction::Done);
        // Existing composer semantics clear the selection and insert at its cursor.
        assert_eq!(app.input.text(), "newdraft stays here");
        assert!(app.input.selected_text().is_none());
        route(&mut app, "one\ntwo\nthree\nfour".to_owned());
        assert_eq!(app.attachments.len(), 1);
    }

    #[test]
    fn pasted_multiline_answers_remain_visible_at_the_real_cursor_after_scrolling() {
        use ratatui::backend::TestBackend;

        for language in [Language::En, Language::ZhCn, Language::Ja] {
            for width in [120, 30, 9] {
                for answer in [
                    "first\nsecondz".to_owned(),
                    "xxxxxxxxxxxx\nz".to_owned(),
                    format!("{}z", "x\n".repeat(40)),
                    format!("{}z", "word ".repeat(50)),
                    format!("{}z", "中🙂e\u{301}".repeat(50)),
                    format!("{}z", "👩🏽‍💻🇨🇳".repeat(50)),
                ] {
                    let mut app = App::new(Vec::new(), language);
                    let _receiver = question(&mut app);
                    route(&mut app, answer.clone());
                    let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
                    terminal
                        .draw(|frame| human_gate_ui::render(frame, &mut app))
                        .unwrap();
                    let cursor = terminal.get_cursor_position().unwrap();
                    assert!(app.question_rect.contains(cursor));
                    let buffer = terminal.backend().buffer();
                    let inner =
                        modal_block(String::new(), Color::LightCyan).inner(app.question_rect);
                    let previous = if cursor.x == inner.x {
                        (inner.right() - 1, cursor.y - 1)
                    } else {
                        (cursor.x - 1, cursor.y)
                    };
                    assert_eq!(
                        buffer[previous].symbol(),
                        "z",
                        "cursor must follow answer: language={language:?}, width={width}, cursor={cursor:?}, previous={previous:?}, rect={:?}, answer={answer:?}, buffer={buffer:?}",
                        app.question_rect
                    );
                    if width == 30 && language == Language::En && answer == "xxxxxxxxxxxx\nz" {
                        let previous_row = (0..width)
                            .map(|x| buffer[(x, cursor.y - 1)].symbol())
                            .collect::<String>();
                        assert!(previous_row.contains("xxxxxxxxxxxx"));
                    }
                    if width == 120 && answer == "first\nsecondz" {
                        let row = |y| {
                            (0..width)
                                .map(|x| buffer[(x, y)].symbol())
                                .collect::<String>()
                        };
                        assert!(row(cursor.y).contains("secondz"));
                        assert!(row(cursor.y - 1).contains("first"));
                    }
                }
            }
        }
    }
}
