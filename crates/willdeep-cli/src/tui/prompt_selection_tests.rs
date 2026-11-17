use super::*;
use ratatui::backend::TestBackend;

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn paint(app: &mut App, terminal: &mut Terminal<TestBackend>) {
    draw(
        terminal,
        app,
        &SkillCatalog::default(),
        &quiet_test_session(),
    )
    .unwrap();
}

fn draft_app(mode: crate::config::UiMode, text: &str) -> App {
    let mut app = App::new(Vec::new(), Language::En);
    app.ui.mode = mode;
    // 所有鼠标测试都禁止自动触碰真实剪贴板；payload 决策另测。
    app.ui.copy_on_select = false;
    app.input.insert(text);
    app
}

#[test]
fn prompt_selection_preserves_unicode_blank_lines_indent_and_trailing_spaces_in_both_directions() {
    let original = "中🙂ab  \n\n  tail \n";
    let mut editor = PromptEditor::default();
    editor.insert(original);
    assert_eq!(editor.wrapped_text(6), "中🙂ab\n  \n\n  tail\n \n");
    for (anchor, head) in [((0, 0), (5, 0)), ((5, 0), (0, 0))] {
        editor.begin_selection_visual(anchor.0, anchor.1, 6);
        editor.extend_selection_visual(head.0, head.1, 6);
        editor.finish_selection();
        assert_eq!(editor.selected_text(), Some(original));
        assert_eq!(editor.text(), original);
        let cells = editor.selection_visual_ranges(6);
        assert!(cells.contains(&(0, 0..6)), "{cells:?}");
        assert!(cells.contains(&(2, 0..1)), "显式空行也需高亮 {cells:?}");
    }
}

#[test]
fn prompt_selection_soft_wrap_never_inserts_a_newline_and_wide_glyphs_are_whole() {
    let mut editor = PromptEditor::default();
    editor.insert("中🙂abcd");
    assert_eq!(editor.wrapped_text(5), "中🙂a\nbcd");
    editor.begin_selection_visual(0, 0, 5);
    editor.extend_selection_visual(1, 3, 5);
    assert_eq!(editor.selected_text(), Some("中🙂abcd"));
    assert_eq!(
        editor.selection_visual_ranges(5),
        vec![(0, 0..5), (1, 0..3)]
    );
    // 两格字符的后半格沿用编辑器的向后吸附语义，不切开 UTF-8。
    editor.begin_selection_visual(0, 1, 5);
    editor.extend_selection_visual(0, 3, 5);
    assert_eq!(editor.selected_text(), Some("🙂"));
    assert_eq!(editor.selection_visual_ranges(5), vec![(0, 2..4)]);
}

#[test]
fn prompt_selection_keeps_zwj_skin_tone_flags_and_combining_marks_whole() {
    let mut editor = PromptEditor::default();
    editor.insert("中👩‍💻👍🏽🇨🇳e\u{301}x");
    assert_eq!(editor.wrapped_text(6), "中👩‍💻👍🏽\n🇨🇳e\u{301}x");
    assert_eq!(editor.cursor_visual(6), (1, 4));
    editor.begin_selection_visual(0, 3, 6);
    editor.extend_selection_visual(1, 2, 6);
    assert_eq!(editor.selected_text(), Some("👍🏽🇨🇳"));
    assert_eq!(
        editor.selection_visual_ranges(6),
        vec![(0, 4..6), (1, 0..2)]
    );
    editor.begin_selection_visual(1, 2, 6);
    editor.extend_selection_visual(1, 3, 6);
    assert_eq!(editor.selected_text(), Some("e\u{301}"));
    assert_eq!(editor.selection_visual_ranges(6), vec![(1, 2..3)]);
}

