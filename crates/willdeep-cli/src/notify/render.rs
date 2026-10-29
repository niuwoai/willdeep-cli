//! 出站通知的人类可读正文。
//!
//! webhook 走结构化报文（`willdeep.webhook.v1`），不经过这里；飞书和邮件要的是
//! 一段能直接读的文字。文案跟 `[agent] language` 走——这是用户会看到的字，不是
//! 日志，不能写死中文。

use super::NotificationEvent;
use super::WebhookPayload;
use crate::i18n::Language;

/// 一次事件渲染出来的正文与标题。
pub(super) struct Rendered {
    pub(super) body: String,
    pub(super) subject: String,
}

/// 邮件主题按 RFC 5322 的软上限截断。主题不是正文：截断处补省略号，不让
/// 一个超长会话标题把整封信的标题挤没。
const MAX_SUBJECT_CHARS: usize = 120;

pub(super) fn render(
    event: NotificationEvent,
    payload: &WebhookPayload,
    language: Language,
) -> Rendered {
    let title = event_title(event, language).to_owned();
    let session_title = payload
        .session_title
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let mut lines = Vec::new();
    if let Some(session) = session_title {
        lines.push(format!("{}: {session}", label_session(language)));
    }
    if let Some(id) = payload.session_id.as_deref() {
        lines.push(format!("{}: {id}", label_session_id(language)));
    }
    lines.push(format!(
        "{}: {}",
        label_status(language),
        status_text(&payload.status, language)
    ));
    if let Some(task) = payload.task_id.as_deref().filter(|v| !v.is_empty()) {
        lines.push(format!("{}: {task}", label_task(language)));
    }
    if let Some(kind) = payload.attention_kind.as_deref().filter(|v| !v.is_empty()) {
        lines.push(format!("{}: {kind}", label_trigger(language)));
    }
    if let Some(summary) = payload
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push(String::new());
        lines.push(format!("{}:", label_summary(language)));
        lines.push(summary.to_owned());
    }
    lines.push(String::new());
    lines.push(format!(
        "WillDeep {} · {}",
        payload.app_version, payload.occurred_at
    ));

    let body = format!("{title}\n\n{}", lines.join("\n"));
    let subject = subject(&title, session_title);
    Rendered { body, subject }
}

fn subject(title: &str, session_title: Option<&str>) -> String {
    let raw = match session_title {
        Some(session) => format!("[WillDeep] {title} · {session}"),
        None => format!("[WillDeep] {title}"),
    };
    // 会话标题是模型写的，可能带换行。邮件头里一个换行就是一个新 header，
    // 必须在拼进 Subject 之前压平。
    let flattened = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = flattened.chars().count();
    if count <= MAX_SUBJECT_CHARS {
        return flattened;
    }
    let head: String = flattened.chars().take(MAX_SUBJECT_CHARS).collect();
    format!("{head}…")
}

fn event_title(event: NotificationEvent, language: Language) -> &'static str {
    match event {
        NotificationEvent::TaskCompleted => {
            language.text("任务完成", "Task completed", "タスク完了")
        }
        NotificationEvent::AttentionRequired => {
            language.text("需要你处理", "Needs your attention", "対応が必要です")
        }
    }
}

fn label_session(language: Language) -> &'static str {
    language.text("会话", "Session", "セッション")
}

fn label_session_id(language: Language) -> &'static str {
    language.text("会话 ID", "Session ID", "セッション ID")
}

fn label_status(language: Language) -> &'static str {
    language.text("状态", "Status", "状態")
}

fn label_task(language: Language) -> &'static str {
    language.text("任务", "Task", "タスク")
}

fn label_trigger(language: Language) -> &'static str {
    language.text("触发", "Trigger", "トリガー")
}

fn label_summary(language: Language) -> &'static str {
    language.text("摘要", "Summary", "概要")
}

