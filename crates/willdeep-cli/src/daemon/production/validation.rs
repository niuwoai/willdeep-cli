use super::*;

// A settled, invalid text response may be regenerated within the existing grant
// and attempt limits. Transport interruptions and media outcomes must be replayed.
pub(super) fn retry(
    workflow: &mut ProductionWorkflow,
    node_id: Uuid,
    result: &Value,
) -> Result<bool> {
    if workflow.cancel_requested
        || workflow.status != ProductionStatus::Running
        || result["error"]["code"] != "production_step_invalid"
        || !result["error"]["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("production_model_validation_failed:"))
    {
        return Ok(false);
    }
    let node = workflow
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .context("production_validation_node_missing")?;
    if node.attempt >= engine::node_attempt_limit(node.stage) || !text_stage(node.stage) {
        return Ok(false);
    }
    anyhow::ensure!(
        node.state == NodeState::Running,
        "production_validation_node_not_running"
    );
    anyhow::ensure!(
        workflow
            .generations
            .iter()
            .filter(|receipt| receipt.node_id == node_id)
            .all(|receipt| receipt.state == GenerationState::Settled
                || recovery::retained_unknown(workflow, receipt)),
        "production_validation_has_unsettled_request"
    );
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == node_id)
        .unwrap();
    node.history.push(
        json!({"attempt":node.attempt,"inputDigest":node.input_digest,
        "result":result,"trigger":"model_validation","preservedReceipts":true}),
    );
    node.state = NodeState::Pending;
    node.result = None;
    node.grant = Uuid::nil();
    node.arguments = json!({});
    Ok(true)
}

pub(super) fn text_stage(stage: ProductionStage) -> bool {
    use ProductionStage::*;
    matches!(
        stage,
        SeriesPlan
            | ReviewSeriesPlan
            | OutlineBatch
            | ReviewOutlineBatch
            | WriteEpisode
            | ReviewEpisode
            | ReviewScripts
            | Storyboard
            | ReviewStoryboard
            | DesignAssets
    )
}
