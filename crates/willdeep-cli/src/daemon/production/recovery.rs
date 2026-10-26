use super::*;

fn synchronous_episode_text(stage: ProductionStage) -> bool {
    matches!(
        stage,
        ProductionStage::WriteEpisode | ProductionStage::ReviewEpisode
    )
}

// Only an explicit, receipt-scoped human authorization may abandon the result
// of an unknown synchronous text call. Its entire reservation remains charged
// against the available budget; this is never a settlement or a replay grant.
pub(super) fn retained_unknown(workflow: &ProductionWorkflow, receipt: &GenerationReceipt) -> bool {
    if receipt.state != GenerationState::Unknown
        || receipt.kind != GenerationKind::Model
        || receipt.external_id.is_some()
        || receipt.actual.is_some()
    {
        return false;
    }
    workflow.nodes.iter().any(|node| {
        node.id == receipt.node_id
            && synchronous_episode_text(node.stage)
            && node.history.iter().any(|entry| {
                entry["trigger"] == "unknown_text_retry_authorized"
                    && (entry["stage"] == json!(node.stage)
                        || (node.stage == ProductionStage::WriteEpisode
                            && entry.get("stage").is_none()))
                    && entry["attempt"]
                        .as_u64()
                        .is_some_and(|attempt| attempt > 0 && attempt <= u64::from(node.attempt))
                    && entry["preservedUnknownReceipts"]
                        .as_array()
                        .is_some_and(|receipts| {
                            receipts.iter().any(|saved| {
                                serde_json::from_value::<GenerationReceipt>(saved.clone())
                                    .is_ok_and(|saved| saved == *receipt)
                            })
                        })
            })
    })
}

pub(super) fn retry_unknown_text(
    workflow: &mut ProductionWorkflow,
    requests: &[Uuid],
    authorization: Uuid,
    at: u64,
) -> Result<()> {
    anyhow::ensure!(
        workflow.status == ProductionStatus::WaitingUser
            && !workflow.cancel_requested
            && !requests.is_empty()
            && requests.len() <= 8,
        "production_unknown_text_retry_not_allowed"
    );
    let active: Vec<_> = workflow
        .nodes
        .iter()
        .filter(|node| matches!(node.state, NodeState::Running | NodeState::WaitingExternal))
        .collect();
    anyhow::ensure!(
        active.len() == 1,
        "production_unknown_text_retry_node_required"
    );
    let node = active[0];
    anyhow::ensure!(
        synchronous_episode_text(node.stage)
            && node.attempt > 0
            && node.attempt < engine::node_attempt_limit(node.stage),
        "production_unknown_text_retry_stage_denied"
    );
    let unresolved: Vec<_> = workflow
        .generations
        .iter()
        .filter(|receipt| {
            receipt.state != GenerationState::Settled && !retained_unknown(workflow, receipt)
        })
        .collect();
    anyhow::ensure!(
        unresolved.len() == requests.len()
            && requests
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == requests.len()
            && unresolved.iter().all(|receipt| {
                requests.contains(&receipt.request_id)
                    && receipt.node_id == node.id
                    && receipt.kind == GenerationKind::Model
                    && receipt.state == GenerationState::Unknown
                    && receipt.actual.is_none()
                    && receipt.external_id.is_none()
            }),
        "production_unknown_text_retry_receipt_mismatch"
    );
    let history = json!({"trigger":"unknown_text_retry_authorized",
        "authorizationRequestID":authorization,"at":at,"attempt":node.attempt,"stage":node.stage,
        "inputDigest":node.input_digest,"result":node.result,
        "preservedUnknownReceipts":unresolved,"preservedReservation":true});
    let id = node.id;
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == id)
        .unwrap();
    node.history.push(history);
    node.state = NodeState::Pending;
    node.grant = Uuid::nil();
    node.arguments = json!({});
    node.result = None;
    Ok(())
}
