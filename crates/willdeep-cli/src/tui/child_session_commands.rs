//! Complete conversations use the coordinator, independently of professional Workers.
use super::*;
use willdeep_runtime_protocol::child_session::{
    ChildSessionTask, ChildSessionTaskPage, ChildSessionTaskStatus,
};

fn word(text: &str) -> (&str, &str) {
    let text = text.trim();
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    (&text[..end], text[end..].trim())
}

fn safe(text: &str) -> String {
    text.chars()
        .filter(|value| !value.is_control() || matches!(value, '\n' | '\t'))
        .collect()
}

pub(super) fn status(status: ChildSessionTaskStatus, language: Language) -> &'static str {
    use ChildSessionTaskStatus::*;
    match status {
        Preparing => language.text("准备中", "Preparing", "準備中"),
        Queued => language.text("排队中", "Queued", "実行待ち"),
        Running => language.text("运行中", "Running", "実行中"),
        WaitingApproval => language.text("等待审批", "Waiting for approval", "承認待ち"),
        WaitingAnswer => language.text("等待回答", "Waiting for an answer", "回答待ち"),
        Completed => language.text(
            "本次任务已结束",
            "This task has ended",
            "今回のタスクは終了しました",
        ),
        Partial => language.text("部分完成", "Partially completed", "一部完了"),
        Failed => language.text(
            "本次任务失败",
            "This task failed",
            "今回のタスクは失敗しました",
        ),
        Cancelled => language.text(
            "本次任务已取消",
            "This task was cancelled",
            "今回のタスクはキャンセル済み",
        ),
        Interrupted => language.text(
            "本次任务已中断",
            "This task was interrupted",
            "今回のタスクは中断済み",
        ),
        Blocked => language.text("需要处理", "Needs attention", "対応が必要"),
    }
}

fn summary(task: &ChildSessionTask, language: Language, full: bool) -> String {
    let mode = match task.workspace_mode {
        willdeep_runtime_protocol::child_session::ChildSessionWorkspaceMode::SharedReadOnly => {
            language.text(
                "共享工作区 · 只读",
                "Shared workspace · read-only",
                "共有作業領域 · 読み取り専用",
            )
        }
        willdeep_runtime_protocol::child_session::ChildSessionWorkspaceMode::Worktree => language
            .text(
                "独立工作树 · 可写",
                "Dedicated worktree · writable",
                "専用ワークツリー · 書き込み可",
            ),
    };
    let mut lines = vec![
        format!("{} · {}", safe(&task.title), status(task.status, language)),
        format!("  {mode}"),
        format!(
            "{}: {}",
            language.text("任务", "Assignment", "タスク"),
            task.assignment_id
        ),
        format!(
            "{}: {}",
            language.text("子会话", "Child", "子セッション"),
            task.child_session_id
        ),
        format!("  /session children open {}", task.child_session_id),
    ];
    if let Some(model) = &task.model {
        lines.push(format!(
            "{}: {}",
            language.text("模型", "Model", "モデル"),
            safe(model)
        ));
    }
    if full {
        lines.push(safe(&task.prompt));
        if let Some(error) = &task.error {
            lines.push(safe(error));
        }
    }
    if let Some(result) = &task.result {
        lines.push(
            language
                .text(
                    if result.superseded_by.is_some() {
                        "已创建后续重试"
                    } else if result.consumed_at.is_some() {
                        "结果已送达"
                    } else {
                        "结果待处理"
                    },
                    if result.superseded_by.is_some() {
                        "A follow-up retry was created"
                    } else if result.consumed_at.is_some() {
                        "Result delivered"
                    } else {
                        "Result pending"
                    },
                    if result.superseded_by.is_some() {
                        "再試行を作成済み"
                    } else if result.consumed_at.is_some() {
                        "結果を送達済み"
                    } else {
                        "結果の処理待ち"
                    },
                )
                .into(),
        );
        if let Some(worktree) = &result.worktree {
            lines.push(format!(
                "{}: {} @ {}",
                language.text("分支", "Branch", "ブランチ"),
                safe(&worktree.branch),
                safe(&worktree.base_commit)
            ));
        }
        if full {
            let report = result
                .report
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(&result.summary);
            lines.push(if report.trim().is_empty() {
                language
                    .text(
                        "本次任务没有可用报告。",
                        "No report is available for this task.",
                        "このタスクの報告はありません。",
                    )
                    .into()
            } else {
                safe(report)
            });
            if !result.report_complete {
                lines.push(
                    language
                        .text(
                            "报告可能不完整，请打开完整对话。",
                            "The report may be incomplete. Open the full conversation.",
                            "報告が不完全な可能性があります。会話全体を開いてください。",
                        )
                        .into(),
                );
            }
        }
    }
    lines.join("\n")
}

