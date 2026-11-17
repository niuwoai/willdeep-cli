use super::*;
use turn_status::{TurnOutcome, TurnPhase};

fn output(app: &mut App, value: serde_json::Value) {
    apply_runtime_output(app, &format!("task_id=test {value}"));
}

fn event(kind: &str, sequence: u64, task: uuid::Uuid) -> crate::daemon::RemoteRuntimeEvent {
    crate::daemon::RemoteRuntimeEvent {
        sequence,
        timestamp: sequence,
        kind: kind.into(),
        message: format!("task_id={task}"),
        session_id: None,
        visible: true,
    }
}

#[test]
fn runtime_preparation_request_and_streaming_use_real_boundaries() {
    let mut app = App::new(Vec::new(), Language::En);
    app.begin_turn(true, "working".into());
    output(
        &mut app,
        serde_json::json!({"type":"turn_started", "turn":1}),
    );
    assert_eq!(app.turn_phase, TurnPhase::Preparing);
    output(
        &mut app,
        serde_json::json!({"type":"compression_started", "estimated_tokens":32000}),
    );
    assert!(app.working_summary().unwrap().contains("Compressing"));
    output(
        &mut app,
        serde_json::json!({"type":"compression_completed", "estimated_tokens":1000}),
    );
    output(
        &mut app,
        serde_json::json!({"type":"model_request_started"}),
    );
    assert_eq!(app.turn_phase, TurnPhase::WaitingModel);
    output(
        &mut app,
        serde_json::json!({"type":"reasoning_delta", "text":"check"}),
    );
    assert_eq!(app.turn_phase, TurnPhase::Reasoning);
    output(
        &mut app,
        serde_json::json!({"type":"assistant_text_delta", "text":"answer"}),
    );
    assert_eq!(app.turn_phase, TurnPhase::ReceivingReply);
    assert!(app.last_progress_at.unwrap().elapsed() < Duration::from_secs(1));
}

#[test]
fn background_retries_cannot_replace_the_foreground_phase() {
    let mut app = App::new(Vec::new(), Language::En);
    app.begin_turn(true, "working".into());
    output(
        &mut app,
        serde_json::json!({"type":"provider_retry_wait", "attempt":2, "delay_ms":1500}),
    );
    let retry = TurnPhase::Retrying {
        attempt: 2,
        delay: Some(Duration::from_millis(1500)),
    };
    assert_eq!(app.turn_phase, retry);
    output(
        &mut app,
        serde_json::json!({"type":"subagent_retry_wait", "attempt":7, "delay_ms":10000}),
    );
    assert_eq!(app.turn_phase, retry);
    output(
        &mut app,
        serde_json::json!({"type":"provider_retry_started", "attempt":2}),
    );
    assert_eq!(
        app.turn_phase,
        TurnPhase::Retrying {
            attempt: 2,
            delay: None
        }
    );
}

#[test]
fn explicit_runtime_outcomes_survive_cleanup_and_late_terminal_events() {
    for (kind, outcome) in [
        ("task.completed", TurnOutcome::Completed),
        ("task.partial", TurnOutcome::Partial),
        ("task.failed", TurnOutcome::Failed),
        ("turn.interrupted", TurnOutcome::Interrupted),
        ("turn.cancelled", TurnOutcome::Cancelled),
    ] {
        let mut app = App::new(Vec::new(), Language::En);
        app.begin_turn(true, "working".into());
        let task = uuid::Uuid::new_v4();
        apply_runtime_event(&mut app, &event(kind, 1, task));
        assert!(!app.running);
        assert_eq!(app.last_turn_outcome, Some(outcome));
        apply_runtime_event(&mut app, &event("turn.completed", 2, task));
        assert_eq!(app.last_turn_outcome, Some(outcome));
    }
}

#[test]
fn an_older_tasks_terminal_event_cannot_finish_the_submitted_turn() {
    let mut app = App::new(Vec::new(), Language::En);
    let task = uuid::Uuid::new_v4();
    begin_submitted_turn(
        &mut app,
        Instant::now(),
        10,
        uuid::Uuid::new_v4(),
        Some(task),
    );
    apply_runtime_event(&mut app, &event("task.failed", 11, uuid::Uuid::new_v4()));
    assert!(app.running);
    assert_eq!(app.last_turn_outcome, None);
    apply_runtime_event(&mut app, &event("task.partial", 12, task));
    assert_eq!(app.last_turn_outcome, Some(TurnOutcome::Partial));
}

#[test]
fn cleanup_without_an_explicit_runtime_result_does_not_invent_success() {
    let mut app = App::new(Vec::new(), Language::En);
    app.begin_turn(true, "working".into());
    app.finish_turn();
    assert_eq!(app.last_turn_outcome, None);
    assert!(!app.status_line().contains("completed"));
}
