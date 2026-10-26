use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reservation {
    pub request_id: Uuid,
    pub node_id: Uuid,
    pub kind: GenerationKind,
    pub request_digest: String,
    pub quantities: Quantities,
    pub provider: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Reserved {
    Fresh,
    Existing(Box<GenerationReceipt>),
}

#[derive(Debug, Serialize)]
pub(super) struct Usage {
    pub limits: ResourceLimits,
    pub used: Quantities,
    pub reserved: Quantities,
    pub remaining: ResourceLimits,
    pub unknown_requests: Vec<Uuid>,
    pub historical_hold: Quantities,
}

// A crash may leave an async provider job in the plugin's local ledger just
// before its external ID reaches Runtime. Resume permits only that node to read
// the existing receipt. reserve() still forbids every new purchase while it is
// unresolved; absence of a recoverable provider ID remains a hard stop.
pub(super) fn can_reconcile(workflow: &ProductionWorkflow) -> Result<bool> {
    let uncertain: Vec<_> = usage(workflow)?
        .unknown_requests
        .into_iter()
        .filter(|id| {
            !workflow.generations.iter().any(|receipt| {
                receipt.request_id == *id && recovery::retained_unknown(workflow, receipt)
            })
        })
        .collect();
    if uncertain.is_empty() {
        return Ok(true);
    }
    let active: Vec<_> = workflow
        .nodes
        .iter()
        .filter(|node| {
            matches!(node.state, NodeState::Running | NodeState::WaitingExternal)
                && node.grant != Uuid::nil()
        })
        .collect();
    Ok(active.len() == 1
        && workflow
            .generations
            .iter()
            .filter(|receipt| uncertain.contains(&receipt.request_id))
            .all(|receipt| {
                receipt.state == GenerationState::Reserved
                    && matches!(receipt.kind, GenerationKind::Music | GenerationKind::Video)
                    && receipt.node_id == active[0].id
            }))
}

pub(super) fn limits(workflow: &ProductionWorkflow) -> &ResourceLimits {
    workflow
        .budget_changes
        .last()
        .map(|change| &change.limits)
        .unwrap_or(&workflow.input.limits)
}

fn add(target: &mut Quantities, values: &Quantities) -> Result<()> {
    for (unit, quantity) in values {
        let current = target.entry(*unit).or_default();
        *current = current
            .checked_add(*quantity)
            .context("production_quantity_overflow")?;
    }
    Ok(())
}

fn accounted(receipt: &GenerationReceipt) -> Quantities {
    let mut actual = receipt.actual.clone().unwrap_or_default();
    // A rejected model/video/music request is still an attempt. Provider metering is
    // preserved in actual, while the call cap cannot be bypassed with failed attempts.
    for unit in [
        ProductionUnit::ModelCalls,
        ProductionUnit::MusicCalls,
        ProductionUnit::VideoCalls,
    ] {
        if let Some(reserved) = receipt.reservation.get(&unit) {
            let value = actual.entry(unit).or_default();
            *value = (*value).max(*reserved);
        }
    }
    actual
}

fn episode_call_limit(
    workflow: &ProductionWorkflow,
    episode: Option<u16>,
    additional: u64,
) -> Result<()> {
    let (Some(episode), Some(limit)) = (episode, workflow.input.max_model_calls_per_episode) else {
        return Ok(());
    };
    let mut calls = additional
        .checked_add(historical::episode_calls(workflow, episode)?)
        .context("production_quantity_overflow")?;
    let episodes: std::collections::HashMap<_, _> = workflow
        .nodes
        .iter()
        .map(|node| (node.id, node.episode))
        .collect();
    for receipt in workflow
        .generations
        .iter()
        .filter(|receipt| receipt.kind == GenerationKind::Model)
    {
        let receipt_episode = episodes
            .get(&receipt.node_id)
            .context("production_receipt_node_missing")?;
        if *receipt_episode != Some(episode) {
            continue;
        }
        let quantities = if receipt.state == GenerationState::Settled {
            accounted(receipt)
        } else {
            receipt.reservation.clone()
        };
        calls = calls
            .checked_add(
                quantities
                    .get(&ProductionUnit::ModelCalls)
                    .copied()
                    .unwrap_or(0),
            )
            .context("production_quantity_overflow")?;
    }
    anyhow::ensure!(
        calls <= limit,
        "production_budget_limited: episode_{episode}_model_calls"
    );
    Ok(())
}

pub(super) fn usage(workflow: &ProductionWorkflow) -> Result<Usage> {
    let mut used = Quantities::new();
    let mut reserved = Quantities::new();
    let mut unknown_requests = Vec::new();
    for receipt in &workflow.generations {
        if receipt.state == GenerationState::Settled {
            anyhow::ensure!(receipt.actual.is_some(), "production_settlement_missing");
            add(&mut used, &accounted(receipt))?;
        } else {
            add(&mut reserved, &receipt.reservation)?;
            if matches!(
                receipt.state,
                GenerationState::Unknown | GenerationState::Reserved
            ) {
                unknown_requests.push(receipt.request_id);
            }
        }
    }
    let historical_hold = historical::quantities(workflow)?;
    add(&mut reserved, &historical_hold)?;
    let remaining = limits(workflow)
        .iter()
        .map(|(unit, limit)| {
            (
                *unit,
                limit.map(|limit| {
                    limit
                        .saturating_sub(used.get(unit).copied().unwrap_or(0))
                        .saturating_sub(reserved.get(unit).copied().unwrap_or(0))
                }),
            )
        })
        .collect();
    Ok(Usage {
        limits: limits(workflow).clone(),
        used,
        reserved,
        remaining,
        unknown_requests,
        historical_hold,
    })
}

pub(super) fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn quantities(kind: GenerationKind, values: &Quantities, reservation: bool) -> Result<()> {
    anyhow::ensure!(
        values.len() == kind.units().len()
            && kind.units().iter().all(|unit| values.contains_key(unit)),
        "production_wrong_resource_units"
    );
    anyhow::ensure!(
        !reservation || values.values().all(|quantity| *quantity > 0),
        "production_positive_reservation_required"
    );
    Ok(())
}

fn stage_allows(stage: ProductionStage, kind: GenerationKind) -> bool {
    use ProductionStage::*;
    match kind {
        GenerationKind::Image => matches!(stage, GenerateAssets | GenerateFrames),
        GenerationKind::Tts => stage == GenerateDialogue,
        GenerationKind::Music => stage == GenerateMusic,
        GenerationKind::Video => matches!(stage, GenerateVideo | RemediateVideo),
        GenerationKind::Model => matches!(
            stage,
            SeriesPlan
                | ReviewSeriesPlan
                | OutlineBatch
                | ReviewOutlineBatch
                | WriteEpisode
                | ReviewEpisode
                | ReviewScripts
                | ReviseEpisode
                | Storyboard
                | ReviewStoryboard
                | DesignAssets
                | ReviewAssets
                | ReviewFrames
                | ReviewDialogue
                | ReviewMusic
                | ReviewVideo
                | ReviewFilm
                | ReviewSeries
                | GenerateAssets
                | GenerateFrames
                | GenerateDialogue
                | GenerateMusic
                | GenerateVideo
                | RemediateVideo
        ),
    }
}

pub(super) fn reserve(
    workflow: &mut ProductionWorkflow,
    request: &Reservation,
) -> Result<Reserved> {
    anyhow::ensure!(
        is_digest(&request.request_digest),
        "production_request_digest_required"
    );
    anyhow::ensure!(
        !request.provider.trim().is_empty(),
        "production_provider_required"
    );
    quantities(request.kind, &request.quantities, true)?;
    if let Some(saved) = workflow
        .generations
        .iter()
        .find(|item| item.request_id == request.request_id)
    {
        anyhow::ensure!(
            saved.node_id == request.node_id
                && saved.kind == request.kind
                && saved.request_digest == request.request_digest
                && saved.reservation == request.quantities
                && saved.provider == request.provider,
            "production_request_id_conflict"
        );
        // Existing never means dispatch again, even after a timeout or restart.
        return Ok(Reserved::Existing(Box::new(saved.clone())));
    }
    anyhow::ensure!(
        workflow.status == ProductionStatus::Running && !workflow.cancel_requested,
        "production_not_running"
    );
    let node = workflow
        .nodes
        .iter()
        .find(|node| node.id == request.node_id)
        .context("production_node_missing")?;
    anyhow::ensure!(
        matches!(node.state, NodeState::Running | NodeState::WaitingExternal)
            && is_digest(&node.input_digest)
            && node.grant != Uuid::nil(),
        "production_node_not_authorized"
    );
    anyhow::ensure!(
        stage_allows(node.stage, request.kind),
        "production_generation_stage_denied"
    );
    let current = usage(workflow)?;
    anyhow::ensure!(
        current.unknown_requests.iter().all(|id| workflow
            .generations
            .iter()
            .any(|receipt| receipt.request_id == *id
                && recovery::retained_unknown(workflow, receipt))),
        "production_uncertain_generation"
    );
    for (unit, quantity) in &request.quantities {
        anyhow::ensure!(
            current
                .remaining
                .get(unit)
                .is_some_and(|remaining| remaining.is_none_or(|limit| *quantity <= limit)),
            "production_budget_limited: {unit:?}"
        );
    }
    if request.kind == GenerationKind::Model {
        episode_call_limit(
            workflow,
            node.episode,
            request.quantities[&ProductionUnit::ModelCalls],
        )?;
    }
    workflow.generations.push(GenerationReceipt {
        request_id: request.request_id,
        node_id: request.node_id,
        kind: request.kind,
        request_digest: request.request_digest.clone(),
        state: GenerationState::Reserved,
        reservation: request.quantities.clone(),
        actual: None,
        provider: request.provider.clone(),
        external_id: None,
        evidence_digest: None,
        succeeded: None,
    });
    Ok(Reserved::Fresh)
}

pub(super) fn submitted(
    workflow: &mut ProductionWorkflow,
    id: Uuid,
    external_id: &str,
) -> Result<()> {
    anyhow::ensure!(
        !external_id.trim().is_empty() && external_id.len() <= 512,
        "production_external_id_required"
    );
    let receipt = workflow
        .generations
        .iter_mut()
        .find(|item| item.request_id == id)
        .context("production_reservation_missing")?;
    if let Some(saved) = &receipt.external_id {
        anyhow::ensure!(saved == external_id, "production_external_id_conflict");
        return Ok(());
    }
    anyhow::ensure!(
        receipt.state == GenerationState::Reserved
            || (receipt.state == GenerationState::Unknown
                && matches!(receipt.kind, GenerationKind::Music | GenerationKind::Video)),
        "production_submission_state_conflict"
    );
    receipt.external_id = Some(external_id.into());
    receipt.state = GenerationState::Submitted;
    Ok(())
}

pub(super) fn unknown(workflow: &mut ProductionWorkflow, id: Uuid) -> Result<()> {
    let receipt = workflow
        .generations
        .iter_mut()
        .find(|item| item.request_id == id)
        .context("production_reservation_missing")?;
    anyhow::ensure!(
        receipt.state != GenerationState::Settled,
        "production_settlement_already_known"
    );
    receipt.state = GenerationState::Unknown;
    if !workflow.cancel_requested {
        workflow.status = ProductionStatus::WaitingUser;
    }
    workflow.error = Some("production_uncertain_generation".into());
    Ok(())
}

pub(super) fn settle(
    workflow: &mut ProductionWorkflow,
    id: Uuid,
    actual: Quantities,
    succeeded: bool,
    evidence_digest: String,
) -> Result<()> {
    anyhow::ensure!(
        is_digest(&evidence_digest),
        "production_evidence_digest_required"
    );
    let receipt = workflow
        .generations
        .iter_mut()
        .find(|item| item.request_id == id)
        .context("production_reservation_missing")?;
    quantities(receipt.kind, &actual, false)?;
    if receipt.state == GenerationState::Settled {
        anyhow::ensure!(
            receipt.actual.as_ref() == Some(&actual)
                && receipt.succeeded == Some(succeeded)
                && receipt.evidence_digest.as_ref() == Some(&evidence_digest),
            "production_settlement_conflict"
        );
        return Ok(());
    }
    receipt.actual = Some(actual);
    receipt.succeeded = Some(succeeded);
    receipt.evidence_digest = Some(evidence_digest);
    receipt.state = GenerationState::Settled;
    let model_node = (receipt.kind == GenerationKind::Model).then_some(receipt.node_id);
    let episode = if let Some(id) = model_node {
        workflow
            .nodes
            .iter()
            .find(|node| node.id == id)
            .context("production_receipt_node_missing")?
            .episode
    } else {
        None
    };
    // Late settlements are recorded after cancellation; exceeding an estimate is
    // recorded truthfully and blocks further spending, never clipped to the limit.
    let totals = usage(workflow)?;
    let exceeded = totals.limits.iter().any(|(unit, limit)| {
        limit.is_some_and(|limit| {
            totals.used.get(unit).copied().unwrap_or(0) > limit
                || totals.reserved.get(unit).copied().unwrap_or(0)
                    > limit.saturating_sub(totals.used.get(unit).copied().unwrap_or(0))
        })
    }) || episode_call_limit(workflow, episode, 0).is_err();
    if exceeded && !workflow.cancel_requested {
        workflow.status = ProductionStatus::BudgetLimited;
        workflow.error = Some("production_actual_usage_exceeds_budget".into());
    }
    Ok(())
}

pub(super) fn increase(
    workflow: &mut ProductionWorkflow,
    request_id: Uuid,
    at: u64,
    expected: &ResourceLimits,
    next: ResourceLimits,
) -> Result<()> {
    if let Some(saved) = workflow
        .budget_changes
        .iter()
        .find(|change| change.request_id == request_id)
    {
        anyhow::ensure!(
            &saved.previous == expected && saved.limits == next,
            "production_budget_request_conflict"
        );
        return Ok(());
    }
    anyhow::ensure!(
        !matches!(
            workflow.status,
            ProductionStatus::Completed | ProductionStatus::Cancelled
        ),
        "production_terminal_budget"
    );
    let current = limits(workflow);
    validate_limits(&next).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(current == expected, "production_budget_revision_conflict");
    anyhow::ensure!(
        next.len() == current.len()
            && current.iter().all(|(unit, limit)| {
                next.get(unit)
                    .is_some_and(|quantity| match (limit, quantity) {
                        (_, None) => true,
                        (Some(old), Some(new)) => new >= old,
                        (None, Some(_)) => false,
                    })
            })
            && &next != current,
        "production_explicit_budget_increase_required"
    );
    workflow.budget_changes.push(ProductionBudgetChange {
        request_id,
        at,
        previous: expected.clone(),
        limits: next,
    });
    if workflow.status == ProductionStatus::BudgetLimited {
        workflow.status = ProductionStatus::WaitingUser;
    }
    Ok(())
}
