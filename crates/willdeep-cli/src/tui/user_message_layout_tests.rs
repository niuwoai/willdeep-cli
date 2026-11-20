use super::*;
use ratatui::backend::TestBackend;

#[test]
fn user_prefix_is_hidden_without_reinterpreting_literal_body_text() {
    let body =
        format!("You: literal\n{ASSISTANT_ROW_PREFIX}quoted\n· + Thought: literal\n**plain**");
    let row = format!("You: {body}");
    let rendered = message_time::render_with_times(
        std::slice::from_ref(&row),
        &[Some("now".into())],
        None,
        200,
        Language::En,
    );
    let rows = text_rows(&rendered);
    assert_eq!(rows[0].trim(), "You: literal");
    assert_eq!(rows[1].trim(), format!("{ASSISTANT_ROW_PREFIX}quoted"));
    assert_eq!(rows[2].trim(), "· + Thought: literal");
    assert_eq!(rows[3].trim(), "**plain** · now");

    let mut app = App::new(vec![row.clone()], Language::En);
    app.transcript_width = 200;
    app.prepare_transcript_frame();
    app.chat_selection = Some(ChatSelection {
        anchor: ChatSelectionPoint { row: 0, column: 0 },
        head: ChatSelectionPoint {
            row: 3,
            column: 199,
        },
    });
    assert_eq!(app.selected_chat_text(), body);
    assert_eq!(app.transcript, [row]);
}

#[test]
fn fold_threshold_uses_body_width_without_the_generated_prefix() {
    let width = 40;
    let body = "x".repeat(content_width(width) * PREVIEW_LINES);
    let mut app = App::new(vec![format!("You: {body}")], Language::En);
    app.transcript_width = width;
    app.viewport_height = 30;
    app.prepare_transcript_frame();
    assert_eq!(app.transcript_rows.len(), PREVIEW_LINES);
    assert!(app.user_message_controls.is_empty());

    app.transcript[0].push('x');
    app.prepare_transcript_frame();
    let (control, _) = app.user_message_controls[0];
    assert_eq!(control, PREVIEW_LINES);
    app.update_user_message_hits(0, 0);
    assert!(app.toggle_user_message_at(control as u16));
    app.prepare_transcript_frame();
    let (control, _) = app.user_message_controls[0];
    assert_eq!(control, PREVIEW_LINES + 1);
    app.update_user_message_hits(0, 0);
    assert!(app.toggle_user_message_at(control as u16));
    app.prepare_transcript_frame();
    assert_eq!(app.user_message_controls[0].0, PREVIEW_LINES);
    assert_eq!(app.transcript, [format!("You: {body}x")]);
}

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
fn indented_user_lines_fit_without_losing_characters() {
    let payload = "x".repeat(60);
    for width in [20, 40, 80] {
        let row = format!("You: code\n{}{payload}", " ".repeat(20));
        let rendered = message_time::render_with_times(&[row], &[None], None, width, Language::En);
        assert!(
            rendered.lines.iter().all(|line| line.width() <= width),
            "user lines overflow the {width}-column viewport"
        );
        let mut terminal = Terminal::new(TestBackend::new(width as u16, 20)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(Paragraph::new(rendered.clone()), frame.area());
            })
            .unwrap();
        let visible = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert_eq!(
            visible.chars().filter(|&ch| ch == 'x').count(),
            payload.len()
        );
    }
}

#[test]
fn multiline_copy_and_quote_exclude_layout_spaces_and_keep_user_indentation() {
    let mut app = App::new(
        vec!["You: first\n    second\n\nlast  ".into()],
        Language::En,
    );
    app.transcript_width = 80;
    for _ in 0..2 {
        app.prepare_transcript_frame();
        let first = &app.transcript_rows[0];
        let last = app.transcript_rows.len() - 1;
        app.chat_selection = Some(ChatSelection {
            anchor: ChatSelectionPoint {
                row: 0,
                column: first.find("first").unwrap(),
            },
            head: ChatSelectionPoint {
                row: last,
                column: 79,
            },
        });
        assert_eq!(app.selected_chat_text(), "first\n    second\n\nlast  ");
    }
    app.quote_chat_selection();
    assert_eq!(app.input.text(), "> first\n>     second\n> \n> last  ");
}

