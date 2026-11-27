//! Deterministic transcript tests; no provider, filesystem, or clipboard access.
use super::*;
use crate::config::UiMode;
use ratatui::backend::TestBackend;

const MODES: [UiMode; 2] = [UiMode::Full, UiMode::Compact];

fn fixture(mode: UiMode, rows: Vec<String>) -> (App, Terminal<TestBackend>) {
    let mut app = App::new(rows, Language::En);
    app.ui.mode = mode;
    app.ui.copy_on_select = false;
    app.ui.compact_status_line = false;
    app.ui.show_thinking = true;
    app.sidebar_visible = false;
    let terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    (app, terminal)
}

fn paint(app: &mut App, terminal: &mut Terminal<TestBackend>) {
    draw(
        terminal,
        app,
        &SkillCatalog::default(),
        &quiet_test_session(),
    )
    .unwrap();
    assert_eq!(app.viewport.frame.rows.len(), app.transcript_rows.len());
    for cell in app.viewport.frame.rows.iter().flat_map(|row| &row.cells) {
        if let Some(raw) = app
            .viewport
            .frame
            .parts
            .get(&(cell.source.point.id, cell.source.point.part))
        {
            assert_eq!(
                raw[cell.source.point.byte..].chars().next(),
                Some(cell.character)
            );
        }
    }
    assert_eq!(
        app.transcript_render_offset,
        app.max_scroll() - app.scroll_from_bottom
    );
}

fn resize(app: &mut App, terminal: &mut Terminal<TestBackend>, width: u16, height: u16) {
    terminal.backend_mut().resize(width, height);
    terminal.resize(Rect::new(0, 0, width, height)).unwrap();
    paint(app, terminal);
}

