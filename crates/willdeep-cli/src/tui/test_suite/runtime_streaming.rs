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

fn apply_output(app: &mut App, value: serde_json::Value) {
    runtime_ui::apply_runtime_output(app, &format!("task_id={} {value}", uuid::Uuid::nil()));
}

#[test]
fn runtime_first_reply_records_the_first_body_delta_and_keeps_usage_separate() {
    let mut app = App::new(Vec::new(), Language::En);
    app.begin_turn(true, "working".into());
    app.turn_started = Some(Instant::now() - Duration::from_secs(3));
    app.context_tokens = 321;
    apply_output(
        &mut app,
        serde_json::json!({"type":"reasoning_delta","text":"thinking"}),
    );
    apply_output(
        &mut app,
        serde_json::json!({"type":"assistant_text_delta","text":""}),
    );
    assert!(app.turn_first_reply.is_none());

    apply_output(
        &mut app,
        serde_json::json!({"type":"assistant_text_delta","text":"first"}),
    );
    let first = app
        .turn_first_reply
        .expect("body delta records first reply before usage");
    assert!(first >= Duration::from_secs(3));
    assert_eq!(app.context_tokens, 321);
    assert_eq!(app.turn_input_tokens, 0);
    assert_eq!(app.turn_output_tokens, 0);

    // 后续生成和第二次调模型不许把首答挪到完整响应的时刻。
    app.turn_started = Some(Instant::now() - Duration::from_secs(30));
    for value in [
        serde_json::json!({"type":"assistant_text_delta","text":" reply"}),
        serde_json::json!({"type":"usage","input_tokens":1000,"output_tokens":200,"total_tokens":1200,"cache_read_tokens":40}),
        serde_json::json!({"type":"turn_started","turn":2}),
        serde_json::json!({"type":"assistant_text","text":"first reply"}),
        serde_json::json!({"type":"usage","input_tokens":1500,"output_tokens":300,"total_tokens":1800,"cache_read_tokens":50}),
    ] {
        apply_output(&mut app, value);
        assert_eq!(app.turn_first_reply, Some(first));
    }
    assert_eq!(app.turn_input_tokens, 2500);
    assert_eq!(app.turn_output_tokens, 500);
    assert_eq!(app.context_tokens, 1500);
    assert_eq!(app.latest_usage.input_tokens, Some(1500));
    assert_eq!(app.latest_usage.output_tokens, Some(300));
    assert_eq!(app.latest_usage.total_tokens, Some(1800));
    assert_eq!(app.latest_usage.cache_read_tokens, Some(50));

    apply_output(
        &mut app,
        serde_json::json!({"type":"completed","text":"first reply"}),
    );
    let line = app.transcript.last().unwrap();
    assert!(line.contains(&format!(
        "first reply {}",
        format_elapsed_span(first.as_secs_f32(), 2)
    )));
    assert!(line.contains("in 2.50K"));
    assert!(line.contains("out 500"));
    assert_eq!(app.turn_first_reply, Some(first));
    assert!(!app.running);
}

#[test]
fn runtime_first_reply_falls_back_to_nonstreaming_text_without_usage() {
    for kind in ["assistant_text", "completed", "partial"] {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(true, "working".into());
        apply_output(
            &mut app,
            serde_json::json!({"type":kind,"text":"complete response"}),
        );
        assert!(app.turn_first_reply.is_some(), "{kind}");
        assert_eq!(app.turn_input_tokens, 0);
        assert_eq!(app.turn_output_tokens, 0);
    }
}

#[test]
fn runtime_first_reply_uses_usage_for_responses_without_body_text() {
    let mut app = App::new(Vec::new(), Language::En);
    app.begin_turn(true, "working".into());
    apply_output(
        &mut app,
        serde_json::json!({"type":"reasoning_delta","text":"thinking"}),
    );
    assert!(app.turn_first_reply.is_none());
    apply_output(
        &mut app,
        serde_json::json!({"type":"usage","input_tokens":100,"output_tokens":20}),
    );
    let first = app
        .turn_first_reply
        .expect("completed tool response is a fallback");
    app.turn_started = Some(Instant::now() - Duration::from_secs(30));
    apply_output(
        &mut app,
        serde_json::json!({"type":"assistant_text_delta","text":"later answer"}),
    );
    assert_eq!(app.turn_first_reply, Some(first));
}

