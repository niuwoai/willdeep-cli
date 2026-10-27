use super::*;

/// Inbox 里打开的如果是 Runtime 任务，就顺带把失败详情取回来。
/// 诊断加载失败也显示具体状态，避免详情只剩一串 UUID。
pub(super) fn load_attention_diagnostics(app: &mut App, runtime: &TuiRuntime) {
    let Some(item) = app.attention_detail.as_ref() else {
        app.attention_diagnostics = None;
        app.attention_diagnostics_loading = None;
        return;
    };
    if app
        .attention_diagnostics
        .as_ref()
        .is_some_and(|loaded| loaded.item_id == item.id)
    {
        return;
    }
    if app.attention_diagnostics_loading.as_deref() == Some(item.id.as_str()) {
        return;
    }
    let Some(id) = item
        .id
        .strip_prefix("runtime-task:")
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
    else {
        return;
    };
    let item_id = item.id.clone();
    let home = runtime.home.clone();
    let ui = runtime.tx.clone();
    let language = app.language;
    app.attention_diagnostics_loading = Some(item_id.clone());
    tokio::spawn(async move {
        let text = match crate::daemon::remote_task_diagnostics(&home, id).await {
            Ok(diagnostics) => {
                format_task_diagnostics(&diagnostics, language).unwrap_or_else(|| {
                    language
                        .text(
                            "未记录失败原因；Enter 重新加载详情",
                            "No failure reason recorded; Enter reloads details",
                            "失敗理由が記録されていません。Enter で再読み込み",
                        )
                        .to_owned()
                })
            }
            Err(error) => format!(
                "{}: {}",
                language.text(
                    "诊断加载失败，Enter 重试",
                    "Diagnostics unavailable; Enter retries",
                    "診断を取得できません。Enter で再試行"
                ),
                safe_error(&error.to_string())
            ),
        };
        let _ = ui.send(UiMessage::AttentionDiagnosticsLoaded { item_id, text });
    });
}

