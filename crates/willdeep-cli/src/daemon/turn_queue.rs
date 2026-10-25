use super::*;
use willdeep_runtime_protocol::{ListTurnsParams, QueuedTurnMessage, UpdateTurnQueueParams};

pub(crate) async fn present(home: &Path, id: uuid::Uuid) -> Result<()> {
    let state = load_state(&DaemonPaths::new(home).state).context("Runtime is not running")?;
    runtime_client(&state)?
        .call::<_, serde_json::Value>(
            "interaction.present",
            &willdeep_runtime_protocol::IdParams { id },
            Some(uuid::Uuid::new_v4()),
        )
        .await?
        .into_result()
        .map_err(|error| anyhow::anyhow!("Runtime interaction: {}", error.message))?;
    Ok(())
}

/// Display projection only; this copy must never be saved as model history.
pub(crate) fn display_session(
    home: &Path,
    session: &willdeep_core::Session,
) -> Result<willdeep_core::Session> {
    let mut display = session.clone();
    let histories = session_store::RuntimeSessionStore::independent_history(home, session.id)?;
    let mut inserted = 0;
    for history in histories {
        let index = if history.display_generation == session.compression_generation {
            history
                .display_after
                .saturating_add(inserted)
                .min(display.messages.len())
        } else {
            display.messages.len()
        };
        inserted += history.messages.len();
        display.messages.splice(index..index, history.messages);
    }
    Ok(display)
}

pub(crate) fn independent_history(
    home: &Path,
    id: uuid::Uuid,
) -> Result<Vec<session_store::IndependentHistory>> {
    session_store::RuntimeSessionStore::independent_history(home, id)
}

/// Queue reads must never start a Runtime as a side effect.
pub(crate) async fn list(home: &Path, session_id: uuid::Uuid) -> Result<Vec<QueuedTurnMessage>> {
    let state = load_state(&DaemonPaths::new(home).state).context("Runtime is not running")?;
    runtime_client(&state)?
        .call("turn.queue.list", &ListTurnsParams { session_id }, None)
        .await?
        .into_result()
        .map_err(|error| anyhow::anyhow!("Runtime queue: {}", error.message))
}

pub(crate) async fn update(
    home: &Path,
    params: &UpdateTurnQueueParams,
) -> Result<Vec<QueuedTurnMessage>> {
    let state = load_state(&DaemonPaths::new(home).state).context("Runtime is not running")?;
    runtime_client(&state)?
        .call("turn.queue.update", params, Some(uuid::Uuid::new_v4()))
        .await?
        .into_result()
        .map_err(|error| anyhow::anyhow!("Runtime queue: {}", error.message))
}
