use super::*;

#[test]
fn both_tui_layouts_expose_clickable_user_disclosures_without_jumping_to_bottom() {
    for mode in [crate::config::UiMode::Compact, crate::config::UiMode::Full] {
        let original = format!("You: {}hidden tail", "line\n".repeat(20));
        let mut app = App::new(vec![original.clone()], Language::En);
        app.ui.mode = mode;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        let row = app.user_message_hit_rows[0].0;
        app.handle_mouse(
            app.transcript_rect.x + 2,
            row,
            &BackgroundTaskRegistry::default(),
            &SkillCatalog::default(),
        );
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
        assert!(
            app.transcript_rows
                .iter()
                .any(|line| line.contains("hidden tail"))
        );
        assert_eq!(app.transcript_render_offset, 0);
        let row = app.user_message_hit_rows[0].0;
        app.handle_mouse(
            app.transcript_rect.x + 2,
            row,
            &BackgroundTaskRegistry::default(),
            &SkillCatalog::default(),
        );
        draw(&mut terminal, &mut app, &SkillCatalog::default()).unwrap();
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
    assert!(
        display
            .rows
            .iter()
            .any(|row| row.contains("2 earlier tool calls folded") && row.contains("1 failed"))
    );
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
