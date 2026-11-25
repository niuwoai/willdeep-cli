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
    // 普通鼠标测试默认禁用自动复制；复制入口测试另行注入剪贴板写入端。
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

const COPY_NOTICE_DRAFT: &str = "中🙂  \n\n  tail ";

type CopyNoticeCase = (Result<code_copy::ClipboardRoute, String>, &'static str);

fn copy_notice_cases() -> [CopyNoticeCase; 3] {
    [
        (
            Ok(code_copy::ClipboardRoute::Terminal),
            "Sent 13 prompt chars to the terminal clipboard · Esc clears selection",
        ),
        (
            Ok(code_copy::ClipboardRoute::Local),
            "Copied 13 prompt chars · Esc clears selection",
        ),
        (
            Err("fixture clipboard unavailable".into()),
            "Copy prompt text to clipboard failed: fixture clipboard unavailable",
        ),
    ]
}

#[derive(Clone, Copy, Debug)]
enum CopyNoticeTrigger {
    MouseRelease,
    ControlC,
    CommandC,
}

const COPY_NOTICE_TRIGGERS: [CopyNoticeTrigger; 3] = [
    CopyNoticeTrigger::MouseRelease,
    CopyNoticeTrigger::ControlC,
    CopyNoticeTrigger::CommandC,
];
// 绘制和后续操作测试手动移动截止点，不依赖真实的两秒可读窗口。
const COPY_NOTICE_TEST_HOLD: Duration = Duration::from_secs(24 * 60 * 60);

fn copy_notice_from_interaction(
    mode: crate::config::UiMode,
    case: CopyNoticeCase,
    trigger: CopyNoticeTrigger,
) -> (App, &'static str, (Instant, Instant)) {
    let mut app = draft_app(mode, COPY_NOTICE_DRAFT);
    app.ui.compact_status_line = false;
    app.ui.copy_on_select = matches!(trigger, CopyNoticeTrigger::MouseRelease);
    app.code_copy.stub_clipboard(case.0);
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    paint(&mut app, &mut terminal);
    let body = app.prompt_content_rect();
    for (kind, column, row) in [
        (MouseEventKind::Down(MouseButton::Left), body.x, body.y),
        (
            MouseEventKind::Drag(MouseButton::Left),
            body.x + 7,
            body.y + 2,
        ),
    ] {
        assert!(app.handle_prompt_selection_mouse(mouse(kind, column, row)));
    }
    assert_eq!(app.input.selected_text(), Some(COPY_NOTICE_DRAFT));
    assert!(app.code_copy.clipboard_writes().is_empty());
    let release = mouse(
        MouseEventKind::Up(MouseButton::Left),
        body.x + 7,
        body.y + 2,
    );
    if !matches!(trigger, CopyNoticeTrigger::MouseRelease) {
        assert!(app.handle_prompt_selection_mouse(release));
        assert!(app.code_copy.clipboard_writes().is_empty());
    }
    let before_copy = Instant::now();
    match trigger {
        CopyNoticeTrigger::MouseRelease => {
            assert!(app.handle_prompt_selection_mouse(release));
        }
        CopyNoticeTrigger::ControlC | CopyNoticeTrigger::CommandC => {
            let modifiers = if matches!(trigger, CopyNoticeTrigger::ControlC) {
                KeyModifiers::CONTROL
            } else {
                KeyModifiers::SUPER
            };
            // true 表示事件已消费，主循环不会落到后续 Ctrl+C 退出分支。
            assert!(app.handle_prompt_selection_key(KeyEvent::new(KeyCode::Char('c'), modifiers)));
        }
    }
    let after_copy = Instant::now();
    assert_eq!(app.code_copy.clipboard_writes(), vec![COPY_NOTICE_DRAFT]);
    assert_eq!(app.input.text(), COPY_NOTICE_DRAFT);
    assert_eq!(app.input.selected_text(), Some(COPY_NOTICE_DRAFT));
    assert!(!app.input.selection_dragging());
    assert!(!app.quit_requested);
    (app, case.1, (before_copy, after_copy))
}

fn pin_copy_notice_deadline(app: &mut App) -> Instant {
    let deadline = Instant::now() + COPY_NOTICE_TEST_HOLD;
    app.notice_hold.as_mut().expect("held copy notice").1 = deadline;
    deadline
}

fn app_with_copy_notice(mode: crate::config::UiMode, case: CopyNoticeCase) -> (App, &'static str) {
    let (mut app, expected, _) =
        copy_notice_from_interaction(mode, case, CopyNoticeTrigger::ControlC);
    pin_copy_notice_deadline(&mut app);
    (app, expected)
}

#[test]
fn prompt_selection_copy_interactions_set_a_two_second_notice_hold() {
    for mode in [crate::config::UiMode::Compact, crate::config::UiMode::Full] {
        for trigger in COPY_NOTICE_TRIGGERS {
            for case in copy_notice_cases() {
                let (app, expected, (before_copy, after_copy)) =
                    copy_notice_from_interaction(mode, case, trigger);
                let (held, deadline) = app.notice_hold.as_ref().expect("copy notice must be held");
                assert_eq!(held, expected);
                assert_eq!(app.notice.as_deref(), Some(expected));
                let readable_duration = Duration::from_secs(2);
                assert!(*deadline >= before_copy + readable_duration);
                assert!(*deadline <= after_copy + readable_duration);
            }
        }
    }
}

#[test]
fn prompt_selection_copy_notices_survive_consecutive_frames_in_both_layouts() {
    for mode in [crate::config::UiMode::Compact, crate::config::UiMode::Full] {
        for trigger in COPY_NOTICE_TRIGGERS {
            for case in copy_notice_cases() {
                let (mut app, expected, _) = copy_notice_from_interaction(mode, case, trigger);
                let deadline = pin_copy_notice_deadline(&mut app);
                let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
                for frame in 1..=3 {
                    paint(&mut app, &mut terminal);
                    let buffer = terminal.backend().buffer();
                    let screen = buffer
                        .content
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect::<String>();
                    assert!(
                        screen.contains(expected),
                        "{mode:?}, {trigger:?}, frame {frame}: missing {expected:?}"
                    );
                    assert_eq!(app.notice.as_deref(), Some(expected));
                    assert_eq!(
                        app.notice_hold.as_ref(),
                        Some(&(expected.to_owned(), deadline))
                    );
                    assert_eq!(app.code_copy.clipboard_writes(), vec![COPY_NOTICE_DRAFT]);
                    assert_eq!(app.input.text(), COPY_NOTICE_DRAFT);
                    assert_eq!(app.input.selected_text(), Some(COPY_NOTICE_DRAFT));
                    assert!(app.queued_prompts.is_empty());
                    assert!(app.transcript.is_empty());
                    assert!(!app.running);
                    assert!(!app.quit_requested);
                }
            }
        }
    }
}

#[test]
fn prompt_selection_copy_notice_hold_expires_without_sleeping() {
    for case in copy_notice_cases() {
        let (mut app, expected) = app_with_copy_notice(crate::config::UiMode::Full, case);
        assert_eq!(app.status_line(), expected);
        app.notice_hold.as_mut().expect("held copy notice").1 = Instant::now();
        // 到期时沿用状态栏取走最后一帧、随后恢复正常状态的语义。
        assert_eq!(app.status_line(), expected);
        assert!(app.notice.is_none());
        assert!(app.notice_hold.is_none());
        assert!(!app.status_line().contains(expected));
        assert_eq!(app.input.text(), COPY_NOTICE_DRAFT);
    }
}

#[test]
fn prompt_selection_escape_cancels_held_copy_notice_and_selection_without_sending() {
    for case in copy_notice_cases() {
        let (mut app, expected) = app_with_copy_notice(crate::config::UiMode::Full, case);
        assert_eq!(app.status_line(), expected);
        assert!(app.handle_prompt_selection_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(!app.input.selection_active());
        assert_eq!(app.input.text(), COPY_NOTICE_DRAFT);
        assert!(app.notice.is_none());
        assert!(!app.status_line().contains(expected));
        assert!(app.notice_hold.is_none());
        assert!(!app.status_line().contains(expected));
        assert!(app.queued_prompts.is_empty());
        assert!(app.transcript.is_empty());
        assert!(!app.running);
    }
}

#[test]
fn prompt_selection_copy_notice_hold_yields_to_a_new_notice() {
    for case in copy_notice_cases() {
        let (mut app, expected) = app_with_copy_notice(crate::config::UiMode::Full, case);
        assert_eq!(app.status_line(), expected);
        app.notice = Some("New action result".into());
        assert_eq!(app.status_line(), "New action result");
        assert!(app.notice_hold.is_none());
        assert!(!app.status_line().contains(expected));
    }
}

#[test]
fn prompt_selection_copy_notice_hold_keeps_asynchronous_approval_first() {
    let (mut app, expected) = app_with_copy_notice(
        crate::config::UiMode::Full,
        copy_notice_cases().into_iter().next().unwrap(),
    );
    let (tx, mut rx) = oneshot::channel();
    app.enqueue_approval(("Run tests".into(), false, tx));
    for _ in 0..3 {
        let status = app.status_line();
        let approval = status.find("Waiting for your approval").unwrap();
        let copy_notice = status.find(expected).unwrap();
        assert!(approval < copy_notice, "{status}");
    }
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::SUPER] {
        assert_eq!(
            app.prompt_selection_key(KeyEvent::new(KeyCode::Char('c'), modifiers)),
            PromptSelectionKey::Pass
        );
    }
    let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    assert!(!app.handle_prompt_selection_key(key));
    app.handle_approval_key(key);
    assert_eq!(rx.try_recv().unwrap(), ApprovalDecision::Deny);
    assert!(app.approval.is_none());
    assert_eq!(app.input.text(), COPY_NOTICE_DRAFT);
    assert!(app.queued_prompts.is_empty());
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
    let mut editor = PromptEditor::with_existing_text(original.to_owned());
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
