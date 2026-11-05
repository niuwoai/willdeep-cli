//! Complete conversations use the coordinator, independently of professional Workers.
use super::*;
use willdeep_runtime_protocol::child_session::{
    ChildSessionControlState, ChildSessionLink, ChildSessionSubscription, ChildSessionTask,
    ChildSessionTaskPage, ChildSessionTaskStatus,
};

mod parser;

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

fn control(state: ChildSessionControlState, language: Language) -> &'static str {
    match state {
        ChildSessionControlState::Delegated => {
            language.text("父会话委派中", "Delegated by parent", "親から委任中")
        }
        ChildSessionControlState::UserOwned => {
            language.text("用户已接管", "Taken over by user", "ユーザーが引継ぎ済み")
        }
        ChildSessionControlState::Detached => {
            language.text("已脱离父会话", "Detached from parent", "親から切り離し済み")
        }
    }
}

fn subscription(value: ChildSessionSubscription, language: Language) -> &'static str {
    match value {
        ChildSessionSubscription::None => {
            language.text("关闭通知", "Notifications off", "通知なし")
        }
        ChildSessionSubscription::Inbox => {
            language.text("仅保留在任务列表", "Task list only", "タスク一覧のみ")
        }
        ChildSessionSubscription::AutoWake => {
            language.text("自动回报", "Automatic delivery", "自動報告")
        }
    }
}

fn legacy_subscription(value: &mut serde_json::Value) {
    if let Some(task) = value.as_object_mut()
        && !task.contains_key("subscription")
    {
        let subscription = if task.get("notify").and_then(serde_json::Value::as_str) == Some("none")
        {
            "none"
        } else {
            "auto_wake"
        };
        task.insert("subscription".into(), subscription.into());
    }
}