#[test]
fn user_copy_bounds_follow_cached_waiting_and_transient_system_output() {
    let mut app = App::new(vec!["You: first\n    second".into()], Language::En);
    app.transcript_width = 80;
    app.note_awaiting_reply();
    for phase in 0..3 {
        if phase == 1 {
            app.note_compression_started();
        } else if phase == 2 {
            app.dismiss_compression_notice();
            app.stream_transient(StreamKind::Reply, "answer");
            app.transcript_width = 40;
        }
        for _ in 0..2 {
            app.prepare_transcript_frame();
            assert_eq!(
                app.transcript_selection_bounds.len(),
                app.transcript_rows.len()
            );
            let start = app.transcript_rows[0].find("first").unwrap();
            let end = app.transcript_rows[1].find("second").unwrap() + "second".len() - 1;
            // 反向拖选使用相同的正文范围。
            app.chat_selection = Some(ChatSelection {
                anchor: ChatSelectionPoint {
                    row: 1,
                    column: end,
                },
                head: ChatSelectionPoint {
                    row: 0,
                    column: start,
                },
            });
            assert_eq!(app.selected_chat_text(), "first\n    second");
            assert!(
                app.transcript_selection_bounds[2..]
                    .iter()
                    .all(Option::is_none)
            );
        }
    }
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
            .position(|row| row.contains("hello"))
            .unwrap();
        let user = &app.transcript_rows[user_row];
        assert!(!user.contains("You:"));
        let start = user.find("hello").unwrap();
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
        .position(|row| row.contains("question"))
        .unwrap();
    assert!(app.transcript_rows[user + 1].is_empty());
    assert!(app.transcript_rows[user + 2].contains("streaming answer"));
}

#[test]
fn local_queue_hint_preserves_fold_and_attachment_hits_without_polluting_copy() {
    let body = "first\n    second\n\nlast  ";
    for language in [Language::ZhCn, Language::En, Language::Ja] {
        for width in [8, 40, 100] {
            let mut app = App::new(Vec::new(), language);
            app.transcript_width = width;
            app.input.insert(body);
            app.queue_submitted_prompt();
            app.prepare_transcript_frame();
            assert!(
                app.transcript_rows
                    .iter()
                    .all(|row| UnicodeWidthStr::width(row.as_str()) <= width)
            );
            assert!(
                !app.transcript_rows
                    .iter()
                    .any(|row| row.contains("[queued]") || row.contains("［待发］"))
            );
            assert_eq!(app.viewport_record_body(0), body);
            app.chat_selection = Some(ChatSelection {
                anchor: ChatSelectionPoint { row: 0, column: 0 },
                head: ChatSelectionPoint {
                    row: app.transcript_rows.len() - 1,
                    column: width - 1,
                },
            });
            assert_eq!(app.selected_chat_text(), body);
            app.transcript_attachments[0] = vec![MessageAttachment::Text {
                name: "details.txt".into(),
                content: "attachment line\n".repeat(18),
            }];
            // The hint is outside the preview budget and is not a fold control
            // or an attachment target, both before and after expansion.
            app.transcript_render_cache = None;
            app.prepare_transcript_frame();
            let (control, source) = app.user_message_controls[0];
            assert_eq!(source, 0);
            let control_text = app
                .user_message_controls
                .iter()
                .filter(|(_, record)| *record == source)
                .map(|(row, _)| app.transcript_rows[*row].trim())
                .collect::<String>();
            let control_text = control_text
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>();
            assert!(control_text.contains(language.text("显示更多", "Showmore", "もっと見る")));
            let (attachment, source, index) = app.attachment_controls[0];
            assert_eq!((source, index), (0, 0));
            assert!(attachment < control);
            let hint_row = app
                .transcript_rows
                .iter()
                .position(|row| row.contains("Ctrl+Q"))
                .unwrap();
            assert!(hint_row > control);
            app.update_user_message_hits(0, 0);
            assert!(!app.toggle_user_message_at(hint_row as u16));
            assert!(app.toggle_user_message_at(control as u16));
            app.prepare_transcript_frame();
            assert!(app.transcript_rows.iter().any(|row| row.contains("Ctrl+Q")));
            assert!(
                app.transcript_rows
                    .iter()
                    .all(|row| UnicodeWidthStr::width(row.as_str()) <= width)
            );
            app.queued_prompts.pop_front();
            app.mark_queued_prompt_sent_at(body, Some(0));
            app.prepare_transcript_frame();
            assert!(!app.transcript_rows.iter().any(|row| row.contains("Ctrl+Q")));
        }
    }
}

