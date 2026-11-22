use super::*;
use ratatui::backend::TestBackend;

fn mouse(app: &mut App, kind: MouseEventKind, x: u16, y: u16) -> bool {
    human_gate_ui::handle_mouse(
        app,
        MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        },
    )
}

fn control(app: &App, action: human_gate_ui::QuestionAction) -> Rect {
    app.question_view
        .hits
        .iter()
        .find(|(_, candidate)| *candidate == action)
        .expect("painted control")
        .0
}

fn paint(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| human_gate_ui::render(frame, app))
        .unwrap();
    terminal
}

#[tokio::test]
async fn approval_single_rows_resolve_the_displayed_action_and_gap_does_nothing() {
    for always in [false, true] {
        for (index, expected) in approval_decisions(always).iter().copied().enumerate() {
            let mut app = App::new(Vec::new(), Language::ZhCn);
            let (tx, mut rx) = oneshot::channel();
            app.approval = Some(("run_command: 探测实例健康与版本".into(), always, tx));
            let terminal = paint(&mut app, 100, 30);
            let (button, _) = app.approval_action_hits[index];
            assert_eq!(button.height, 1);
            if index > 0 {
                let previous = app.approval_action_hits[index - 1].0;
                assert_eq!(button.y - previous.bottom(), 1);
                app.handle_mouse(
                    button.x + 2,
                    previous.bottom(),
                    &BackgroundTaskRegistry::default(),
                    &SkillCatalog::default(),
                );
                assert!(matches!(
                    rx.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
            }
            assert!(terminal.backend().buffer()[(button.x + 3, button.y)].symbol() != " ");
            app.handle_mouse(
                button.x + 2,
                button.y,
                &BackgroundTaskRegistry::default(),
                &SkillCatalog::default(),
            );
            assert_eq!(rx.await.unwrap(), expected);
        }
    }
}

#[test]
fn small_approval_windows_keep_all_actions_inside_the_border() {
    for height in [5, 6, 8, 12, 20] {
        let mut app = App::new(Vec::new(), Language::En);
        let (tx, _rx) = oneshot::channel();
        app.approval = Some(("Run tests".into(), true, tx));
        paint(&mut app, 40, height);
        assert_eq!(app.approval_action_hits.len(), 3, "height={height}");
        for (rect, _) in &app.approval_action_hits {
            assert!(rect.y > app.approval_rect.y);
            assert!(rect.bottom() < app.approval_rect.bottom());
        }
    }
}

fn question(app: &mut App, selected: usize) -> oneshot::Receiver<Option<String>> {
    let (tx, rx) = oneshot::channel();
    app.question = Some(AskDialog {
        request: UserQuestion {
            question: "请选择目标\nWhich target?".into(),
            options: vec![
                "第一个选项 mixed language with a long wrapped description".into(),
                "第二个选项".into(),
                "最后一个选项".into(),
            ],
            multi_select: false,
        },
        selected,
        checked: vec![false; 3],
        answer: PromptEditor::default(),
        sender: tx,
    });
    rx
}

#[tokio::test]
async fn wrapped_question_rows_click_the_correct_option_and_gaps_are_inert() {
    let mut app = App::new(Vec::new(), Language::ZhCn);
    let mut rx = question(&mut app, 0);
    app.question.as_mut().unwrap().request.options[0].push_str("\n显式换行后的选项说明");
    let terminal = paint(&mut app, 40, 30);
    let rows = app.question_hits.clone();
    let first = rows[0].0;
    assert_eq!(
        terminal.backend().buffer()[(app.question_rect.x + 2, first)].bg,
        Color::LightCyan
    );
    let first_end = rows
        .iter()
        .filter(|(_, index)| *index == 0)
        .map(|(y, _)| *y)
        .max()
        .unwrap();
    let second = rows.iter().find(|(_, index)| *index == 1).unwrap().0;
    assert_eq!(second, first_end + 2);
    app.handle_mouse(
        app.question_rect.x + 3,
        first_end + 1,
        &BackgroundTaskRegistry::default(),
        &SkillCatalog::default(),
    );
    assert!(matches!(
        rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    app.handle_mouse(
        app.question_rect.x + 3,
        second,
        &BackgroundTaskRegistry::default(),
        &SkillCatalog::default(),
    );
    assert_eq!(rx.await.unwrap().as_deref(), Some("第二个选项"));
}

#[tokio::test]
async fn question_controls_follow_their_painted_rectangles_in_all_languages() {
    use human_gate_ui::QuestionAction::{Send, Skip};
    for language in [Language::ZhCn, Language::En, Language::Ja] {
        for (width, height) in [(80, 24), (24, 8), (18, 5)] {
            for action in [Send, Skip] {
                let mut app = App::new(Vec::new(), language);
                let mut rx = question(&mut app, 0);
                app.question.as_mut().unwrap().request.multi_select = true;
                let terminal = paint(&mut app, width, height);
                let send = control(&app, Send);
                let skip = control(&app, Skip);
                assert!(send.width > 0 && skip.width > 0);
                assert!(!send.intersects(skip));
                for rect in [send, skip] {
                    assert!(rect.y > app.question_rect.y);
                    assert!(rect.bottom() < app.question_rect.bottom());
                    assert_ne!(terminal.backend().buffer()[(rect.x, rect.y)].symbol(), " ");
                }
                // Help and unused footer cells never acquire button semantics.
                let inert_x = app.question_rect.right().saturating_sub(3);
                if inert_x >= send.right() {
                    mouse(
                        &mut app,
                        MouseEventKind::Down(MouseButton::Left),
                        inert_x,
                        send.y,
                    );
                    assert!(matches!(
                        rx.try_recv(),
                        Err(oneshot::error::TryRecvError::Empty)
                    ));
                }
                let option = app
                    .question_hits
                    .iter()
                    .find(|(_, index)| *index == 0)
                    .unwrap()
                    .0;
                let x = app.question_view.body.x;
                mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, option);
                assert!(app.question.as_ref().unwrap().checked[0]);
                assert!(matches!(
                    rx.try_recv(),
                    Err(oneshot::error::TryRecvError::Empty)
                ));
                let rect = control(&app, action);
                mouse(
                    &mut app,
                    MouseEventKind::Down(MouseButton::Left),
                    rect.x,
                    rect.y,
                );
                let result = rx.await.unwrap();
                if action == Send {
                    assert_eq!(
                        result.as_deref(),
                        Some("第一个选项 mixed language with a long wrapped description")
                    );
                } else {
                    assert_eq!(result, None);
                }
            }
        }
    }
}

#[test]
fn question_help_row_does_not_submit_or_skip() {
    let mut app = App::new(Vec::new(), Language::En);
    let mut rx = question(&mut app, 0);
    paint(&mut app, 100, 30);
    let send = control(&app, human_gate_ui::QuestionAction::Send);
    let body = app.question_view.body;
    for x in [body.x, body.right() - 1] {
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            x,
            send.y - 1,
        );
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }
}

#[tokio::test]
async fn question_scroll_persists_and_clicks_use_the_scrolled_rows() {
    let mut app = App::new(Vec::new(), Language::En);
    let rx = question(&mut app, 0);
    app.question.as_mut().unwrap().request.options =
        (0..12).map(|index| format!("Choice {index}")).collect();
    app.question.as_mut().unwrap().checked = vec![false; 12];
    app.scroll_from_bottom = 7;
    paint(&mut app, 40, 8);
    let initial = app.question_view.offset;
    let body = app.question_view.body;
    mouse(&mut app, MouseEventKind::ScrollDown, body.x, body.y);
    mouse(&mut app, MouseEventKind::ScrollDown, body.x, body.y);
    let offset = app.question_view.offset;
    assert!(offset > initial);
    assert_eq!(app.scroll_from_bottom, 7);
    paint(&mut app, 40, 8);
    assert_eq!(app.question_view.offset, offset);
    let (row, index) = *app.question_hits.last().unwrap();
    assert!(index > 0);
    let expected = app.question.as_ref().unwrap().request.options[index].clone();
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        body.x,
        row,
    );
    assert_eq!(rx.await.unwrap(), Some(expected));
}

#[test]
fn question_navigation_and_resize_reveal_the_target_after_manual_scroll() {
    let mut app = App::new(Vec::new(), Language::En);
    let _rx = question(&mut app, 0);
    app.question.as_mut().unwrap().request.question = "Long introduction\n".repeat(12);
    paint(&mut app, 40, 8);
    let body = app.question_view.body;
    for _ in 0..8 {
        mouse(&mut app, MouseEventKind::ScrollUp, body.x, body.y);
    }
    paint(&mut app, 40, 8);
    assert_eq!(app.question_view.offset, 0);
    app.handle_question_key(KeyEvent::from(KeyCode::Down));
    paint(&mut app, 40, 8);
    assert!(app.question_hits.iter().any(|(_, index)| *index == 1));
    paint(&mut app, 24, 5);
    assert!(app.question_hits.iter().any(|(_, index)| *index == 1));
    assert!(
        app.question_view
            .hits
            .iter()
            .all(|(rect, _)| rect.right() <= app.question_rect.right()
                && rect.bottom() <= app.question_rect.bottom())
    );
    app.handle_question_key(KeyEvent::from(KeyCode::Char('界')));
    paint(&mut app, 24, 5);
    assert!(
        app.question_view
            .hits
            .iter()
            .any(|(_, action)| matches!(action, human_gate_ui::QuestionAction::Answer(_)))
    );
}

#[test]
fn question_scroll_and_hits_reset_when_the_next_dialog_is_promoted_or_withdrawn() {
    let mut app = App::new(Vec::new(), Language::En);
    let rx = question(&mut app, 0);
    app.question.as_mut().unwrap().request.question = "Long introduction\n".repeat(10);
    let (sender, next_rx) = oneshot::channel();
    app.enqueue_question(AskDialog {
        request: UserQuestion {
            question: "Next question".into(),
            options: vec!["Next choice".into()],
            multi_select: false,
        },
        selected: 0,
        checked: vec![false],
        answer: PromptEditor::default(),
        sender,
    });
    paint(&mut app, 40, 8);
    assert!(app.question_view.offset > 0);
    app.handle_question_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!(rx.blocking_recv().unwrap(), None);
    assert_eq!(app.question_view.offset, 0);
    assert!(app.question_view.hits.is_empty());
    paint(&mut app, 40, 20);
    assert_eq!(app.question_view.offset, 0);
    drop(next_rx);
    assert_eq!(app.withdraw_abandoned_dialogs(), 1);
    assert!(app.question.is_none());
    assert!(app.question_view.hits.is_empty());
    assert_eq!(app.question_view.offset, 0);
    let _rx = question(&mut app, 0);
    paint(&mut app, 40, 8);
    app.discard_pending_questions();
    assert!(app.question_view.hits.is_empty());
    assert_eq!(app.question_view.offset, 0);
}

#[test]
fn approvals_consume_outside_mouse_and_take_precedence_over_search() {
    let mut app = App::new(Vec::new(), Language::En);
    let (tx, mut rx) = oneshot::channel();
    app.approval = Some(("Run tests".into(), false, tx));
    paint(&mut app, 80, 24);
    let mut search = SearchState::default();
    search.editor.insert("abc");
    app.search = Some(search);
    app.search_rect = Rect::new(0, 0, 80, 24);
    app.sidebar_rect = Rect::new(0, 0, 5, 24);
    let focus = app.focus;
    app.handle_mouse(
        1,
        1,
        &BackgroundTaskRegistry::default(),
        &SkillCatalog::default(),
    );
    assert!(app.focus == focus);
    assert!(matches!(
        rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    app.search.as_mut().unwrap().editor.insert("X");
    assert_eq!(app.search.as_ref().unwrap().editor.text(), "abcX");
    app.scroll_from_bottom = 7;
    for kind in [
        MouseEventKind::Moved,
        MouseEventKind::ScrollUp,
        MouseEventKind::ScrollDown,
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        assert!(mouse(&mut app, kind, 1, 1));
    }
    assert_eq!(app.scroll_from_bottom, 7);
    let button = app.approval_action_hits[0].0;
    app.handle_mouse(
        button.x,
        button.y,
        &BackgroundTaskRegistry::default(),
        &SkillCatalog::default(),
    );
    assert_eq!(rx.try_recv().unwrap(), ApprovalDecision::AllowOnce);
}

#[test]
fn tiny_question_windows_never_panic_and_keep_hits_within_the_frame() {
    for width in 0..12 {
        for height in 0..6 {
            let mut app = App::new(Vec::new(), Language::Ja);
            let _rx = question(&mut app, 0);
            paint(&mut app, width, height);
            for (rect, _) in &app.question_view.hits {
                assert!(rect.width > 0 && rect.height > 0);
                assert!(rect.right() <= width && rect.bottom() <= height);
            }
        }
    }
}

#[test]
fn question_selected_option_remains_visible_after_resizing() {
    for height in [5, 8, 12] {
        let mut app = App::new(Vec::new(), Language::En);
        let _rx = question(&mut app, 2);
        paint(&mut app, 32, height);
        assert!(app.question_hits.iter().any(|(_, index)| *index == 2));
        assert!(app.question_hits.iter().all(|(row, _)| *row > app.question_rect.y && *row < app.question_rect.bottom() - 1));
    }
}

fn gate_click(app: &mut App, rect: Rect) {
    assert!(mouse(
        app,
        MouseEventKind::Down(MouseButton::Left),
        rect.x,
        rect.y
    ));
}

#[tokio::test]
async fn painted_buttons_send_or_skip_and_keyboard_hint_is_inert() {
    use human_gate_ui::QuestionAction;
    for action in [QuestionAction::Send, QuestionAction::Skip] {
        let mut app = App::new(Vec::new(), Language::En);
        let mut rx = question(&mut app, 1);
        paint(&mut app, 80, 30);
        let send = control(&app, QuestionAction::Send);
        let skip = control(&app, QuestionAction::Skip);
        assert!(skip.right() < send.x);
        // Both halves of the hint and blank cells on the controls row are inert.
        for (x, y) in [
            (skip.x, send.y - 1),
            (send.x, send.y - 1),
            (send.right(), send.y),
        ] {
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), x, y);
            assert!(matches!(
                rx.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
        }
        let target = control(&app, action);
        gate_click(&mut app, target);
        let expected = if action == QuestionAction::Send {
            Some("第二个选项")
        } else {
            None
        };
        assert_eq!(rx.await.unwrap().as_deref(), expected);
    }
}

#[tokio::test]
async fn scrolling_persists_then_keyboard_and_resize_share_the_painted_hits() {
    use human_gate_ui::QuestionAction;
    let mut app = App::new(Vec::new(), Language::En);
    let rx = question(&mut app, 0);
    let dialog = app.question.as_mut().unwrap();
    dialog.request.options = (0..20).map(|index| format!("Choice {index}")).collect();
    dialog.checked = vec![false; 20];
    paint(&mut app, 40, 10);
    let body = app.question_view.body;
    let before = app.question_view.offset;
    app.scroll_from_bottom = 7;
    assert!(mouse(&mut app, MouseEventKind::ScrollDown, body.x, body.y));
    assert!(app.question_view.offset > before);
    assert_eq!(app.scroll_from_bottom, 7);
    assert!(
        app.question_hits.is_empty(),
        "stale body targets must not survive scrolling"
    );
    let offset = app.question_view.offset;
    paint(&mut app, 40, 10);
    assert_eq!(app.question_view.offset, offset);
    paint(&mut app, 40, 10);
    assert_eq!(app.question_view.offset, offset);
    for _ in 0..4 {
        mouse(&mut app, MouseEventKind::ScrollDown, body.x, body.y);
        paint(&mut app, 40, 10);
    }
    app.handle_question_key(KeyEvent::from(KeyCode::Down));
    paint(&mut app, 40, 10);
    assert!(app.question_hits.iter().any(|(_, index)| *index == 1));
    paint(&mut app, 24, 5);
    let target = control(&app, QuestionAction::Option(1));
    let terminal = paint(&mut app, 24, 5);
    assert_eq!(
        terminal.backend().buffer()[(target.x, target.y)].bg,
        Color::LightCyan
    );
    gate_click(&mut app, target);
    assert_eq!(rx.await.unwrap().as_deref(), Some("Choice 1"));
}

#[tokio::test]
async fn clicked_scrolled_option_submits_the_visible_value() {
    use human_gate_ui::QuestionAction;
    let mut app = App::new(Vec::new(), Language::En);
    let rx = question(&mut app, 0);
    let dialog = app.question.as_mut().unwrap();
    dialog.request.options = (0..20).map(|index| format!("Choice {index}")).collect();
    dialog.checked = vec![false; 20];
    paint(&mut app, 40, 10);
    let body = app.question_view.body;
    mouse(&mut app, MouseEventKind::ScrollDown, body.x, body.y);
    let terminal = paint(&mut app, 40, 10);
    let (rect, index) = app
        .question_view
        .hits
        .iter()
        .rev()
        .find_map(|(rect, action)| match action {
            QuestionAction::Option(index) => Some((*rect, *index)),
            _ => None,
        })
        .unwrap();
    let visible = (rect.x..rect.right())
        .map(|x| terminal.backend().buffer()[(x, rect.y)].symbol())
        .collect::<String>();
    let expected = format!("Choice {index}");
    assert!(visible.contains(&expected));
    gate_click(&mut app, rect);
    assert_eq!(rx.await.unwrap().as_deref(), Some(expected.as_str()));
}

#[test]
fn answer_clicks_use_editor_rows_with_localized_prefix_unicode_and_newlines() {
    use human_gate_ui::QuestionAction;
    let value = "甲乙🙂 e\u{301}Z\nnext 行尾";
    for language in [Language::ZhCn, Language::En, Language::Ja] {
        for width in [16, 24, 40] {
            for prefix_value in ["甲", "甲乙🙂 ", "甲乙🙂 e\u{301}Z\nnext "] {
                let mut app = App::new(Vec::new(), language);
                let _rx = question(&mut app, 0);
                let dialog = app.question.as_mut().unwrap();
                dialog.request.options.clear();
                dialog.checked.clear();
                dialog.answer.insert(value);
                let prefix = format!(
                    "{}: ",
                    language.text("其他答案", "Other answer", "その他の回答")
                );
                let mut target = dialog.answer.clone();
                for _ in value[prefix_value.len()..].chars() {
                    target.left();
                }
                let (row, column) =
                    target.cursor_visual_with_prefix(&prefix, usize::from(width - 4));
                paint(&mut app, width, 30);
                let rect = control(&app, QuestionAction::Answer(row));
                assert!(column < usize::from(rect.width));
                mouse(
                    &mut app,
                    MouseEventKind::Down(MouseButton::Left),
                    rect.x + column as u16,
                    rect.y,
                );
                app.handle_question_key(KeyEvent::from(KeyCode::Char('X')));
                let expected = format!("{prefix_value}X{}", &value[prefix_value.len()..]);
                assert_eq!(app.question.as_ref().unwrap().answer.text(), expected);
            }
        }
    }
}
