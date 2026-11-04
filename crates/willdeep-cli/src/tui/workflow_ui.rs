//! Workflow commands and transcript cards share Runtime run identities.
use super::*;
use willdeep_core::workflow::{WorkflowNodeStatus, WorkflowRun, WorkflowRunStatus, WorkflowStore};

const WORKFLOW_PREFIX: &str = "Workflow: ";

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct WorkflowTranscript {
    pub run: WorkflowRun,
    pub expanded: bool,
}

pub(super) fn decode(entry: &str) -> Option<WorkflowTranscript> {
    serde_json::from_str(entry.strip_prefix(WORKFLOW_PREFIX)?).ok()
}

pub(super) fn entry(run: WorkflowRun, expanded: bool) -> String {
    format!(
        "{WORKFLOW_PREFIX}{}",
        serde_json::to_string(&WorkflowTranscript { run, expanded }).expect("workflow serializes")
    )
}

/// Update only this execution. Retry/resume produce separate cards.
pub(super) fn update(entries: &mut Vec<String>, run: WorkflowRun, expand: bool) {
    if let Some((index, previous)) = entries.iter().enumerate().find_map(|(index, row)| {
        decode(row)
            .filter(|card| card.run.id == run.id)
            .map(|card| (index, card))
    }) {
        let run = if run.revision < previous.run.revision {
            previous.run
        } else {
            run
        };
        entries[index] = entry(run, expand || previous.expanded);
    } else {
        entries.push(entry(run, expand));
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Command {
    operation: &'static str,
    run_id: Option<uuid::Uuid>,
}

fn parse(prompt: &str) -> Option<Result<Command>> {
    let mut words = prompt.split_whitespace();
    if words.next()? != "/workflow" {
        return None;
    }
    let operation = match words.next().unwrap_or("list") {
        "list" => "list",
        "history" => "history",
        "get" => "get",
        "stop" => "stop",
        "retry" => "retry",
        "resume" => "resume",
        _ => return Some(Err(anyhow::anyhow!("unsupported workflow action"))),
    };
    let id = words.next();
    let valid = words.next().is_none() && (matches!(operation, "list" | "history") == id.is_none());
    Some(if !valid {
        Err(anyhow::anyhow!("invalid workflow arguments"))
    } else {
        id.map(uuid::Uuid::parse_str)
            .transpose()
            .map(|run_id| Command { operation, run_id })
            .map_err(Into::into)
    })
}

pub(super) fn usage(language: Language) -> &'static str {
    language.text(
        "用法：/workflow [list|history|get <运行ID>|stop <运行ID>|retry <运行ID>|resume <运行ID>]",
        "Usage: /workflow [list|history|get <run-id>|stop <run-id>|retry <run-id>|resume <run-id>]",
        "使用法：/workflow [list|history|get <実行ID>|stop <実行ID>|retry <実行ID>|resume <実行ID>]",
    )
}

/// Network requests never block the terminal event loop. External controls admit
/// in the daemon and its per-session executor waits for any local execution owner.
pub(super) fn handle_command(
    prompt: &str,
    app: &mut App,
    session: &mut Session,
    store: &SessionStore,
    agent: &Arc<Agent>,
    runtime: &TuiRuntime,
) -> Result<bool> {
    let Some(command) = parse(prompt) else {
        return Ok(false);
    };
    let command = match command {
        Ok(command) => command,
        Err(_) => {
            app.append_transcript(format!("System: {}", usage(app.language)));
            return Ok(true);
        }
    };
    let mut params = serde_json::json!({"parent_session_id":session.id});
    if let Some(id) = command.run_id {
        params["run_id"] = serde_json::json!(id);
    }
    let control = matches!(command.operation, "stop" | "retry" | "resume");
    if control {
        runtime_ui::persist_missing_core_session(session, store)?;
        // Merge only the ownership flag; the Runtime owns any newer history.
        if !app.running {
            *session = store.update(session.id, |latest| latest.runtime_managed = true)?;
        }
    }
    let session_id = session.id;
    let workspace = session.workspace.clone();
    let profile = session.profile.clone();
    let model = session.model.clone();
    let home = runtime.home.clone();
    let approval_mode = app.approval_mode;
    let tx = runtime.tx.clone();
    let agent = agent.clone();
    let operation = command.operation;
    app.notice = Some(
        app.language
            .text(
                "正在读取工作流…",
                "Loading workflow…",
                "ワークフローを取得中…",
            )
            .into(),
    );
    tokio::spawn(async move {
        let result: Result<serde_json::Value> = async {
            if !control {
                return agent
                    .workflow_operation(operation, params)
                    .await
                    .map_err(Into::into);
            }
            crate::daemon::ensure_runtime_session(&home, session_id, &workspace, profile, model)
                .await?;
            crate::daemon::update_remote_session_approval_mode(&home, session_id, approval_mode)
                .await?;
            let request_id = uuid::Uuid::new_v4();
            params["request_id"] = serde_json::json!(request_id);
            let response = crate::daemon::schedule_response(
                &home,
                &format!("workflow.{operation}"),
                params,
                Some(request_id),
            )
            .await?;
            response
                .into_result()
                .map_err(|error| anyhow::anyhow!("{error:?}"))
        }
        .await;
        let _ = tx.send(UiMessage::WorkflowResult {
            session_id,
            operation: operation.into(),
            result: result.map_err(|error| format!("{error:#}")),
        });
    });
    Ok(true)
}

pub(super) fn receive(
    app: &mut App,
    operation: &str,
    result: std::result::Result<serde_json::Value, String>,
) {
    let value = match result {
        Ok(value) => value,
        Err(error) => {
            let message = format!(
                "{}: {error}",
                app.language.text(
                    "工作流操作失败",
                    "Workflow action failed",
                    "ワークフロー操作に失敗しました"
                )
            );
            app.notice = Some(message.clone());
            app.append_transcript(format!("Error: {message}"));
            return;
        }
    };
    if let Some(run) = value
        .get("workflow")
        .and_then(|run| serde_json::from_value::<WorkflowRun>(run.clone()).ok())
    {
        update(&mut app.transcript, run, operation == "get");
    } else if let Some(runs) = value
        .get("runs")
        .and_then(|runs| serde_json::from_value::<Vec<WorkflowRun>>(runs.clone()).ok())
    {
        if runs.is_empty() {
            app.append_transcript(format!(
                "System: {}",
                app.language.text(
                    "此会话暂无工作流运行",
                    "No workflow runs in this session",
                    "このセッションにはワークフローの実行がありません"
                )
            ));
        }
        for run in runs {
            update(&mut app.transcript, run, false);
        }
    }
    app.notice = Some(
        app.language
            .text(
                "工作流已更新",
                "Workflow updated",
                "ワークフローを更新しました",
            )
            .into(),
    );
    app.refresh_transcript_height();
}

/// Catch reconnects and updates admitted by other clients; persisted state wins.
pub(super) fn refresh(app: &mut App, home: &std::path::Path, session_id: uuid::Uuid) {
    match WorkflowStore::new(home, session_id).list_runs() {
        Ok(runs) => {
            for run in runs {
                update(&mut app.transcript, run, false);
            }
            app.refresh_transcript_height();
        }
        Err(error) => {
            app.notice = Some(format!(
                "{}: {error}",
                app.language.text(
                    "工作流状态读取失败",
                    "Cannot read workflow state",
                    "ワークフローの状態を取得できません"
                )
            ))
        }
    }
}

fn run_status(status: WorkflowRunStatus, language: Language) -> &'static str {
    match status {
        WorkflowRunStatus::Running => language.text("运行中", "Running", "実行中"),
        WorkflowRunStatus::Stopping => language.text("正在停止", "Stopping", "停止中"),
        WorkflowRunStatus::Completed => language.text("已完成", "Completed", "完了"),
        WorkflowRunStatus::Failed => language.text("失败", "Failed", "失敗"),
        WorkflowRunStatus::Cancelled => language.text("已取消", "Cancelled", "キャンセル済み"),
        WorkflowRunStatus::Interrupted => language.text("已中断", "Interrupted", "中断"),
    }
}