#[test]
fn prompt_selection_clamps_to_the_clicked_row_when_wide_glyph_wraps_early() {
    let mut editor = PromptEditor::default();
    editor.insert("ab中🙂x");
    assert_eq!(editor.wrapped_text(3), "ab\n中\n🙂x\n");
    editor.begin_selection_visual(0, 3, 3);
    editor.extend_selection_visual(1, 3, 3);
    assert_eq!(editor.selected_text(), Some("中"));
    assert_eq!(editor.selection_visual_ranges(3), vec![(1, 0..2)]);
    editor.set_cursor_visual(1, 0, 3);
    assert_eq!(editor.cursor_visual(3), (1, 0));
    editor.insert("X");
    assert_eq!(editor.text(), "abX中🙂x");
}

#[test]
fn prompt_selection_is_invalidated_by_every_editor_edit_or_cursor_move() {
    let operations: &[fn(&mut PromptEditor)] = &[
        |e| e.insert("y"),
        |e| e.backspace(),
        |e| e.delete(),
        |e| e.left(),
        |e| e.right(),
        |e| e.home(),
        |e| e.end(),
        |e| e.up_visual(10),
        |e| e.down_visual(10),
        |e| e.set_cursor_visual(0, 1, 10),
        |e| e.replace_before_cursor(0, "new"),
        |e| {
            e.take();
        },
    ];
    for operation in operations {
        let mut editor = PromptEditor::default();
        editor.insert("draft");
        editor.begin_selection_visual(0, 0, 10);
        editor.extend_selection_visual(0, 3, 10);
        operation(&mut editor);
        assert!(!editor.selection_active());
        assert!(!editor.selection_dragging());
        assert_eq!(editor.selected_text(), None);
    }
}

#[test]
fn prompt_selection_payload_respects_auto_copy_and_keeps_whitespace() {
    let mut app = draft_app(crate::config::UiMode::Full, "  \n\n");
    app.input.begin_selection_visual(0, 0, 10);
    app.input.extend_selection_visual(2, 0, 10);
    assert_eq!(app.prompt_selection_copy_text(true), None);
    assert_eq!(
        app.prompt_selection_copy_text(false).as_deref(),
        Some("  \n\n")
    );
    app.ui.copy_on_select = true;
    assert_eq!(
        app.prompt_selection_copy_text(true).as_deref(),
        Some("  \n\n")
    );
    app.input.clear_selection();
    assert_eq!(app.prompt_selection_copy_text(true), None);
    app.input.begin_selection_visual(0, 0, 10);
    app.input.finish_selection();
    assert!(!app.input.selection_active());
}

