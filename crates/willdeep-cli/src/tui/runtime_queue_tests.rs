use super::*;
use willdeep_runtime_protocol::TurnStatus;

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

fn message(receipt: &RuntimeTurn, text: &str) -> QueuedTurnMessage {
    QueuedTurnMessage {
        id: receipt.id,
        session_id: receipt.session_id,
        text: text.into(),
        text_preview: text.into(),
        can_edit: true,
        image_count: 1,
        text_attachment_count: 1,
        held: false,
        priority_requested: false,
        independence_confirmed: false,
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn unconfirmed_submission_retains_editor_cursor_attachments_and_request_key() {
    let mut app = App::new(Vec::new(), Language::En);
    app.input.insert("keep cursor");
    app.input.left();
    app.composer_expanded = true;
    let image = encode_clipboard_image(1, 1, vec![255, 0, 0, 255]).unwrap();
    let note = DraftAttachment {
        id: uuid::Uuid::new_v4(),
        message: MessageAttachment::Text {
            name: "draft.txt".into(),
            content: "keep".into(),
        },
    };
    app.attachments = vec![image.clone(), note.clone()];
    let attachments = vec![image.message.clone(), note.message.clone()];
    let session_id = uuid::Uuid::new_v4();
    let request_id =
        submission_request_id(&mut app, "keep cursor", &attachments, session_id, false).unwrap();
    let (draft, expanded, row) = prepare_input_submission(&mut app, false);
    assert!(app.input.is_empty());
    assert_eq!(app.transcript[row], "You: ? keep cursor");
    finish_input_submission(
        &mut app,
        draft,
        expanded,
        row,
        false,
        &Err(anyhow::anyhow!("receipt lost after acceptance")),
    );
    assert!(app.composer_expanded);
    assert_eq!(
        app.attachments
            .iter()
            .map(|item| item.id)
            .collect::<Vec<_>>(),
        vec![image.id, note.id]
    );
    assert_eq!(app.transcript[row], "You: ? keep cursor");
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("Submission unconfirmed")
    );
    assert_eq!(app.pending_runtime_echoes[0].request_id, request_id);
    assert!(app.pending_runtime_echoes[0].needs_reconciliation());
    assert!(!app.transcript[row].contains("not sent"));
    assert_eq!(
        submission_request_id(&mut app, "keep cursor", &attachments, session_id, false).unwrap(),
        request_id
    );
    app.input.insert("!");
    assert_eq!(app.input.text(), "keep curso!r");
}

#[test]
fn queueing_a_supplement_keeps_reading_mode_on_success_and_failure() {
    for succeeds in [true, false] {
        let mut app = App::new(vec!["history".into()], Language::En);
        app.follow_bottom = false;
        app.scroll_from_bottom = 10;
        app.input.insert("supplement");
        let (draft, expanded, row) = prepare_input_submission(&mut app, false);
        assert!(!app.follow_bottom);
        let result = if succeeds {
            Ok(turn(TurnStatus::Queued))
        } else {
            Err(anyhow::anyhow!("lost receipt"))
        };
        finish_input_submission(&mut app, draft, expanded, row, false, &result);
        assert!(!app.follow_bottom);
    }
}

#[test]
fn success_uses_the_real_queued_running_or_completed_receipt_and_exact_record() {
    for status in [
        TurnStatus::Queued,
        TurnStatus::Running,
        TurnStatus::Completed,
    ] {
        let mut app = App::new(Vec::new(), Language::En);
        app.input.insert("accepted");
        app.attachments.push(DraftAttachment {
            id: uuid::Uuid::new_v4(),
            message: MessageAttachment::Text {
                name: "attached.txt".into(),
                content: "retained".into(),
            },
        });
        let (draft, expanded, row) = prepare_input_submission(&mut app, false);
        let recorded_at = app.message_times.time_at(row);
        let receipt = turn(status);
        finish_input_submission(&mut app, draft, expanded, row, false, &Ok(receipt.clone()));
        let echo = &app.pending_runtime_echoes[0];
        assert_eq!(echo.turn_id, receipt.id);
        assert_eq!(echo.request_id, receipt.request_id);
        assert_eq!(echo.record, Some(row));
        assert_eq!(app.message_times.time_at(row), recorded_at);
        assert_eq!(app.transcript_attachments[row].len(), 1);
        assert_eq!(
            app.notice.as_deref(),
            Some(echo.status.feedback(Language::En))
        );
        assert!(app.input.is_empty());
        assert!(app.attachments.is_empty());
        assert_eq!(app.last_turn_outcome, echo.status.outcome());
        assert!(app.transcript.iter().all(|row| !row.starts_with("System:")));
    }
}

#[test]
fn held_submission_keeps_its_attachment_and_does_not_claim_automatic_delivery() {
    let mut app = App::new(Vec::new(), Language::En);
    let session_id = uuid::Uuid::new_v4();
    let immediate = submission_request_id(&mut app, "later", &[], session_id, false).unwrap();
    let held = submission_request_id(&mut app, "later", &[], session_id, true).unwrap();
    assert_ne!(immediate, held);
    app.input.insert("later");
    app.attachments
        .push(encode_clipboard_image(1, 1, vec![255, 0, 0, 255]).unwrap());
    let (draft, expanded, row) = prepare_input_submission(&mut app, true);
    finish_input_submission(
        &mut app,
        draft,
        expanded,
        row,
        true,
        &Ok(turn(TurnStatus::Queued)),
    );
    assert_eq!(app.transcript[row], "You: ‖ later");
    assert_eq!(app.transcript_attachments[row].len(), 1);
    assert!(
        app.notice
            .as_deref()
            .unwrap()
            .contains("will not auto-send")
    );
    assert!(app.awaiting_since.is_none());
}

#[test]
fn replayed_receipt_after_a_lost_response_reuses_the_existing_started_record() {
    let mut app = App::new(Vec::new(), Language::En);
    let receipt = turn(TurnStatus::Running);
    app.note_runtime_input_started(receipt.id, "accepted before timeout");
    app.input.insert("accepted before timeout");
    let (draft, expanded, row) = prepare_input_submission(&mut app, false);
    finish_input_submission(&mut app, draft, expanded, row, false, &Ok(receipt));
    assert_eq!(
        app.transcript
            .iter()
            .filter(|row| row.starts_with("You: "))
            .count(),
        1
    );
    assert_eq!(app.pending_runtime_echoes.len(), 1);
    assert_eq!(app.pending_runtime_echoes[0].record, Some(0));
}

#[test]
fn actual_output_events_keep_same_text_with_different_ids_and_replay_once() {
    let mut app = App::new(Vec::new(), Language::En);
    let own = turn(TurnStatus::Queued);
    app.echo_runtime_submission("same", &own, false, None);
    app.sync_runtime_queue(Vec::new());
    let other_id = uuid::Uuid::new_v4();
    for id in [other_id, own.id, own.id, other_id] {
        let event = serde_json::json!({"type":"queued_user_message", "turn_id":id, "text":"same"});
        runtime_ui::apply_runtime_output(&mut app, &format!("task {event}"));
    }
    assert_eq!(
        app.transcript
            .iter()
            .filter(|row| row.starts_with("You: "))
            .count(),
        2
    );
    assert_eq!(app.pending_runtime_echoes.len(), 2);
    assert!(
        app.pending_runtime_echoes
            .iter()
            .all(|echo| !echo.is_pending())
    );
    assert!(
        !app.running,
        "start input replay alone must not recreate a busy turn"
    );
}

#[test]
fn edit_conflicts_keep_original_id_expected_text_and_rewrite_without_local_send() {
    for conflict in ["claimed", "changed", "removed"] {
        let mut app = App::new(Vec::new(), Language::En);
        let receipt = turn(TurnStatus::Queued);
        let original = message(&receipt, "original");
        app.sync_runtime_queue(vec![original.clone()]);
        app.open_prompt_queue();
        app.handle_queue_key(key(KeyCode::Enter));
        app.prompt_queue
            .as_mut()
            .unwrap()
            .editor
            .as_mut()
            .unwrap()
            .insert(" rewrite");
        let current = match conflict {
            "changed" => vec![message(&receipt, "changed on mobile")],
            _ => Vec::new(),
        };
        if conflict == "claimed" {
            app.note_runtime_input_started(receipt.id, "original");
        }
        app.sync_runtime_queue(current);
        let before = app.transcript.clone();
        assert!(remote_edit_conflict(&app, &original).is_some());
        assert!(!app.handle_queue_key(key(KeyCode::Enter)));
        let panel = app.prompt_queue.as_ref().unwrap();
        assert_eq!(panel.remote_edit.as_ref().unwrap().id, original.id);
        assert_eq!(panel.remote_edit.as_ref().unwrap().text, "original");
        assert_eq!(panel.editor.as_ref().unwrap().text(), "original rewrite");
        assert!(panel.status.is_some());
        assert_eq!(app.transcript, before);
        assert!(app.input.is_empty());
    }
}

#[test]
fn updating_one_of_identical_remote_messages_preserves_both_attachment_slots() {
    let mut app = App::new(Vec::new(), Language::En);
    let first = turn(TurnStatus::Queued);
    let second = turn(TurnStatus::Queued);
    app.echo_runtime_submission("same", &first, false, None);
    app.echo_runtime_submission("same", &second, false, None);
    app.transcript_attachments[0] = vec![MessageAttachment::Text {
        name: "first.txt".into(),
        content: "first".into(),
    }];
    app.transcript_attachments[1] = vec![MessageAttachment::Text {
        name: "second.txt".into(),
        content: "second".into(),
    }];
    app.sync_runtime_queue(vec![message(&first, "changed"), message(&second, "same")]);
    assert!(app.transcript[0].ends_with("changed"));
    assert!(app.transcript[1].ends_with("same"));
    assert!(
        matches!(&app.transcript_attachments[0][0], MessageAttachment::Text { name, .. } if name == "first.txt")
    );
    assert!(
        matches!(&app.transcript_attachments[1][0], MessageAttachment::Text { name, .. } if name == "second.txt")
    );
}
