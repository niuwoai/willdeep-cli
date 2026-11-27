use super::*;
use serde::Deserialize;
use willdeep_runtime_protocol::child_session::{
    ChildSessionWorkspaceMode, StartChildSessionParams,
};

#[derive(Deserialize)]
struct Proposal {
    id: uuid::Uuid,
    params: StartChildSessionParams,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ProposalList {
    proposals: Vec<Proposal>,
}

#[derive(Default)]
pub(super) struct Panel {
    parent: Option<uuid::Uuid>,
    pending: Vec<Proposal>,
    scroll: u16,
}

impl Panel {
    pub(super) fn is_open(&self) -> bool {
        !self.pending.is_empty()
    }
}

pub(super) fn proposal_may_have_arrived(event: &crate::daemon::RemoteRuntimeEvent) -> bool {
    event.kind == "task.output"
        && event.message.split_once(' ').is_some_and(|(_, payload)| {
            serde_json::from_str::<serde_json::Value>(payload).is_ok_and(|value| {
                value["type"] == "tool_completed"
                    && value["name"] == "start_session"
                    && value["is_error"] == false
            })
        })
}

pub(super) async fn open(app: &mut App, runtime: &TuiRuntime, parent: uuid::Uuid) -> Result<bool> {
    let result = crate::daemon::child_operation_in_language(
        &runtime.home,
        "session.child.proposal.list",
        serde_json::json!({"parent_session_id": parent}),
        None,
        app.language,
    )
    .await?;
    let list: ProposalList = serde_json::from_value(result)?;
    app.child_proposals = Panel {
        parent: Some(parent),
        pending: list.proposals,
        scroll: 0,
    };
    Ok(!app.child_proposals.pending.is_empty())
}

pub(super) async fn handle_key(
    app: &mut App,
    runtime: &TuiRuntime,
    parent: uuid::Uuid,
    key: KeyEvent,
) -> bool {
    let panel = &mut app.child_proposals;
    if panel.parent != Some(parent) {
        *panel = Panel::default();
        return false;
    }
    let Some(proposal) = panel.pending.first() else {
        return false;
    };
    let operation = match (key.code, key.modifiers) {
        (KeyCode::Esc, KeyModifiers::NONE) => {
            *panel = Panel::default();
            return true;
        }
        (KeyCode::Up, KeyModifiers::NONE) => {
            panel.scroll = panel.scroll.saturating_sub(1);
            return true;
        }
        (KeyCode::Down, KeyModifiers::NONE) => {
            panel.scroll = panel.scroll.saturating_add(1);
            return true;
        }
        (KeyCode::Char('y'), KeyModifiers::CONTROL) => "session.child.proposal.start",
        (KeyCode::Char('n'), KeyModifiers::CONTROL) => "session.child.proposal.dismiss",
        _ => return true,
    };
    let started = key.code == KeyCode::Char('y');
    let result = crate::daemon::child_operation_in_language(
        &runtime.home,
        operation,
        serde_json::json!({"parent_session_id": parent, "proposal_id": proposal.id}),
        None,
        app.language,
    )
    .await;
    match result {
        Ok(_) => {
            panel.pending.remove(0);
            panel.scroll = 0;
            app.notice = Some(
                if started {
                    app.language.text(
                        "已提交子会话",
                        "Child conversation submitted",
                        "子セッションを送信しました",
                    )
                } else {
                    app.language
                        .text("已拒绝提议", "Proposal declined", "提案を拒否しました")
                }
                .to_owned(),
            );
        }
        Err(error) => panel.pending[0].error = Some(error.to_string()),
    }
    true
}

pub(super) fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let panel = &app.child_proposals;
    let Some(proposal) = panel.pending.first() else {
        return;
    };
    let lang = app.language;
    let area = centered_rect(78, 16, frame.area());
    let scope = match proposal.params.workspace_mode {
        ChildSessionWorkspaceMode::SharedReadOnly => lang.text(
            "共享工作区（只读）",
            "Shared workspace (read-only)",
            "共有作業領域（読み取り専用）",
        ),
        ChildSessionWorkspaceMode::Worktree => {
            lang.text("独立工作树", "Separate worktree", "独立した作業ツリー")
        }
    };
    let mut text = format!(
        "{}\n{scope}\n\n{}",
        proposal.params.title, proposal.params.prompt
    );
    if let Some(error) = &proposal.error {
        text.push_str("\n\n");
        text.push_str(error);
    }
    text.retain(|character| !character.is_control() || character == '\n');
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(lang.text(
            "子会话提议",
            "Child conversation proposal",
            "子セッションの提案",
        ))
        .title_bottom(lang.text(
            " Ctrl+Y 开始 · Ctrl+N 拒绝 · Esc 稍后 · ↑↓ 滚动 ",
            " Ctrl+Y Start · Ctrl+N Decline · Esc Later · ↑↓ Scroll ",
            " Ctrl+Y 開始 · Ctrl+N 拒否 · Esc 後で · ↑↓ スクロール ",
        ))
        .style(MODAL_PANEL);
    let body = Paragraph::new(text).wrap(Wrap { trim: false });
    let inner = block.inner(area);
    let max_scroll = body
        .line_count(inner.width)
        .saturating_sub(usize::from(inner.height));
    let scroll = panel
        .scroll
        .min(u16::try_from(max_scroll).unwrap_or(u16::MAX));
    frame.render_widget(body.block(block).scroll((scroll, 0)), area);
}
