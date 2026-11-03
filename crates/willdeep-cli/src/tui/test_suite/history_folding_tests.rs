use super::*;

/// 模拟一次真实的“按下 → 松手”：事件循环里先是选区逻辑记下落点，再交给
/// `handle_mouse` 记下待点开的那一行，最后在松手时决定要不要点开。
fn click(app: &mut App, x: u16, y: u16) {
    for kind in [
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
    ] {
        let mouse = crossterm::event::MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        if app.handle_chat_selection_mouse(mouse) {
            continue;
        }
        if matches!(kind, crossterm::event::MouseEventKind::Down(_)) {
            app.handle_mouse(
                x,
                y,
                &BackgroundTaskRegistry::default(),
                &SkillCatalog::default(),
            );
        }
    }
}

#[test]
fn both_tui_layouts_expose_clickable_user_disclosures_without_jumping_to_bottom() {
    for mode in [crate::config::UiMode::Compact, crate::config::UiMode::Full] {
        let original = format!("You: {}hidden tail", "line\n".repeat(20));
        let mut app = App::new(vec![original.clone()], Language::En);
        app.ui.mode = mode;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
        draw(
            &mut terminal,
            &mut app,
            &SkillCatalog::default(),
            &quiet_test_session(),
        )
        .unwrap();
        let row = app.user_message_hit_rows[0].0;
        let x = app.transcript_rect.x + 2;
        click(&mut app, x, row);
        draw(
            &mut terminal,
            &mut app,
            &SkillCatalog::default(),
            &quiet_test_session(),
        )
        .unwrap();
        assert!(
            app.transcript_rows
                .iter()
                .any(|line| line.contains("hidden tail"))
        );
        assert_eq!(app.transcript_render_offset, 0);
        let row = app.user_message_hit_rows[0].0;
        let x = app.transcript_rect.x + 2;
        click(&mut app, x, row);
        draw(
            &mut terminal,
            &mut app,
            &SkillCatalog::default(),
            &quiet_test_session(),
        )
        .unwrap();
        assert!(
            !app.transcript_rows
                .iter()
                .any(|line| line.contains("hidden tail"))
        );
        assert_eq!(app.transcript, vec![original]);
    }
}

#[test]
fn long_user_messages_toggle_independently_and_preserve_original_text() {
    let original = format!("You: {}hidden tail", "中文内容\n".repeat(18));
    let mut app = App::new(vec![original.clone(), original.clone()], Language::En);
    app.transcript_width = 40;
    app.viewport_height = 60;
    app.prepare_transcript_frame();
    assert_eq!(app.transcript_height, 18);
    assert!(
        !app.transcript_rows
            .iter()
            .any(|line| line.contains("hidden tail"))
    );
    app.update_user_message_hits(0, 2);
    assert!(!app.toggle_user_message_at(2));
    assert!(app.toggle_user_message_at(10));
    app.prepare_transcript_frame();
    assert_eq!(
        app.transcript_rows
            .iter()
            .filter(|line| line.contains("hidden tail"))
            .count(),
        1
    );
    assert!(
        app.transcript_rows
            .iter()
            .any(|line| line.contains("Show less"))
    );
    assert_eq!(app.transcript, vec![original.clone(), original]);
    app.update_user_message_hits(0, 2);
    let collapse = app.user_message_hit_rows[0].0;
    assert!(app.toggle_user_message_at(collapse));
    app.prepare_transcript_frame();
    assert_eq!(app.transcript_height, 18);
}

#[test]
fn user_folding_tracks_wrapping_width_and_leaves_assistant_content_visible() {
    let mut app = App::new(
        vec![
            format!("You: {}", "中".repeat(60)),
            format!("WillDeep: {}", "answer\n".repeat(15)),
        ],
        Language::En,
    );
    app.transcript_width = 80;
    app.prepare_transcript_frame();
    assert!(app.user_message_controls.is_empty());
    app.transcript_width = 10;
    app.prepare_transcript_frame();
    assert_eq!(app.user_message_controls.len(), 2); // Narrow button wraps too.
    assert_eq!(
        app.transcript_rows
            .iter()
            .filter(|line| line.contains("answer"))
            .count(),
        15
    );
}

