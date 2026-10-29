use super::tests::{message, push, terminal};
use super::*;
use crate::provider::stream_test_support::{Events, frame, server};
use serde_json::json;

#[tokio::test]
async fn http_tool_followup_sends_deepseek_reasoning_in_input() {
    let payload = frame(delta("先检查文件"))
        + &frame(terminal(
            "completed",
            vec![
                reasoning("先检查文件"),
                json!({"type":"function_call","id":"fc1","call_id":"call1","name":"read_file","arguments":"{}"}),
            ],
        ));
    let (url, requests, task) = server(payload).await;
    let provider = ResponsesProvider::new(ProviderConfig::new(
        crate::provider::ProviderKind::SomeIm,
        crate::provider::ApiDialect::Responses,
        url,
        "test-key",
        "deepseek-v4-flash",
    ))
    .unwrap();
    let events = Events::default();
    let mut messages = vec![Message::user("检查文件")];
    let completion = provider
        .complete_with_events(&messages, &[], &events)
        .await
        .unwrap();
    let tool = Message::tool(&completion.tool_calls[0], "contents");
    messages.push(
        Message::assistant(completion.content, completion.tool_calls)
            .with_reasoning(completion.reasoning),
    );
    messages.push(tool);
    provider
        .complete_with_events(&messages, &[], &events)
        .await
        .unwrap();
    task.abort();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["input"][1]["type"], "reasoning");
    assert_eq!(
        requests[1]["input"][1]["content"][0],
        json!({"type":"reasoning_text","text":"先检查文件"})
    );
    assert_eq!(
        requests[1]["input"][2]["call_id"],
        requests[1]["input"][3]["call_id"]
    );
}

fn reasoning(text: &str) -> Value {
    json!({"type":"reasoning","content":[{"type":"reasoning_text","text":text}],"summary":[]})
}

fn delta(text: &str) -> Value {
    json!({"type":"response.reasoning_text.delta","output_index":0,"content_index":0,"delta":text})
}

#[test]
fn streamed_reasoning_survives_tool_continuation_without_duplication() {
    let mut state = State::default();
    let (_, updates) = push(&mut state, delta("先检查")).unwrap();
    assert!(matches!(&updates[..], [ProviderEvent::ReasoningDelta(text)] if text == "先检查"));
    push(&mut state, delta("文件。\n")).unwrap();
    let (_, updates) = push(&mut state, terminal("completed", vec![reasoning("先检查文件。\n"),
        json!({"type":"function_call","id":"fc1","call_id":"call1","name":"read_file","arguments":"{}"})])).unwrap();
    assert!(
        !updates
            .iter()
            .any(|event| matches!(event, ProviderEvent::ReasoningDelta(_)))
    );
    let completion = state.completed.unwrap();
    assert_eq!(completion.reasoning.as_deref(), Some("先检查文件。\n"));
    let tool = Message::tool(&completion.tool_calls[0], "contents");
    let assistant = Message::assistant(completion.content, completion.tool_calls)
        .with_reasoning(completion.reasoning);
    let (_, input) = encode_input(&[assistant, tool]);
    let wire = serde_json::to_value(input).unwrap();
    assert_eq!(wire[0]["content"][0]["text"], "先检查文件。\n");
    assert_eq!(wire[1]["type"], "function_call");
    assert_eq!(wire[2]["type"], "function_call_output");
}

#[test]
fn terminal_snapshot_supplies_reasoning_when_deltas_are_absent() {
    let mut state = State::default();
    let (_, updates) = push(
        &mut state,
        terminal("completed", vec![reasoning("完整推理"), message("正文")]),
    )
    .unwrap();
    assert_eq!(
        state.completed.unwrap().reasoning.as_deref(),
        Some("完整推理")
    );
    assert!(
        updates.iter().any(
            |event| matches!(event, ProviderEvent::ReasoningDelta(text) if text == "完整推理")
        )
    );
}

#[test]
fn interrupted_and_incomplete_streams_keep_observed_reasoning() {
    let mut state = State::default();
    push(&mut state, delta("未完推理")).unwrap();
    assert_eq!(state.partial().reasoning.as_deref(), Some("未完推理"));
    push(&mut state, terminal("incomplete", vec![])).unwrap();
    assert_eq!(
        state.completed.unwrap().reasoning.as_deref(),
        Some("未完推理")
    );
}

#[test]
fn contradictory_reasoning_cannot_become_successful_history() {
    let mut state = State::default();
    push(&mut state, delta("原始推理")).unwrap();
    assert!(
        push(
            &mut state,
            terminal("completed", vec![reasoning("其他推理"), message("正文")])
        )
        .is_err()
    );
    assert!(state.completed.is_none());
    assert_eq!(state.partial().reasoning.as_deref(), Some("原始推理"));
}