pub(super) async fn handle(
    arguments: &str,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
) -> Result<String> {
    let language = app.language;
    let (action, rest) = word(arguments);
    let usage = language.text(
        "用法：/session children start [--fork <完成轮次ID>] [--worktree [--snapshot]] [--model <模型>] <名称> | <任务>；list [分页游标]；show <任务ID>；open <子会话ID>；parent；continue <子会话ID> <后续任务>；retry <任务ID> <结果版本>；stop <任务ID>。默认在共享工作区只读；--worktree 使用独立可写工作树。",
        "Usage: /session children start [--fork <completed-turn>] [--worktree [--snapshot]] [--model <model>] <title> | <task>; list [cursor]; show <assignment>; open <child>; parent; continue <child> <follow-up>; retry <assignment> <result-revision>; stop <assignment>. The default is a shared read-only workspace; --worktree uses a dedicated writable worktree.",
        "使用法：/session children start [--fork <完了ターンID>] [--worktree [--snapshot]] [--model <モデル>] <名前> | <タスク>; list [カーソル]; show <タスクID>; open <子セッションID>; parent; continue <子セッションID> <追加タスク>; retry <タスクID> <結果バージョン>; stop <タスクID>。既定は共有作業領域の読み取り専用、--worktree は専用の書き込み可能ワークツリーです。",
    );
    if action == "parent" && rest.is_empty() {
        let parent = session.parent_session_id.context(language.text(
            "当前会话没有父会话",
            "This conversation has no parent",
            "親セッションはありません",
        ))?;
        return session_commands::switch_related(app, session, store, runtime, &parent.to_string())
            .await;
    }
    if action == "open" && !rest.is_empty() {
        let child: uuid::Uuid = rest.parse().context("invalid child conversation ID")?;
        let target = store.load(child).context("load child conversation")?;
        anyhow::ensure!(
            target.parent_session_id == Some(session.id),
            "conversation is not a child of this parent"
        );
        anyhow::ensure!(
            app.attachments.is_empty(),
            "save or remove the current attachment draft before switching"
        );
        return session_commands::switch_related(app, session, store, runtime, rest).await;
    }
    if action.is_empty() {
        return Ok(usage.into());
    }
    let parent = session.parent_session_id.unwrap_or(session.id);
    let (operation, params) = match action {
        "list" => {
            let after = if rest.is_empty() {
                None
            } else {
                Some(
                    rest.parse::<uuid::Uuid>()
                        .context("invalid child task cursor")?,
                )
            };
            (
                "session.child.task.list",
                serde_json::json!({"parent_session_id":parent,"after":after,"limit":50}),
            )
        }
        "show" if !rest.is_empty() => (
            "session.child.task.get",
            serde_json::json!({"parent_session_id":parent,"assignment_id":rest.parse::<uuid::Uuid>()?}),
        ),
        "start" => {
            anyhow::ensure!(
                session.parent_session_id.is_none() && !session.delegation_read_only,
                "child conversations cannot delegate further tasks"
            );
            let mut tail = rest;
            let mut context = "fresh";
            let mut through_turn = None;
            let mut workspace = "shared_read_only";
            let mut worktree_base = "head";
            let mut model = None;
            loop {
                let (flag, remaining) = word(tail);
                match flag {
                    "--fork" => {
                        let (value, remaining) = word(remaining);
                        anyhow::ensure!(
                            !value.is_empty(),
                            "missing completed Turn ID after --fork"
                        );
                        through_turn = Some(value.parse::<uuid::Uuid>()?);
                        context = "fork";
                        tail = remaining;
                    }
                    "--worktree" => {
                        workspace = "worktree";
                        tail = remaining;
                    }
                    "--snapshot" => {
                        worktree_base = "snapshot";
                        tail = remaining;
                    }
                    "--model" => {
                        let (value, remaining) = word(remaining);
                        anyhow::ensure!(!value.is_empty(), "missing model name after --model");
                        model = Some(value.to_owned());
                        tail = remaining;
                    }
                    _ => break,
                }
            }
            let Some((title, prompt)) = tail.split_once('|') else {
                return Ok(usage.into());
            };
            if title.trim().is_empty() || prompt.trim().is_empty() {
                return Ok(usage.into());
            }
            (
                "session.child.start",
                serde_json::json!({"parent_session_id":parent,"title":title.trim(),"prompt":prompt.trim(),
                    "context":context,"through_turn_id":through_turn,
                    "workspace_mode":workspace,"worktree_base":worktree_base,"model":model}),
            )
        }
        "continue" => {
            let (child, prompt) = word(rest);
            if prompt.is_empty() {
                return Ok(usage.into());
            }
            (
                "session.child.continue",
                serde_json::json!({"parent_session_id":parent,"child_session_id":child.parse::<uuid::Uuid>()?,"prompt":prompt}),
            )
        }
        "retry" => {
            let (assignment, revision) = word(rest);
            if revision.is_empty() {
                return Ok(usage.into());
            }
            (
                "session.child.retry",
                serde_json::json!({"parent_session_id":parent,"assignment_id":assignment.parse::<uuid::Uuid>()?,"expected_result_revision":revision.parse::<u64>()?}),
            )
        }
        "stop" if !rest.is_empty() => (
            "session.child.task.stop",
            serde_json::json!({"parent_session_id":parent,"assignment_id":rest.parse::<uuid::Uuid>()?}),
        ),
        _ => return Ok(usage.into()),
    };
    crate::daemon::ensure_runtime_session(
        &runtime.home,
        session.id,
        &session.workspace,
        session.profile.clone(),
        session.model.clone(),
    )
    .await?;
    let fingerprint = format!("{}:{operation}:{params}", runtime.home.display());
    let request = *app
        .child_request_ids
        .entry(fingerprint.clone())
        .or_insert_with(uuid::Uuid::new_v4);
    let value =
        crate::daemon::child_operation(&runtime.home, operation, params, Some(request)).await?;
    app.child_request_ids.remove(&fingerprint);
    if action == "list" {
        let page: ChildSessionTaskPage = serde_json::from_value(value)?;
        let mut lines = vec![
            language
                .text("子会话", "Child conversations", "子セッション")
                .to_owned(),
        ];
        if page.tasks.is_empty() {
            lines.push(
                language
                    .text(
                        "暂无子会话任务",
                        "No delegated tasks yet",
                        "委任したタスクはありません",
                    )
                    .into(),
            );
        }
        lines.extend(page.tasks.iter().map(|task| summary(task, language, false)));
        if let Some(cursor) = page.next_cursor {
            lines.push(format!("/session children list {cursor}"));
        }
        return Ok(lines.join("\n\n"));
    }
    let task: ChildSessionTask = serde_json::from_value(value)?;
    Ok(summary(&task, language, action == "show"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_wording_refers_to_assignment_and_waiting_stays_open() {
        assert_eq!(
            status(ChildSessionTaskStatus::Completed, Language::En),
            "This task has ended"
        );
        assert_eq!(
            status(ChildSessionTaskStatus::WaitingAnswer, Language::En),
            "Waiting for an answer"
        );
        assert!(!ChildSessionTaskStatus::WaitingAnswer.is_terminal());
        assert_eq!(safe("report\u{1b}[2J\nnext"), "report[2J\nnext");
    }
}
