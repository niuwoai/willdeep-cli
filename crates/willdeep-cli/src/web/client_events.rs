//! 把 Runtime 输出投影成浏览器事件。
//!
//! 只带界面要显示的字段。模型名仅在事件自己带了的时候传下去，没有记录就留空。
use super::*;

pub(super) fn client_event(
    value: serde_json::Value,
    language: Language,
) -> Option<serde_json::Value> {
    let kind = value.get("type")?.as_str()?;
    if matches!(kind, "provider_retry_started" | "subagent_retry_started") {
        return Some(
            serde_json::json!({"type":kind,"label":language.text("正在重试", "Retrying", "再試行中")}),
        );
    }
    if kind == "assistant_text_delta" {
        let mut event = serde_json::json!({"type":kind,"text":value.get("text")?.as_str()?});
        // 模型名只在事件确实带了时才往下传；旧 Runtime 没有这个字段，留空。
        if let Some(model) = value
            .get("model")
            .and_then(|model| model.as_str())
            .map(str::trim)
            .filter(|model| !model.is_empty())
            && let Some(object) = event.as_object_mut()
        {
            object.insert(
                "model".to_owned(),
                serde_json::Value::String(model.to_owned()),
            );
        }
        return Some(event);
    }
    // 思维链增量与正文增量一样只带文本；前端做灰色「思考中」尾巴行，正文一来就隐藏。
    if kind == "reasoning_delta" {
        return Some(serde_json::json!({"type":kind,"text":value.get("text")?.as_str()?}));
    }
    if kind == "subagent_retry_wait" {
        let id = value.get("id")?.as_str()?.parse::<uuid::Uuid>().ok()?;
        let attempt = value.get("attempt")?.as_u64()?;
        let delay = value.get("delay_ms")?.as_u64()?;
        return Some(serde_json::json!({
            "type":kind, "id":id, "attempt":attempt, "delay_ms":delay,
            "label":format!("{} {} · {} {attempt} · {}s",
                language.text("子 Agent", "Subagent", "サブエージェント"),
                &id.to_string()[..8],
                language.text("等待重试", "Waiting to retry", "再試行を待機中"),
                delay.div_ceil(1000)),
        }));
    }
    if kind == "provider_retry_wait" {
        let delay = value
            .get("delay_ms")
            .and_then(|value| value.as_u64())
            .unwrap_or_default();
        return Some(serde_json::json!({
            "type":kind,
            "label":format!("{} · {}s", language.text("等待重试", "Waiting to retry", "再試行を待機中"), delay.div_ceil(1000)),
        }));
    }
    if kind == "usage_snapshot" {
        return None;
    }
    let label = match kind {
        "turn_started" => format!(
            "{} {}",
            language.text("正在思考 · 第", "Thinking · turn", "思考中 · ターン"),
            value.get("turn").and_then(|v| v.as_u64()).unwrap_or(1)
        ),
        "tool_requested" => format!(
            "{} {}",
            language.text("正在使用", "Using", "使用中"),
            value.get("name").and_then(|v| v.as_str()).unwrap_or("tool")
        ),
        "tool_completed" => format!(
            "{} {}",
            if value
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                language.text("执行失败", "Failed", "実行失敗")
            } else {
                language.text("已完成", "Finished", "完了")
            },
            value.get("name").and_then(|v| v.as_str()).unwrap_or("tool")
        ),
        "compression_started" => language
            .text(
                "正在压缩上下文",
                "Compressing context",
                "コンテキストを圧縮中",
            )
            .to_owned(),
        "compression_completed" => language
            .text(
                "上下文压缩完成",
                "Context compressed",
                "コンテキストを圧縮しました",
            )
            .to_owned(),
        "usage" => language
            .text("正在整理结果", "Preparing result", "結果を整理中")
            .to_owned(),
        "completed" => return Some(value),
        "workflow_updated" => {
            return Some(serde_json::json!({"type":kind, "workflow":value.get("workflow")}));
        }
        "assistant_text" => {
            let thought = value
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let mut event =
                serde_json::json!({"type":"thought","text":super::truncate(thought,240)});
            if let Some(model) = value
                .get("model")
                .and_then(|model| model.as_str())
                .map(str::trim)
                .filter(|model| !model.is_empty())
                && let Some(object) = event.as_object_mut()
            {
                object.insert(
                    "model".to_owned(),
                    serde_json::Value::String(model.to_owned()),
                );
            }
            return Some(event);
        }
        _ => kind.to_owned(),
    };
    Some(serde_json::json!({
        "type":kind,
        "label":label,
        // 这一步具体在干什么。摘要在 `tool_detail` 那边就收敛并打码过了，这里
        // 只负责原样带出去——界面把它显示成一行淡色小字。
        "detail":value.get("detail").and_then(|item| item.as_str()),
        "id":value.get("id").and_then(|item| item.as_str()),
        "name":value.get("name").and_then(|item| item.as_str()),
        "is_error":value.get("is_error").and_then(|item| item.as_bool()),
        "plan_update":value.get("plan_update"),
    }))
}