#[test]
fn runtime_first_reply_uses_tool_request_when_provider_omits_usage() {
    let mut app = App::new(Vec::new(), Language::En);
    app.begin_turn(true, "working".into());
    app.turn_started = Some(Instant::now() - Duration::from_secs(3));
    apply_output(
        &mut app,
        serde_json::json!({"type":"reasoning_delta","text":"thinking"}),
    );
    assert!(app.turn_first_reply.is_none());
    // Core emits no Usage for usage=None, but ToolRequested follows the
    // completed response and precedes execution of the potentially slow tool.
    apply_output(
        &mut app,
        serde_json::json!({"type":"tool_requested","name":"run_command"}),
    );
    let first = app.turn_first_reply.expect("tool response has arrived");
    assert!(first >= Duration::from_secs(3));
    app.turn_started = Some(Instant::now() - Duration::from_secs(30));
    apply_output(
        &mut app,
        serde_json::json!({"type":"tool_completed","name":"run_command","is_error":false}),
    );
    apply_output(
        &mut app,
        serde_json::json!({"type":"assistant_text_delta","text":"later answer"}),
    );
    assert_eq!(app.turn_first_reply, Some(first));
    assert_eq!(app.turn_input_tokens, 0);
    assert_eq!(app.turn_output_tokens, 0);
}

#[test]
fn runtime_first_reply_matches_local_nonempty_whitespace_semantics() {
    let mut app = App::new(Vec::new(), Language::En);
    app.begin_turn(true, "working".into());
    apply_output(
        &mut app,
        serde_json::json!({"type":"assistant_text_delta","text":""}),
    );
    assert!(app.turn_first_reply.is_none());
    // Core 的 StreamEvents 用 is_empty 判断，空白也是正文流的一部分。
    apply_output(
        &mut app,
        serde_json::json!({"type":"assistant_text_delta","text":" "}),
    );
    assert!(app.turn_first_reply.is_some());
}

#[test]
fn runtime_first_reply_resets_for_a_new_turn_and_requires_an_active_turn() {
    let mut app = App::new(Vec::new(), Language::En);
    app.record_turn_first_reply();
    assert!(app.turn_first_reply.is_none());
    // 第一次历史输出会恢复运行态，但不能据此凭空产生首答。
    apply_output(
        &mut app,
        serde_json::json!({"type":"assistant_text_delta","text":"restored"}),
    );
    assert!(app.running);
    assert!(app.turn_first_reply.is_none());
    app.begin_turn(true, "working".into());
    apply_output(
        &mut app,
        serde_json::json!({"type":"assistant_text_delta","text":"live"}),
    );
    let first = app.turn_first_reply.unwrap();
    app.finish_turn();
    app.record_turn_first_reply();
    assert_eq!(app.turn_first_reply, Some(first));
    app.begin_turn(true, "next".into());
    assert!(app.turn_first_reply.is_none());
    apply_output(
        &mut app,
        serde_json::json!({"type":"reasoning_delta","text":"thinking"}),
    );
    assert!(app.turn_first_reply.is_none());
    apply_output(
        &mut app,
        serde_json::json!({"type":"assistant_text_delta","text":"next reply"}),
    );
    assert!(app.turn_first_reply.is_some());
}

