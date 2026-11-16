use super::*;
use ratatui::backend::TestBackend;

#[test]
fn user_blocks_share_a_right_edge_and_keep_body_lines_left_aligned() {
    for width in [8, 19, 20, 40, 80, 200] {
        let row = format!("You: {}\nsecond line\n\nlast", "中文内容 ".repeat(10));
        let rendered = message_time::render_with_times(
            &[row],
            &[Some("1分前".into())],
            None,
            width,
            Language::ZhCn,
        );
        assert!(
            rendered.lines.iter().all(|line| line.width() == width),
            "width {width}"
        );
        let indent = rendered.lines[0].spans[0].content.len();
        assert!(
            rendered
                .lines
                .iter()
                .all(|line| line.spans[0].content.len() == indent)
        );
        assert!(
            rendered
                .lines
                .iter()
                .all(|line| line.spans[0].style.bg.is_none())
        );
        assert!(rendered.lines.iter().all(|line| {
            line.spans
                .iter()
                .skip(1)
                .all(|span| span.style.bg == Some(BLOCK_BG))
        }));
        if width >= NARROW_VIEWPORT_WIDTH {
            assert!(indent > 0);
            assert!(width - indent <= MAX_BLOCK_WIDTH);
        }
    }
}

#[test]
fn user_background_preserves_search_highlights_and_visual_selection_columns() {
    let mut app = App::new(vec!["You: 中文 needle".into()], Language::En);
    app.transcript_width = 80;
    app.prepare_transcript_frame();
    let row = &app.transcript_rows[0];
    let byte = row.find("中文").unwrap();
    let column = UnicodeWidthStr::width(&row[..byte]);
    app.chat_selection = Some(ChatSelection {
        anchor: ChatSelectionPoint { row: 0, column },
        head: ChatSelectionPoint {
            row: 0,
            column: column + 3,
        },
    });
    assert_eq!(app.selected_chat_text(), "中文");
    assert_eq!(app.transcript, ["You: 中文 needle"]);

    let rendered =
        message_time::render_with_times(&app.transcript, &[None], Some("needle"), 80, Language::En);
    assert!(
        rendered.lines[0]
            .spans
            .iter()
            .any(|span| span.content.contains("needle") && span.style.bg == Some(Color::Yellow))
    );
}

#[test]
fn rendered_buffers_paint_only_the_user_block_in_full_and_compact_layouts() {
    for mode in [crate::config::UiMode::Full, crate::config::UiMode::Compact] {
        let mut app = App::new(
            vec!["You: hello".into(), format!("{ASSISTANT_ROW_PREFIX}reply")],
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
        let user_row = app
            .transcript_rows
            .iter()
            .position(|row| row.contains("You: hello"))
            .unwrap();
        let user = &app.transcript_rows[user_row];
        let start = user.find("You:").unwrap();
        let y = app.transcript_content.y + user_row as u16;
        let x = app.transcript_content.x + start as u16;
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(x, y)].bg, BLOCK_BG);
        assert_ne!(buffer[(app.transcript_content.x, y)].bg, BLOCK_BG);
        assert_eq!(buffer[(app.transcript_content.right() - 1, y)].bg, BLOCK_BG);
        assert!(start > 0);
        let reply = app
            .transcript_rows
            .iter()
            .position(|row| row.contains("reply"))
            .unwrap();
        assert!(app.transcript_rows[user_row + 1].is_empty());
        assert_eq!(reply, user_row + 2);
        assert_eq!(
            app.transcript_rows[reply].find(&app.assistant_name()),
            Some(0)
        );
    }
}

#[test]
fn streaming_replies_keep_the_same_role_gap_as_completed_replies() {
    let mut app = App::new(vec!["You: question".into()], Language::En);
    app.transcript_width = 80;
    app.stream_transient(StreamKind::Reply, "streaming answer");
    app.prepare_transcript_frame();
    let user = app
        .transcript_rows
        .iter()
        .position(|row| row.contains("You:"))
        .unwrap();
    assert!(app.transcript_rows[user + 1].is_empty());
    assert!(app.transcript_rows[user + 2].contains("streaming answer"));
}

#[test]
fn wrapped_attachments_and_fold_controls_stay_inside_the_user_block() {
    let mut app = App::new(vec!["You: inspect".into()], Language::En);
    app.transcript_width = 40;
    app.viewport_height = 60;
    app.transcript_attachments[0] = vec![MessageAttachment::Text {
        name: "long-attachment-name.txt".into(),
        content: format!("{}hidden tail", "attachment line\n".repeat(18)),
    }];
    app.prepare_transcript_frame();
    let (control, source) = app.user_message_controls[0];
    assert_eq!(source, 0);
    assert!(app.transcript_rows[control].contains("Show more"));
    let (attachment, source, index) = app.attachment_controls[0];
    assert_eq!((source, index), (0, 0));
    assert!(app.transcript_rows[attachment].contains("Attachment"));
    app.update_user_message_hits(0, 0);
    assert!(app.toggle_user_message_at(control as u16));
    app.prepare_transcript_frame();
    assert!(
        app.transcript_rows
            .iter()
            .any(|row| row.contains("hidden tail"))
    );
    assert!(
        app.transcript_rows
            .iter()
            .all(|row| UnicodeWidthStr::width(row.as_str()) <= 40)
    );
}