fn link_summary(link: &ChildSessionLink, language: Language) -> String {
    format!(
        "{}: {}\n{} · {}: {}",
        language.text("子会话", "Child", "子セッション"),
        link.child_session_id,
        control(link.control_state, language),
        language.text("控制版本", "Control revision", "制御バージョン"),
        link.control_revision
    )
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
    let state = if task.waiting_on_dependencies {
        language.text(
            "等待前置任务",
            "Waiting for prerequisites",
            "前提タスク待ち",
        )
    } else {
        status(task.status, language)
    };
    let mut lines = vec![
        format!("{} · {state}", safe(&task.title)),
        format!("  {mode}"),
        format!(
            "{} · {}: {}",
            control(task.control_state, language),
            language.text("控制版本", "Control revision", "制御バージョン"),
            task.control_revision
        ),
        format!(
            "{} · {}: {}",
            subscription(task.subscription, language),
            language.text("通知版本", "Subscription revision", "通知バージョン"),
            task.subscription_revision
        ),
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
    if !task.depends_on.is_empty() {
        lines.push(format!(
            "{}: {}",
            language.text("前置任务", "Prerequisites", "前提タスク"),
            task.depends_on
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
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
            if result.superseded_by.is_some() {
                language.text(
                    "已创建后续重试",
                    "A follow-up retry was created",
                    "再試行を作成済み",
                )
            } else if result.consumed_at.is_some() {
                language.text("结果已送达", "Result delivered", "結果を送達済み")
            } else {
                language.text("结果待处理", "Result pending", "結果の処理待ち")
            }
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

fn usage(language: Language) -> &'static str {
    language.text(
        "用法：/session children start [--fork <完成轮次ID>] [--worktree [--snapshot]] [--model <模型>] [--subscription none|inbox|auto_wake] [--depends-on <任务UUID>（可重复，最多16项）] [--request-id <UUID>] <名称> | <任务>；continue <子会话ID> [同上通知/前置任务选项] <后续任务>；list [分页游标]；show <任务ID>；open <子会话ID>；parent；link <子会话ID>；takeover|detach <子会话ID> --expected-control-revision <控制版本> [--request-id <UUID>] [--yes]；subscription set <任务ID> --subscription none|inbox|auto_wake --expected-subscription-revision <通知版本> [--request-id <UUID>]；retry <任务ID> <结果版本>；stop <任务ID>。默认共享工作区只读；inbox 仅保留结果，不唤醒父会话。",
        "Usage: /session children start [--fork <completed-turn>] [--worktree [--snapshot]] [--model <model>] [--subscription none|inbox|auto_wake] [--depends-on <assignment-UUID> (repeat, max 16)] [--request-id <UUID>] <title> | <task>; continue <child> [subscription/dependency options above] <follow-up>; list [cursor]; show <assignment>; open <child>; parent; link <child>; takeover|detach <child> --expected-control-revision <revision> [--request-id <UUID>] [--yes]; subscription set <assignment> --subscription none|inbox|auto_wake --expected-subscription-revision <revision> [--request-id <UUID>]; retry <assignment> <result-revision>; stop <assignment>. Shared read-only workspace by default. Inbox keeps results without waking the parent.",
        "使用法：/session children start [--fork <完了ターンID>] [--worktree [--snapshot]] [--model <モデル>] [--subscription none|inbox|auto_wake] [--depends-on <タスクUUID>（繰り返し可、最大16件）] [--request-id <UUID>] <名前> | <タスク>; continue <子セッションID> [上記の通知・前提タスクオプション] <追加タスク>; list [カーソル]; show <タスクID>; open <子セッションID>; parent; link <子セッションID>; takeover|detach <子セッションID> --expected-control-revision <制御バージョン> [--request-id <UUID>] [--yes]; subscription set <タスクID> --subscription none|inbox|auto_wake --expected-subscription-revision <通知バージョン> [--request-id <UUID>]; retry <タスクID> <結果バージョン>; stop <タスクID>。既定は共有作業領域の読み取り専用。inbox は親を起動せず結果を保持します。",
    )
}

/// Handle before consuming the composer so errors preserve its exact draft and focus.
pub(super) async fn dispatch(
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    runtime: &mut TuiRuntime,
) -> bool {
    let prompt = app.input.text().trim().to_owned();
    let Some(arguments) = prompt
        .strip_prefix("/session children")
        .filter(|tail| tail.is_empty() || tail.starts_with(char::is_whitespace))
    else {
        return false;
    };
    match handle(arguments, app, session, store, runtime).await {
        Ok(result) => {
            let confirmation = matches!(word(arguments).0, "takeover" | "detach")
                && !arguments.split_whitespace().any(|value| value == "--yes");
            if !confirmation && result != usage(app.language) {
                app.take_submitted_input();
            }
            app.append_transcript(format!("System: {result}"));
        }
        Err(error) => app.append_transcript(format!(
            "Error: {}: {error:#}",
            app.language.text(
                "子会话操作失败，输入已保留",
                "Child action failed; input retained",
                "子セッション操作に失敗。入力を保持しました"
            )
        )),
    }
    true
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
    if action == "parent" && rest.is_empty() {
        anyhow::ensure!(
            !session.delegation_detached,
            "{}",
            language.text(
                "当前会话已脱离父会话，可在历史会话中查看来源",
                "This conversation is detached; find its source in history",
                "このセッションは切り離し済みです。履歴で元のセッションを確認できます"
            )
        );
        let parent = session.parent_session_id.context(language.text(
            "当前会话没有父会话",
            "This conversation has no parent",
            "親セッションはありません",
        ))?;
        anyhow::ensure!(
            app.attachments.is_empty(),
            "{}",
            language.text(
                "切换前请保存或移除附件草稿",
                "Save or remove the attachment draft before switching",
                "切替前に添付の下書きを保存または削除してください"
            )
        );
        return session_commands::switch_related(app, session, store, runtime, &parent.to_string())
            .await;
    }
    if action == "open" && !rest.is_empty() {
        let child: uuid::Uuid = rest.parse().context(language.text(
            "无效的子会话ID",
            "Invalid child ID",
            "子セッションID が無効です",
        ))?;
        let target = store.load(child).context(language.text(
            "读取子会话失败",
            "Could not load child",
            "子セッションを読み込めません",
        ))?;
        anyhow::ensure!(
            target.parent_session_id == Some(session.id),
            "{}",
            language.text(
                "此会话不属于当前来源",
                "This conversation does not belong to this source",
                "このセッションは現在の依頼元に属していません"
            )
        );
        anyhow::ensure!(
            app.attachments.is_empty(),
            "{}",
            language.text(
                "切换前请保存或移除附件草稿",
                "Save or remove the attachment draft before switching",
                "切替前に添付の下書きを保存または削除してください"
            )
        );
        return session_commands::switch_related(app, session, store, runtime, rest).await;
    }
    let parent = if session.delegation_detached {
        session.id
    } else {
        session.parent_session_id.unwrap_or(session.id)
    };
    let Some(request) = parser::parse(arguments, parent, language)? else {
        return Ok(usage(language).into());
    };
    if action == "start" {
        anyhow::ensure!(
            session.parent_session_id.is_none() && !session.delegation_read_only,
            "{}",
            language.text(
                "子会话不能继续委派完整会话",
                "Child conversations cannot delegate further conversations",
                "子セッションからさらに会話を委任できません"
            )
        );
    }
    if request.needs_confirmation {
        return Ok(language.text(
            "请确认：父会话将失去后续控制与自动通知，当前执行继续，历史和权限上限保留；接管后仍可脱离，但不能恢复委派控制。确认后使用相同命令并添加 --yes。",
            "Confirm: the parent loses future control and automatic results. Current execution continues; history and permission limits remain. Takeover can proceed to detach, but delegation cannot be restored. Repeat the same command with --yes.",
            "確認：親は今後の制御権と自動通知を失います。現在の実行は続き、履歴と権限制限は保持されます。引継ぎ後の切り離しは可能ですが、委任制御には戻せません。同じコマンドに --yes を追加してください。",
        ).into());
    }
    crate::daemon::ensure_runtime_session(
        &runtime.home,
        session.id,
        &session.workspace,
        session.profile.clone(),
        session.model.clone(),
    )
    .await?;
    let fingerprint = format!(
        "{}:{}:{}",
        runtime.home.display(),
        request.operation,
        request.params
    );
    let request_id = *app
        .child_request_ids
        .entry(fingerprint.clone())
        .or_insert_with(|| request.request_id.unwrap_or_else(uuid::Uuid::new_v4));
    if let Some(explicit) = request.request_id {
        anyhow::ensure!(
            explicit == request_id,
            "{}: {request_id}",
            language.text(
                "重试须保留原请求ID",
                "Retry with the original request ID",
                "元のリクエストIDで再試行してください"
            )
        );
    }
    let mut value = crate::daemon::child_operation_in_language(
        &runtime.home,
        request.operation,
        request.params,
        Some(request_id),
        language,
    )
    .await
    .with_context(|| format!("request_id={request_id}"))?;
    let result = if action == "list" {
        if let Some(tasks) = value
            .get_mut("tasks")
            .and_then(serde_json::Value::as_array_mut)
        {
            for task in tasks {
                legacy_subscription(task);
            }
        }
        let page: ChildSessionTaskPage =
            serde_json::from_value(value).with_context(|| format!("request_id={request_id}"))?;
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
        lines.join("\n\n")
    } else if matches!(action, "link" | "takeover" | "detach") {
        let link: ChildSessionLink =
            serde_json::from_value(value).with_context(|| format!("request_id={request_id}"))?;
        if link.child_session_id == session.id && action == "detach" {
            *session = store.load_baselined(session.id)?;
            refresh_session_lineage(app, session, store);
        }
        link_summary(&link, language)
    } else {
        legacy_subscription(&mut value);
        let task: ChildSessionTask =
            serde_json::from_value(value).with_context(|| format!("request_id={request_id}"))?;
        summary(&task, language, action == "show")
    };
    app.child_request_ids.remove(&fingerprint);
    Ok(result)
}

#[cfg(test)]
mod tests;
