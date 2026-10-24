use super::*;

fn fixture() -> (std::path::PathBuf, SessionStore, Session, App) {
    let root =
        std::env::temp_dir().join(format!("willdeep-runtime-stream-{}", uuid::Uuid::new_v4()));
    let store = SessionStore::new(&root);
    let mut session = Session::new(root.clone(), None, "streaming");
    session.runtime_managed = true;
    session.messages.push(Message::user("preserved history"));
    store.save(&mut session).unwrap();
    (root, store, session, App::new(Vec::new(), Language::En))
}

fn output(
    sequence: u64,
    session_id: uuid::Uuid,
    kind: &str,
    text: &str,
) -> crate::daemon::RemoteRuntimeEvent {
    crate::daemon::RemoteRuntimeEvent {
        timestamp: 0,
        sequence,
        kind: "task.output".into(),
        message: format!(
            "task_id={} {}",
            uuid::Uuid::nil(),
            serde_json::json!({"type":kind,"text":text})
        ),
        visible: true,
        session_id: Some(session_id),
    }
}

#[test]
fn runtime_text_and_reasoning_streams_do_not_rewrite_the_session() {
    let (root, store, mut session, mut app) = fixture();
    let path = root.join("sessions").join(format!("{}.json", session.id));
    let before = std::fs::read(&path).unwrap();
    for sequence in 1..=200 {
        let event = output(sequence, session.id, "reasoning_delta", "思考");
        runtime_ui::apply_runtime_events(&mut app, vec![event], &mut session, &store).unwrap();
    }
    assert_eq!(
        app.transient_thought.as_deref(),
        Some("思考".repeat(200).as_str())
    );
    for sequence in 201..=400 {
        let event = output(sequence, session.id, "assistant_text_delta", "新字");
        runtime_ui::apply_runtime_events(&mut app, vec![event], &mut session, &store).unwrap();
    }
    assert_eq!(
        app.transient_thought.as_deref(),
        Some("新字".repeat(200).as_str())
    );
    assert_eq!(app.runtime_event_cursor, 400);
    assert_eq!(
        std::fs::read(path).unwrap(),
        before,
        "live previews must not contend with the Runtime's Session writer"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn another_sessions_event_stream_never_rewrites_local_history() {
    let (root, store, mut session, mut app) = fixture();
    let path = root.join("sessions").join(format!("{}.json", session.id));
    let before = std::fs::read(&path).unwrap();
    let event = output(1, uuid::Uuid::new_v4(), "assistant_text", "other Session");
    runtime_ui::apply_runtime_events(&mut app, vec![event], &mut session, &store).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert!(app.transcript.is_empty());
    assert_eq!(app.runtime_event_cursor, 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn settled_turn_merges_cursor_without_replacing_a_new_runtime_checkpoint() {
    let (root, store, mut session, mut app) = fixture();
    let delta = output(1, session.id, "assistant_text_delta", "partial");
    runtime_ui::apply_runtime_events(&mut app, vec![delta], &mut session, &store).unwrap();
    store
        .update(session.id, |latest| {
            latest
                .messages
                .push(Message::assistant("complete Runtime answer", Vec::new()));
        })
        .unwrap();
    let boundary = output(2, session.id, "assistant_text", "complete Runtime answer");
    runtime_ui::apply_runtime_events(&mut app, vec![boundary.clone()], &mut session, &store)
        .unwrap();
    runtime_ui::apply_runtime_events(&mut app, vec![boundary], &mut session, &store).unwrap();
    assert_eq!(store.load(session.id).unwrap().runtime_event_cursor, 0);
    let mut finished = output(3, session.id, "", "");
    finished.kind = "turn.completed".into();
    runtime_ui::apply_runtime_events(&mut app, vec![finished], &mut session, &store).unwrap();
    let saved = store.load(session.id).unwrap();
    assert_eq!(saved.runtime_event_cursor, 3);
    assert_eq!(saved.messages.len(), 2);
    assert_eq!(saved.messages[1].content, "complete Runtime answer");
    assert_eq!(transcript(&session.messages), transcript(&saved.messages));
    assert_eq!(
        app.transcript
            .iter()
            .filter(|row| row.contains("complete Runtime answer"))
            .count(),
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn graceful_exit_flushes_pending_cursor_and_preserves_runtime_partial_text() {
    let (root, store, mut session, mut app) = fixture();
    let delta = output(7, session.id, "assistant_text_delta", "partial");
    runtime_ui::apply_runtime_events(&mut app, vec![delta], &mut session, &store).unwrap();
    store
        .update(session.id, |latest| {
            latest
                .messages
                .push(Message::assistant("durable partial", Vec::new()));
        })
        .unwrap();
    runtime_ui::sync_runtime_session(&mut app, &mut session, &store).unwrap();
    let saved = store.load(session.id).unwrap();
    assert_eq!(saved.runtime_event_cursor, 7);
    assert_eq!(saved.messages[1].content, "durable partial");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_startup_and_tool_boundaries_do_not_write_during_an_active_turn() {
    let (root, store, mut session, mut app) = fixture();
    let path = root.join("sessions").join(format!("{}.json", session.id));
    let before = std::fs::read(&path).unwrap();
    app.begin_turn(true, "starting".into());
    let mut started = output(1, session.id, "turn_started", "");
    started.kind = "task.started".into();
    runtime_ui::apply_runtime_events(&mut app, vec![started], &mut session, &store).unwrap();
    let boundary = output(2, session.id, "assistant_text", "narration before tool");
    runtime_ui::apply_runtime_events(&mut app, vec![boundary], &mut session, &store).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let mut finished = output(3, session.id, "", "");
    finished.kind = "turn.completed".into();
    runtime_ui::apply_runtime_events(&mut app, vec![finished], &mut session, &store).unwrap();
    assert_eq!(store.load(session.id).unwrap().runtime_event_cursor, 3);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn first_submission_rebases_a_stopped_runtimes_subscription_to_its_existing_head() {
    let (root, _store, mut session, mut app) = fixture();
    runtime_ui::initialize_submission_cursor(&mut app, &mut session, 20_000);
    assert_eq!(app.runtime_event_cursor, 20_000);
    assert_eq!(session.runtime_event_cursor, 20_000);
    assert_eq!(app.runtime_follow_after.take(), Some(20_000));
    runtime_ui::initialize_submission_cursor(&mut app, &mut session, 21_000);
    assert_eq!(
        app.runtime_event_cursor, 20_000,
        "an established cursor must keep replaying missed events"
    );
    assert!(app.runtime_follow_after.is_none());
    std::fs::remove_dir_all(root).unwrap();
}