#[test]
fn prompt_selection_highlights_unicode_in_full_and_compact_layouts_without_changing_draft() {
    for mode in [crate::config::UiMode::Full, crate::config::UiMode::Compact] {
        let mut app = draft_app(mode, "中🙂ab tail");
        let original = app.input.text().to_owned();
        let mut terminal = Terminal::new(TestBackend::new(50, 30)).unwrap();
        paint(&mut app, &mut terminal);
        let body = app.prompt_content_rect();
        app.selection_mode = true;
        app.chat_selection = Some(ChatSelection {
            anchor: ChatSelectionPoint { row: 0, column: 0 },
            head: ChatSelectionPoint { row: 0, column: 1 },
        });
        app.pending_chat_click = Some(app.transcript_content.y);
        for (kind, x) in [
            (MouseEventKind::Down(MouseButton::Left), body.x),
            (MouseEventKind::Drag(MouseButton::Left), body.x + 6),
            (MouseEventKind::Up(MouseButton::Left), body.x + 6),
        ] {
            assert!(app.handle_prompt_selection_mouse(mouse(kind, x, body.y)));
            paint(&mut app, &mut terminal);
        }
        assert_eq!(app.input.selected_text(), Some("中🙂ab"));
        assert_eq!(app.input.text(), original);
        assert_eq!(app.focus, FocusPane::Prompt);
        assert!(!app.selection_mode);
        assert!(app.chat_selection.is_none());
        assert!(app.pending_chat_click.is_none());
        assert_eq!(app.input.cursor_visual(body.width as usize), (0, 0));
        let buffer = terminal.backend().buffer();
        // Terminal 的 diff 会跳过宽字符续格；可见样式取整个字素的起始格。
        for column in [0, 2, 4, 5] {
            assert_eq!(buffer[(body.x + column, body.y)].bg, MODAL_BG);
            assert_eq!(buffer[(body.x + column, body.y)].fg, MODAL_FG);
        }
        assert_ne!(buffer[(body.x + 6, body.y)].bg, MODAL_BG);
        assert!(app.handle_prompt_selection_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        paint(&mut app, &mut terminal);
        assert_eq!(app.input.text(), original);
        assert_ne!(terminal.backend().buffer()[(body.x, body.y)].bg, MODAL_BG);
    }
}

#[test]
fn prompt_selection_crosses_soft_wraps_and_explicit_empty_rows_in_both_layouts() {
    for mode in [crate::config::UiMode::Full, crate::config::UiMode::Compact] {
        let original = "中🙂abcd  \n\n  x ";
        let mut app = draft_app(mode, original);
        let mut terminal = Terminal::new(TestBackend::new(10, 30)).unwrap();
        paint(&mut app, &mut terminal);
        let body = app.prompt_content_rect();
        assert_eq!(body.width, 8);
        // 第一行恰好满宽，余下空格在软折行之后；空行在视觉第 2 行。
        for (kind, x, y) in [
            (
                MouseEventKind::Down(MouseButton::Left),
                body.x + 4,
                body.y + 3,
            ),
            (MouseEventKind::Drag(MouseButton::Left), body.x, body.y),
            (MouseEventKind::Up(MouseButton::Left), body.x, body.y),
        ] {
            assert!(app.handle_prompt_selection_mouse(mouse(kind, x, y)));
        }
        paint(&mut app, &mut terminal);
        assert_eq!(app.input.selected_text(), Some(original));
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(body.x, body.y + 2)].bg, MODAL_BG);
        assert_eq!(buffer[(body.x + 3, body.y + 3)].bg, MODAL_BG);
        assert_ne!(buffer[(body.x + 4, body.y + 3)].bg, MODAL_BG);
    }
}

#[test]
fn prompt_selection_uses_scrolled_rows_and_does_not_jump_when_anchor_moves_cursor() {
    for mode in [crate::config::UiMode::Full, crate::config::UiMode::Compact] {
        let original = (0..14)
            .map(|row| format!("row{row:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = draft_app(mode, &original);
        let mut terminal = Terminal::new(TestBackend::new(50, 30)).unwrap();
        paint(&mut app, &mut terminal);
        let body = app.prompt_content_rect();
        let scroll = app.prompt_scroll;
        assert!(scroll >= 3);
        for (kind, x) in [
            (MouseEventKind::Down(MouseButton::Left), body.x),
            (MouseEventKind::Drag(MouseButton::Left), body.x + 5),
            (MouseEventKind::Up(MouseButton::Left), body.x + 5),
        ] {
            assert!(app.handle_prompt_selection_mouse(mouse(kind, x, body.y)));
            paint(&mut app, &mut terminal);
            assert_eq!(app.prompt_scroll, scroll);
        }
        let selected = format!("row{scroll:02}");
        assert_eq!(app.input.selected_text(), Some(selected.as_str()));
        assert!(app.handle_prompt_selection_mouse(mouse(MouseEventKind::ScrollUp, body.x, body.y)));
        paint(&mut app, &mut terminal);
        assert_eq!(app.prompt_scroll, scroll - 3);
        assert_eq!(app.input.selected_text(), Some(selected.as_str()));
        assert_eq!(
            terminal.backend().buffer()[(body.x, body.y + 3)].bg,
            MODAL_BG
        );
        assert_ne!(terminal.backend().buffer()[(body.x, body.y)].bg, MODAL_BG);
        assert_eq!(app.input.text(), original);
    }
}

#[test]
fn prompt_selection_drag_outside_stays_owned_by_prompt_and_click_only_positions_cursor() {
    let mut app = draft_app(crate::config::UiMode::Full, "draft");
    let mut terminal = Terminal::new(TestBackend::new(50, 30)).unwrap();
    paint(&mut app, &mut terminal);
    let body = app.prompt_content_rect();
    assert!(app.handle_prompt_selection_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        body.x + 2,
        body.y
    )));
    assert!(app.handle_prompt_selection_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        body.x + 2,
        body.y
    )));
    assert!(!app.input.selection_active());
    assert_eq!(app.input.cursor_visual(body.width as usize), (0, 2));
    assert!(app.handle_prompt_selection_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        body.x,
        body.y
    )));
    assert!(app.handle_prompt_selection_mouse(mouse(
        MouseEventKind::Drag(MouseButton::Left),
        body.right() + 5,
        body.y
    )));
    assert_eq!(app.input.selected_text(), Some("draft"));
    assert!(app.handle_prompt_selection_mouse(mouse(
        MouseEventKind::Up(MouseButton::Left),
        body.right() + 5,
        body.y
    )));
    assert!(app.chat_selection.is_none());
    assert!(!app.handle_prompt_selection_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        app.transcript_content.x,
        app.transcript_content.y
    )));
    assert!(!app.input.selection_active());
    assert_eq!(app.input.text(), "draft");
}

