use super::*;
use std::collections::HashSet;

pub(super) const POLICY: &str = "per_material_v1";
pub(super) const MATERIAL_LIMIT: usize = 4;
// Scheduling is bounded separately from actual paid requests per material.
pub(super) const BATCH_LIMIT: u16 = 1_536;
const POLICY_EVENT: &str = "production_image_policy_v1";
const SUBJECT_EVENT: &str = "production_image_subject_v1";

pub(super) fn image_stage(stage: ProductionStage) -> bool {
    matches!(
        stage,
        ProductionStage::GenerateAssets
            | ProductionStage::ReviewAssets
            | ProductionStage::GenerateFrames
            | ProductionStage::ReviewFrames
    )
}

pub(super) fn enabled(workflow: &ProductionWorkflow) -> bool {
    workflow
        .control_history
        .iter()
        .any(|entry| entry["trigger"] == POLICY_EVENT)
}

pub(super) fn node_limit(workflow: &ProductionWorkflow, stage: ProductionStage) -> u16 {
    if enabled(workflow) && image_stage(stage) {
        BATCH_LIMIT
    } else {
        engine::node_attempt_limit(stage)
    }
}

pub(super) fn activate(
    workflow: &mut ProductionWorkflow,
    store: &Store,
    value: &Value,
    authorization: Uuid,
    at: u64,
) -> Result<()> {
    anyhow::ensure!(value["mode"] == POLICY, "production_image_policy_invalid");
    let bindings = value["legacyBindings"]
        .as_array()
        .context("production_image_bindings_required")?;
    if let Some(previous) = workflow
        .control_history
        .iter()
        .find(|entry| entry["trigger"] == POLICY_EVENT)
    {
        anyhow::ensure!(
            previous["legacyBindings"] == value["legacyBindings"],
            "production_image_policy_cannot_reset"
        );
        return Ok(());
    }
    let mut seen = HashSet::new();
    for binding in bindings {
        let id: Uuid = serde_json::from_value(binding["requestID"].clone())?;
        let subject = binding["subject"]
            .as_str()
            .context("production_image_subject_required")?;
        anyhow::ensure!(
            budget::is_digest(subject) && seen.insert(id),
            "production_image_binding_invalid"
        );
        let receipt = workflow
            .generations
            .iter()
            .find(|receipt| receipt.request_id == id && receipt.kind == GenerationKind::Image)
            .context("production_image_binding_receipt_missing")?;
        anyhow::ensure!(
            receipt.state == GenerationState::Settled,
            "production_uncertain_generation"
        );
        let saved = store
            .receipt(workflow.id, id)?
            .context("production_saved_response_missing")?;
        anyhow::ensure!(
            receipt.evidence_digest.as_ref() == Some(&engine::digest(&saved)?),
            "production_receipt_digest_mismatch"
        );
        let file = saved["response"]["filePath"]
            .as_str()
            .and_then(|path| Path::new(path).file_name())
            .and_then(|name| name.to_str());
        anyhow::ensure!(
            file.is_some() && binding["fileName"].as_str() == file,
            "production_image_binding_file_mismatch"
        );
    }
    anyhow::ensure!(
        workflow
            .generations
            .iter()
            .filter(|receipt| receipt.kind == GenerationKind::Image)
            .all(|receipt| seen.contains(&receipt.request_id)),
        "production_image_bindings_incomplete"
    );
    workflow.control_history.push(
        json!({"trigger":POLICY_EVENT,"at":at,"authorizationRequestID":authorization,
        "maxGenerationsPerMaterial":MATERIAL_LIMIT,"legacyBindings":bindings}),
    );
    Ok(())
}

pub(super) fn subject_for(workflow: &ProductionWorkflow, id: Uuid) -> Result<&str> {
    for event in &workflow.control_history {
        if event["trigger"] == SUBJECT_EVENT && event["requestID"] == json!(id) {
            return event["subject"]
                .as_str()
                .context("production_image_subject_invalid");
        }
        if event["trigger"] == POLICY_EVENT
            && let Some(binding) = event["legacyBindings"].as_array().and_then(|bindings| {
                bindings
                    .iter()
                    .find(|binding| binding["requestID"] == json!(id))
            })
        {
            return binding["subject"]
                .as_str()
                .context("production_image_subject_invalid");
        }
    }
    anyhow::bail!("production_image_subject_unmapped")
}

pub(super) fn status(workflow: &ProductionWorkflow, subject: &str) -> Result<Value> {
    anyhow::ensure!(
        enabled(workflow) && budget::is_digest(subject),
        "production_image_policy_required"
    );
    let mut attempts = 0;
    let mut unresolved = false;
    for receipt in workflow
        .generations
        .iter()
        .filter(|receipt| receipt.kind == GenerationKind::Image)
    {
        let mapped = subject_for(workflow, receipt.request_id)?;
        if mapped == subject {
            attempts += 1;
            unresolved |= receipt.state != GenerationState::Settled
                && !image_recovery::retained_unknown(workflow, receipt);
        }
    }
    Ok(json!({"ok":true,"limit":MATERIAL_LIMIT,"attempts":attempts,"unresolved":unresolved}))
}

// Called under the same Store transaction as budget::reserve. A failed reserve
// persists neither a new receipt nor a subject event; replays do not add counts.
pub(super) fn bind_reservation(
    workflow: &mut ProductionWorkflow,
    request: &budget::Reservation,
    subject: Option<&str>,
) -> Result<()> {
    if request.kind != GenerationKind::Image || !enabled(workflow) {
        return Ok(());
    }
    let subject = subject.context("production_image_subject_required")?;
    anyhow::ensure!(
        budget::is_digest(subject),
        "production_image_subject_invalid"
    );
    if workflow
        .generations
        .iter()
        .any(|receipt| receipt.request_id == request.request_id)
    {
        anyhow::ensure!(
            subject_for(workflow, request.request_id)? == subject,
            "production_image_subject_conflict"
        );
        return Ok(());
    }
    let current = status(workflow, subject)?;
    anyhow::ensure!(
        current["unresolved"] == false,
        "production_uncertain_generation"
    );
    anyhow::ensure!(
        current["attempts"].as_u64().unwrap_or(u64::MAX) < MATERIAL_LIMIT as u64,
        "production_image_material_limit"
    );
    image_recovery::bind_replacement(workflow, request, subject)?;
    workflow
        .control_history
        .push(json!({"trigger":SUBJECT_EVENT,"requestID":request.request_id,"subject":subject}));
    Ok(())
}
