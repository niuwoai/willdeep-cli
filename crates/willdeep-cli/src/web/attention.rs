use std::collections::{HashMap, HashSet};
use std::path::Path;

use futures_util::{StreamExt, TryStreamExt};
use serde::Serialize;
use uuid::Uuid;
use willdeep_core::{SessionDigest, SessionStore};
use willdeep_runtime_protocol::{RuntimeTask, RuntimeTurn, SessionStatus, TaskStatus, TurnStatus};

use super::{WebError, WebState, failure_domain_name, now_seconds, registered_web_workspaces};
use crate::daemon::tui_bridge::{self, RemoteAttentionSnapshot};

const ATTENTION_TITLE_CHARS: usize = 160;
const LATEST_TURN_CONCURRENCY: usize = 4;

#[derive(Debug, Serialize)]
pub(super) struct WebAttentionSession {
    pub session_id: Uuid,
    /// None lets each browser language supply its own safe, identifiable fallback.
    pub title: Option<String>,
    pub active_turn_id: Option<Uuid>,
    pub reasons: Vec<WebAttentionReason>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum AttentionStatus {
    WaitingApproval,
    WaitingAnswer,
    Failed,
    Interrupted,
    Partial,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(super) struct WebAttentionReason {
    status: AttentionStatus,
    failure_domain: Option<&'static str>,
    exit_code: Option<i32>,
}

/// The project scope is independent of the currently selected chat. Reuse the
/// same registered allowlist and parent-worktree attribution as the Session list.
pub(super) async fn attention_sessions(
    state: &WebState,
    workspace: &Path,
) -> Result<Vec<WebAttentionSession>, WebError> {
    let allowed = registered_web_workspaces(state)
        .await?
        .into_iter()
        .map(|workspace| workspace.root)
        .collect::<Vec<_>>();
    let home = state.home.clone();
    let digests = tokio::task::spawn_blocking(move || SessionStore::new(&home).digests())
        .await
        .map_err(|error| {
            WebError::internal(format!("load attention Session summaries: {error}"))
        })?;
    let visible = visible_sessions(&digests, &allowed, workspace);
    let snapshot = tui_bridge::remote_attention_snapshot(&state.home)
        .await
        .map_err(WebError::from_anyhow)?;
    let timestamp = now_seconds();
    let superseded = tui_bridge::superseded_runtime_tasks(&snapshot.tasks);
    let navigable = snapshot
        .sessions
        .iter()
        .filter(|session| {
            session.status != SessionStatus::Archived && visible.contains_key(&session.id)
        })
        .map(|session| session.id)
        .collect::<HashSet<_>>();
    // Task timestamps have one-second precision, and a queued Turn may have no
    // Task yet. Consult Turn identity before retaining a previous failure.
    let candidates = snapshot
        .tasks
        .iter()
        .filter(|task| unsettled(task.status) && tui_bridge::runtime_task_visible(task, timestamp))
        .filter(|task| !superseded.contains(&task.id))
        .filter_map(|task| task.session_id.filter(|id| navigable.contains(id)))
        .collect::<HashSet<_>>();
    let latest = futures_util::stream::iter(candidates)
        .map(|id| async move {
            let turn = tui_bridge::remote_latest_turn(&state.home, id)
                .await
                .map_err(WebError::from_anyhow)?;
            Ok::<_, WebError>((id, turn))
        })
        .buffer_unordered(LATEST_TURN_CONCURRENCY)
        .try_collect::<HashMap<_, _>>()
        .await?;
    Ok(aggregate_attention(&snapshot, &visible, &latest, timestamp))
}

fn visible_sessions<'a>(
    digests: &'a [SessionDigest],
    allowed: &[std::path::PathBuf],
    workspace: &Path,
) -> HashMap<Uuid, &'a SessionDigest> {
    let index = digests
        .iter()
        .map(|session| (session.id, session))
        .collect::<HashMap<_, _>>();
    digests
        .iter()
        .filter(|session| {
            let project = if allowed.contains(&session.workspace) {
                Some(&session.workspace)
            } else {
                session
                    .parent_session_id
                    .and_then(|parent| index.get(&parent))
                    .map(|parent| &parent.workspace)
                    .filter(|root| allowed.contains(root))
            };
            project.is_some_and(|root| root == workspace)
        })
        .map(|session| (session.id, session))
        .collect()
}

fn unsettled(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Failed | TaskStatus::Interrupted | TaskStatus::Partial
    )
}