#[test]
fn runtime_first_reply_does_not_time_historical_replay() {
    for age_seconds in [0, 120] {
        let (root, store, mut session, mut app) = fixture();
        let mut events = vec![
            output(1, session.id, "turn_started", ""),
            output(2, session.id, "assistant_text_delta", "old answer"),
            output(3, session.id, "usage", ""),
            output(4, session.id, "assistant_text", "old answer"),
            output(5, session.id, "completed", "old answer"),
        ];
        for event in &mut events {
            event.timestamp = message_time::now().saturating_sub(age_seconds);
        }
        runtime_ui::apply_runtime_events(&mut app, events, &mut session, &store).unwrap();
        assert!(app.turn_first_reply.is_none(), "replay age={age_seconds}");
        assert!(!app.running);
        assert!(!app.transcript.last().unwrap().contains("first reply"));
        assert!(app.transcript.iter().any(|row| row.contains("old answer")));
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn restored_running_state_cannot_grant_first_reply_timing() {
    let (root, store, mut session, mut app) = fixture();
    // The same entry point is used by fresh active-task snapshots.
    app.ensure_runtime_turn();
    let mut body = output(1, session.id, "assistant_text_delta", "replayed body");
    body.timestamp = message_time::now();
    runtime_ui::apply_runtime_events(&mut app, vec![body], &mut session, &store).unwrap();
    assert!(app.running);
    assert!(app.turn_first_reply.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_first_reply_ignores_old_turns_duplicate_and_unrelated_events() {
    let (root, store, mut session, mut app) = fixture();
    app.begin_turn(true, "working".into());
    app.runtime_first_reply_after = 2;
    app.turn_started = Some(Instant::now() - Duration::from_secs(10));
    let mut old = output(1, session.id, "assistant_text_delta", "old turn");
    old.timestamp = message_time::now().saturating_sub(120);
    let mut old_usage = output(2, session.id, "usage", "");
    old_usage.timestamp = old.timestamp;
    runtime_ui::apply_runtime_events(&mut app, vec![old, old_usage], &mut session, &store).unwrap();
    assert!(app.turn_first_reply.is_none());

    let mut current = output(3, session.id, "assistant_text_delta", "live answer");
    current.timestamp = message_time::now();
    runtime_ui::apply_runtime_events(&mut app, vec![current.clone()], &mut session, &store)
        .unwrap();
    assert!(app.turn_first_reply.unwrap() >= Duration::from_secs(10));

    app.finish_turn();
    app.begin_turn(true, "next".into());
    let mut unrelated = output(
        4,
        uuid::Uuid::new_v4(),
        "assistant_text_delta",
        "other session",
    );
    unrelated.timestamp = message_time::now();
    let mut hidden = output(5, session.id, "assistant_text_delta", "hidden");
    hidden.timestamp = message_time::now();
    hidden.visible = false;
    runtime_ui::apply_runtime_events(
        &mut app,
        vec![current, unrelated, hidden],
        &mut session,
        &store,
    )
    .unwrap();
    assert!(app.turn_first_reply.is_none());
    assert_eq!(app.runtime_event_cursor, 5);

    let mut next = output(6, session.id, "assistant_text_delta", "next reply");
    next.timestamp = message_time::now();
    runtime_ui::apply_runtime_events(&mut app, vec![next], &mut session, &store).unwrap();
    assert!(app.turn_first_reply.is_some());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_first_reply_excludes_same_second_output_before_submission_head() {
    let (root, store, mut session, mut app) = fixture();
    app.runtime_event_cursor = 99;
    app.begin_turn(true, "working".into());
    app.runtime_first_reply_after = 103;
    let started = app.turn_started;
    let mut old_body = output(100, session.id, "assistant_text_delta", "old answer");
    old_body.timestamp = message_time::now();
    let mut old_usage = output(101, session.id, "usage", "");
    old_usage.timestamp = old_body.timestamp;
    let mut old_completed = output(102, session.id, "completed", "old answer");
    old_completed.timestamp = old_body.timestamp;
    let mut old_terminal = output(103, session.id, "", "");
    old_terminal.kind = "task.completed".into();
    old_terminal.timestamp = old_body.timestamp;
    runtime_ui::apply_runtime_events(
        &mut app,
        vec![old_body, old_usage, old_completed, old_terminal],
        &mut session,
        &store,
    )
    .unwrap();
    assert!(app.turn_first_reply.is_none());
    assert!(app.running, "an old completion cannot finish the new turn");
    assert_eq!(app.turn_started, started);
    assert_eq!(app.runtime_first_reply_after, 103);
    assert_eq!(app.turn_input_tokens, 0);
    let mut live_body = output(104, session.id, "assistant_text_delta", "new answer");
    live_body.timestamp = message_time::now();
    runtime_ui::apply_runtime_events(&mut app, vec![live_body], &mut session, &store).unwrap();
    assert!(app.turn_first_reply.is_some());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_submission_wait_is_measured_and_pre_receipt_output_is_accepted() {
    let (root, store, mut session, mut app) = fixture();
    let started = Instant::now() - Duration::from_secs(5);
    let task_id = uuid::Uuid::nil();
    runtime_ui::begin_submitted_turn(&mut app, started, 10, uuid::Uuid::new_v4(), Some(task_id));
    // The task answered while submit_runtime_turn was awaiting its receipt
    // and follow-up Turn read. Its timestamp may precede local begin_turn.
    let mut body = output(11, session.id, "assistant_text_delta", "fast answer");
    body.timestamp = message_time::now().saturating_sub(3);
    runtime_ui::apply_runtime_events(&mut app, vec![body], &mut session, &store).unwrap();
    assert_eq!(app.turn_started, Some(started));
    assert!(app.turn_first_reply.unwrap() >= Duration::from_secs(5));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn runtime_submission_binds_the_receipted_turn_before_accepting_task_output() {
    let (root, store, mut session, mut app) = fixture();
    let started = Instant::now() - Duration::from_secs(3);
    let turn_id = uuid::Uuid::new_v4();
    let other_turn_id = uuid::Uuid::new_v4();
    let task_id = uuid::Uuid::new_v4();
    runtime_ui::begin_submitted_turn(&mut app, started, 10, turn_id, None);
    let mut other_input = output(11, session.id, "", "");
    other_input.message = format!(
        "task_id={} {}",
        uuid::Uuid::nil(),
        serde_json::json!({"type":"queued_user_message","turn_id":other_turn_id,"text":"other input"})
    );
    let other_body = output(12, session.id, "assistant_text_delta", "other answer");
    let other_done = output(13, session.id, "completed", "other answer");
    runtime_ui::apply_runtime_events(
        &mut app,
        vec![other_input, other_body, other_done],
        &mut session,
        &store,
    )
    .unwrap();
    assert!(app.running);
    assert_eq!(app.turn_started, Some(started));
    assert!(app.turn_first_reply.is_none());
    assert_eq!(app.runtime_submitted_task, None);

    // A queued receipt has no Task ID yet. Runtime emits this identity event
    // before any Harness output, allowing the submitted Turn to claim its Task.
    let mut own_input = output(14, session.id, "", "");
    own_input.message = format!(
        "task_id={task_id} {}",
        serde_json::json!({"type":"queued_user_message","turn_id":turn_id,"text":"my input"})
    );
    runtime_ui::apply_runtime_events(&mut app, vec![own_input], &mut session, &store).unwrap();
    assert_eq!(app.runtime_submitted_task, Some(task_id));
    assert!(app.turn_first_reply.is_none());

    let other_body = output(15, session.id, "assistant_text_delta", "still not ours");
    runtime_ui::apply_runtime_events(&mut app, vec![other_body], &mut session, &store).unwrap();
    assert!(app.turn_first_reply.is_none());
    let mut own_body = output(16, session.id, "assistant_text_delta", "own answer");
    own_body.message = format!(
        "task_id={task_id} {}",
        serde_json::json!({"type":"assistant_text_delta","text":"own answer"})
    );
    own_body.timestamp = message_time::now().saturating_sub(120);
    runtime_ui::apply_runtime_events(&mut app, vec![own_body], &mut session, &store).unwrap();
    assert!(app.turn_first_reply.unwrap() >= Duration::from_secs(3));
    assert_eq!(app.turn_started, Some(started));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn local_first_reply_still_prefers_core_timing_over_usage_fallback() {
    let mut app = App::new(Vec::new(), Language::En);
    app.begin_turn(false, "working".into());
    app.record_turn_usage(&Usage::default());
    app.turn_first_reply = Some(Duration::from_secs(20));
    let outcome = willdeep_core::AgentOutcome {
        final_text: "done".into(),
        turns: 1,
        messages: Vec::new(),
        stop_reason: willdeep_core::AgentStopReason::Finished,
        input_tokens: 0,
        output_tokens: 0,
        first_response_millis: Some(500),
        context_summary: None,
    };
    app.append_turn_stats(Some(&outcome));
    assert!(
        app.transcript
            .last()
            .unwrap()
            .contains(&format!("first reply {}", format_elapsed_span(0.5, 2)))
    );
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