fn node_status(status: WorkflowNodeStatus, language: Language) -> &'static str {
    match status {
        WorkflowNodeStatus::Pending => language.text("等待", "Waiting", "待機中"),
        WorkflowNodeStatus::DispatchPending => language.text("待派工", "Queued", "キュー待機"),
        WorkflowNodeStatus::Dispatched => language.text("已派工", "Dispatched", "割当済み"),
        WorkflowNodeStatus::Running => language.text("运行中", "Running", "実行中"),
        WorkflowNodeStatus::Stopping => language.text("正在停止", "Stopping", "停止中"),
        WorkflowNodeStatus::Completed => language.text("已完成", "Completed", "完了"),
        WorkflowNodeStatus::Failed => language.text("失败", "Failed", "失敗"),
        WorkflowNodeStatus::Blocked => language.text("已阻塞", "Blocked", "ブロック中"),
        WorkflowNodeStatus::Interrupted => language.text("已中断", "Interrupted", "中断"),
        WorkflowNodeStatus::Cancelled => language.text("已取消", "Cancelled", "キャンセル済み"),
    }
}

pub(super) fn render(
    card: &WorkflowTranscript,
    width: usize,
    language: Language,
) -> Vec<Line<'static>> {
    let run = &card.run;
    let done = run
        .nodes
        .iter()
        .filter(|node| node.status == WorkflowNodeStatus::Completed)
        .count();
    let mut lines = vec![
        Line::styled(
            format!(
                "{} · {} · {done}/{}",
                language.text("工作流", "Workflow", "ワークフロー"),
                run_status(run.status, language),
                run.nodes.len()
            ),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(terminal_safe_text(&run.spec.name)),
        Line::raw(run.id.to_string()),
    ];
    if let Some(source) = run.source_run_id {
        lines.push(Line::raw(format!(
            "{}: {source}",
            language.text("来源运行", "Source run", "元の実行")
        )));
    }
    if !card.expanded {
        lines.push(Line::raw(format!("/workflow get {}", run.id)));
        return lines;
    }
    let phase_of = |node: &willdeep_core::workflow::WorkflowNodeInstance| {
        run.spec
            .nodes
            .iter()
            .find(|definition| node.path.split('/').next() == Some(definition.id.as_str()))
            .and_then(|definition| definition.phase.as_deref())
    };
    let mut ordered = Vec::new();
    for phase in &run.spec.phases {
        let members: Vec<_> = run
            .nodes
            .iter()
            .filter(|node| phase_of(node) == Some(phase.id.as_str()))
            .collect();
        let finished = members
            .iter()
            .filter(|node| node.status == WorkflowNodeStatus::Completed)
            .count();
        ordered.push((
            Some(format!(
                "{} · {finished}/{}",
                terminal_safe_text(&phase.name),
                members.len()
            )),
            members,
        ));
    }
    ordered.push((
        None,
        run.nodes
            .iter()
            .filter(|node| phase_of(node).is_none())
            .collect(),
    ));
    for (phase, nodes) in ordered {
        if let Some(phase) = phase {
            lines.push(Line::styled(phase, Style::default().fg(Color::Cyan)));
        }
        for node in nodes {
            lines.push(Line::raw(format!(
                "{} · {}",
                terminal_safe_text(&node.path),
                node_status(node.status, language)
            )));
            let model = node
                .agent_run
                .as_ref()
                .and_then(|run| run.model.as_deref())
                .unwrap_or("?");
            let tokens = node
                .usage
                .total_tokens
                .map_or_else(|| "?".into(), |tokens| tokens.to_string());
            let duration = node.started_at_ms.zip(node.finished_at_ms).map_or_else(
                || "?".into(),
                |(start, end)| format!("{}ms", end.saturating_sub(start)),
            );
            lines.push(Line::styled(
                format!("  {model} · {tokens} Token · {duration}"),
                Style::default().fg(Color::DarkGray),
            ));
            if node.reused_from.is_some() {
                lines.push(Line::raw(language.text(
                    "  已复用结果",
                    "  Reused result",
                    "  結果を再利用",
                )));
            }
            for text in [node.error.as_deref(), node.report.as_deref()]
                .into_iter()
                .flatten()
            {
                lines.extend(render_assistant_markdown(&terminal_safe_text(text), width));
            }
            if let Some(output) = &node.output {
                lines.extend(render_assistant_markdown(
                    &terminal_safe_text(&output.to_string()),
                    width,
                ));
            }
        }
    }
    for text in [run.error.as_deref(), run.report.as_deref()]
        .into_iter()
        .flatten()
    {
        lines.extend(render_assistant_markdown(&terminal_safe_text(text), width));
    }
    if let Some(output) = &run.output {
        lines.extend(render_assistant_markdown(
            &terminal_safe_text(&output.to_string()),
            width,
        ));
    }
    lines.push(Line::raw(format!("/workflow stop|retry|resume {}", run.id)));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_require_exact_run_ids() {
        assert!(parse("/workflow").unwrap().is_ok());
        assert!(parse("/workflow history").unwrap().is_ok());
        assert!(parse("/workflow stop nope").unwrap().is_err());
        assert!(parse("/workflow get").unwrap().is_err());
        assert!(parse("/workflow list extra").unwrap().is_err());
        assert!(parse("/workflowXYZ").is_none());
        let id = uuid::Uuid::new_v4();
        assert_eq!(
            parse(&format!("/workflow stop {id}"))
                .unwrap()
                .unwrap()
                .run_id,
            Some(id)
        );
    }

    #[test]
    fn cards_update_the_same_run_and_preserve_retry_history_and_unknown_usage() {
        let home = std::env::temp_dir().join(format!("workflow-ui-{}", uuid::Uuid::new_v4()));
        let store = WorkflowStore::new(&home, uuid::Uuid::new_v4());
        let mut spec = willdeep_core::workflow::example_spec();
        spec.phases.push(willdeep_core::workflow::WorkflowPhase {
            id: "review".into(),
            name: "Review phase".into(),
        });
        spec.nodes[0].phase = Some("review".into());
        store.save_definition(&spec).unwrap();
        let mut run = store
            .start(
                &spec.id,
                serde_json::json!({"topic":"test"}),
                Default::default(),
            )
            .unwrap();
        run = store.poll(run.id).unwrap().run;
        let mut entries = Vec::new();
        update(&mut entries, run.clone(), true);
        run.status = WorkflowRunStatus::Failed;
        run.revision += 1;
        update(&mut entries, run.clone(), false);
        assert_eq!(entries.len(), 1);
        let mut stale = run.clone();
        stale.revision -= 1;
        stale.status = WorkflowRunStatus::Running;
        update(&mut entries, stale, false);
        assert_eq!(
            decode(&entries[0]).unwrap().run.status,
            WorkflowRunStatus::Failed
        );
        let mut retry = run.clone();
        retry.id = uuid::Uuid::new_v4();
        retry.source_run_id = Some(run.id);
        update(&mut entries, retry, false);
        assert_eq!(entries.len(), 2);
        let card = decode(&entries[0]).unwrap();
        assert!(card.expanded);
        for language in [Language::ZhCn, Language::En, Language::Ja] {
            let text = Text::from(render(&card, 24, language)).to_string();
            assert!(text.contains(&run.id.to_string()));
            assert!(text.contains(run_status(WorkflowRunStatus::Failed, language)));
            assert!(!text.contains(WORKFLOW_PREFIX));
            assert!(text.contains("Review phase"));
            assert!(text.contains("? Token"));
            assert!(!text.contains("0 Token"));
        }
        std::fs::remove_dir_all(home).unwrap();
    }
}
