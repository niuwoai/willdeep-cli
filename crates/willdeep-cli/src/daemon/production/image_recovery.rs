use super::*;
use serde::Deserialize;

const EVENT: &str = "unknown_image_retry_authorized";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Authorization {
    #[serde(rename = "requestID")]
    request_id: Uuid,
    subject: String,
}

fn generation_stage(stage: ProductionStage) -> bool {
    matches!(
        stage,
        ProductionStage::GenerateAssets | ProductionStage::GenerateFrames
    )
}

fn event<'a>(workflow: &'a ProductionWorkflow, receipt: &GenerationReceipt) -> Option<&'a Value> {
    if receipt.kind != GenerationKind::Image
        || receipt.state != GenerationState::Unknown
        || receipt.actual.is_some()
        || receipt.external_id.is_some()
        || receipt.evidence_digest.is_some()
        || receipt.succeeded.is_some()
    {
        return None;
    }
    let subject = image_retries::subject_for(workflow, receipt.request_id).ok()?;
    let node = workflow
        .nodes
        .iter()
        .find(|node| node.id == receipt.node_id && generation_stage(node.stage))?;
    workflow.control_history.iter().rev().find(|entry| {
        entry["trigger"] == EVENT
            && entry["nodeID"] == json!(receipt.node_id)
            && entry["subject"] == subject
            && entry["attempt"]
                .as_u64()
                .is_some_and(|attempt| attempt > 0 && attempt <= u64::from(node.attempt))
            && serde_json::from_value::<GenerationReceipt>(entry["preservedUnknownReceipt"].clone())
                .is_ok_and(|saved| saved == *receipt)
    })
}

pub(super) fn retained_unknown(workflow: &ProductionWorkflow, receipt: &GenerationReceipt) -> bool {
    event(workflow, receipt).is_some()
}

pub(super) fn authorize(
    workflow: &mut ProductionWorkflow,
    value: &Value,
    authorization: Uuid,
    at: u64,
) -> Result<()> {
    let requests: Vec<Authorization> = serde_json::from_value(value.clone())?;
    anyhow::ensure!(
        workflow.status == ProductionStatus::WaitingUser
            && !workflow.cancel_requested
            && image_retries::enabled(workflow)
            && requests.len() == 1
            && authorization != Uuid::nil(),
        "production_unknown_image_retry_not_allowed"
    );
    let request = &requests[0];
    let active: Vec<_> = workflow
        .nodes
        .iter()
        .filter(|node| matches!(node.state, NodeState::Running | NodeState::WaitingExternal))
        .collect();
    anyhow::ensure!(
        active.len() == 1,
        "production_unknown_image_retry_node_required"
    );
    let node = active[0];
    anyhow::ensure!(
        generation_stage(node.stage)
            && node.attempt > 0
            && node.attempt < image_retries::node_limit(workflow, node.stage),
        "production_unknown_image_retry_stage_denied"
    );
    let unresolved: Vec<_> = workflow
        .generations
        .iter()
        .filter(|receipt| {
            receipt.state != GenerationState::Settled
                && !recovery::retained_unknown(workflow, receipt)
        })
        .collect();
    anyhow::ensure!(
        unresolved.len() == 1,
        "production_unknown_image_retry_receipt_mismatch"
    );
    let receipt = unresolved[0];
    anyhow::ensure!(
        receipt.request_id == request.request_id
            && receipt.node_id == node.id
            && receipt.kind == GenerationKind::Image
            && receipt.state == GenerationState::Unknown
            && receipt.actual.is_none()
            && receipt.external_id.is_none()
            && receipt.evidence_digest.is_none()
            && receipt.succeeded.is_none()
            && receipt.reservation.len() == 1
            && receipt.reservation.get(&ProductionUnit::Images) == Some(&1)
            && image_retries::subject_for(workflow, receipt.request_id)? == request.subject,
        "production_unknown_image_retry_receipt_mismatch"
    );
    anyhow::ensure!(
        !workflow
            .control_history
            .iter()
            .any(|entry| entry["trigger"] == EVENT
                && entry["preservedUnknownReceipt"]["request_id"] == json!(request.request_id)),
        "production_unknown_image_retry_already_authorized"
    );
    let counts = image_retries::status(workflow, &request.subject)?;
    anyhow::ensure!(
        counts["attempts"]
            .as_u64()
            .is_some_and(|count| count < image_retries::MATERIAL_LIMIT as u64),
        "production_image_material_limit"
    );
    let history = json!({"trigger":EVENT,"authorizationRequestID":authorization,"at":at,
        "nodeID":node.id,"subject":request.subject,"attempt":node.attempt,"stage":node.stage,
        "preservedUnknownReceipt":receipt,"preservedReservation":true,"replacementRequestID":null});
    let node_id = node.id;
    let node = workflow
        .nodes
        .iter_mut()
        .find(|node| node.id == node_id)
        .unwrap();
    node.history.push(json!({"trigger":EVENT,"authorizationRequestID":authorization,"at":at,
        "attempt":node.attempt,"stage":node.stage,"inputDigest":node.input_digest,"result":node.result}));
    node.state = NodeState::Pending;
    node.grant = Uuid::nil();
    node.arguments = json!({});
    node.result = None;
    workflow.control_history.push(history);
    Ok(())
}

// The same Store transaction binds this single replacement and reserves its cost.
// A successful or failed replacement consumes the permit; another unknown is not covered.
pub(super) fn bind_replacement(
    workflow: &mut ProductionWorkflow,
    request: &budget::Reservation,
    subject: &str,
) -> Result<()> {
    let index = workflow.control_history.iter().rposition(|entry| {
        entry["trigger"] == EVENT
            && entry["subject"] == subject
            && workflow.generations.iter().any(|receipt| {
                event(workflow, receipt).is_some_and(|saved| std::ptr::eq(saved, entry))
            })
    });
    let Some(index) = index else {
        return Ok(());
    };
    let permit = &workflow.control_history[index];
    anyhow::ensure!(
        permit["nodeID"] == json!(request.node_id),
        "production_unknown_image_retry_node_mismatch"
    );
    anyhow::ensure!(
        permit["replacementRequestID"].is_null()
            || permit["replacementRequestID"] == json!(request.request_id),
        "production_unknown_image_retry_consumed"
    );
    workflow.control_history[index]["replacementRequestID"] = json!(request.request_id);
    Ok(())
}