#[test]
fn hidden_thoughts_and_archives_do_not_shift_user_message_click_targets() {
    let mut app = App::new(
        vec![
            "You: archived".into(),
            "You: old".into(),
            "· + Thought: 1s\nhidden".into(),
            format!("You: {}", "line\n".repeat(20)),
        ],
        Language::En,
    );
    app.archive_floor = 1;
    app.transcript_width = 60;
    app.viewport_height = 40;
    app.prepare_transcript_frame();
    assert_eq!(app.user_message_controls[0].1, 3);
    app.update_user_message_hits(2, 5);
    assert!(app.toggle_user_message_at(app.user_message_hit_rows[0].0));
    assert!(app.expanded_user_messages.contains_key(&3));
}

#[test]
fn search_reveals_a_hidden_user_message_tail() {
    let mut app = App::new(
        vec![format!("You: {}needle", "line\n".repeat(20))],
        Language::En,
    );
    app.transcript_width = 60;
    app.search = Some(SearchState::default());
    app.search.as_mut().unwrap().editor.insert("needle");
    app.refresh_search_matches();
    app.jump_to_search_match();
    app.prepare_transcript_frame();
    assert!(
        app.transcript_rows
            .iter()
            .any(|line| line.contains("needle"))
    );
}

#[test]
fn older_thoughts_do_not_split_tools_or_steal_latest_thought_clicks() {
    let mut app = App::new(
        vec![
            "You: old".into(),
            "· + Thought: 1.0s\nold reasoning".into(),
            "· ✓ read_file".into(),
            "· + Thought: 2.0s\nmore old reasoning".into(),
            "· ✗ run_command".into(),
            "WillDeep: finished".into(),
            "You: latest".into(),
            "· + Thought: 3.0s\nlatest reasoning".into(),
        ],
        Language::En,
    );
    app.ui.show_thinking = true;
    let display = app.display_transcript();
    assert!(!display.rows.iter().any(|row| row.contains("old reasoning")));
    // 更早那段工具调用连汇总都不显示：最近一段只有一条思考，没有工具行。
    assert!(
        !display
            .rows
            .iter()
            .any(|row| row.contains("tool calls folded"))
    );
    assert!(!display.rows.iter().any(|row| row.contains('×')));
    assert!(
        display
            .rows
            .iter()
            .any(|row| row.contains("latest reasoning"))
    );
    app.thought_hit_rows = vec![(7, 0)];
    assert!(app.toggle_thought_at(7));
    assert!(app.transcript[7].starts_with("· − Thought:"));
    assert!(app.transcript[1].starts_with("· + Thought:"));
}

/// 首屏判断只看 `transcript.is_empty()` 会漏掉「列表非空、屏幕全白」：
/// 会话里只剩空白行，或者思考行全被关掉，用户进来看到的是一整屏空白。
#[test]
fn a_blank_or_thought_only_history_still_gets_the_welcome_screen() {
    let workspace = std::path::Path::new("/tmp/willdeep-rs");
    let id = uuid::Uuid::nil();

    // 空白行不算内容。
    let mut app = App::new(vec![String::new(), "   ".to_owned()], Language::En);
    assert!(!app.has_visible_transcript());
    app.ensure_welcome(workspace, id, Language::En);
    assert!(app.has_visible_transcript());
    assert!(app.transcript.iter().any(|row| row.contains("WILLDEEP")));
    assert!(
        !app.transcript.iter().any(String::is_empty),
        "blank rows should be dropped"
    );

    // 思考行在关掉显示时也不算内容。
    let thought = "· + Thought: 1.2s hidden reasoning".to_owned();
    let mut app = App::new(vec![thought.clone()], Language::En);
    app.ui.show_thinking = false;
    assert!(!app.has_visible_transcript());
    app.ensure_welcome(workspace, id, Language::En);
    assert!(app.transcript.iter().any(|row| row.contains("WILLDEEP")));

    // 打开显示时它就是内容，不该再补一遍欢迎语。
    let mut app = App::new(vec![thought], Language::En);
    app.ui.show_thinking = true;
    assert!(app.has_visible_transcript());
    app.ensure_welcome(workspace, id, Language::En);
    assert!(!app.transcript.iter().any(|row| row.contains("WILLDEEP")));
}