#[test]
fn prompt_selection_copy_routes_before_ctrl_c_exit_and_y_q_remain_typing() {
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::SUPER] {
        let mut app = draft_app(crate::config::UiMode::Full, "draft");
        let key = KeyEvent::new(KeyCode::Char('c'), modifiers);
        assert_eq!(app.prompt_selection_key(key), PromptSelectionKey::Pass);
        app.input.begin_selection_visual(0, 0, 20);
        app.input.extend_selection_visual(0, 5, 20);
        assert_eq!(
            app.prompt_selection_key(key),
            PromptSelectionKey::Copy("draft".into())
        );
        assert_eq!(app.input.text(), "draft");
        assert!(app.input.selection_active());
    }
    for character in ['y', 'q'] {
        let mut app = draft_app(crate::config::UiMode::Full, "draft");
        app.input.begin_selection_visual(0, 0, 20);
        app.input.extend_selection_visual(0, 5, 20);
        assert!(!app.handle_prompt_selection_key(KeyEvent::new(
            KeyCode::Char(character),
            KeyModifiers::NONE
        )));
        app.edit_input(|input| input.insert(&character.to_string()));
        assert_eq!(app.input.text(), format!("{character}draft"));
        assert!(!app.input.selection_active());
        assert!(!app.selection_mode);
    }
}

#[test]
fn prompt_selection_complex_emoji_highlight_reflows_after_resize_in_both_layouts() {
    for mode in [crate::config::UiMode::Full, crate::config::UiMode::Compact] {
        let original = "中👩‍💻👍🏽🇨🇳e\u{301}x";
        let mut app = draft_app(mode, original);
        let mut terminal = Terminal::new(TestBackend::new(12, 30)).unwrap();
        paint(&mut app, &mut terminal);
        let body = app.prompt_content_rect();
        for (kind, column) in [
            (MouseEventKind::Down(MouseButton::Left), body.x),
            (MouseEventKind::Drag(MouseButton::Left), body.x + 8),
            (MouseEventKind::Up(MouseButton::Left), body.x + 8),
        ] {
            assert!(app.handle_prompt_selection_mouse(mouse(kind, column, body.y)));
        }
        paint(&mut app, &mut terminal);
        assert_eq!(app.input.selected_text(), Some("中👩‍💻👍🏽🇨🇳"));
        for column in [0, 2, 4, 6] {
            assert_eq!(
                terminal.backend().buffer()[(body.x + column, body.y)].bg,
                MODAL_BG
            );
        }
        terminal.backend_mut().resize(8, 30);
        terminal.resize(Rect::new(0, 0, 8, 30)).unwrap();
        paint(&mut app, &mut terminal);
        let body = app.prompt_content_rect();
        assert_eq!(body.width, 6);
        for column in [0, 2, 4] {
            assert_eq!(
                terminal.backend().buffer()[(body.x + column, body.y)].bg,
                MODAL_BG
            );
        }
        assert_eq!(
            terminal.backend().buffer()[(body.x, body.y + 1)].bg,
            MODAL_BG
        );
        assert_ne!(
            terminal.backend().buffer()[(body.x + 2, body.y + 1)].bg,
            MODAL_BG
        );
        assert_eq!(app.input.selected_text(), Some("中👩‍💻👍🏽🇨🇳"));
        assert_eq!(app.input.text(), original);
    }
}