#[test]
fn session_switch_does_not_rebind_local_queue_rows() {
    let mut app = App::new(Vec::new(), Language::En);
    app.transcript_width = 100;
    app.input.insert("LOCAL_QUEUED_TEXT");
    app.queue_submitted_prompt();
    assert_eq!(app.queued_prompts[0].transcript_row, Some(0));

    let mut target = willdeep_core::Session::new(std::env::temp_dir(), None, "target");
    target.messages = vec![willdeep_core::types::Message::user(
        "OTHER_SESSION_QUESTION",
    )];
    app.load_session(&target);

    assert_eq!(app.queued_prompts.len(), 1);
    assert_eq!(app.queued_prompts[0].transcript_row, None);
    app.prepare_transcript_frame();
    let shown = app.transcript_rows.join("\n");
    assert!(shown.contains("OTHER_SESSION_QUESTION"), "{shown}");
    assert!(!shown.contains("LOCAL_QUEUED_TEXT"), "{shown}");
    assert!(!shown.contains("Ctrl+Q"), "{shown}");
    assert!(app.queued_user_records().is_empty());
    assert_eq!(app.viewport_record_body(0), "OTHER_SESSION_QUESTION");
}

#[test]
fn failed_local_submission_keeps_the_unconfirmed_prefix_and_full_body_copy() {
    let mut app = App::new(Vec::new(), Language::En);
    app.transcript_width = 100;
    app.viewport_height = 60;
    app.input.insert("abcdef");
    app.queue_submitted_prompt();
    app.note_runtime_submission_unconfirmed("abcdef", uuid::Uuid::new_v4(), false, Some(0));
    app.prepare_transcript_frame();

    let shown = app.transcript_rows.join("\n");
    assert!(shown.contains("? abcdef"), "{shown}");
    assert!(
        !app.queued_user_records().contains(&0),
        "an unconfirmed row must not claim queue ownership"
    );
    app.chat_selection = Some(ChatSelection {
        anchor: ChatSelectionPoint { row: 0, column: 0 },
        head: ChatSelectionPoint { row: 0, column: 99 },
    });
    assert_eq!(app.selected_chat_text(), "abcdef");
}

#[test]
fn editing_a_queued_prompt_refreshes_the_cached_frame() {
    let mut app = App::new(Vec::new(), Language::En);
    app.transcript_width = 100;
    app.viewport_height = 60;
    app.input.insert("first text");
    app.queue_submitted_prompt();
    app.prepare_transcript_frame();
    assert!(app.transcript_rows.join("\n").contains("first text"));
    // The queue panel save path rewrites the placeholder record in place.
    app.queued_prompts[0].text = "second text".into();
    app.transcript[0] = app.queued_prompt_row("second text");
    app.refresh_transcript_height();
    app.prepare_transcript_frame();
    let shown = app.transcript_rows.join("\n");
    assert!(shown.contains("second text"), "{shown}");
    assert!(!shown.contains("first text"), "{shown}");
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