/// 简洁 UI 同样要显示首屏：同一段历史，两种布局都能看到产品名。
#[test]
fn the_welcome_screen_is_visible_in_both_layouts() {
    use ratatui::backend::TestBackend;
    for mode in [crate::config::UiMode::Compact, crate::config::UiMode::Full] {
        let mut app = App::new(vec![String::new()], Language::En);
        app.ensure_welcome(
            std::path::Path::new("/tmp/willdeep-rs"),
            uuid::Uuid::nil(),
            Language::En,
        );
        app.ui.mode = mode;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        draw(
            &mut terminal,
            &mut app,
            &SkillCatalog::default(),
            &quiet_test_session(),
        )
        .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            screen.contains("WILLDEEP"),
            "{mode:?} layout lost the welcome screen"
        );
    }
}

/// 启动时装载聊天记录是整体替换，必须排在 startup notices 之前。
/// 之前 daemon 的 display session 在 notices 之后装载，把「首次使用 / 旧配置写法」
/// 那几条系统提示又抹掉了；这条测试把顺序钉住。
#[test]
fn loading_the_display_session_before_notices_keeps_both() {
    let mut app = App::new(Vec::new(), Language::En);
    let notices = vec!["System: no config file yet".to_owned()];
    // 模拟 display session：只有空白行，属于「列表非空但屏幕空白」。
    let mut display = quiet_test_session();
    display.messages.clear();
    app.load_transcript(&display);
    app.transcript.extend(notices.clone());

    assert!(app.has_visible_transcript());
    assert!(app.transcript.iter().any(|row| row.contains("WILLDEEP")));
    assert!(
        app.transcript
            .iter()
            .any(|row| row.contains("no config file yet"))
    );
}

/// 拖选的落点必须落在**文字**所在的那一行，两种布局都要对得上。
///
/// 简洁 UI 没有边框（只有左右各一列内边距），完整 UI 有一圈边框。以前统一按
/// 「外框 + 固定偏移」算，简洁 UI 里每一行都错开一行：拖选 AI 回复，松手复制到
/// 的却是上一行（常常是空行），于是看起来像「选不中、也不复制」。
#[test]
fn dragging_maps_to_the_row_under_the_cursor_in_both_layouts() {
    use ratatui::backend::TestBackend;
    for mode in [crate::config::UiMode::Compact, crate::config::UiMode::Full] {
        let mut app = App::new(
            vec![
                "System: first line".to_owned(),
                "System: second line".to_owned(),
                "System: third line".to_owned(),
            ],
            Language::En,
        );
        app.ui.mode = mode;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        draw(
            &mut terminal,
            &mut app,
            &SkillCatalog::default(),
            &quiet_test_session(),
        )
        .unwrap();

        let content = app.transcript_content;
        assert!(content.height >= 3, "{mode:?}: the chat area is too small");
        assert!(
            app.transcript_rows.len() >= 3,
            "{mode:?}: transcript rows are missing"
        );
        // 画面上第 k 行正文，应该落在离可见起点第 k 行的记录上。错一行的老 bug
        // 在这里会直接对不上。
        let offset = app.transcript_render_offset;
        for visible_row in 0..3u16 {
            let point = app
                .transcript_selection_point(content.x, content.y + visible_row, false)
                .unwrap_or_else(|| panic!("{mode:?}: row {visible_row} is not selectable"));
            assert_eq!(
                point.row,
                offset + visible_row as usize,
                "{mode:?}: drawing row {visible_row} should map to transcript row {}",
                offset + visible_row as usize
            );
        }
        // 正文最后一列也必须可点：收紧到右边框会让最右侧一列选不中。
        assert!(
            app.transcript_selection_point(content.right() - 1, content.y, false)
                .is_some(),
            "{mode:?}: the last content column should be selectable"
        );
        // 正文末尾那一行同样可点：老实现把底部两行当边框排掉，最后一行选不中。
        assert!(
            app.transcript_selection_point(content.x, content.bottom() - 1, false)
                .is_some(),
            "{mode:?}: the bottom content row should be selectable"
        );
    }
}
