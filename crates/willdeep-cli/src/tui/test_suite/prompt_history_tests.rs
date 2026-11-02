use super::*;

fn press(app: &mut App, code: KeyCode, messages: &[Message]) -> bool {
    app.handle_prompt_history_key(KeyEvent::new(code, KeyModifiers::NONE), messages)
}

#[test]
fn prompt_history_opens_at_latest_and_refills_without_sending() {
    let mut app = App::new(Vec::new(), Language::En);
    let messages = vec![Message::user("old"), Message::user("新しい\n输入")];
    assert!(press(&mut app, KeyCode::Up, &messages));
    assert!(app.input.text().is_empty());
    assert!(press(&mut app, KeyCode::Enter, &messages));
    assert_eq!(app.input.text(), "新しい\n输入");
    assert!(!app.running);
    assert!(app.queued_prompts.is_empty());
    assert!(app.prompt_history.is_none());
}

#[test]
fn prompt_history_navigates_deduplicated_operator_inputs() {
    let mut app = App::new(Vec::new(), Language::En);
    let messages = vec![
        Message::user("old"),
        Message::user("latest"),
        Message::host_instruction("hidden"),
        Message::user("latest"),
        Message::user("  "),
    ];
    press(&mut app, KeyCode::Up, &messages);
    press(&mut app, KeyCode::Up, &messages);
    press(&mut app, KeyCode::Tab, &messages);
    assert_eq!(app.input.text(), "old");
    app.input.take();
    press(&mut app, KeyCode::Up, &messages);
    press(&mut app, KeyCode::Up, &messages);
    press(&mut app, KeyCode::Down, &messages);
    press(&mut app, KeyCode::Enter, &messages);
    assert_eq!(app.input.text(), "latest");
}

#[test]
fn prompt_history_does_not_capture_drafts_or_modified_arrows() {
    let mut app = App::new(Vec::new(), Language::En);
    for draft in ["text", "\n", " "] {
        app.input.insert(draft);
        assert!(!press(&mut app, KeyCode::Up, &[]));
        assert_eq!(app.input.take(), draft);
    }
    for modifier in [
        KeyModifiers::ALT,
        KeyModifiers::CONTROL,
        KeyModifiers::SHIFT,
    ] {
        assert!(!app.handle_prompt_history_key(KeyEvent::new(KeyCode::Up, modifier), &[]));
    }
    app.focus = FocusPane::Chat;
    assert!(!press(&mut app, KeyCode::Up, &[]));
}

#[test]
fn prompt_history_escape_and_edit_leave_draft_intact() {
    let mut app = App::new(Vec::new(), Language::En);
    let messages = vec![Message::user("old")];
    press(&mut app, KeyCode::Up, &messages);
    assert!(press(&mut app, KeyCode::Esc, &messages));
    assert!(app.input.text().is_empty());
    assert!(app.prompt_history.is_none());
    press(&mut app, KeyCode::Up, &messages);
    assert!(!press(&mut app, KeyCode::Char('x'), &messages));
    assert!(app.prompt_history.is_none());
    press(&mut app, KeyCode::Up, &messages);
    app.handle_paste("pasted".to_owned());
    assert!(app.prompt_history.is_none());
    assert_eq!(app.input.text(), "pasted");
}

#[test]
fn prompt_history_empty_session_and_inline_render_are_safe() {
    let mut app = App::new(Vec::new(), Language::En);
    press(&mut app, KeyCode::Up, &[]);
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(90, 20)).unwrap();
    terminal
        .draw(|frame| prompt_history::draw(frame, &app, Rect::new(0, 16, 90, 4)))
        .unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("No prompt history in this Session"));
    assert!(terminal.backend().buffer().cell((0, 13)).unwrap().symbol() != " ");
    press(&mut app, KeyCode::Enter, &[]);
    assert!(app.input.text().is_empty());
    assert!(app.prompt_history.is_none());
}

#[test]
fn prompt_history_refill_preserves_existing_attachments() {
    let mut app = App::new(Vec::new(), Language::En);
    app.handle_paste("attached\ncontext".to_owned());
    let attachment_id = app.attachments[0].id;
    let messages = vec![Message::user("previous request")];
    press(&mut app, KeyCode::Up, &messages);
    press(&mut app, KeyCode::Enter, &messages);
    assert_eq!(app.attachments.len(), 1);
    assert_eq!(app.attachments[0].id, attachment_id);
    assert_eq!(app.input.text(), "previous request");
}

#[test]
fn prompt_history_includes_a_submission_before_it_is_persisted() {
    let mut app = App::new(Vec::new(), Language::ZhCn);
    app.note_prompt_submitted("刚发出去，还没落盘");
    let messages = vec![Message::user("更早的一条")];
    press(&mut app, KeyCode::Up, &messages);
    press(&mut app, KeyCode::Enter, &messages);
    assert_eq!(app.input.text(), "刚发出去，还没落盘");

    // 落进会话文件之后不再另记一份，菜单里只出现一次，仍是最新的。
    app.input.take();
    let messages = vec![
        Message::user("更早的一条"),
        Message::user("刚发出去，还没落盘"),
    ];
    app.note_prompt_history_persisted(&messages);
    press(&mut app, KeyCode::Up, &messages);
    press(&mut app, KeyCode::Enter, &messages);
    assert_eq!(app.input.text(), "刚发出去，还没落盘");
    app.input.take();
    press(&mut app, KeyCode::Up, &messages);
    press(&mut app, KeyCode::Up, &messages);
    press(&mut app, KeyCode::Enter, &messages);
    assert_eq!(app.input.text(), "更早的一条");
}

#[test]
fn prompt_history_scrolls_inline_in_both_layouts() {
    for mode in [crate::config::UiMode::Compact, crate::config::UiMode::Full] {
        let mut app = App::new(Vec::new(), Language::En);
        app.ui.mode = mode;
        let messages: Vec<_> = (0..120)
            .map(|i| Message::user(format!("request-{i:03}")))
            .collect();
        press(&mut app, KeyCode::Up, &messages);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        draw(
            &mut terminal,
            &mut app,
            &SkillCatalog::default(),
            &quiet_test_session(),
        )
        .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("request-119"));
        assert!(!rendered.contains("request-000"));
        for _ in 0..120 {
            press(&mut app, KeyCode::Up, &messages);
        }
        draw(
            &mut terminal,
            &mut app,
            &SkillCatalog::default(),
            &quiet_test_session(),
        )
        .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("request-020"));
        press(&mut app, KeyCode::Enter, &messages);
        assert_eq!(app.input.text(), "request-020");
    }
}
