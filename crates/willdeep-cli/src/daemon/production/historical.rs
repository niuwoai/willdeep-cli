//! Conservative capacity held after a supplier audit of legacy transport replay.
//! This is never a fabricated generation receipt or an assertion of actual cost.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const TRIGGER: &str = "historical_usage_reservation";

pub(super) fn receipts_digest(receipts: &[GenerationReceipt]) -> Result<String> {
    let values: Vec<_> = receipts.iter().map(|receipt| json!({
        "request_id":receipt.request_id,"node_id":receipt.node_id,"kind":receipt.kind,
        "request_digest":receipt.request_digest,"reservation":receipt.reservation,"provider":receipt.provider
    })).collect();
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&values)?)
    ))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Hold {
    pub tokens: u64,
    pub model_calls: u64,
    pub episode_calls: BTreeMap<u16, u64>,
    pub evidence_digest: String,
    pub generation_count: usize,
    pub generations_digest: String,
}

fn validate(workflow: &ProductionWorkflow, hold: &Hold) -> Result<()> {
    anyhow::ensure!(
        hold.tokens > 0
            && hold.model_calls > 0
            && budget::is_digest(&hold.evidence_digest)
            && budget::is_digest(&hold.generations_digest)
            && hold.generation_count > 0
            && hold.generation_count <= workflow.generations.len(),
        "production_historical_hold_invalid"
    );
    let digest = receipts_digest(&workflow.generations[..hold.generation_count])?;
    anyhow::ensure!(
        digest == hold.generations_digest,
        "production_historical_receipts_changed"
    );
    let mut episode_total = 0u64;
    for (episode, calls) in &hold.episode_calls {
        anyhow::ensure!(
            *episode > 0 && *episode <= workflow.input.spec.episode_count && *calls > 0,
            "production_historical_episode_invalid"
        );
        episode_total = episode_total
            .checked_add(*calls)
            .context("production_quantity_overflow")?;
    }
    anyhow::ensure!(
        episode_total <= hold.model_calls,
        "production_historical_calls_invalid"
    );
    Ok(())
}

pub(super) fn current(workflow: &ProductionWorkflow) -> Result<Option<Hold>> {
    let entries: Vec<_> = workflow
        .control_history
        .iter()
        .filter(|entry| entry["trigger"] == TRIGGER)
        .collect();
    anyhow::ensure!(entries.len() <= 1, "production_historical_hold_conflict");
    let Some(entry) = entries.first() else {
        return Ok(None);
    };
    let hold: Hold = serde_json::from_value(entry["hold"].clone())?;
    validate(workflow, &hold)?;
    Ok(Some(hold))
}

pub(super) fn apply(
    workflow: &mut ProductionWorkflow,
    hold: Hold,
    request: Uuid,
    at: u64,
) -> Result<()> {
    if let Some(existing) = current(workflow)? {
        anyhow::ensure!(existing == hold, "production_historical_hold_immutable");
        return Ok(());
    }
    anyhow::ensure!(
        hold.generation_count == workflow.generations.len(),
        "production_historical_snapshot_stale"
    );
    validate(workflow, &hold)?;
    let mut candidate = workflow.clone();
    candidate
        .control_history
        .push(json!({"trigger":TRIGGER,"requestID":request,"at":at,"hold":hold}));
    let usage = budget::usage(&candidate)?;
    for (unit, limit) in usage.limits {
        if let Some(limit) = limit {
            let total = usage
                .used
                .get(&unit)
                .copied()
                .unwrap_or(0)
                .checked_add(usage.reserved.get(&unit).copied().unwrap_or(0))
                .context("production_quantity_overflow")?;
            anyhow::ensure!(total <= limit, "production_historical_hold_exceeds_budget");
        }
    }
    *workflow = candidate;
    Ok(())
}

pub(super) fn snapshot(workflow: &ProductionWorkflow) -> Result<Value> {
    Ok(
        json!({"generationCount":workflow.generations.len(),"generationsDigest":receipts_digest(&workflow.generations)?}),
    )
}

pub(super) fn quantities(workflow: &ProductionWorkflow) -> Result<Quantities> {
    Ok(current(workflow)?
        .map(|hold| {
            [
                (ProductionUnit::Tokens, hold.tokens),
                (ProductionUnit::ModelCalls, hold.model_calls),
            ]
            .into_iter()
            .collect()
        })
        .unwrap_or_default())
}

pub(super) fn episode_calls(workflow: &ProductionWorkflow, episode: u16) -> Result<u64> {
    Ok(current(workflow)?
        .and_then(|hold| hold.episode_calls.get(&episode).copied())
        .unwrap_or(0))
}
