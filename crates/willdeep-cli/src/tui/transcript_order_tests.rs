//! Existing-record moves must preserve identity, not simulate delete + append.
use super::*;
use crate::config::UiMode;
use ratatui::backend::TestBackend;
use rendering::source::{SourceFrame, SourceId, SourcePart, SourcePoint};

fn record_id(app: &App, index: usize) -> SourceId {
    let mut frame = SourceFrame::default();
    frame
        .parts
        .insert((SourceId::Record(index), SourcePart::Body), String::new());
    app.viewport_record_sources(&mut frame);
    frame.parts.keys().next().unwrap().0
}

fn point(app: &App, index: usize, part: SourcePart, byte: usize) -> SourcePoint {
    SourcePoint {
        id: record_id(app, index),
        part,
        byte,
    }
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

#[test]
fn moving_first_or_middle_keeps_text_attachments_models_times_and_receipts() {
    for moved in [0, 1] {
        let mut app = App::new(Vec::new(), Language::En);
        for (index, row) in [
            "You: duplicate".to_owned(),
            "You: duplicate".to_owned(),
            format!("{ASSISTANT_ROW_PREFIX}answer"),
            "You: tail".to_owned(),
        ]
        .into_iter()
        .enumerate()
        {
            app.append_transcript_at(row, 100 + index as u64);
        }
        app.transcript_models[2] = Some("recorded-model".into());
        app.transcript_attachments[0] = vec![MessageAttachment::Text {
            name: "first.txt".into(),
            content: "first original 中文".into(),
        }];
        app.transcript_attachments[1] = vec![MessageAttachment::Image {
            name: "second.png".into(),
            media_type: "image/png".into(),
            data: "fixture".into(),
            width: 2,
            height: 3,
        }];
        let first = queued_turn();
        let second = queued_turn();
        app.echo_runtime_submission("duplicate", &first, false, Some(0));
        app.echo_runtime_submission("duplicate", &second, false, Some(1));
        app.prepare_transcript_frame();
        let rows = app.transcript.clone();
        let attachments = app.transcript_attachments.clone();
        let models = app.transcript_models.clone();
        let ids = (0..rows.len())
            .map(|index| record_id(&app, index))
            .collect::<Vec<_>>();
        let order = (0..rows.len())
            .filter(|index| *index != moved)
            .chain(std::iter::once(moved))
            .collect::<Vec<_>>();

        assert_eq!(app.move_transcript_row_to_end(moved), rows.len() - 1);
        assert!(app.transcript_render_cache.is_none());
        assert_eq!(app.transcript.len(), rows.len());
        for (index, original) in order.iter().copied().enumerate() {
            assert_eq!(app.transcript[index], rows[original]);
            assert_eq!(app.transcript_models[index], models[original]);
            assert_eq!(
                app.message_times.time_at(index),
                Some(100 + original as u64)
            );
            assert_eq!(record_id(&app, index), ids[original]);
            assert_eq!(
                serde_json::to_value(&app.transcript_attachments[index]).unwrap(),
                serde_json::to_value(&attachments[original]).unwrap()
            );
        }
        for (echo, original) in app.pending_runtime_echoes.iter().zip([0, 1]) {
            assert_eq!(
                echo.record,
                order.iter().position(|index| *index == original)
            );
            assert!(
                echo.is_pending(),
                "a move must not create a receipt tombstone"
            );
        }
        assert_eq!(app.pending_runtime_echoes[0].turn_id, first.id);
        assert_eq!(app.pending_runtime_echoes[1].turn_id, second.id);
        app.prepare_transcript_frame();
        assert_eq!(app.transcript_height, app.transcript_rows.len());
        assert_eq!(app.transcript_height, app.timed_transcript_height());
    }
}

#[test]
fn row_keyed_state_follows_its_record_and_search_keeps_the_selected_match() {
    for moved in [0, 1] {
        let mut app = App::new(
            vec![
                "You: match first".into(),
                "You: match middle".into(),
                "· ✓ read_file · match tool".into(),
                "You: match tail".into(),
            ],
            Language::En,
        );
        app.expanded_user_messages
            .insert(0, app.transcript[0].clone());
        app.expanded_user_messages
            .insert(1, app.transcript[1].clone());
        app.tool_expansions.insert(2, "tool detail".into());
        app.expanded_tool_rows.insert(2);
        for row in [Some(0), Some(1), Some(2), Some(3), None] {
            app.queued_prompts.push_back(QueuedPrompt {
                text: "queued".into(),
                attachments: Vec::new(),
                remote: None,
                submission_order: 0,
                transcript_row: row,
            });
        }
        let mut search = SearchState::default();
        search.editor.insert("match");
        app.search = Some(search);
        app.refresh_search_matches();
        app.search.as_mut().unwrap().selected = moved;
        app.archive_floor = 2;
        app.prepare_transcript_frame();
        app.user_message_hit_rows.push((3, 0));
        app.tool_row_hits.push((4, 2, false));
        app.attachment_hit_rows.push((5, 0, 0));
        app.thought_hit_rows.push((6, 0));
        app.pending_chat_click = Some((4, app.tool_row_hit_at(4)));

        assert_eq!(app.move_transcript_row_to_end(moved), 3);
        assert_eq!(app.archive_floor, 1);
        let order = (0..4)
            .filter(|index| *index != moved)
            .chain(std::iter::once(moved))
            .collect::<Vec<_>>();
        for original in [0, 1] {
            let index = order.iter().position(|row| *row == original).unwrap();
            assert_eq!(app.expanded_user_messages[&index], app.transcript[index]);
        }
        assert_eq!(
            app.tool_expansions,
            BTreeMap::from([(1, "tool detail".into())])
        );
        assert_eq!(app.expanded_tool_rows, BTreeSet::from([1]));
        let expected = (0..4)
            .map(|original| order.iter().position(|row| *row == original))
            .chain(std::iter::once(None))
            .collect::<Vec<_>>();
        assert_eq!(
            app.queued_prompts
                .iter()
                .map(|item| item.transcript_row)
                .collect::<Vec<_>>(),
            expected
        );
        let search = app.search.as_ref().unwrap();
        assert_eq!(search.matches, [0, 1, 2, 3]);
        assert_eq!(search.matches[search.selected], 3);
        assert!(app.user_message_controls.is_empty());
        assert!(app.user_message_hit_rows.is_empty());
        assert!(app.tool_row_controls.is_empty());
        assert!(app.tool_row_hits.is_empty());
        assert!(app.attachment_controls.is_empty());
        assert!(app.attachment_hit_rows.is_empty());
        assert!(app.thought_hit_rows.is_empty());
        assert!(app.pending_chat_click.is_none());
        assert_eq!(app.transcript_height, app.timed_transcript_height());
    }
}

#[test]
fn sparse_search_matches_keep_a_selected_surviving_neighbour() {
    let mut app = App::new(
        vec![
            "You: match first".into(),
            "You: other".into(),
            "You: match last".into(),
        ],
        Language::En,
    );
    let mut search = SearchState::default();
    search.editor.insert("match");
    app.search = Some(search);
    app.refresh_search_matches();
    app.search.as_mut().unwrap().selected = 1;
    app.move_transcript_row_to_end(0);
    let search = app.search.as_ref().unwrap();
    assert_eq!(search.matches, [1, 2]);
    assert_eq!(search.selected, 0);
    assert_eq!(
        app.transcript[search.matches[search.selected]],
        "You: match last"
    );
}

#[test]
fn moving_visible_rows_does_not_archive_the_live_suffix_and_tail_or_invalid_are_noops() {
    let mut app = App::new(
        vec!["You: old".into(), "You: live".into(), "You: tail".into()],
        Language::En,
    );
    app.archive_floor = 1;
    assert_eq!(app.move_transcript_row_to_end(1), 2);
    assert_eq!(app.archive_floor, 1);
    assert_eq!(app.transcript, ["You: old", "You: tail", "You: live"]);
    app.prepare_transcript_frame();
    let rows = app.transcript.clone();
    for index in [2, 3, usize::MAX] {
        assert_eq!(app.move_transcript_row_to_end(index), index);
        assert_eq!(app.transcript, rows);
        assert!(app.transcript_render_cache.is_some());
        assert_eq!(app.archive_floor, 1);
    }
    let mut empty = App::new(Vec::new(), Language::En);
    assert_eq!(empty.move_transcript_row_to_end(0), 0);
    assert!(empty.transcript.is_empty());
}

fn fixture(mode: UiMode, rows: Vec<String>) -> (App, Terminal<TestBackend>) {
    let mut app = App::new(rows, Language::En);
    app.ui.mode = mode;
    app.ui.copy_on_select = false;
    app.ui.compact_status_line = false;
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
}

#[test]
fn moving_a_record_preserves_attachment_selection_and_reading_on_it_or_its_neighbour() {
    for mode in [UiMode::Full, UiMode::Compact] {
        for reading_moved_record in [true, false] {
            let body = (0..40)
                .map(|i| format!("line {i:02} 中文"))
                .collect::<Vec<_>>()
                .join("\n");
            let row = format!("You: {body}");
            let attachment = (0..20)
                .map(|i| format!("attachment {i:02} 中文"))
                .collect::<Vec<_>>()
                .join("\n");
            let (mut app, mut terminal) = fixture(
                mode,
                vec![row.clone(), format!("{ASSISTANT_ROW_PREFIX}{body}")],
            );
            app.expanded_user_messages.insert(0, row);
            app.transcript_attachments[0] = vec![MessageAttachment::Text {
                name: "original.txt".into(),
                content: attachment.clone(),
            }];
            paint(&mut app, &mut terminal);
            let reading_record = usize::from(!reading_moved_record);
            let byte = body.lines().take(12).map(|line| line.len() + 1).sum();
            let reading = point(&app, reading_record, SourcePart::Body, byte);
            let row = app.viewport.frame.resolve(reading).unwrap().row;
            app.follow_bottom = false;
            app.scroll_from_bottom = app.max_scroll() - row;
            app.viewport_scrolled();
            paint(&mut app, &mut terminal);
            let screen_row =
                app.viewport.frame.resolve(reading).unwrap().row - app.transcript_render_offset;
            let byte = attachment.lines().take(4).map(|line| line.len() + 1).sum();
            let anchor = point(&app, 0, SourcePart::AttachmentBody(0), byte);
            let selected = "attachment 04 中文";
            let head = SourcePoint {
                byte: byte + selected.char_indices().last().unwrap().0,
                ..anchor
            };
            select(&mut app, anchor, head);
            assert_eq!(app.selected_chat_text(), selected);

            assert_eq!(app.move_transcript_row_to_end(0), 1);
            assert_eq!(record_id(&app, 1), anchor.id);
            for width in [80, 43, 80] {
                terminal.backend_mut().resize(width, 24);
                terminal.resize(Rect::new(0, 0, width, 24)).unwrap();
                paint(&mut app, &mut terminal);
                assert!(!app.follow_bottom);
                assert_eq!(app.selected_chat_text(), selected);
                assert_eq!(
                    app.viewport.frame.resolve(reading).unwrap().row - app.transcript_render_offset,
                    screen_row
                );
                let selection = app.chat_selection.unwrap();
                assert_eq!(app.viewport.frame.point_at(selection.anchor), Some(anchor));
                assert_eq!(app.viewport.frame.point_at(selection.head), Some(head));
            }
        }
    }
}

fn highlighted(app: &mut App) -> String {
    let (transient, notice, waiting) = app.prepare_transcript_frame();
    let mut text = app.transcript_viewport(&transient, notice.as_ref(), waiting.as_ref(), 0);
    app.highlight_chat_selection(&mut text, 0);
    text.lines
        .iter()
        .flat_map(|line| &line.spans)
        .filter(|span| span.style.bg == Some(MODAL_BG))
        .flat_map(|span| span.content.chars())
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

#[test]
fn fresh_cross_record_selections_follow_current_order_in_both_directions() {
    let mut app = App::new(
        vec!["You: alpha".into(), "You: beta".into(), "You: gamma".into()],
        Language::En,
    );
    app.viewport_height = 100;
    app.prepare_transcript_frame();
    app.move_transcript_row_to_end(0);
    app.prepare_transcript_frame();
    let first = point(&app, 1, SourcePart::Body, 0);
    let last = point(&app, 2, SourcePart::Body, 4);
    for (anchor, head) in [(first, last), (last, first)] {
        app.viewport_clear_selection();
        select(&mut app, anchor, head);
        assert_eq!(app.selected_chat_text(), "gamma\nalpha");
        assert_eq!(highlighted(&mut app), "gammaalpha");
    }
}

#[test]
fn an_existing_cross_record_selection_keeps_original_bytes_without_highlighting_neighbours() {
    let mut app = App::new(
        vec!["You: alpha".into(), "You: beta".into(), "You: gamma".into()],
        Language::En,
    );
    app.viewport_height = 100;
    app.prepare_transcript_frame();
    let anchor = point(&app, 0, SourcePart::Body, 3);
    let head = point(&app, 1, SourcePart::Body, 1);
    select(&mut app, anchor, head);
    assert_eq!(app.selected_chat_text(), "ha\nbe");
    app.move_transcript_row_to_end(0);
    assert_eq!(app.selected_chat_text(), "ha\nbe");
    assert_eq!(highlighted(&mut app), "beha");
    assert_eq!(app.selected_chat_text(), "ha\nbe");
    app.move_transcript_row_to_end(1);
    assert_eq!(highlighted(&mut app), "beha");
    assert_eq!(app.selected_chat_text(), "ha\nbe");
}

#[test]
fn lost_receipt_merge_preserves_cross_record_selection_bytes_and_highlights() {
    for (beta_row, selected, before, after) in [
        ("You: beta".into(), "ha\nbe", "habe", "beha"),
        (
            format!("{ASSISTANT_ROW_PREFIX}beta"),
            "ha\nWillDeep: be",
            "haWillDeep:be",
            "WillDeep:beha",
        ),
    ] {
        for mode in [UiMode::Full, UiMode::Compact] {
            for reverse in [false, true] {
                let (mut app, mut terminal) = fixture(
                    mode,
                    vec!["You: alpha".into(), beta_row.clone(), "You: gamma".into()],
                );
                let mut receipt = queued_turn();
                receipt.id = uuid::Uuid::from_u128(1);
                receipt.request_id = uuid::Uuid::from_u128(2);
                receipt.status = willdeep_runtime_protocol::TurnStatus::Running;
                app.note_runtime_submission_unconfirmed(
                    "alpha",
                    receipt.request_id,
                    false,
                    Some(0),
                );
                app.note_runtime_input_started(receipt.id, "alpha");
                paint(&mut app, &mut terminal);
                assert_eq!(app.transcript.len(), 4);
                assert_eq!(app.pending_runtime_echoes.len(), 2);
                let survivor = record_id(&app, 3);
                let alpha = point(&app, 0, SourcePart::Body, 3);
                let beta = point(&app, 1, SourcePart::Body, 1);
                let (anchor, head) = if reverse {
                    (beta, alpha)
                } else {
                    (alpha, beta)
                };
                select(&mut app, anchor, head);
                assert_eq!(app.selected_chat_text(), selected);
                assert_eq!(highlighted(&mut app), before);

                app.reconcile_runtime_deliveries(std::slice::from_ref(&receipt));
                assert_eq!(app.transcript.len(), 3);
                assert_eq!(app.pending_runtime_echoes.len(), 1);
                assert_eq!(app.pending_runtime_echoes[0].record, Some(2));
                assert_eq!(record_id(&app, 2), survivor);
                assert_eq!(app.selected_chat_text(), selected);
                assert_eq!(
                    highlighted(&mut app),
                    after,
                    "mode={mode:?}, reverse={reverse}, beta={beta_row}"
                );
                assert_eq!(app.selected_chat_text(), selected);

                paint(&mut app, &mut terminal);
                assert_eq!(app.selected_chat_text(), selected);
                assert_eq!(
                    highlighted(&mut app),
                    after,
                    "mode={mode:?}, reverse={reverse}, beta={beta_row}"
                );
                assert_eq!(app.selected_chat_text(), selected);
            }
        }
    }
}

#[test]
fn lost_receipt_merge_preserves_selection_on_the_positioned_copy() {
    for mode in [UiMode::Full, UiMode::Compact] {
        for reverse in [false, true] {
            let (mut app, mut terminal) = fixture(
                mode,
                vec!["You: alpha".into(), "You: beta".into(), "You: gamma".into()],
            );
            let mut receipt = queued_turn();
            receipt.id = uuid::Uuid::from_u128(3);
            receipt.request_id = uuid::Uuid::from_u128(4);
            receipt.status = willdeep_runtime_protocol::TurnStatus::Running;
            app.note_runtime_submission_unconfirmed("alpha", receipt.request_id, false, Some(0));
            app.note_runtime_input_started(receipt.id, "alpha");
            paint(&mut app, &mut terminal);
            let beta = point(&app, 1, SourcePart::Body, 0);
            let alpha = point(&app, 3, SourcePart::Body, 1);
            let (anchor, head) = if reverse {
                (alpha, beta)
            } else {
                (beta, alpha)
            };
            select(&mut app, anchor, head);
            assert_eq!(app.selected_chat_text(), "beta\ngamma\nal");
            assert_eq!(highlighted(&mut app), "betagammaal");

            app.reconcile_runtime_deliveries(std::slice::from_ref(&receipt));
            assert_eq!(app.transcript.len(), 3);
            assert_eq!(app.selected_chat_text(), "beta\ngamma\nal");
            assert_eq!(
                highlighted(&mut app),
                "betagammaal",
                "mode={mode:?}, reverse={reverse}"
            );
            paint(&mut app, &mut terminal);
            assert_eq!(app.selected_chat_text(), "beta\ngamma\nal");
            assert_eq!(
                highlighted(&mut app),
                "betagammaal",
                "mode={mode:?}, reverse={reverse}"
            );
        }
    }
}

#[test]
fn lost_receipt_merge_does_not_select_attachments_added_to_the_positioned_copy() {
    for mode in [UiMode::Full, UiMode::Compact] {
        for reverse in [false, true] {
            let (mut app, mut terminal) = fixture(mode, vec!["You: alpha".into()]);
            let mut receipt = queued_turn();
            receipt.status = willdeep_runtime_protocol::TurnStatus::Running;
            app.note_runtime_submission_unconfirmed("alpha", receipt.request_id, false, Some(0));
            app.transcript_attachments[0] = vec![MessageAttachment::Text {
                name: "original.txt".into(),
                content: "UNSELECTED_ATTACHMENT_中文".into(),
            }];
            app.note_runtime_input_started(receipt.id, "alpha");
            app.append_transcript("You: gamma".into());
            paint(&mut app, &mut terminal);
            assert!(app.transcript_attachments[1].is_empty());
            let alpha = point(&app, 1, SourcePart::Body, 0);
            let gamma = point(&app, 2, SourcePart::Body, 0);
            let (anchor, head) = if reverse {
                (gamma, alpha)
            } else {
                (alpha, gamma)
            };
            select(&mut app, anchor, head);
            assert_eq!(app.selected_chat_text(), "alpha\ng");
            assert_eq!(highlighted(&mut app), "alphag");

            app.reconcile_runtime_deliveries(std::slice::from_ref(&receipt));
            assert_eq!(app.transcript.len(), 2);
            assert_eq!(app.transcript_attachments[0].len(), 1);
            assert_eq!(app.selected_chat_text(), "alpha\ng");
            assert_eq!(
                highlighted(&mut app),
                "alphag",
                "mode={mode:?}, reverse={reverse}"
            );
            paint(&mut app, &mut terminal);
            assert_eq!(app.selected_chat_text(), "alpha\ng");
            assert_eq!(
                highlighted(&mut app),
                "alphag",
                "mode={mode:?}, reverse={reverse}"
            );
        }
    }
}

#[test]
fn lost_receipt_merge_unions_disjoint_copy_ranges_and_all_selected_middle_records() {
    for (body, move_beta) in [
        ("alpha", false),
        ("alpha", true),
        (
            "al0123456789中文0123456789中文0123456789中文0123456789ha",
            false,
        ),
    ] {
        for mode in [UiMode::Full, UiMode::Compact] {
            for reverse in [false, true] {
                let (mut app, mut terminal) = fixture(
                    mode,
                    vec![
                        format!("You: {body}"),
                        "You: beta".into(),
                        "You: gamma".into(),
                    ],
                );
                let mut receipt = queued_turn();
                receipt.id = uuid::Uuid::from_u128(5);
                receipt.request_id = uuid::Uuid::from_u128(6);
                receipt.status = willdeep_runtime_protocol::TurnStatus::Running;
                app.note_runtime_submission_unconfirmed(body, receipt.request_id, false, Some(0));
                app.note_runtime_input_started(receipt.id, body);
                paint(&mut app, &mut terminal);
                assert_eq!(app.transcript.len(), 4);
                let survivor = record_id(&app, 3);
                let tail = point(&app, 0, SourcePart::Body, body.len() - 2);
                let head = point(&app, 3, SourcePart::Body, 1);
                let (anchor, head) = if reverse { (head, tail) } else { (tail, head) };
                select(&mut app, anchor, head);
                assert_eq!(app.selected_chat_text(), "ha\nbeta\ngamma\nal");
                assert_eq!(highlighted(&mut app), "habetagammaal");
                if move_beta {
                    app.move_transcript_row_to_end(1);
                    assert_eq!(app.selected_chat_text(), "ha\nbeta\ngamma\nal");
                    assert_eq!(highlighted(&mut app), "hagammaalbeta");
                }
                let survivor_index = if move_beta { 1 } else { 2 };
                let merged_highlight = if move_beta {
                    "gammaalhabeta"
                } else {
                    "betagammaalha"
                };

                app.reconcile_runtime_deliveries(std::slice::from_ref(&receipt));
                assert_eq!(app.transcript.len(), 3);
                assert_eq!(app.pending_runtime_echoes.len(), 1);
                assert_eq!(app.pending_runtime_echoes[0].record, Some(survivor_index));
                assert_eq!(record_id(&app, survivor_index), survivor);
                assert_eq!(app.selected_chat_text(), "ha\nbeta\ngamma\nal");
                assert_eq!(
                    highlighted(&mut app),
                    merged_highlight,
                    "mode={mode:?}, reverse={reverse}, body={body}"
                );
                for width in [80, 43, 80] {
                    terminal.backend_mut().resize(width, 24);
                    terminal.resize(Rect::new(0, 0, width, 24)).unwrap();
                    paint(&mut app, &mut terminal);
                    assert_eq!(app.selected_chat_text(), "ha\nbeta\ngamma\nal");
                    assert_eq!(
                        highlighted(&mut app),
                        merged_highlight,
                        "mode={mode:?}, reverse={reverse}, width={width}, body={body}, move_beta={move_beta}"
                    );
                }

                // A fresh drag must use the current frame and discard the old
                // disjoint ranges, without needing an explicit clear first.
                let first = point(&app, 0, SourcePart::Body, 2);
                let alpha = point(&app, survivor_index, SourcePart::Body, 1);
                let (anchor, head) = if reverse {
                    (alpha, first)
                } else {
                    (first, alpha)
                };
                let fresh_text = if move_beta {
                    "mma\nal"
                } else {
                    "ta\ngamma\nal"
                };
                let fresh_highlight = if move_beta { "mmaal" } else { "tagammaal" };
                select(&mut app, anchor, head);
                assert_eq!(app.selected_chat_text(), fresh_text);
                assert_eq!(highlighted(&mut app), fresh_highlight);
                app.viewport_clear_selection();
                app.chat_selection = None;
                assert_eq!(highlighted(&mut app), "");
                select(&mut app, anchor, head);
                assert_eq!(app.selected_chat_text(), fresh_text);
                assert_eq!(highlighted(&mut app), fresh_highlight);
            }
        }
    }
}

#[test]
fn mouse_down_after_identity_merge_starts_a_fresh_selection_at_the_same_point() {
    for mode in [UiMode::Full, UiMode::Compact] {
        for reverse in [false, true] {
            for (first_byte, selected) in [(3, "ha\nbeta\ngamma\nal"), (1, "lpha\nbeta\ngamma\nal")]
            {
                let (mut app, mut terminal) = fixture(
                    mode,
                    vec!["You: alpha".into(), "You: beta".into(), "You: gamma".into()],
                );
                let mut receipt = queued_turn();
                receipt.status = willdeep_runtime_protocol::TurnStatus::Running;
                app.note_runtime_submission_unconfirmed(
                    "alpha",
                    receipt.request_id,
                    false,
                    Some(0),
                );
                app.note_runtime_input_started(receipt.id, "alpha");
                paint(&mut app, &mut terminal);
                let first = point(&app, 0, SourcePart::Body, first_byte);
                let last = point(&app, 3, SourcePart::Body, 1);
                let (anchor, head) = if reverse {
                    (last, first)
                } else {
                    (first, last)
                };
                select(&mut app, anchor, head);
                app.selection_mode = true;
                assert_eq!(app.selected_chat_text(), selected);
                app.reconcile_runtime_deliveries(std::slice::from_ref(&receipt));
                paint(&mut app, &mut terminal);
                assert_eq!(app.selected_chat_text(), selected);

                let first = point(&app, 2, SourcePart::Body, first_byte);
                let position = app.viewport.frame.resolve(first).unwrap();
                let mouse = |kind, position: ChatSelectionPoint, app: &App| MouseEvent {
                    kind,
                    column: app.transcript_content.x + position.column as u16,
                    row: app.transcript_content.y
                        + (position.row - app.transcript_render_offset) as u16,
                    modifiers: KeyModifiers::NONE,
                };
                let down = mouse(MouseEventKind::Down(MouseButton::Left), position, &app);
                assert!(app.handle_chat_selection_mouse(down));
                let initial = if first_byte == 3 { "h" } else { "l" };
                assert_eq!(
                    app.selected_chat_text(),
                    initial,
                    "mode={mode:?}, reverse={reverse}, first_byte={first_byte}"
                );
                assert_eq!(highlighted(&mut app), initial);

                let last = point(
                    &app,
                    2,
                    SourcePart::Body,
                    if first_byte == 3 { 1 } else { 3 },
                );
                let position = app.viewport.frame.resolve(last).unwrap();
                let drag = mouse(MouseEventKind::Drag(MouseButton::Left), position, &app);
                assert!(app.handle_chat_selection_mouse(drag));
                assert_eq!(app.selected_chat_text(), "lph");
                assert_eq!(highlighted(&mut app), "lph");
            }
        }
    }
}

#[test]
fn merging_a_placeholder_keeps_its_selected_bytes_reading_anchor_and_original_time() {
    for (from, to) in [(0, 1), (1, 0)] {
        let body = (0..40)
            .map(|i| format!("line {i:02} 中文"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = App::new(Vec::new(), Language::En);
        app.append_transcript_at(format!("You: {body}"), 100);
        app.append_transcript_at(format!("You: {body}"), 200);
        for index in [0, 1] {
            app.expanded_user_messages
                .insert(index, app.transcript[index].clone());
        }
        app.prepare_transcript_frame();
        let byte = body.lines().take(12).map(|line| line.len() + 1).sum();
        let anchor = point(&app, from, SourcePart::Body, byte);
        let survivor = record_id(&app, to);
        let head = SourcePoint {
            byte: byte + "line 12".len() - 1,
            ..anchor
        };
        let row = app.viewport.frame.resolve(anchor).unwrap().row;
        app.follow_bottom = false;
        app.scroll_from_bottom = app.max_scroll() - row;
        // Leave reading uncaptured to exercise the merge's own capture hook.
        app.transcript_render_offset = row;
        select(&mut app, anchor, head);
        let original_time = app.message_times.time_at(from);
        app.message_times.copy_time(from, to);
        app.viewport_merge_record(from, to);
        app.drop_transcript_row(from);
        app.prepare_transcript_frame();

        assert_eq!(app.transcript.len(), 1);
        assert_eq!(record_id(&app, 0), survivor);
        assert_eq!(app.message_times.time_at(0), original_time);
        assert_eq!(app.selected_chat_text(), "line 12");
        let migrated = SourcePoint {
            id: survivor,
            ..anchor
        };
        assert_eq!(
            app.viewport.frame.resolve(migrated).unwrap().row,
            app.transcript_render_offset
        );
        assert_eq!(
            app.viewport
                .frame
                .point_at(app.chat_selection.unwrap().anchor),
            Some(migrated)
        );
    }
}