/// 线上值是 `succeeded` / `pendingApproval` 这类给机器看的词。直接塞进邮件
/// 也能读，但收信人第一眼看到的是 `awaitingUserAnswer` 就很出戏。
fn status_text(status: &str, language: Language) -> String {
    let text = match status {
        "succeeded" => language.text("成功", "succeeded", "成功"),
        "partial" => language.text("部分完成", "partial", "一部完了"),
        "failed" => language.text("失败", "failed", "失敗"),
        "blocked" => language.text("已阻塞", "blocked", "ブロック"),
        "pendingApproval" => language.text("等待批准", "awaiting approval", "承認待ち"),
        "awaitingUserAnswer" => language.text("等待回答", "awaiting answer", "回答待ち"),
        "queued" => language.text("排队中", "queued", "待機中"),
        "running" => language.text("运行中", "running", "実行中"),
        "interrupted" => language.text("已中断", "interrupted", "中断"),
        "denied" => language.text("已拒绝", "denied", "拒否"),
        "idle" => language.text("空闲", "idle", "アイドル"),
        _ => language.text("未知", "unknown", "不明"),
    };
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(summary: &str) -> WebhookPayload {
        WebhookPayload {
            schema_version: "willdeep.webhook.v1",
            source: "willdeep",
            event: "task_completed",
            r#type: "agent-turn-complete",
            hook_event_name: "Stop",
            occurred_at: "2026-10-29T12:00:00Z".to_owned(),
            application: "WillDeep",
            app_version: env!("CARGO_PKG_VERSION"),
            runtime_id: Some("runtime-1".to_owned()),
            session_id: Some("runtime-1".to_owned()),
            thread_id: Some("runtime-1".to_owned()),
            session_title: Some("整理发布说明".to_owned()),
            executor: "willdeep-cli",
            executor_kind: "willdeep",
            status: "succeeded".to_owned(),
            task_id: None,
            attention_kind: None,
            notification_type: None,
            message: None,
            summary: Some(summary.to_owned()),
        }
    }

    #[test]
    fn chinese_body_carries_session_status_and_summary() {
        let rendered = render(
            NotificationEvent::TaskCompleted,
            &payload("全部完成"),
            Language::ZhCn,
        );
        assert!(rendered.body.starts_with("任务完成\n\n"));
        assert!(rendered.body.contains("会话: 整理发布说明"));
        assert!(rendered.body.contains("状态: 成功"));
        assert!(rendered.body.contains("摘要:\n全部完成"));
        assert_eq!(rendered.subject, "[WillDeep] 任务完成 · 整理发布说明");
    }

    #[test]
    fn english_and_japanese_switch_every_label() {
        let english = render(
            NotificationEvent::AttentionRequired,
            &payload("approve me"),
            Language::En,
        );
        assert!(english.body.starts_with("Needs your attention\n\n"));
        assert!(english.body.contains("Status: succeeded"));
        assert!(english.body.contains("Summary:\napprove me"));

        let japanese = render(
            NotificationEvent::TaskCompleted,
            &payload("完了"),
            Language::Ja,
        );
        assert!(japanese.body.starts_with("タスク完了\n\n"));
        assert!(japanese.body.contains("状態: 成功"));
    }

    /// 会话标题来自模型，可能带换行。邮件主题里一个换行就是一个新 header。
    #[test]
    fn subject_flattens_newlines_from_the_session_title() {
        let mut payload = payload("done");
        payload.session_title = Some("line one\nBcc: evil@example.com".to_owned());
        let rendered = render(NotificationEvent::TaskCompleted, &payload, Language::En);
        assert!(!rendered.subject.contains('\n'));
        assert_eq!(
            rendered.subject,
            "[WillDeep] Task completed · line one Bcc: evil@example.com"
        );
    }

    #[test]
    fn a_missing_summary_leaves_no_empty_summary_block() {
        let mut payload = payload("");
        payload.summary = None;
        let rendered = render(NotificationEvent::TaskCompleted, &payload, Language::En);
        assert!(!rendered.body.contains("Summary:"));
    }

    #[test]
    fn statuses_are_translated_not_pasted_from_the_wire() {
        for status in [
            "succeeded",
            "pendingApproval",
            "awaitingUserAnswer",
            "failed",
        ] {
            let mut payload = payload("x");
            payload.status = status.to_owned();
            let rendered = render(NotificationEvent::TaskCompleted, &payload, Language::ZhCn);
            assert!(
                !rendered.body.contains(status),
                "{status} leaked into the body"
            );
        }
    }
}
