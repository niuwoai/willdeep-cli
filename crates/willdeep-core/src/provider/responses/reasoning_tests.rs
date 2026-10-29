use super::*;
use serde_json::json;

#[test]
fn tool_continuation_replays_raw_reasoning_before_assistant_output() {
    let response = json!({"status":"completed","output":[
        {"type":"reasoning","summary":[],"content":[
            {"type":"reasoning_text","text":"先检查"},
            {"type":"reasoning_text","text":"文件。\n"}
        ]},
        {"type":"message","content":[{"type":"output_text","text":"正在检查"}]},
        {"type":"function_call","id":"fc1","call_id":"call1","name":"read_file","arguments":"{}"}
    ]});
    let completion = decode_completion(&serde_json::to_vec(&response).unwrap()).unwrap();
    assert_eq!(completion.reasoning.as_deref(), Some("先检查文件。\n"));
    let tool = Message::tool(&completion.tool_calls[0], "file contents");
    let assistant = Message::assistant(completion.content, completion.tool_calls)
        .with_reasoning(completion.reasoning);
    let (_, input) = encode_input(&[Message::user("检查文件"), assistant, tool]);
    let wire = serde_json::to_value(input).unwrap();
    assert_eq!(wire[1]["type"], "reasoning");
    assert_eq!(
        wire[1]["content"][0],
        json!({"type":"reasoning_text","text":"先检查文件。\n"})
    );
    assert_eq!(wire[2]["type"], "message");
    assert_eq!(wire[3]["type"], "function_call");
    assert_eq!(wire[4]["type"], "function_call_output");
    assert_eq!(wire[3]["call_id"], wire[4]["call_id"]);
}

#[test]
fn summary_is_not_replayed_as_raw_reasoning() {
    let response = json!({"status":"completed","output":[
        {"type":"reasoning","summary":[{"type":"summary_text","text":"摘要"}]},
        {"type":"message","content":[{"type":"output_text","text":"正文"}]}
    ]});
    let completion = decode_completion(&serde_json::to_vec(&response).unwrap()).unwrap();
    assert!(completion.reasoning.is_none());
    assert_eq!(completion.content, "正文");
}