fn history() -> String {
    (0..80)
        .map(|i| format!("original {i:03} 中文 e\u{301} **same words** repeated repeated"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn original(app: &App, record: usize, part: SourcePart, byte: usize) -> SourcePoint {
    SourcePoint {
        id: app.viewport.records[record],
        part,
        byte,
    }
}

fn read_at(app: &mut App, point: SourcePoint) {
    let row = app.viewport.frame.resolve(point).unwrap().row;
    let offset = row.min(app.max_scroll());
    app.follow_bottom = false;
    app.scroll_from_bottom = app.max_scroll() - offset;
    app.viewport_scrolled();
}

fn assert_original_stays_on_screen(app: &App, point: SourcePoint, screen_row: usize) {
    let resolved = app.viewport.frame.resolve(point).unwrap();
    assert_eq!(resolved.row - app.transcript_render_offset, screen_row);
    let mapped = &app.viewport.frame.rows[resolved.row];
    let cell = mapped
        .cells
        .iter()
        .find(|cell| cell.source.point == point)
        .unwrap();
    let raw = &app.viewport.frame.parts[&(point.id, point.part)];
    assert_eq!(raw[point.byte..].chars().next(), Some(cell.character));
}

fn select(app: &mut App, anchor: SourcePoint, head: SourcePoint) {
    app.chat_selection = Some(ChatSelection {
        anchor: app.viewport.frame.resolve(anchor).unwrap(),
        head: app.viewport.frame.resolve(head).unwrap(),
    });
    app.viewport_capture_selection();
}

fn queued_turn() -> willdeep_runtime_protocol::RuntimeTurn {
    willdeep_runtime_protocol::RuntimeTurn {
        execution_session_id: None,
        id: uuid::Uuid::new_v4(),
        session_id: uuid::Uuid::new_v4(),
        request_id: uuid::Uuid::new_v4(),
        queue_sequence: 1,
        status: willdeep_runtime_protocol::TurnStatus::Queued,
        active_task_id: None,
        attempts: 0,
        created_at: 1,
        started_at: None,
        completed_at: None,
        message_start: None,
        message_end: None,
        message_generation: 0,
        workspace_checkpoint: None,
    }
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn mouse_at(app: &App, kind: MouseEventKind, point: SourcePoint) -> MouseEvent {
    let resolved = app.viewport.frame.resolve(point).unwrap();
    assert!(resolved.row >= app.transcript_render_offset);
    mouse(
        kind,
        app.transcript_content.x + resolved.column as u16,
        app.transcript_content.y + (resolved.row - app.transcript_render_offset) as u16,
    )
}

#[test]
fn reading_original_bytes_survives_stream_growth_and_transient_removal_in_both_layouts() {
    for mode in MODES {
        let body = history();
        let (mut app, mut terminal) = fixture(mode, vec![format!("{ASSISTANT_ROW_PREFIX}{body}")]);
        paint(&mut app, &mut terminal);
        let byte = body.lines().take(24).map(|line| line.len() + 1).sum();
        let point = original(&app, 0, SourcePart::Body, byte);
        read_at(&mut app, point);
        paint(&mut app, &mut terminal);
        let screen_row =
            app.viewport.frame.resolve(point).unwrap().row - app.transcript_render_offset;
        for _ in 0..6 {
            app.stream_transient(StreamKind::Reply, "new text 中文\n".repeat(8).as_str());
            paint(&mut app, &mut terminal);
            assert_original_stays_on_screen(&app, point, screen_row);
        }
        assert!(app.viewport.new_content > 0);
        app.transient_thought = None;
        paint(&mut app, &mut terminal);
        assert_original_stays_on_screen(&app, point, screen_row);
        assert!(!app.follow_bottom);
    }
}

#[test]
fn repeated_text_resolves_by_original_byte_through_repeated_resize() {
    for mode in MODES {
        let body = "identical **words** 中文 e\u{301} ".repeat(180);
        let (mut app, mut terminal) = fixture(mode, vec![format!("{ASSISTANT_ROW_PREFIX}{body}")]);
        paint(&mut app, &mut terminal);
        let point = original(
            &app,
            0,
            SourcePart::Body,
            "identical **words** 中文 e\u{301} ".len() * 60,
        );
        read_at(&mut app, point);
        paint(&mut app, &mut terminal);
        let anchor = app.viewport.reading.unwrap();
        for (width, height) in [(43, 24), (102, 27), (32, 24), (80, 24)] {
            resize(&mut app, &mut terminal, width, height);
            assert_original_stays_on_screen(&app, anchor.point, anchor.screen_row);
            assert_eq!(app.viewport.new_content, 0);
        }
    }
}

#[test]
fn mouse_down_stops_follow_before_delta_and_selection_survives_long_stream_and_commit() {
    for mode in MODES {
        let (mut app, mut terminal) = fixture(mode, vec!["You: question".into()]);
        app.stream_transient(StreamKind::Reply, "alpha beta\n");
        paint(&mut app, &mut terminal);
        let (_, id) = app.viewport.stream.unwrap();
        let anchor = SourcePoint {
            id,
            part: SourcePart::Body,
            byte: 0,
        };
        let head = SourcePoint {
            id,
            part: SourcePart::Body,
            byte: "alpha bet".len(),
        };
        let down = mouse_at(&app, MouseEventKind::Down(MouseButton::Left), anchor);
        app.handle_chat_selection_mouse(down);
        assert!(
            !app.follow_bottom,
            "Down must close the pre-Drag follow window"
        );
        app.stream_transient(StreamKind::Reply, "first delta\n");
        paint(&mut app, &mut terminal);
        let drag = mouse_at(&app, MouseEventKind::Drag(MouseButton::Left), head);
        assert!(app.handle_chat_selection_mouse(drag));
        app.stream_transient(StreamKind::Reply, &"x".repeat(1600));
        assert!(
            app.transient_thought
                .as_deref()
                .unwrap()
                .starts_with("alpha beta\n"),
            "the execution path must retain the full Reply source"
        );
        paint(&mut app, &mut terminal);
        let up = mouse_at(&app, MouseEventKind::Up(MouseButton::Left), head);
        assert!(app.handle_chat_selection_mouse(up));
        assert_eq!(app.selected_chat_text(), "alpha beta");
        assert_eq!(app.selection_copy_text(), None);
        resize(&mut app, &mut terminal, 45, 24);
        assert_eq!(app.selected_chat_text(), "alpha beta");
        let body = app.transient_thought.clone().unwrap();
        let news_before_commit = app.viewport.new_content;
        let index = app.transcript.len();
        assert_eq!(app.note_reply(&body), Some(body));
        assert!(app.transient_thought.is_none());
        paint(&mut app, &mut terminal);
        assert_eq!(app.selected_chat_text(), "alpha beta");
        let selection = app.viewport.selection.unwrap();
        assert_eq!(selection.anchor.id, app.viewport.records[index]);
        assert_eq!(selection.anchor.byte, 0);
        assert_eq!(app.viewport.new_content, news_before_commit);
    }
}

#[test]
fn source_selection_copy_and_highlight_agree_after_reflow_and_soft_wrap() {
    for mode in MODES {
        let body = "中e\u{301} alpha **beta** gamma\nlast line";
        let (mut app, mut terminal) = fixture(mode, vec![format!("{ASSISTANT_ROW_PREFIX}{body}")]);
        paint(&mut app, &mut terminal);
        let start = original(&app, 0, SourcePart::Body, 0);
        let end = original(&app, 0, SourcePart::Body, body.len() - 1);
        app.chat_selection = Some(ChatSelection {
            anchor: app.viewport.frame.resolve(start).unwrap(),
            head: app.viewport.frame.resolve(end).unwrap(),
        });
        app.viewport_capture_selection();
        app.follow_bottom = false;
        for width in [23, 47, 80] {
            resize(&mut app, &mut terminal, width, 30);
            assert_eq!(
                app.selected_chat_text(),
                "中e\u{301} alpha beta gamma\nlast line"
            );
            let (transient, notice, waiting) = app.prepare_transcript_frame();
            let offset = app.transcript_render_offset;
            let mut text =
                app.transcript_viewport(&transient, notice.as_ref(), waiting.as_ref(), offset);
            app.highlight_chat_selection(&mut text, offset);
            let highlighted = text
                .lines
                .iter()
                .flat_map(|line| &line.spans)
                .filter(|span| span.style.bg == Some(MODAL_BG))
                .map(|span| span.content.as_ref())
                .collect::<String>();
            assert_eq!(
                highlighted
                    .chars()
                    .filter(|ch| !ch.is_whitespace())
                    .collect::<String>(),
                "中e\u{301}alphabetagammalastline"
            );
        }
    }
}

#[test]
fn user_tool_and_thought_folding_fall_back_to_the_same_record_then_restore_original_bytes() {
    for mode in MODES {
        for kind in ["user", "tool", "thought"] {
            let detail = (0..30)
                .map(|i| format!("detail {i:02} 中文"))
                .collect::<Vec<_>>()
                .join("\n");
            let row = match kind {
                "user" => format!("You: {detail}"),
                "tool" => "· ✓ run_command · fixture".to_owned(),
                _ => format!("· − Thought: 1.0s\n{detail}"),
            };
            let rows = vec![
                "You: initial".to_owned(),
                row.clone(),
                format!("{ASSISTANT_ROW_PREFIX}{}", history()),
            ];
            let (mut app, mut terminal) = fixture(mode, rows);
            app.tool_rows_expanded = true;
            match kind {
                "user" => {
                    app.expanded_user_messages.insert(1, row);
                }
                "tool" => {
                    app.tool_expansions.insert(1, detail.clone());
                    app.expanded_tool_rows.insert(1);
                }
                _ => {}
            }
            paint(&mut app, &mut terminal);
            let part = if kind == "tool" {
                SourcePart::ToolDetail
            } else {
                SourcePart::Body
            };
            let point = original(
                &app,
                1,
                part,
                detail.lines().take(12).map(|line| line.len() + 1).sum(),
            );
            read_at(&mut app, point);
            paint(&mut app, &mut terminal);
            match kind {
                "user" => {
                    app.expanded_user_messages.remove(&1);
                }
                "tool" => {
                    app.expanded_tool_rows.remove(&1);
                }
                _ => {
                    app.transcript[1] = app.transcript[1].replacen("− Thought:", "+ Thought:", 1);
                }
            }
            paint(&mut app, &mut terminal);
            let position = app.viewport.frame.resolve(point).unwrap();
            assert_eq!(app.viewport.frame.rows[position.row].fallback.id, point.id);
            assert!(
                !app.viewport
                    .frame
                    .rows
                    .iter()
                    .flat_map(|row| &row.cells)
                    .any(|cell| cell.source.point == point)
            );
            assert_eq!(app.viewport.new_content, 0);
            match kind {
                "user" => {
                    app.expanded_user_messages
                        .insert(1, format!("You: {detail}"));
                }
                "tool" => {
                    app.expanded_tool_rows.insert(1);
                }
                _ => {
                    app.transcript[1] = app.transcript[1].replacen("+ Thought:", "− Thought:", 1);
                }
            }
            paint(&mut app, &mut terminal);
            assert_original_stays_on_screen(&app, point, 0);
        }
    }
}

#[test]
fn warmed_archive_cache_reveals_real_records_without_moving_the_reading_source() {
    for mode in MODES {
        let mut rows = Vec::new();
        for i in 0..120 {
            rows.extend([
                format!("You: round {i}"),
                format!("{ASSISTANT_ROW_PREFIX}answer {i}"),
                format!("{TURN_DIVIDER_PREFIX}round {i} ──"),
            ]);
        }
        let (mut app, mut terminal) = fixture(mode, rows);
        app.archive_old_rounds();
        assert!(app.archive_floor > 0);
        paint(&mut app, &mut terminal);
        paint(&mut app, &mut terminal); // warm the cache before changing archive_floor
        app.scroll_to_top();
        app.viewport_scrolled();
        paint(&mut app, &mut terminal);
        let anchor = app.viewport.reading.unwrap();
        let old_floor = app.archive_floor;
        let old_height = app.transcript_rows.len();
        assert!(app.reveal_archived_rounds());
        assert!(app.archive_floor < old_floor);
        paint(&mut app, &mut terminal);
        assert!(app.transcript_rows.len() > old_height);
        let revealed = app.viewport.records[app.archive_floor];
        assert!(
            app.viewport
                .frame
                .rows
                .iter()
                .any(|row| row.fallback.id == revealed)
        );
        assert_original_stays_on_screen(&app, anchor.point, anchor.screen_row);
        assert_eq!(app.viewport.new_content, 0);
    }
}

#[test]
fn duplicate_records_keep_identity_when_a_preceding_record_is_removed() {
    for mode in MODES {
        let (mut app, mut terminal) = fixture(
            mode,
            vec![
                "You: duplicate".into(),
                "You: duplicate".into(),
                format!("{ASSISTANT_ROW_PREFIX}{}", history()),
            ],
        );
        paint(&mut app, &mut terminal);
        let point = original(&app, 1, SourcePart::Body, 0);
        read_at(&mut app, point);
        paint(&mut app, &mut terminal);
        app.drop_transcript_row(0);
        paint(&mut app, &mut terminal);
        assert_eq!(app.viewport.records[0], point.id);
        assert_original_stays_on_screen(&app, point, 0);
    }
}

#[test]
fn short_stream_delta_resumes_following_one_line_from_the_bottom() {
    for mode in MODES {
        let (mut app, mut terminal) =
            fixture(mode, vec![format!("{ASSISTANT_ROW_PREFIX}{}", history())]);
        app.stream_transient(StreamKind::Reply, "alpha");
        paint(&mut app, &mut terminal);
        app.scroll_up(1);
        paint(&mut app, &mut terminal);
        assert_eq!(app.scroll_from_bottom, 1);
        assert!(!app.follow_bottom);
        let height = app.transcript_height;

        app.stream_transient(StreamKind::Reply, " beta");
        paint(&mut app, &mut terminal);
        assert_eq!(app.transcript_height, height);
        assert!(app.follow_bottom);
        assert_eq!(app.scroll_from_bottom, 0);
        assert_eq!(app.viewport.new_content, 0);
        assert_eq!(app.viewport.indicator, Rect::default());
    }
}

#[test]
fn near_bottom_scroll_resumes_following_without_blocking_upward_reading() {
    let tick = super::super::event_loop::SCROLL_LINES_PER_TICK;
    for mode in MODES {
        let (mut app, mut terminal) =
            fixture(mode, vec![format!("{ASSISTANT_ROW_PREFIX}{}", history())]);
        paint(&mut app, &mut terminal);
        app.scroll_up(tick);
        paint(&mut app, &mut terminal);
        assert_eq!(app.scroll_from_bottom, tick);
        assert!(!app.follow_bottom);
        assert!(app.viewport.indicator.width > 0);
        app.scroll_up(tick);
        paint(&mut app, &mut terminal);
        assert_eq!(app.scroll_from_bottom, 2 * tick);
        app.scroll_down(tick);
        paint(&mut app, &mut terminal);
        assert!(app.follow_bottom);
        assert_eq!(app.viewport.indicator, Rect::default());
        app.append_transcript("System: newest line".into());
        paint(&mut app, &mut terminal);
        assert_eq!(app.scroll_from_bottom, 0);
    }
}

#[test]
fn growing_the_viewport_clears_reading_when_the_transcript_fits() {
    for mode in MODES {
        let (mut app, mut terminal) =
            fixture(mode, vec![format!("{ASSISTANT_ROW_PREFIX}{}", history())]);
        paint(&mut app, &mut terminal);
        app.scroll_up(10);
        paint(&mut app, &mut terminal);
        assert!(app.viewport.reading.is_some());
        resize(&mut app, &mut terminal, 120, 100);
        assert_eq!(app.max_scroll(), 0);
        assert!(app.follow_bottom);
        assert!(app.viewport.reading.is_none());
        assert_eq!(app.viewport.indicator, Rect::default());

        let tick = App::NEAR_BOTTOM_LINES;
        for height in (24..100).rev() {
            resize(&mut app, &mut terminal, 120, height);
            if app.max_scroll() == tick {
                break;
            }
        }
        assert_eq!(app.max_scroll(), tick);
        app.scroll_up(tick);
        paint(&mut app, &mut terminal);
        assert!(app.follow_bottom);
        assert_eq!(app.viewport.indicator, Rect::default());
    }
}

#[test]
fn mouse_expanding_system_notices_returns_to_latest_in_both_layouts() {
    let dispatch = |app: &mut App, kind, y| {
        let x = app.transcript_content.x + 2;
        if !app.handle_chat_selection_mouse(mouse(kind, x, y))
            && matches!(kind, MouseEventKind::Down(_))
        {
            app.handle_mouse(
                x,
                y,
                &BackgroundTaskRegistry::default(),
                &SkillCatalog::default(),
            );
        }
    };
    let down = MouseEventKind::Down(MouseButton::Left);
    let up = MouseEventKind::Up(MouseButton::Left);
    let control = |app: &App, source, collapse| {
        app.tool_row_hits
            .iter()
            .find(|(_, record, close)| (*record, *close) == (source, collapse))
            .expect("disclosure control is visible")
            .0
    };
    const SYSTEM_SOURCE: usize = 1;
    const TOOL_SOURCE: usize = 3;

    for mode in MODES {
        for was_reading in [false, true] {
            let (mut app, mut terminal) = fixture(
                mode,
                vec![
                    format!("{ASSISTANT_ROW_PREFIX}{}", history()),
                    "System: model changed".into(),
                    "System: settings saved".into(),
                    narration::tool_line(narration::TOOL_DONE, "read_file", None),
                    format!(
                        "{ASSISTANT_ROW_PREFIX}{}latest reply",
                        "reply line\n".repeat(10)
                    ),
                ],
            );
            app.tool_expansions
                .insert(TOOL_SOURCE, "tool detail".into());
            resize(&mut app, &mut terminal, 80, 32);
            const SCROLL_DISTANCE: usize = App::NEAR_BOTTOM_LINES + 5;
            if was_reading {
                app.scroll_up(SCROLL_DISTANCE);
                paint(&mut app, &mut terminal);
                assert!(app.viewport.indicator.height > 0);
            }
            assert_eq!(app.follow_bottom, !was_reading);

            // A repaint can replace the tool hit with the system group at the same y.
            let row = control(&app, TOOL_SOURCE, false);
            dispatch(&mut app, down, row);
            app.tool_row_hits.retain(|(hit_row, _, _)| *hit_row != row);
            app.tool_row_hits.push((row, SYSTEM_SOURCE, false));
            dispatch(&mut app, up, row);
            assert!(!app.tool_row_open(SYSTEM_SOURCE));
            assert!(!app.follow_bottom);
            paint(&mut app, &mut terminal);

            let row = control(&app, SYSTEM_SOURCE, false);
            dispatch(&mut app, down, row);
            app.append_transcript("New output between mouse events".into());
            paint(&mut app, &mut terminal);
            assert!(!app.tool_row_open(SYSTEM_SOURCE));
            assert_eq!(control(&app, SYSTEM_SOURCE, false), row);
            assert!(!app.follow_bottom);
            assert!(app.viewport.new_content > 0);

            dispatch(&mut app, up, row);
            assert!(app.tool_row_open(SYSTEM_SOURCE));
            assert!(app.follow_bottom, "{mode:?}, was_reading={was_reading}");
            assert!(app.viewport.reading.is_none());
            assert_eq!(app.viewport.new_content, 0);
            paint(&mut app, &mut terminal);
            assert_eq!(app.transcript_render_offset, app.max_scroll());
            assert_eq!(app.viewport.indicator, Rect::default());
            control(&app, SYSTEM_SOURCE, true);

            let before = app.transcript_render_offset;
            app.append_transcript("Output after expansion".into());
            paint(&mut app, &mut terminal);
            assert!(app.transcript_render_offset > before);
            assert_eq!(app.transcript_render_offset, app.max_scroll());
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .contains("Output after expansion")
            );

            app.scroll_up(SCROLL_DISTANCE);
            paint(&mut app, &mut terminal);
            let reading_offset = app.transcript_render_offset;
            let row = control(&app, SYSTEM_SOURCE, true);
            dispatch(&mut app, down, row);
            dispatch(&mut app, up, row);
            paint(&mut app, &mut terminal);
            assert!(!app.tool_row_open(SYSTEM_SOURCE));
            assert!(!app.follow_bottom);
            assert_eq!(app.transcript_render_offset, reading_offset);

            let row = control(&app, TOOL_SOURCE, false);
            dispatch(&mut app, down, row);
            dispatch(&mut app, up, row);
            paint(&mut app, &mut terminal);
            assert!(app.tool_row_open(TOOL_SOURCE));
            assert!(!app.follow_bottom);
            assert_eq!(app.transcript_render_offset, reading_offset);
        }
    }
}

#[test]
fn indicator_uses_the_separator_outside_the_body_and_the_whole_row_is_clickable() {
    for mode in MODES {
        for language in [Language::ZhCn, Language::En, Language::Ja] {
            let (mut app, mut terminal) =
                fixture(mode, vec![format!("{ASSISTANT_ROW_PREFIX}{}", history())]);
            app.language = language;
            paint(&mut app, &mut terminal);
            app.scroll_up(5);
            app.viewport_scrolled();
            paint(&mut app, &mut terminal);
            app.stream_transient(StreamKind::Reply, "news");
            paint(&mut app, &mut terminal);
            assert!(app.viewport.new_content > 0);
            let height = app.viewport_height;
            let content = app.transcript_content;
            paint(&mut app, &mut terminal);
            assert_eq!(app.viewport_height, height);
            assert_eq!(app.transcript_content, content);
            let indicator = app.viewport.indicator;
            let y = indicator.y;
            assert!(y >= content.bottom());
            assert_eq!(
                y,
                if mode == UiMode::Compact {
                    app.prompt_rect.y
                } else {
                    app.transcript_rect.bottom() - 1
                }
            );
            let expected = language.text(
                "有新内容 · 点击返回最新 (End)",
                "New content · Click for latest (End)",
                "新しい内容 · クリックで最新へ (End)",
            );
            let mut tail = indicator.x;
            for character in expected.chars() {
                assert_eq!(
                    terminal.backend().buffer()[(tail, y)].symbol(),
                    character.to_string()
                );
                tail += unicode_width::UnicodeWidthChar::width(character).unwrap() as u16;
            }
            for x in tail..indicator.right() {
                assert_eq!(terminal.backend().buffer()[(x, y)].symbol(), " ");
            }
            for x in [content.x, 4, content.right() - 1] {
                assert!(app.reading_indicator_hit(x, y));
            }
            assert!(!app.reading_indicator_hit(content.x, y - 1));
            assert!(app.handle_chat_selection_mouse(mouse(
                MouseEventKind::Down(MouseButton::Left),
                4,
                y
            )));
            paint(&mut app, &mut terminal);
            assert!(app.follow_bottom);
            assert_eq!(app.scroll_from_bottom, 0);
            assert_eq!(app.viewport.new_content, 0);
            assert!(!app.reading_indicator_hit(4, y));
        }
    }
}

#[test]
fn footer_keeps_its_complete_cells_during_click_drag_new_content_and_resize() {
    for mode in MODES {
        for language in [Language::ZhCn, Language::En, Language::Ja] {
            let footer = language.text(
                "── 本轮结束 · 首答 8.62s · 总耗时 9.53s · 输入 77.53K · 输出 106 ──",
                "── Turn ended · first reply 8.62s · total 9.53s · in 77.53K · out 106 ──",
                "── ターン終了 · 初答 8.62s · 合計 9.53s · 入力 77.53K · 出力 106 ──",
            );
            let (mut app, mut terminal) = fixture(
                mode,
                vec![
                    format!("{ASSISTANT_ROW_PREFIX}{}", history()),
                    footer.into(),
                ],
            );
            app.language = language;
            // Match the normal UI: a copy notice must not introduce a new status
            // row and shrink the viewport while comparing the bottom statistics.
            app.ui.compact_status_line = true;
            app.input.insert("keep this draft");
            resize(&mut app, &mut terminal, 120, 32);
            let point = original(&app, 1, SourcePart::Body, 0);
            let row = app.viewport.frame.resolve(point).unwrap().row;
            let y = app.transcript_content.y + (row - app.transcript_render_offset) as u16;
            assert_eq!(y, app.transcript_content.bottom() - 1);
            let cells = |app: &App, terminal: &Terminal<TestBackend>| {
                // Status feedback can change the compact footer's height. Resolve
                // the original statistics again instead of inspecting an old y.
                let row = app.viewport.frame.resolve(point).unwrap().row;
                let y = app.transcript_content.y + (row - app.transcript_render_offset) as u16;
                assert!(y < app.transcript_content.bottom());
                let left = app.transcript_content.x;
                (left..left + UnicodeWidthStr::width(footer) as u16)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol().to_owned())
                    .collect::<Vec<_>>()
            };
            let before = cells(&app, &terminal);
            let down = mouse_at(&app, MouseEventKind::Down(MouseButton::Left), point);
            app.handle_chat_selection_mouse(down);
            paint(&mut app, &mut terminal);
            assert_eq!(
                cells(&app, &terminal),
                before,
                "Down changed {mode:?}/{language:?} footer"
            );
            assert!(!app.follow_bottom);
            assert_eq!(app.scroll_from_bottom, 0);
            assert_eq!(app.viewport.indicator, Rect::default());
            let up = mouse_at(&app, MouseEventKind::Up(MouseButton::Left), point);
            app.handle_chat_selection_mouse(up);
            paint(&mut app, &mut terminal);
            assert_eq!(
                cells(&app, &terminal),
                before,
                "single click changed footer"
            );
            let end = original(&app, 1, SourcePart::Body, footer.rfind('─').unwrap());
            let down = mouse_at(&app, MouseEventKind::Down(MouseButton::Left), point);
            app.handle_chat_selection_mouse(down);
            let drag = mouse_at(&app, MouseEventKind::Drag(MouseButton::Left), end);
            app.handle_chat_selection_mouse(drag);
            let up = mouse_at(&app, MouseEventKind::Up(MouseButton::Left), end);
            app.handle_chat_selection_mouse(up);
            paint(&mut app, &mut terminal);
            assert_eq!(cells(&app, &terminal), before, "drag changed footer");
            let copied = app.selected_chat_text();
            assert!(
                copied.is_empty(),
                "generated footer must remain outside body copying"
            );
            for width in [84, 120] {
                resize(&mut app, &mut terminal, width, 32);
                assert_eq!(app.selected_chat_text(), copied);
                if app.scroll_from_bottom == 0 {
                    assert_eq!(app.viewport.indicator, Rect::default());
                } else {
                    assert!(app.viewport.indicator.y >= app.transcript_content.bottom());
                }
            }
            assert_eq!(cells(&app, &terminal), before, "resize changed footer");
            app.stream_transient(StreamKind::Reply, "new content\n");
            paint(&mut app, &mut terminal);
            assert!(app.viewport.new_content > 0);
            assert_eq!(
                cells(&app, &terminal),
                before,
                "new content covered the anchored footer"
            );
            assert_eq!(app.input.text(), "keep this draft");
            app.viewport_return_to_latest();
            paint(&mut app, &mut terminal);
            assert_eq!(app.viewport.indicator, Rect::default());
        }
    }
}

#[test]
fn waiting_ticks_layout_and_fold_changes_do_not_count_as_new_body() {
    for mode in MODES {
        let (mut app, mut terminal) = fixture(
            mode,
            vec![
                format!("{ASSISTANT_ROW_PREFIX}{}", history()),
                "You: pending".into(),
            ],
        );
        paint(&mut app, &mut terminal);
        app.scroll_up(15);
        app.viewport_scrolled();
        paint(&mut app, &mut terminal);
        app.note_awaiting_reply();
        paint(&mut app, &mut terminal);
        app.note_compression_started();
        paint(&mut app, &mut terminal);
        app.note_compression_completed();
        paint(&mut app, &mut terminal);
        app.dismiss_compression_notice();
        resize(&mut app, &mut terminal, 61, 28);
        assert_eq!(app.viewport.new_content, 0);
        app.transcript[0].push_str("new original body");
        paint(&mut app, &mut terminal);
        assert_eq!(app.viewport.new_content, 1);
        paint(&mut app, &mut terminal);
        assert_eq!(app.viewport.new_content, 1);
    }
}

#[test]
fn markdown_table_cells_keep_raw_offsets_without_selecting_box_drawing() {
    for mode in MODES {
        let body = "| **same** | `same` |\n| --- | --- |\n| same<br>again | 中文 |";
        let (mut app, mut terminal) = fixture(mode, vec![format!("{ASSISTANT_ROW_PREFIX}{body}")]);
        paint(&mut app, &mut terminal);
        let start = original(&app, 0, SourcePart::Body, "| **".len());
        let end = original(&app, 0, SourcePart::Body, "| **sam".len());
        app.chat_selection = Some(ChatSelection {
            anchor: app.viewport.frame.resolve(start).unwrap(),
            head: app.viewport.frame.resolve(end).unwrap(),
        });
        app.viewport_capture_selection();
        assert_eq!(app.selected_chat_text(), "same");
        for width in [29, 50, 80] {
            resize(&mut app, &mut terminal, width, 24);
            assert_eq!(app.selected_chat_text(), "same");
            let resolved = app.viewport.frame.resolve(start).unwrap();
            let cell = app.viewport.frame.rows[resolved.row]
                .cells
                .iter()
                .find(|cell| cell.source.point == start)
                .unwrap();
            assert_eq!(cell.character, 's');
        }
    }
}

#[test]
fn finalized_narration_keeps_its_source_separate_from_the_next_reply() {
    for mode in MODES {
        for concise in [false, true] {
            let (mut app, mut terminal) = fixture(mode, vec!["You: question".into()]);
            app.begin_turn(true, "working".into());
            app.response_style = Some(if concise {
                willdeep_core::ResponseStyle::Short
            } else {
                willdeep_core::ResponseStyle::Detailed
            });
            app.stream_transient(StreamKind::Reply, "alpha beta");
            paint(&mut app, &mut terminal);
            let id = app.viewport.stream.unwrap().1;
            let anchor = SourcePoint {
                id,
                part: SourcePart::Body,
                byte: 0,
            };
            let head = SourcePoint { byte: 9, ..anchor };
            read_at(&mut app, anchor);
            select(&mut app, anchor, head);
            let news = app.viewport.new_content;
            app.note_narration("alpha beta");
            paint(&mut app, &mut terminal);
            assert_eq!(app.selected_chat_text(), "alpha beta");
            assert_eq!(app.viewport.new_content, news);
            app.stream_transient(StreamKind::Reply, "next paragraph");
            paint(&mut app, &mut terminal);
            let next = app.viewport.stream.unwrap().1;
            assert_ne!(next, id);
            app.note_narration("next paragraph");
            app.flush_held_narration();
            paint(&mut app, &mut terminal);
            assert_eq!(
                app.viewport.selection.unwrap().anchor.id,
                app.viewport.records[1]
            );
            assert_eq!(app.selected_chat_text(), "alpha beta");
            assert!(app.viewport.held_stream.is_none());
            assert!(app.viewport.stream.is_none());
        }
    }
}

#[test]
fn reasoning_and_unfinalized_replies_commit_at_real_source_tool_and_model_boundaries() {
    for mode in MODES {
        for boundary in ["kind", "tool", "model"] {
            let (mut app, mut terminal) = fixture(mode, vec!["You: question".into()]);
            app.stream_transient(StreamKind::Reasoning, "reason");
            paint(&mut app, &mut terminal);
            let reasoning = app.viewport.stream.unwrap().1;
            app.follow_bottom = false;
            app.viewport.reading = Some(ReadingAnchor {
                point: SourcePoint {
                    id: reasoning,
                    part: SourcePart::Body,
                    byte: 0,
                },
                screen_row: 0,
            });
            app.stream_transient(StreamKind::Reply, "alpha beta");
            assert_eq!(
                app.viewport.reading.unwrap().point.id,
                app.viewport.records[1]
            );
            paint(&mut app, &mut terminal);
            let reply = app.viewport.stream.unwrap().1;
            let anchor = SourcePoint {
                id: reply,
                part: SourcePart::Body,
                byte: 0,
            };
            select(&mut app, anchor, SourcePoint { byte: 9, ..anchor });
            match boundary {
                "kind" => app.stream_transient(StreamKind::Reasoning, "next reason"),
                "tool" => app.note_tool_requested("read_file", None, None),
                _ => app.note_model_step_started(),
            }
            paint(&mut app, &mut terminal);
            assert_eq!(
                app.transcript[2],
                format!("{ASSISTANT_ROW_PREFIX}alpha beta")
            );
            assert_eq!(
                app.viewport.selection.unwrap().anchor.id,
                app.viewport.records[2]
            );
            assert_eq!(app.selected_chat_text(), "alpha beta");
        }
    }
}

#[test]
fn user_selection_keeps_original_text_while_folded_archived_and_restored() {
    for mode in MODES {
        let body = (0..30)
            .map(|i| format!("detail {i:02} 中文"))
            .collect::<Vec<_>>()
            .join("\n");
        let row = format!("You: {body}");
        let (mut app, mut terminal) = fixture(
            mode,
            vec![row.clone(), format!("{ASSISTANT_ROW_PREFIX}{}", history())],
        );
        app.expanded_user_messages.insert(0, row.clone());
        paint(&mut app, &mut terminal);
        let byte = body.lines().take(12).map(|line| line.len() + 1).sum();
        let anchor = original(&app, 0, SourcePart::Body, byte);
        let head = SourcePoint {
            byte: byte + "detail 12".len() - 1,
            ..anchor
        };
        read_at(&mut app, anchor);
        let reading = app.viewport.reading.unwrap();
        select(&mut app, anchor, head);
        assert_eq!(app.selected_chat_text(), "detail 12");
        app.expanded_user_messages.remove(&0);
        paint(&mut app, &mut terminal);
        assert_eq!(app.selected_chat_text(), "detail 12");
        assert_eq!(app.viewport.reading.unwrap().point, reading.point);
        app.archive_floor = 1;
        paint(&mut app, &mut terminal);
        assert_eq!(app.selected_chat_text(), "detail 12");
        assert_eq!(app.viewport.reading.unwrap().point, reading.point);
        app.archive_floor = 0;
        app.expanded_user_messages.insert(0, row);
        resize(&mut app, &mut terminal, 45, 24);
        assert_eq!(app.viewport.reading.unwrap().point, reading.point);
        assert_original_stays_on_screen(&app, reading.point, reading.screen_row);
        assert_eq!(app.viewport.selection.unwrap().anchor.id, anchor.id);
        assert_eq!(app.viewport.selection.unwrap().anchor.byte, byte);
        assert_eq!(app.selected_chat_text(), "detail 12");
    }
}

#[test]
fn runtime_status_prefix_changes_do_not_move_or_recount_original_body() {
    for mode in MODES {
        let (mut app, mut terminal) = fixture(mode, Vec::new());
        let receipt = queued_turn();
        let record = app
            .echo_runtime_submission("alpha 中文 beta", &receipt, false, None)
            .unwrap();
        app.append_transcript(format!("{ASSISTANT_ROW_PREFIX}{}", history()));
        paint(&mut app, &mut terminal);
        let anchor = original(&app, record, SourcePart::Body, "alpha ".len());
        let head = SourcePoint {
            byte: "alpha 中".len(),
            ..anchor
        };
        read_at(&mut app, anchor);
        paint(&mut app, &mut terminal);
        select(&mut app, anchor, head);
        assert_eq!(app.selected_chat_text(), "中文");
        let news = app.viewport.new_content;
        app.note_runtime_input_started(receipt.id, "alpha 中文 beta");
        paint(&mut app, &mut terminal);
        assert_eq!(app.viewport.new_content, news);
        assert_eq!(app.viewport.selection.unwrap().anchor.byte, anchor.byte);
        assert_eq!(app.selected_chat_text(), "中文");
        resize(&mut app, &mut terminal, 43, 24);
        assert_eq!(app.selected_chat_text(), "中文");
    }
}

#[test]
fn deleting_duplicate_receipt_records_preserves_survivor_identity_and_tombstones() {
    for mode in MODES {
        let (mut app, mut terminal) = fixture(mode, Vec::new());
        let first = queued_turn();
        let second = queued_turn();
        app.echo_runtime_submission("same", &first, false, None);
        app.echo_runtime_submission("same", &second, false, None);
        paint(&mut app, &mut terminal);
        let survivor = original(&app, 1, SourcePart::Body, 0);
        select(
            &mut app,
            survivor,
            SourcePoint {
                byte: 3,
                ..survivor
            },
        );
        app.drop_transcript_row(0);
        paint(&mut app, &mut terminal);
        assert_eq!(app.pending_runtime_echoes[0].record, None);
        assert_eq!(app.pending_runtime_echoes[1].record, Some(0));
        assert_eq!(app.viewport.records[0], survivor.id);
        assert_eq!(app.selected_chat_text(), "same");
        app.note_runtime_input_started(first.id, "same");
        assert_eq!(app.transcript.len(), 1);
        assert_eq!(app.pending_runtime_echoes[1].record, Some(0));
    }
}

#[test]
fn identical_folded_thoughts_keep_the_explicit_surviving_record_source() {
    for mode in MODES {
        let row = "· + Thought: 1.0s\nsame body";
        let (mut app, mut terminal) =
            fixture(mode, vec!["You: question".into(), row.into(), row.into()]);
        paint(&mut app, &mut terminal);
        let folded = app.display_transcript();
        assert_eq!(folded.sources, vec![Some(0), Some(2)]);
        let survivor = app.viewport.records[2];
        assert!(
            app.viewport
                .frame
                .rows
                .iter()
                .any(|row| row.fallback.id == survivor)
        );
        assert!(
            !app.viewport
                .frame
                .rows
                .iter()
                .any(|row| row.fallback.id == app.viewport.records[1])
        );
    }
}
