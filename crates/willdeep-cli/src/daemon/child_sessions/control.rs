use super::*;
use store::MutationReceipt;

pub(super) fn link(ledger: &Ledger, parent: Uuid, child: Uuid) -> Result<&ChildSessionLink> {
    ledger
        .links
        .values()
        .find(|link| link.parent_session_id == parent && link.child_session_id == child)
        .context("child Session does not belong to this parent")
}

pub(super) fn require_delegated(ledger: &Ledger, task: &ChildSessionTask) -> Result<()> {
    anyhow::ensure!(
        ledger.is_delegated(task),
        "child relation is no longer delegated"
    );
    Ok(())
}

pub(super) fn mutate(
    state: &ServerState,
    ledger: &mut Ledger,
    request: &willdeep_runtime_protocol::ApiRequest,
) -> Result<serde_json::Value> {
    let fingerprint = store::fingerprint(&request.operation, &request.params)?;
    if let Some(receipt) = ledger.mutations.get(&request.request_id) {
        anyhow::ensure!(
            receipt.fingerprint == fingerprint,
            "child request_id conflicts with different parameters"
        );
        let response = receipt.response.clone();
        sync_detachments(&state.sessions, ledger)?;
        return Ok(response);
    }
    anyhow::ensure!(
        !ledger
            .assignments
            .values()
            .any(|entry| entry.task.request_id == request.request_id),
        "child request_id already belongs to an assignment"
    );
    let response = match request.operation.as_str() {
        "session.child.takeover" | "session.child.detach" => {
            let params: ChangeChildSessionControlParams =
                serde_json::from_value(request.params.clone())?;
            let current = link(ledger, params.parent_session_id, params.child_session_id)?.clone();
            anyhow::ensure!(
                current.control_revision == params.expected_control_revision,
                "child control revision changed"
            );
            let desired = if request.operation == "session.child.detach" {
                ChildSessionControlState::Detached
            } else {
                ChildSessionControlState::UserOwned
            };
            anyhow::ensure!(
                current.control_state == ChildSessionControlState::Delegated
                    || (current.control_state == ChildSessionControlState::UserOwned
                        && desired == ChildSessionControlState::Detached),
                "child control transition is not available"
            );
            let current = ledger.links.get_mut(&current.link_id).unwrap();
            current.control_state = desired;
            current.control_revision = current
                .control_revision
                .checked_add(1)
                .context("child control revision exhausted")?;
            current.control_updated_at = now();
            let response = serde_json::to_value(&*current)?;
            let waiting = ledger
                .assignments
                .iter()
                .filter(|(_, entry)| {
                    entry.task.child_session_id == params.child_session_id
                        && !entry.task.status.is_terminal()
                        && entry.task.child_turn_id.is_none()
                })
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            for id in waiting {
                // Enqueue can commit before the ledger pointer. An accepted Turn
                // survives takeover even when recovery has not repaired it yet.
                if !dependencies::restore_turn(&state.sessions, ledger, id)? {
                    dependencies::block(
                        &state.sessions,
                        ledger,
                        id,
                        "child control was revoked before a Turn existed".into(),
                    )?;
                }
            }
            response
        }
        "session.child.task.subscription.set" => {
            let params: SetChildSessionSubscriptionParams =
                serde_json::from_value(request.params.clone())?;
            let task = &owned(ledger, params.parent_session_id, params.assignment_id)?.task;
            let relation = link(ledger, task.parent_session_id, task.child_session_id)?;
            anyhow::ensure!(
                relation.link_id == task.link_id,
                "child task relation changed"
            );
            anyhow::ensure!(
                task.subscription_revision == params.expected_subscription_revision,
                "child subscription revision changed"
            );
            let task = &mut ledger
                .assignments
                .get_mut(&params.assignment_id)
                .unwrap()
                .task;
            task.subscription = params.subscription;
            task.notify = params.subscription.legacy_notify();
            task.subscription_revision = task
                .subscription_revision
                .checked_add(1)
                .context("child subscription revision exhausted")?;
            task.updated_at = now();
            // Policy changes never forge a receipt or change the result identity.
            serde_json::to_value(
                ledger.project(ledger.assignments.get(&params.assignment_id).unwrap()),
            )?
        }
        _ => bail!("unsupported child user control operation"),
    };
    ledger.mutations.insert(
        request.request_id,
        MutationReceipt {
            fingerprint,
            response: response.clone(),
        },
    );
    // Link is authoritative. Recover the monotonic Session projection if a
    // process stops between this write and the Core/Runtime metadata updates.
    state.sessions.children.persist(ledger)?;
    sync_detachments(&state.sessions, ledger)?;
    Ok(response)
}

pub(super) fn sync_detachments(sessions: &RuntimeSessionStore, ledger: &Ledger) -> Result<()> {
    for relation in ledger
        .links
        .values()
        .filter(|link| link.control_state == ChildSessionControlState::Detached)
    {
        sessions.detach_child_projection(relation.child_session_id)?;
    }
    Ok(())
}