#[test]
fn prompt_selection_inline_and_attachment_paste_both_clear_selection() {
    for paste in ["x", "a\nb\nc\nd"] {
        let mut app = draft_app(crate::config::UiMode::Full, "draft");
        app.input.begin_selection_visual(0, 0, 20);
        app.input.extend_selection_visual(0, 3, 20);
        app.handle_paste(paste.to_owned());
        assert!(!app.input.selection_active());
        assert!(!app.input.selection_dragging());
    }
}

#[test]
fn prompt_selection_defers_escape_and_copy_keys_to_existing_overlays() {
    let overlays: &[fn(&mut App)] = &[
        |app| app.help_visible = true,
        |app| {
            let (sender, _) = oneshot::channel();
            app.approval = Some(("Run fixture".into(), false, sender));
        },
        |app| {
            let (sender, _) = oneshot::channel();
            app.question = Some(AskDialog {
                request: UserQuestion {
                    question: "Fixture?".into(),
                    options: Vec::new(),
                    multi_select: false,
                },
                selected: 0,
                checked: Vec::new(),
                answer: PromptEditor::default(),
                sender,
            });
        },
        |app| app.open_permission_picker(false),
    ];
    for open in overlays {
        for key in [
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::SUPER),
        ] {
            let mut app = draft_app(crate::config::UiMode::Full, "draft");
            app.input.begin_selection_visual(0, 0, 20);
            app.input.extend_selection_visual(0, 5, 20);
            open(&mut app);
            assert_eq!(app.prompt_selection_key(key), PromptSelectionKey::Pass);
            assert!(!app.handle_prompt_selection_key(key));
            assert_eq!(app.input.text(), "draft");
            assert!(!app.input.selection_active());
        }
    }
}

#[test]
fn prompt_selection_cannot_start_through_an_overlay_or_on_composer_border() {
    let mut app = draft_app(crate::config::UiMode::Full, "draft");
    let mut terminal = Terminal::new(TestBackend::new(50, 30)).unwrap();
    paint(&mut app, &mut terminal);
    let body = app.prompt_content_rect();
    assert!(!app.handle_prompt_selection_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        body.x,
        body.y - 1
    )));
    app.help_visible = true;
    assert!(!app.handle_prompt_selection_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        body.x,
        body.y
    )));
    assert!(!app.input.selection_active());
    assert_eq!(app.input.text(), "draft");
}

#[test]
fn prompt_selection_esc_reaches_an_asynchronous_approval() {
    let mut app = draft_app(crate::config::UiMode::Full, "draft");
    app.input.begin_selection_visual(0, 0, 20);
    app.input.extend_selection_visual(0, 5, 20);
    let (tx, mut rx) = oneshot::channel();
    app.enqueue_approval(("Run tests".into(), false, tx));
    let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.handle_prompt_selection_key(key));
    app.handle_approval_key(key);
    assert_eq!(rx.try_recv().unwrap(), ApprovalDecision::Deny);
    assert!(app.approval.is_none());
    assert_eq!(app.input.text(), "draft");
}

#[test]
fn prompt_selection_preserves_tabs_and_crlf_in_original_coordinates() {
    let original = "\t中\r\n\t🙂x";
    let mut editor = PromptEditor::default();
    editor.insert(original);
    editor.begin_selection_visual(0, 0, 6);
    // 与 Ratatui 一致：单独 Tab/CR 的字符串宽度为一列，字节偏移仍保留原文。
    editor.extend_selection_visual(1, 4, 6);
    editor.finish_selection();
    assert_eq!(editor.selected_text(), Some(original));
    assert_eq!(
        editor.selection_visual_ranges(6),
        vec![(0, 0..5), (1, 0..4)]
    );
    assert_eq!(editor.text(), original);
}
