use super::*;
use willdeep_runtime_protocol::{QueueAction, QueuedTurnMessage, UpdateTurnQueueParams};

const MAX_QUEUE_PROMPT_BYTES: usize = 1024 * 1024;

impl RuntimeSessionStore {
    pub(crate) fn queued_messages(&self, session_id: uuid::Uuid) -> Result<Vec<QueuedTurnMessage>> {
        let sessions = self.lock()?;
        let session = sessions
            .get(&session_id)
            .context("Runtime Session not found")?;
        let turns = self.turns_lock()?;
        let mut pending = turns
            .values()
            .filter(|turn| is_pending(turn, session))
            .collect::<Vec<_>>();
        pending.sort_by_key(|turn| turn.metadata.queue_sequence);
        Ok(pending
            .into_iter()
            .map(|turn| QueuedTurnMessage {
                priority_requested: turn.metadata.independent_requested,
                independence_confirmed: turn.metadata.independent_override,
                id: turn.metadata.id,
                session_id,
                text: editable_text(&turn.prompt).unwrap_or_default().to_owned(),
                can_edit: editable_text(&turn.prompt).is_some(),
                text_preview: editable_text(&turn.prompt)
                    .unwrap_or_default()
                    .chars()
                    .take(240)
                    .collect(),
                image_count: turn
                    .attachments
                    .iter()
                    .filter(|a| matches!(a, willdeep_core::MessageAttachment::Image { .. }))
                    .count(),
                text_attachment_count: turn
                    .attachments
                    .iter()
                    .filter(|a| matches!(a, willdeep_core::MessageAttachment::Text { .. }))
                    .count(),
            })
            .collect())
    }

    /// Use the same lock order as claim_next. Persist a copy before publishing it,
    /// so a failed write cannot remove or edit a pending message in memory.
    pub(crate) fn update_turn_queue(
        &self,
        params: &UpdateTurnQueueParams,
    ) -> Result<Option<uuid::Uuid>> {
        let sessions = self.lock()?;
        let session = sessions
            .get(&params.session_id)
            .context("Runtime Session not found")?;
        if session.status == RuntimeSessionStatus::Archived {
            bail!("Runtime Session is archived");
        }
        let mut turns = self.turns_lock()?;
        let mut updated = turns.clone();
        let mut pending = updated
            .values()
            .filter(|turn| is_pending(turn, session))
            .map(|turn| (turn.metadata.queue_sequence, turn.metadata.id))
            .collect::<Vec<_>>();
        pending.sort();
        let id = if params.action == QueueAction::Clear {
            None
        } else {
            let id = params
                .message_id
                .context("Queue action needs a message_id")?;
            if !pending.iter().any(|(_, pending_id)| *pending_id == id) {
                bail!("Queued message no longer exists in this session or has started");
            }
            Some(id)
        };
        match params.action {
            QueueAction::Edit => {
                let turn = updated.get_mut(&id.unwrap()).unwrap();
                let expected = params
                    .expected_text
                    .as_deref()
                    .context("Queue edit needs expected_text")?;
                let original = editable_text(&turn.prompt)
                    .context("Legacy queue context cannot be edited safely")?;
                if original != expected {
                    bail!("Queued message changed; refresh before editing");
                }
                let text = params.text.as_deref().context("Queue edit needs text")?;
                if text.len() > MAX_QUEUE_PROMPT_BYTES {
                    bail!("Turn prompt exceeds 1 MiB");
                }
                if text.trim().is_empty() && turn.attachments.is_empty() {
                    bail!("Message text is empty");
                }
                let start = original.as_ptr() as usize - turn.prompt.as_ptr() as usize;
                let end = start + original.len();
                let replacement =
                    format!("{}{}{}", &turn.prompt[..start], text, &turn.prompt[end..]);
                if replacement.len() > MAX_QUEUE_PROMPT_BYTES {
                    bail!("Turn prompt exceeds 1 MiB");
                }
                turn.prompt = replacement;
                turn.metadata.independent_override = false;
            }
            QueueAction::Remove | QueueAction::Clear => {
                for (_, pending_id) in &pending {
                    if id.is_some_and(|id| id != *pending_id) {
                        continue;
                    }
                    let turn = updated.get_mut(pending_id).unwrap();
                    turn.metadata.status = RuntimeTurnStatus::Cancelled;
                    turn.metadata.completed_at = Some(now());
                }
            }
            QueueAction::SendNow | QueueAction::RunIndependently => {
                let id = id.unwrap();
                updated.get_mut(&id).unwrap().metadata.independent_requested = true;
                updated.get_mut(&id).unwrap().metadata.independent_override =
                    params.action == QueueAction::RunIndependently;
                // Explicit send-now precedes internal wakeups too, matching Mac.
                let mut slots = updated
                    .values()
                    .filter(|turn| is_unclaimed(turn, session))
                    .map(|turn| (turn.metadata.queue_sequence, turn.metadata.id))
                    .collect::<Vec<_>>();
                slots.sort();
                let mut order = slots.iter().map(|(_, id)| *id).collect::<Vec<_>>();
                order.retain(|pending_id| *pending_id != id);
                order.insert(0, id);
                for ((sequence, _), pending_id) in slots.iter().zip(order) {
                    updated
                        .get_mut(&pending_id)
                        .unwrap()
                        .metadata
                        .queue_sequence = *sequence;
                }
            }
        }
        persist_turns(&self.turns_path, &updated)?;
        *turns = updated;
        Ok(
            if matches!(
                params.action,
                QueueAction::SendNow | QueueAction::RunIndependently
            ) {
                session.active_turn_id
            } else {
                None
            },
        )
    }
}

fn is_pending(turn: &StoredRuntimeTurn, session: &RuntimeSession) -> bool {
    is_unclaimed(turn, session)
        && turn.team_control.is_none()
        && !turn.origin_client.as_deref().is_some_and(|origin| {
            origin.starts_with(super::super::control_api::peer_sessions::WAKE_ORIGIN)
                || origin.starts_with(super::super::goal_resume::ORIGIN_PREFIX)
        })
}

fn is_unclaimed(turn: &StoredRuntimeTurn, session: &RuntimeSession) -> bool {
    turn.metadata.session_id == session.id
        && turn.metadata.status == RuntimeTurnStatus::Queued
        && session.active_turn_id != Some(turn.metadata.id)
}

pub(super) fn editable_text(prompt: &str) -> Option<&str> {
    if prompt
        .trim_start()
        .starts_with(willdeep_runtime_protocol::ATTACHED_CONTEXT_HEADER)
        && willdeep_core::conversation::user_authored_text(prompt).is_none()
    {
        return None;
    }
    Some(willdeep_core::conversation::visible_user_text(prompt))
}
