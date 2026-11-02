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

/// 压缩会把带推理的工具批次换成没有推理的摘要或归档引用，直给的步骤
/// （`git commit` 这类）本身也不吐思维链。thinking 模式带 tools 时缺一条
/// `reasoning_text` 就 400，所以历史里出现过思维链时，缺的 assistant 消息补空串。
#[test]
fn thinking_history_backfills_empty_reasoning_before_every_assistant_message() {
    let first = ToolCall {
        id: "call-1".into(),
        name: "run".into(),
        arguments: "{\"cmd\":\"git status\"}".into(),
    };
    let second = ToolCall {
        id: "call-2".into(),
        name: "run".into(),
        arguments: "{\"cmd\":\"git commit\"}".into(),
    };
    let history = vec![
        Message::system("sys"),
        Message::user("提交一下"),
        Message::assistant("", vec![first.clone()]).with_reasoning(Some("先看状态".into())),
        Message::tool(&first, "clean"),
        Message::assistant("", vec![second.clone()]),
        Message::tool(&second, "done"),
        Message::assistant("<context-summary>…</context-summary>", Vec::new()),
    ];
    let (_, input) = encode_input(&history);
    let wire = serde_json::to_value(input).unwrap();
    assert_eq!(wire[1]["type"], "reasoning", "{wire}");
    assert_eq!(wire[1]["content"][0]["text"], "先看状态", "{wire}");
    // 补空的 reasoning 项插在正文与工具调用之前，所以直给步骤的 function_call
    // 比「只回传有原文的」那种排法多往后一位。
    assert_eq!(
        wire[4]["content"][0],
        json!({"type":"reasoning_text","text":""})
    );
    assert_eq!(
        wire[7]["content"][0],
        json!({"type":"reasoning_text","text":""})
    );
    assert_eq!(wire[2]["type"], "function_call");
    assert_eq!(wire[5]["type"], "function_call");
}

/// rc5 的补空只认非空原文。压缩把最后一段原文替换掉之后，摘要上只剩一个空标记，
/// 后面的工具轮又没吐思维链。标记也得算「历史里出现过思维链」，否则下一轮整条 400。
#[test]
fn compression_replay_marker_backfills_reasoning_after_real_text_is_gone() {
    let call = ToolCall {
        id: "call-1".into(),
        name: "run".into(),
        arguments: "{}".into(),
    };
    let history = vec![
        Message::user("继续"),
        Message::assistant(
            "<context-summary source=\"derived-untrusted-history\">\nOBJECTIVE: fix\n</context-summary>",
            Vec::new(),
        )
        .with_reasoning_replay_marker(),
        Message::assistant("", vec![call.clone()]),
        Message::tool(&call, "ok"),
    ];
    let (_, input) = encode_input(&history);
    let wire = serde_json::to_value(input).unwrap();
    let reasoning: Vec<_> = wire
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "reasoning")
        .collect();
    assert_eq!(reasoning.len(), 2, "{wire}");
    for item in reasoning {
        assert_eq!(
            item["content"][0],
            json!({"type":"reasoning_text","text":""})
        );
    }
    assert!(
        wire.as_array().unwrap().iter().any(|item| {
            item["type"] == "message"
                && item["content"][0]["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("OBJECTIVE"))
        }),
        "the summary stays a message, not reasoning text: {wire}"
    );
    let kinds: Vec<_> = wire
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect();
    let call_at = kinds
        .iter()
        .position(|kind| *kind == "function_call")
        .unwrap();
    assert_eq!(kinds[call_at - 1], "reasoning");
}

/// 没见过思维链的端点不能凭空多出 reasoning 项：严格校验的上游会拒未知项。
#[test]
fn history_without_reasoning_sends_no_reasoning_item() {
    let call = ToolCall {
        id: "call-1".into(),
        name: "run".into(),
        arguments: "{}".into(),
    };
    let history = vec![
        Message::user("hi"),
        Message::assistant("", vec![call.clone()]),
        Message::tool(&call, "ok"),
        Message::assistant("done", Vec::new()),
    ];
    let (_, input) = encode_input(&history);
    let wire = serde_json::to_value(input).unwrap();
    assert!(
        wire.as_array()
            .unwrap()
            .iter()
            .all(|item| item["type"] != "reasoning")
    );
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
