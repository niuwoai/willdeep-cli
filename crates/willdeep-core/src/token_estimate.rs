//! 请求体积估算：Provider 侧据此把 `max_tokens` 收进窗口，Agent 侧据此决定
//! 压缩水位。
//!
//! 两边必须共用同一把尺子。各算各的时，Agent 按一种估算「预留够了」把请求放
//! 出去，Provider 按另一种估算报一个更大的 `max_tokens`，服务端只认自己的
//! tokenizer——结果就是 400，而 400 不重试，整轮直接失败。
//!
//! 这是估算不是计数：ASCII 按 4 字符一 token、非 ASCII 按 2 token 一字。
//! 英文代码大致准确，中文偏保守（Qwen 系实际更低），宁可高估。

use crate::types::{Message, MessageAttachment, ToolDefinition};

pub fn text_tokens(text: &str) -> u64 {
    let ascii = text.bytes().filter(u8::is_ascii).count() as u64;
    let non_ascii = text
        .chars()
        .filter(|character| !character.is_ascii())
        .count() as u64;
    ascii
        .div_ceil(4)
        .saturating_add(non_ascii.saturating_mul(2))
}

pub fn estimate_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|message| {
            text_tokens(&message.content)
                + message.reasoning.as_deref().map(text_tokens).unwrap_or(0)
                + 8
                + message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        text_tokens(&call.arguments)
                            + text_tokens(&call.name)
                            + text_tokens(&call.id)
                            + 8
                    })
                    .sum::<u64>()
                + message
                    .attachments
                    .iter()
                    .map(|attachment| match attachment {
                        MessageAttachment::Text { name, content } => {
                            text_tokens(name) + text_tokens(content) + 16
                        }
                        MessageAttachment::Image { .. } => 1_024,
                    })
                    .sum::<u64>()
        })
        .sum()
}

pub fn tool_schema_tokens(tools: &[ToolDefinition]) -> u64 {
    tools
        .iter()
        .map(|tool| {
            text_tokens(&tool.name)
                + text_tokens(&tool.description)
                + text_tokens(&tool.parameters.to_string())
                + 8
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_four_chars_and_cjk_is_two_tokens_per_char() {
        assert_eq!(text_tokens("abcdefgh"), 2);
        assert_eq!(text_tokens("中文"), 4);
        assert_eq!(text_tokens(""), 0);
    }

    #[test]
    fn tool_schema_counts_every_definition() {
        let tool = ToolDefinition {
            name: "read_file".to_owned(),
            description: "Read a file".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let single = tool_schema_tokens(std::slice::from_ref(&tool));
        assert!(single > 0);
        assert_eq!(
            tool_schema_tokens(&[tool.clone(), tool]),
            single.saturating_mul(2)
        );
    }
}
