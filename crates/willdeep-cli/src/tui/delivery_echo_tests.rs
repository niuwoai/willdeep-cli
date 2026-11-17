use super::*;

fn turn(status: TurnStatus) -> RuntimeTurn {
    RuntimeTurn {
        execution_session_id: None,
        id: uuid::Uuid::new_v4(),
        session_id: uuid::Uuid::new_v4(),
        request_id: uuid::Uuid::new_v4(),
        queue_sequence: 1,
        status,
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

fn queued(turn: &RuntimeTurn, text: &str) -> QueuedTurnMessage {
    QueuedTurnMessage {
        id: turn.id,
        session_id: turn.session_id,
        text: text.to_owned(),
        text_preview: text.to_owned(),
        can_edit: true,
        image_count: 0,
        text_attachment_count: 0,
        held: false,
        priority_requested: false,
        independence_confirmed: false,
    }
}

#[test]
fn lost_held_receipt_reconciles_by_request_without_a_start_event_or_retry() {
    let mut app = App::new(Vec::new(), Language::En);
    let receipt = turn(TurnStatus::Queued);
    app.append_transcript_at("You: submitting later".into(), 42);
    app.note_runtime_submission_unconfirmed("later", receipt.request_id, true, Some(0));
    assert!(app.pending_runtime_echoes[0].needs_reconciliation());
    assert!(app.pending_runtime_echoes[0].turn_id.is_nil());
    app.reconcile_runtime_deliveries(std::slice::from_ref(&receipt));
    assert_eq!(app.pending_runtime_echoes[0].turn_id, receipt.id);
    assert_eq!(app.pending_runtime_echoes[0].status, DeliveryStatus::Held);
    assert_eq!(app.message_times.time_at(0), Some(42));
    assert_eq!(app.transcript.len(), 1);
}

#[test]
fn lost_receipt_and_early_start_merge_only_the_proven_identity() {
    let mut app = App::new(Vec::new(), Language::En);
    let receipt = turn(TurnStatus::Running);
    app.append_transcript_at("You: original".into(), 42);
    app.transcript_attachments[0] = vec![MessageAttachment::Text {
        name: "keep.txt".into(),
        content: "original attachment".into(),
    }];
    app.note_runtime_submission_unconfirmed("same", receipt.request_id, false, Some(0));
    let other = turn(TurnStatus::Running);
    app.note_runtime_input_started(other.id, "same");
    app.note_runtime_input_started(receipt.id, "same");
    assert_eq!(app.transcript.len(), 3);
    app.reconcile_runtime_deliveries(&[receipt.clone(), other]);
    app.reconcile_runtime_deliveries(std::slice::from_ref(&receipt));
    assert_eq!(app.transcript.len(), 2);
    assert_eq!(app.pending_runtime_echoes.len(), 2);
    assert_eq!(app.pending_runtime_echoes[0].record, Some(1));
    assert_eq!(
        app.pending_runtime_echoes[0].status,
        DeliveryStatus::Started
    );
    assert_eq!(app.pending_runtime_echoes[1].record, Some(0));
    assert_eq!(app.message_times.time_at(1), Some(42));
    assert_eq!(app.transcript_attachments[1].len(), 1);
    app.note_runtime_input_started(receipt.id, "same");
    assert_eq!(app.transcript.len(), 2);
}

#[test]
fn queued_runtime_receipt_reuses_the_exact_placeholder_once() {
    let mut app = App::new(Vec::new(), Language::En);
    let receipt = turn(TurnStatus::Queued);
    app.append_transcript_at("System: previous".into(), 1);
    app.append_transcript_at("You: [queued] same".into(), 42);
    app.append_transcript_at("You: same".into(), 99);
    assert_eq!(
        app.echo_runtime_submission("same", &receipt, false, Some(1)),
        Some(1)
    );
    app.note_runtime_input_started(receipt.id, "same");
    assert_eq!(app.transcript.len(), 3);
    assert_eq!(app.pending_runtime_echoes[0].record, Some(2));
    assert_eq!(app.message_times.time_at(2), Some(42));
    assert_eq!(app.transcript[1], "You: same");
}

#[test]
fn equal_text_with_different_turn_ids_never_claims_the_local_receipt() {
    let mut app = App::new(Vec::new(), Language::En);
    let local = turn(TurnStatus::Queued);
    let other = turn(TurnStatus::Running);
    app.echo_runtime_submission("same", &local, false, None);
    app.note_runtime_input_started(other.id, "same");
    assert_eq!(app.transcript.len(), 2);
    assert!(app.pending_runtime_echoes[0].is_pending());
    assert_eq!(app.pending_runtime_echoes[1].turn_id, other.id);
    assert_eq!(
        app.pending_runtime_echoes[1].status,
        DeliveryStatus::Started
    );
    app.note_runtime_input_started(local.id, "same");
    assert_eq!(app.transcript.len(), 2);
    assert!(
        app.pending_runtime_echoes
            .iter()
            .all(|echo| !echo.is_pending())
    );
}

#[test]
fn repeated_start_and_submit_receipt_keep_one_record_for_the_same_turn() {
    let mut app = App::new(Vec::new(), Language::En);
    let receipt = turn(TurnStatus::Queued);
    app.note_runtime_input_started(receipt.id, "same");
    app.note_runtime_input_started(receipt.id, "same");
    app.echo_runtime_submission("same", &receipt, false, None);
    assert_eq!(app.transcript.len(), 1);
    assert_eq!(app.pending_runtime_echoes.len(), 1);
    assert_eq!(
        app.pending_runtime_echoes[0].status,
        DeliveryStatus::Started
    );
    assert_eq!(app.pending_runtime_echoes[0].request_id, receipt.request_id);
}

#[test]
fn absent_or_late_queue_snapshots_cannot_prove_non_delivery() {
    let mut app = App::new(Vec::new(), Language::En);
    let receipt = turn(TurnStatus::Queued);
    app.echo_runtime_submission("claimed between snapshots", &receipt, false, None);
    app.sync_runtime_delivery_queue(&[]);
    app.reconcile_runtime_deliveries(&[]);
    assert_eq!(app.pending_runtime_echoes[0].status, DeliveryStatus::Queued);
    app.note_runtime_input_started(receipt.id, "claimed between snapshots");
    app.sync_runtime_delivery_queue(&[queued(&receipt, "stale text")]);
    app.reconcile_runtime_deliveries(&[receipt]);
    assert_eq!(
        app.pending_runtime_echoes[0].status,
        DeliveryStatus::Started
    );
    assert!(app.transcript[0].contains("claimed between snapshots"));
    assert!(!app.transcript[0].contains("not sent"));
}

#[test]
fn cancelling_one_of_two_identical_inputs_changes_only_its_exact_record() {
    let mut app = App::new(Vec::new(), Language::En);
    let first = turn(TurnStatus::Queued);
    let mut second = turn(TurnStatus::Queued);
    app.echo_runtime_submission("same", &first, false, None);
    app.echo_runtime_submission("same", &second, false, None);
    app.sync_runtime_delivery_queue(&[queued(&first, "same")]);
    assert!(
        app.pending_runtime_echoes
            .iter()
            .all(PendingRuntimeEcho::is_pending)
    );
    second.status = TurnStatus::Cancelled;
    app.reconcile_runtime_deliveries(&[second]);
    assert!(app.transcript[0].starts_with("You: ◇ "));
    assert!(app.transcript[1].starts_with("You: × "));
    assert_eq!(app.pending_runtime_echoes.len(), 2);
}

#[test]
fn terminal_receipt_survives_out_of_order_start_and_queue_replay() {
    for status in [
        TurnStatus::Completed,
        TurnStatus::Partial,
        TurnStatus::Failed,
        TurnStatus::Interrupted,
        TurnStatus::Cancelled,
    ] {
        let mut app = App::new(Vec::new(), Language::En);
        let mut receipt = turn(status);
        app.echo_runtime_submission("settled", &receipt, false, None);
        let before = app.transcript.clone();
        app.note_runtime_input_started(receipt.id, "settled");
        receipt.status = TurnStatus::Queued;
        app.reconcile_runtime_deliveries(std::slice::from_ref(&receipt));
        app.sync_runtime_delivery_queue(&[queued(&receipt, "stale edit")]);
        assert_eq!(app.transcript, before);
        assert!(!app.pending_runtime_echoes[0].is_pending());
    }
}

#[test]
fn editing_and_claiming_preserve_the_original_timestamp_and_attachments() {
    let mut app = App::new(Vec::new(), Language::En);
    let receipt = turn(TurnStatus::Queued);
    app.append_transcript_at("You: [submitting] original".to_owned(), 42);
    let attachment = MessageAttachment::Text {
        name: "keep.txt".into(),
        content: "contents".into(),
    };
    app.transcript_attachments[0] = vec![attachment.clone()];
    app.echo_runtime_submission("original", &receipt, false, Some(0));
    app.sync_runtime_delivery_queue(&[queued(&receipt, "edited elsewhere")]);
    app.note_runtime_input_started(uuid::Uuid::new_v4(), "edited elsewhere");
    app.note_runtime_input_started(receipt.id, "edited elsewhere");
    assert_eq!(app.message_times.time_at(1), Some(42));
    assert_eq!(app.transcript_attachments[1].len(), 1);
    assert!(
        matches!(&app.transcript_attachments[1][0], MessageAttachment::Text { name, content } if name == "keep.txt" && content == "contents")
    );
    assert!(app.transcript_attachments[0].is_empty());
    assert!(app.transcript[1].contains("edited elsewhere"));
}

#[test]
fn index_hook_shifts_surviving_echoes_and_keeps_removed_identity_tombstones() {
    let mut app = App::new(Vec::new(), Language::En);
    app.append_transcript("System: preceding record".to_owned());
    let first = turn(TurnStatus::Queued);
    let second = turn(TurnStatus::Queued);
    app.echo_runtime_submission("first", &first, false, None);
    app.echo_runtime_submission("second", &second, false, None);
    app.drop_transcript_row(0);
    assert_eq!(app.pending_runtime_echoes[0].record, Some(0));
    assert_eq!(app.pending_runtime_echoes[1].record, Some(1));
    app.drop_transcript_row(0);
    assert_eq!(app.pending_runtime_echoes[0].record, None);
    assert_eq!(app.pending_runtime_echoes[1].record, Some(0));
    app.note_runtime_input_started(first.id, "first");
    app.note_runtime_input_started(second.id, "second");
    assert_eq!(app.transcript.len(), 1);
    assert!(app.transcript[0].contains("second"));
}

#[test]
fn clearing_display_keeps_turn_identity_and_detaches_old_record_indices() {
    let mut app = App::new(Vec::new(), Language::En);
    let receipt = turn(TurnStatus::Queued);
    app.echo_runtime_submission("same", &receipt, false, None);
    assert!(app.handle_slash_command("/clear", &SkillCatalog::default()));
    let cleared = app.transcript.clone();
    app.note_runtime_input_started(receipt.id, "same");
    assert_eq!(app.transcript, cleared);
    assert_eq!(app.pending_runtime_echoes[0].record, None);
    let other = turn(TurnStatus::Queued);
    app.echo_runtime_submission("same", &other, false, None);
    assert_eq!(app.transcript.len(), cleared.len() + 1);
    assert_eq!(app.pending_runtime_echoes[1].turn_id, other.id);
}

#[test]
fn held_priority_and_independence_are_visible_in_all_three_languages() {
    for language in [Language::ZhCn, Language::En, Language::Ja] {
        let mut app = App::new(Vec::new(), language);
        let receipt = turn(TurnStatus::Queued);
        app.echo_runtime_submission("later", &receipt, true, None);
        assert!(app.transcript[0].contains(DeliveryStatus::Held.symbol()));
        assert!(DeliveryStatus::Held.feedback(language).contains("Ctrl+Q"));
        let mut item = queued(&receipt, "later");
        item.held = true;
        item.priority_requested = true;
        item.independence_confirmed = true;
        app.sync_runtime_delivery_queue(&[item.clone()]);
        assert!(app.transcript[0].contains(language.text("优先", "priority", "優先")));
        assert!(app.transcript[0].contains(language.text("独立处理", "independent", "独立処理")));
        item.held = false;
        app.sync_runtime_delivery_queue(&[item]);
        assert_eq!(app.pending_runtime_echoes[0].status, DeliveryStatus::Queued);
    }
}

#[test]
fn receipt_status_follows_the_actual_turn_including_human_waits() {
    for (status, expected) in [
        (TurnStatus::Queued, DeliveryStatus::Queued),
        (TurnStatus::Running, DeliveryStatus::Started),
        (TurnStatus::WaitingApproval, DeliveryStatus::Started),
        (TurnStatus::WaitingAnswer, DeliveryStatus::Started),
        (TurnStatus::Completed, DeliveryStatus::Completed),
    ] {
        let mut app = App::new(Vec::new(), Language::En);
        let receipt = turn(status);
        app.echo_runtime_submission("receipt", &receipt, true, None);
        let expected = if status == TurnStatus::Queued {
            DeliveryStatus::Held
        } else {
            expected
        };
        assert_eq!(app.pending_runtime_echoes[0].status, expected);
        assert!(app.transcript[0].contains(expected.symbol()));
    }
}

#[test]
fn start_event_positions_after_prior_output_even_when_snapshot_advanced_first() {
    for status in [TurnStatus::Running, TurnStatus::Completed] {
        let mut app = App::new(Vec::new(), Language::En);
        let mut receipt = turn(TurnStatus::Queued);
        app.append_transcript_at("You: queued".into(), 42);
        app.echo_runtime_submission("queued", &receipt, false, Some(0));
        app.append_transcript_at(format!("{ASSISTANT_ROW_PREFIX}previous answer"), 99);
        receipt.status = status;
        app.reconcile_runtime_deliveries(std::slice::from_ref(&receipt));
        assert_eq!(app.pending_runtime_echoes[0].record, Some(0));
        app.note_runtime_input_started(receipt.id, "queued");
        assert_eq!(app.pending_runtime_echoes[0].record, Some(1));
        assert_eq!(app.message_times.time_at(1), Some(42));
        assert!(app.transcript[0].ends_with("previous answer"));
        app.append_transcript(format!("{ASSISTANT_ROW_PREFIX}next answer"));
        let settled = app.transcript.clone();
        app.note_runtime_input_started(receipt.id, "stale replay");
        app.reconcile_runtime_deliveries(&[receipt]);
        assert_eq!(app.transcript, settled);
    }
}

#[test]
fn identical_queued_inputs_move_independently_in_actual_execution_order() {
    let mut app = App::new(Vec::new(), Language::En);
    let first = turn(TurnStatus::Queued);
    let second = turn(TurnStatus::Queued);
    app.echo_runtime_submission("same", &first, false, None);
    app.echo_runtime_submission("same", &second, false, None);
    app.append_transcript(format!("{ASSISTANT_ROW_PREFIX}previous answer"));
    app.note_runtime_input_started(first.id, "same");
    app.append_transcript(format!("{ASSISTANT_ROW_PREFIX}first answer"));
    app.note_runtime_input_started(second.id, "same");
    app.append_transcript(format!("{ASSISTANT_ROW_PREFIX}second answer"));
    assert_eq!(app.pending_runtime_echoes[0].record, Some(1));
    assert_eq!(app.pending_runtime_echoes[1].record, Some(3));
    assert!(app.transcript[0].ends_with("previous answer"));
    assert!(app.transcript[1].ends_with("same"));
    assert!(app.transcript[2].ends_with("first answer"));
    assert!(app.transcript[3].ends_with("same"));
    assert!(app.transcript[4].ends_with("second answer"));
    let settled = app.transcript.clone();
    app.note_runtime_input_started(first.id, "same");
    app.note_runtime_input_started(second.id, "same");
    assert_eq!(app.transcript, settled);
}

#[test]
fn runtime_input_boundary_commits_the_previous_live_or_held_output_first() {
    for held in [false, true] {
        let mut app = App::new(Vec::new(), Language::En);
        let receipt = turn(TurnStatus::Queued);
        app.begin_turn(true, "working".into());
        app.echo_runtime_submission("next question", &receipt, false, None);
        if held {
            app.note_narration("previous answer");
        } else {
            app.stream_transient(StreamKind::Reply, "previous answer");
        }
        app.note_runtime_input_started(receipt.id, "next question");
        let user = app.pending_runtime_echoes[0].record.unwrap();
        assert!(user > 0);
        assert!(
            app.transcript[user - 1].ends_with("previous answer"),
            "{:?}",
            app.transcript
        );
        assert!(app.transcript[user].ends_with("next question"));
        assert!(app.transient_thought.is_none());
        assert!(app.held_narration.is_none());
    }
}

#[test]
fn receipt_symbols_follow_real_turns_without_changing_body_copy_or_width() {
    let body = "✓ 中文😀\n  keep  spaces";
    for language in [Language::ZhCn, Language::En, Language::Ja] {
        for width in [8, 40, 100] {
            for (status, held, symbol, color) in [
                (TurnStatus::Queued, false, "◇", Color::Gray),
                (TurnStatus::Queued, true, "‖", Color::Yellow),
                (TurnStatus::Running, false, "▶", Color::Cyan),
                (TurnStatus::WaitingApproval, false, "▶", Color::Cyan),
                (TurnStatus::WaitingAnswer, false, "▶", Color::Cyan),
                (TurnStatus::Completed, false, "✓", Color::Green),
                (TurnStatus::Partial, false, "◒", Color::Yellow),
                (TurnStatus::Failed, false, "✗", Color::Red),
                (TurnStatus::Interrupted, false, "■", Color::DarkGray),
                (TurnStatus::Cancelled, false, "×", Color::DarkGray),
            ] {
                let mut app = App::new(Vec::new(), language);
                app.transcript_width = width;
                app.echo_runtime_submission(body, &turn(status), held, None);
                assert_eq!(app.transcript[0], format!("You: {symbol} {body}"));
                let folded = app.display_transcript();
                let frame = app.render_user_transcript(
                    &folded,
                    &vec![None; folded.rows.len()],
                    None,
                    folded.rows.len(),
                );
                assert!(frame.text.lines.iter().all(|line| line.width() <= width));
                assert!(
                    frame
                        .text
                        .lines
                        .iter()
                        .flat_map(|line| &line.spans)
                        .any(|span| {
                            span.content == symbol
                                && span.style.fg == Some(color)
                                && span.style.add_modifier.contains(Modifier::BOLD)
                                && span.style.bg == Some(user_message::BLOCK_BG)
                        })
                );
                app.prepare_transcript_frame();
                app.chat_selection = Some(ChatSelection {
                    anchor: ChatSelectionPoint { row: 0, column: 0 },
                    head: ChatSelectionPoint {
                        row: app.transcript_rows.len() - 1,
                        column: width - 1,
                    },
                });
                assert_eq!(app.selected_chat_text(), body);
            }
        }
    }
}

#[test]
fn unconfirmed_submission_has_no_acceptance_or_success_symbol() {
    let mut app = App::new(Vec::new(), Language::ZhCn);
    app.note_runtime_submission_unconfirmed("still a draft", uuid::Uuid::new_v4(), false, None);
    // A missing placeholder is not revived by an unconfirmed receipt.
    assert!(app.transcript.is_empty());
    app.append_transcript("You: still a draft".into());
    app.note_runtime_submission_unconfirmed("still a draft", uuid::Uuid::new_v4(), false, Some(0));
    assert_eq!(app.transcript, ["You: ? still a draft"]);
    assert_eq!(
        app.pending_runtime_echoes[1].status,
        DeliveryStatus::Unconfirmed
    );
}

#[test]
fn a_delivered_message_clears_only_the_last_pending_delivery_notice() {
    let mut app = App::new(Vec::new(), Language::En);
    let first = turn(TurnStatus::Queued);
    let second = turn(TurnStatus::Queued);
    app.echo_runtime_submission("one", &first, false, None);
    app.echo_runtime_submission("two", &second, false, None);
    app.notice = Some(DeliveryStatus::Queued.feedback(Language::En).into());
    app.note_runtime_input_started(first.id, "one");
    assert!(app.notice.is_some());
    app.note_runtime_input_started(second.id, "two");
    assert!(app.notice.is_none());
    app.notice = Some("unrelated feedback".into());
    app.note_runtime_input_started(second.id, "two");
    assert_eq!(app.notice.as_deref(), Some("unrelated feedback"));
}
