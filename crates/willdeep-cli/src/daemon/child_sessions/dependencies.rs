use super::*;
use std::collections::HashSet;

/// References are immutable and may point only backwards to existing same-parent
/// assignments. Validate the historical graph too, rather than trusting old disk data.
pub(super) fn validate(ledger: &Ledger, parent: Uuid, depends_on: &[Uuid]) -> Result<()> {
    anyhow::ensure!(
        depends_on.len() <= CHILD_SESSION_DEPENDENCY_LIMIT,
        "child task allows at most {CHILD_SESSION_DEPENDENCY_LIMIT} dependencies"
    );
    let mut unique = HashSet::new();
    for id in depends_on {
        anyhow::ensure!(unique.insert(*id), "child dependency is duplicated");
    }
    let mut complete = HashSet::new();
    let mut visiting = HashSet::new();
    // Completed task chains may be much deeper than the live queue. Traverse
    // iteratively so historical data cannot overflow the Runtime call stack.
    let mut stack = depends_on
        .iter()
        .rev()
        .map(|id| (*id, false))
        .collect::<Vec<_>>();
    while let Some((id, exiting)) = stack.pop() {
        if exiting {
            visiting.remove(&id);
            complete.insert(id);
            continue;
        }
        if complete.contains(&id) {
            continue;
        }
        anyhow::ensure!(
            visiting.insert(id),
            "child dependency graph contains a cycle"
        );
        let task = &owned(ledger, parent, id)?.task;
        control::require_delegated(ledger, task)?;
        anyhow::ensure!(
            task.depends_on.len() <= CHILD_SESSION_DEPENDENCY_LIMIT,
            "historical child dependency limit exceeded"
        );
        let mut unique = HashSet::new();
        stack.push((id, true));
        for next in task.depends_on.iter().rev() {
            anyhow::ensure!(
                *next != id && unique.insert(*next),
                "historical child dependency is self-referential or duplicated"
            );
            stack.push((*next, false));
        }
    }
    Ok(())
}

pub(super) enum Readiness {
    Ready,
    Waiting,
    Blocked(String),
}

pub(super) fn readiness(ledger: &Ledger, task: &ChildSessionTask) -> Readiness {
    if !ledger.is_delegated(task) {
        return Readiness::Blocked("child control was revoked before a Turn existed".into());
    }
    if task.depends_on.contains(&task.assignment_id) {
        return Readiness::Blocked("child task depends on itself".into());
    }
    if let Err(error) = validate(ledger, task.parent_session_id, &task.depends_on) {
        return Readiness::Blocked(format!("child dependency is unavailable: {error:#}"));
    }
    let mut waiting = false;
    for id in &task.depends_on {
        let dependency = &ledger.assignments.get(id).unwrap().task;
        if dependency.status.is_terminal() {
            if dependency.status != ChildSessionTaskStatus::Completed {
                return Readiness::Blocked(format!(
                    "dependency {id} ended with {:?}: {}",
                    dependency.status,
                    dependency
                        .error
                        .as_deref()
                        .unwrap_or("no additional reason")
                ));
            }
            // Terminal status alone can precede the durable report write.
            waiting |= dependency.result.is_none();
        } else {
            waiting = true;
        }
    }
    if waiting {
        Readiness::Waiting
    } else {
        Readiness::Ready
    }
}

pub(super) fn block(
    sessions: &RuntimeSessionStore,
    ledger: &mut Ledger,
    id: Uuid,
    reason: String,
) -> Result<()> {
    let task = &mut ledger
        .assignments
        .get_mut(&id)
        .context("child task missing while blocking")?
        .task;
    if task.result.is_some() {
        return Ok(());
    }
    task.status = ChildSessionTaskStatus::Blocked;
    task.waiting_on_dependencies = false;
    task.error = Some(reason);
    task.updated_at = now();
    reconcile::record_result(sessions, ledger, id, None)
}

/// Recover only an already committed Turn; never create one during a control change.
pub(super) fn restore_turn(
    sessions: &RuntimeSessionStore,
    ledger: &mut Ledger,
    id: Uuid,
) -> Result<bool> {
    let entry = ledger
        .assignments
        .get(&id)
        .context("child task missing during Turn recovery")?;
    if entry.task.child_turn_id.is_some() {
        return Ok(true);
    }
    let existing = sessions
        .list_turns(entry.task.child_session_id)?
        .into_iter()
        .find(|turn| turn.request_id == entry.turn_request_id);
    if let Some(turn) = existing {
        let task = &mut ledger.assignments.get_mut(&id).unwrap().task;
        task.child_turn_id = Some(turn.id);
        task.waiting_on_dependencies = false;
        task.status = reconcile::task_status(turn.status);
        task.updated_at = now();
        return Ok(true);
    }
    Ok(false)
}
