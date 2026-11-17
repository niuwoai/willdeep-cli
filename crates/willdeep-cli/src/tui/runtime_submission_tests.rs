use super::*;

#[test]
fn failed_idle_submission_restores_original_command_cursor_and_expansion() {
    for original in ["plain 中文😀 draft", "/runtime keep original"] {
        let mut app = App::new(Vec::new(), Language::En);
        app.input.insert(original);
        app.input.left();
        app.composer_expanded = true;
        let saved = SubmissionDraft::capture(&app);
        let prompt = app.take_prompt_for_submission();
        assert_eq!(prompt, original);
        assert!(app.input.is_empty());
        assert!(!app.composer_expanded);
        saved.restore(&mut app);
        assert!(app.composer_expanded);
        assert_eq!(app.input.text(), original);
        app.input.insert("!");
        let last = original.char_indices().last().unwrap().0;
        assert_eq!(
            app.input.text(),
            format!("{}!{}", &original[..last], &original[last..])
        );
    }
}

#[test]
fn queued_dispatch_restores_an_empty_or_edited_draft_without_a_message_copy() {
    for original in ["", "unrelated draft"] {
        let mut app = App::new(Vec::new(), Language::En);
        app.input.insert(original);
        app.input.left();
        app.composer_expanded = true;
        let saved = SubmissionDraft::take(&mut app);
        // A dispatch may use the otherwise empty composer. Its failure must not
        // promote the queued message to a second independently sendable draft.
        app.input.insert("queued message");
        saved.restore(&mut app);
        assert_eq!(app.input.text(), original);
        assert!(app.composer_expanded);
        if !original.is_empty() {
            app.input.insert("!");
            assert_eq!(app.input.text(), "unrelated draf!t");
        }
    }
}

#[test]
fn snapshot_restoration_binds_task_identity_before_replayed_terminal_events() {
    let session_id = uuid::Uuid::new_v4();
    let task_id = uuid::Uuid::new_v4();
    let turn_id = uuid::Uuid::new_v4();
    let task = crate::daemon::tui_bridge::RemoteTask {
        id: task_id,
        session_id: Some(session_id),
        turn_id: Some(turn_id),
        agent_id: None,
        status: willdeep_runtime_protocol::TaskStatus::Running,
        profile: None,
        created_at: 1,
        started_at: Some(2),
        completed_at: None,
        exit_code: None,
        failure_domain: None,
    };
    let mut app = App::new(Vec::new(), Language::En);
    app.runtime_event_cursor = 10;
    app.observe_runtime_tasks(&[task], session_id, Some(10));
    assert!(app.running);
    assert!(!app.runtime_first_reply_eligible);
    let event = |id| crate::daemon::RemoteRuntimeEvent {
        sequence: 11,
        timestamp: 11,
        kind: "task.failed".into(),
        message: format!("task_id={id}"),
        session_id: Some(session_id),
        visible: true,
    };
    apply_runtime_event(&mut app, &event(uuid::Uuid::new_v4()));
    assert!(app.running);
    assert_eq!(app.last_turn_outcome, None);
    apply_runtime_event(&mut app, &event(task_id));
    assert!(!app.running);
    assert_eq!(
        app.last_turn_outcome,
        Some(turn_status::TurnOutcome::Failed)
    );
}