/// 把诊断对象排成人能读的几行。没有任何失败痕迹时返回 `None`，
/// 免得在成功的任务详情下面挂一个空的「失败详情」标题。
pub(super) fn format_task_diagnostics(
    diagnostics: &willdeep_runtime_protocol::RuntimeTaskDiagnostics,
    language: Language,
) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(exit_code) = diagnostics.task.exit_code {
        lines.push(format!(
            "{}: {exit_code}",
            language.text("退出码", "Exit code", "終了コード")
        ));
    }
    if let Some(domain) = diagnostics.task.failure_domain {
        lines.push(format!(
            "{}: {domain:?}",
            language.text("失败域", "Failure domain", "失敗ドメイン")
        ));
    }
    if let Some(failure) = &diagnostics.failure {
        // 事件原文形如 `task_id=… exit_code=1 error=…`，task_id 详情里已经有了。
        let failure = failure_text(failure);
        if !failure.trim().is_empty() {
            lines.push(format!(
                "{}: {failure}",
                language.text("失败原因", "Failure", "失敗理由")
            ));
        }
    }
    if diagnostics.failure.is_none()
        && diagnostics.failed_tools.is_empty()
        && matches!(
            diagnostics.task.status,
            willdeep_runtime_protocol::TaskStatus::Failed
                | willdeep_runtime_protocol::TaskStatus::Interrupted
        )
    {
        lines.push(
            language
                .text(
                    "未记录具体失败原因（只有失败分类）；可按 Enter 重新加载",
                    "No specific failure reason recorded (classification only); Enter reloads",
                    "具体的な失敗理由が記録されていません（分類のみ）。Enter で再読み込み",
                )
                .to_owned(),
        );
    }
    for tool in &diagnostics.failed_tools {
        lines.push(String::new());
        lines.push(format!(
            "{} {}",
            language.text("失败的工具", "Failed tool", "失敗したツール"),
            safe_error(&tool.name)
        ));
        if let Some(arguments) = &tool.arguments {
            for (key, value) in tool_arguments(arguments) {
                lines.push(format!("  {key}: {value}"));
            }
        }
        if let Some(output) = tool
            .output
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            lines.push(format!("  {}:", language.text("输出", "Output", "出力")));
            lines.extend(safe_error(output).lines().map(|line| format!("    {line}")));
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn failure_text(failure: &str) -> String {
    safe_error(
        failure
            .split_once(" error=")
            .map_or(failure, |(_, error)| error),
    )
}

fn safe_error(error: &str) -> String {
    willdeep_core::judge::redact_credentials(error)
        .chars()
        .filter(|character| !character.is_control())
        .take(16_000)
        .collect()
}

const MAX_AUTOMATIC_FAILURE_DIAGNOSTICS: usize = 8;

pub(super) fn surface_failures(
    ui: tokio::sync::mpsc::UnboundedSender<UiMessage>,
    home: std::path::PathBuf,
    language: Language,
    ids: Vec<uuid::Uuid>,
) {
    tokio::spawn(async move {
        let total = ids.len();
        for id in ids.into_iter().take(MAX_AUTOMATIC_FAILURE_DIAGNOSTICS) {
            let text = match crate::daemon::remote_task_diagnostics(&home, id).await {
                Ok(diagnostics) => diagnostics
                    .failure
                    .as_deref()
                    .map(failure_text)
                    .filter(|failure| !failure.trim().is_empty()),
                Err(_) => Some(
                    language
                        .text(
                            "具体失败原因加载失败；打开 Inbox 后按 Enter 重试",
                            "Failure details could not be loaded; open Inbox and press Enter to retry",
                            "失敗詳細を取得できません。Inbox を開き Enter で再試行してください",
                        )
                        .to_owned(),
                ),
            };
            if let Some(text) = text {
                let summary: String = text.chars().take(400).collect();
                let message = format!(
                    "{}: {summary}{}",
                    language.text("失败原因", "Failure", "失敗理由"),
                    if text.chars().count() > 400 {
                        "…"
                    } else {
                        ""
                    }
                );
                let _ = ui.send(UiMessage::RuntimeResult(message));
            }
        }
        if total > MAX_AUTOMATIC_FAILURE_DIAGNOSTICS {
            let message = language.text(
                "还有其他任务失败；请在 Inbox 中逐条查看诊断。",
                "Additional tasks failed; review their details individually in Inbox.",
                "ほかにも失敗したタスクがあります。Inbox で個別に診断を確認してください。",
            );
            let _ = ui.send(UiMessage::RuntimeResult(format!("System: {message}")));
        }
    });
}

/// 工具入参是 JSON 字符串。能解析成对象就按 `键: 值` 逐行摊开，
/// `run_command` 的 `command` 就落在这里；解析不了就原样给一行。
fn tool_arguments(arguments: &str) -> Vec<(String, String)> {
    let Ok(serde_json::Value::Object(object)) = serde_json::from_str(arguments) else {
        return vec![(
            "arguments".to_owned(),
            safe_error(arguments).replace('\n', " "),
        )];
    };
    object
        .into_iter()
        .map(|(key, value)| {
            let value = match value {
                serde_json::Value::String(value) => value,
                other => other.to_string(),
            };
            (safe_error(&key), safe_error(&value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_text_preserves_plain_errors_and_strips_event_metadata_and_credentials() {
        let error = "cannot persist agent execution checkpoint: this Agent team is already owned by another execution host";
        assert_eq!(failure_text(error), error);
        assert_eq!(
            failure_text(&format!("task_id=example exit_code=none error={error}")),
            error
        );
        let text = failure_text("task_id=example error=HTTP 401 API_KEY=test-secret-value");
        assert!(text.contains("HTTP 401"));
        assert!(!text.contains("test-secret-value"));
        assert!(!failure_text("bad\u{1b}error").contains('\u{1b}'));
    }

    #[test]
    fn failed_task_with_only_a_classification_reports_missing_reason() {
        let task = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4(), "event_start_sequence": 0,
            "status": "failed", "created_at": 0, "failure_domain": "harness"
        }))
        .unwrap();
        let diagnostics = willdeep_runtime_protocol::RuntimeTaskDiagnostics {
            task,
            failure: None,
            failed_tools: Vec::new(),
        };
        let text = format_task_diagnostics(&diagnostics, Language::En).unwrap();
        assert!(text.contains("No specific failure reason recorded"));
    }

    #[test]
    fn inbox_details_can_scroll_to_the_actual_error_below_metadata() {
        let mut app = App::new(Vec::new(), Language::En);
        let item_id = "runtime-task:test".to_owned();
        app.attention_detail = Some(AttentionItem {
            id: item_id.clone(),
            source: AttentionSource::BackgroundShell,
            status: RuntimeStatus::Failed,
            title: "test task".into(),
            detail: "Failure domain: Harness".into(),
            elapsed_millis: None,
        });
        app.attention_diagnostics = Some(AttentionDiagnostics {
            item_id,
            text: format!("{}root cause example", "metadata line\n".repeat(20)),
        });
        app.attention_detail_scroll = u16::MAX;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(80, 10)).unwrap();
        terminal
            .draw(|frame| sidebar::render_attention_detail(frame, &mut app))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("root cause example"), "{text}");
        assert!(app.attention_detail_scroll < u16::MAX);
    }
}