fn superseded_turn(task: &RuntimeTask, latest: Option<&RuntimeTurn>) -> bool {
    let Some(latest) = latest else { return false };
    match task.turn_id {
        Some(turn_id) if turn_id != latest.id => true,
        Some(_) => {
            matches!(latest.status, TurnStatus::Completed | TurnStatus::Cancelled)
                || latest.active_task_id.is_some_and(|id| id != task.id)
        }
        None => latest.created_at > task.created_at,
    }
}

fn safe_title(session: &SessionDigest) -> Option<String> {
    let text = if willdeep_core::session_title::is_placeholder(&session.title) {
        session.preview.as_deref().unwrap_or_default()
    } else {
        &session.title
    };
    // Redact before truncation so a cutoff never turns a credential into an
    // unrecognizable prefix. Control characters do not belong in navigation.
    let title = willdeep_core::judge::redact_credentials(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .take(ATTENTION_TITLE_CHARS)
        .collect::<String>();
    (!title.is_empty()).then_some(title)
}

fn aggregate_attention(
    snapshot: &RemoteAttentionSnapshot,
    visible: &HashMap<Uuid, &SessionDigest>,
    latest: &HashMap<Uuid, Option<RuntimeTurn>>,
    timestamp: u64,
) -> Vec<WebAttentionSession> {
    let runtime_sessions = snapshot
        .sessions
        .iter()
        .filter(|session| session.status != SessionStatus::Archived)
        .map(|session| (session.id, session))
        .collect::<HashMap<_, _>>();
    let superseded = tui_bridge::superseded_runtime_tasks(&snapshot.tasks);
    let mut gates = HashMap::<Uuid, Vec<AttentionStatus>>::new();
    for approval in &snapshot.approvals {
        gates
            .entry(approval.task_id)
            .or_default()
            .push(AttentionStatus::WaitingApproval);
    }
    for question in &snapshot.questions {
        gates
            .entry(question.task_id)
            .or_default()
            .push(AttentionStatus::WaitingAnswer);
    }
    let mut sessions = HashMap::<Uuid, WebAttentionSession>::new();
    for task in &snapshot.tasks {
        let Some(session_id) = task.session_id else {
            continue;
        };
        let (Some(digest), Some(runtime)) =
            (visible.get(&session_id), runtime_sessions.get(&session_id))
        else {
            continue;
        };
        if !tui_bridge::runtime_task_visible(task, timestamp)
            || matches!(task.status, TaskStatus::Completed | TaskStatus::Cancelled)
        {
            continue;
        }
        if unsettled(task.status)
            && (superseded.contains(&task.id)
                || superseded_turn(task, latest.get(&session_id).and_then(Option::as_ref))
                || runtime
                    .active_turn_id
                    .is_some_and(|id| task.turn_id.is_some_and(|turn| turn != id)))
        {
            continue;
        }
        let status = match task.status {
            TaskStatus::WaitingApproval => Some(AttentionStatus::WaitingApproval),
            TaskStatus::WaitingAnswer => Some(AttentionStatus::WaitingAnswer),
            TaskStatus::Failed => Some(AttentionStatus::Failed),
            TaskStatus::Interrupted => Some(AttentionStatus::Interrupted),
            TaskStatus::Partial => Some(AttentionStatus::Partial),
            _ => None,
        };
        for status in status
            .into_iter()
            .chain(gates.get(&task.id).into_iter().flatten().copied())
        {
            let entry = sessions
                .entry(session_id)
                .or_insert_with(|| WebAttentionSession {
                    session_id,
                    title: safe_title(digest),
                    active_turn_id: runtime.active_turn_id,
                    reasons: Vec::new(),
                });
            let failed = matches!(
                status,
                AttentionStatus::Failed | AttentionStatus::Interrupted | AttentionStatus::Partial
            );
            entry.reasons.push(WebAttentionReason {
                status,
                failure_domain: failed
                    .then_some(task.failure_domain)
                    .flatten()
                    .map(failure_domain_name),
                exit_code: failed.then_some(task.exit_code).flatten(),
            });
        }
    }
    let mut sessions = sessions.into_values().collect::<Vec<_>>();
    for session in &mut sessions {
        session.reasons.sort();
        session.reasons.dedup();
    }
    sessions.sort_by(|left, right| {
        left.reasons[0]
            .status
            .cmp(&right.reasons[0].status)
            .then_with(|| {
                right
                    .active_turn_id
                    .is_some()
                    .cmp(&left.active_turn_id.is_some())
            })
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    sessions
}

#[cfg(test)]
mod tests;
