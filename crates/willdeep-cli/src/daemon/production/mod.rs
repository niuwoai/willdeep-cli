//! Persistent full-film production. Independent from legacy screenplay archives.
pub(crate) mod accounting;
mod budget;
mod engine;
mod graph;
mod historical;
mod host;
mod image_recovery;
mod image_retries;
mod recovery;
mod scheduling;
mod store;
#[cfg(test)]
mod tests;
mod validation;

use anyhow::{Context, Result};
use graph::initial_nodes;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use store::Store;
use uuid::Uuid;
use willdeep_runtime_protocol::production::*;

const PLUGIN: &str = "willdeep-video-studio";

pub(super) async fn operation(
    home: &Path,
    request_id: Uuid,
    operation: &str,
    params: Value,
) -> Result<Value> {
    let store = Store::new(home);
    if operation == "production.list" {
        return store
            .list()?
            .into_iter()
            .map(summary)
            .collect::<Result<Vec<_>>>()
            .map(|items| json!(items));
    }
    if operation == "production.start" {
        let input: ProductionStart = serde_json::from_value(params)?;
        let mut workflow = store.create(request_id, input)?;
        if workflow.generations.is_empty() && !image_retries::enabled(&workflow) {
            store.update(workflow.id, |saved| {
                image_retries::activate(
                    saved,
                    &store,
                    &json!({"mode":image_retries::POLICY,"legacyBindings":[]}),
                    request_id,
                    super::now(),
                )
            })?;
            workflow = store.get(workflow.id)?;
        }
        if workflow.status == ProductionStatus::Running {
            engine::launch(home.to_path_buf(), workflow.id);
        }
        return public(workflow);
    }
    let id = serde_json::from_value::<Uuid>(params["id"].clone())?;
    match operation {
        "production.status" => {}
        "production.pause" | "production.music" => {
            let deferred = params
                .get("deferred")
                .map(|value| serde_json::from_value(value.clone()))
                .transpose()?;
            store.update(id, |workflow| {
                scheduling::control(
                    workflow,
                    operation.trim_start_matches("production."),
                    deferred,
                    request_id,
                    super::now(),
                )
            })?;
        }
        "production.cancel" => {
            store.update(id, |workflow| {
                anyhow::ensure!(
                    workflow.status != ProductionStatus::Completed,
                    "production_already_completed"
                );
                workflow.cancel_requested = true;
                workflow.status = ProductionStatus::Cancelled;
                Ok(())
            })?;
            engine::launch(home.to_path_buf(), id);
        }
        "production.budget" => {
            let expected = serde_json::from_value(params["expected"].clone())?;
            let limits = serde_json::from_value(params["limits"].clone())?;
            store.update(id, |workflow| {
                budget::increase(workflow, request_id, super::now(), &expected, limits)
            })?;
        }
        "production.resume" => {
            let lease = store.lock(id, true)?;
            host::reconcile(home, id)?;
            store.update(id, |workflow| {
                anyhow::ensure!(
                    !workflow.cancel_requested
                        && !matches!(
                            workflow.status,
                            ProductionStatus::Running
                                | ProductionStatus::Completed
                                | ProductionStatus::Cancelled
                        ),
                    "production_cannot_resume"
                );
                if let Some(hold) = params.get("historicalUsageHold") {
                    historical::apply(
                        workflow,
                        serde_json::from_value(hold.clone())?,
                        request_id,
                        super::now(),
                    )?;
                }
                if let Some(requests) = params.get("retryUnknownTextRequests") {
                    let requests: Vec<Uuid> = serde_json::from_value(requests.clone())?;
                    recovery::retry_unknown_text(workflow, &requests, request_id, super::now())?;
                }
                if let Some(requests) = params.get("retryUnknownImageRequests") {
                    image_recovery::authorize(workflow, requests, request_id, super::now())?;
                }
                if let Some(policy) = params.get("imageRetryPolicy") {
                    image_retries::activate(workflow, &store, policy, request_id, super::now())?;
                }
                anyhow::ensure!(
                    budget::can_reconcile(workflow)?,
                    "production_uncertain_generation"
                );
                workflow.status = ProductionStatus::Running;
                workflow.pause_requested = false;
                workflow.error = None;
                Ok(())
            })?;
            drop(lease);
            engine::launch(home.to_path_buf(), id);
        }
        _ => anyhow::bail!("production_operation_unknown"),
    }
    let workflow = store.get(id)?;
    if params["detail"] == false {
        summary(workflow)
    } else {
        public(workflow)
    }
}

pub(super) fn recover(home: &Path) {
    match Store::new(home).list() {
        Ok(workflows) => {
            for workflow in workflows {
                if workflow.status == ProductionStatus::Running
                    || host::needs_reconciliation(&workflow)
                {
                    engine::launch(home.to_path_buf(), workflow.id);
                }
            }
        }
        Err(error) => eprintln!(
            "production recovery refused: {}",
            willdeep_core::judge::redact_credentials(&format!("{error:#}"))
        ),
    }
}

fn public(workflow: ProductionWorkflow) -> Result<Value> {
    let usage = budget::usage(&workflow)?;
    let historical_snapshot = historical::snapshot(&workflow)?;
    let nodes: Vec<_> = workflow
        .nodes
        .iter()
        .map(|node| {
            json!({
                "id":node.id, "stage":node.stage, "episode":node.episode, "range":node.range,
                "subject":node.subject, "state":node.state, "attempt":node.attempt,
                "inputDigest":node.input_digest, "result":node.result,
                "historyCount":node.history.len()
            })
        })
        .collect();
    Ok(
        json!({"id":workflow.id, "input":workflow.input, "status":workflow.status,
        "createdAt":workflow.created_at, "updatedAt":workflow.updated_at, "nodes":nodes,
        "usage":usage, "historicalSnapshot":historical_snapshot, "budgetChanges":workflow.budget_changes, "generations":workflow.generations,
        "domain":workflow.domain, "error":workflow.error, "cancelRequested":workflow.cancel_requested,
        "pauseRequested":workflow.pause_requested,"musicDeferred":workflow.music_deferred,"controlHistory":workflow.control_history}),
    )
}

fn summary(workflow: ProductionWorkflow) -> Result<Value> {
    let usage = budget::usage(&workflow)?;
    let historical_snapshot = historical::snapshot(&workflow)?;
    let episodes: Vec<_> = (1..=workflow.input.spec.episode_count)
        .map(|order| {
            let nodes: Vec<_> = workflow
                .nodes
                .iter()
                .filter(|node| node.episode == Some(order))
                .collect();
            let stages: Vec<_> = nodes
                .iter()
                .map(|node| {
                    json!({"id":node.id,"stage":node.stage,
            "state":node.state,"attempt":node.attempt,"historyCount":node.history.len()})
                })
                .collect();
            json!({"order":order,"stages":stages,"completed":nodes.iter().any(|node|
            node.stage == ProductionStage::ReviewFilm && node.state == NodeState::Completed)})
        })
        .collect();
    Ok(
        json!({"id":workflow.id,"input":workflow.input,"status":workflow.status,
        "createdAt":workflow.created_at,"updatedAt":workflow.updated_at,"usage":usage,"historicalSnapshot":historical_snapshot,
        "completedNodes":workflow.nodes.iter().filter(|node|node.state == NodeState::Completed).count(),
        "totalNodes":workflow.nodes.len(),"episodes":episodes,
        "active":workflow.nodes.iter().find(|node|matches!(node.state,NodeState::Running|NodeState::WaitingExternal))
            .map(|node|json!({"id":node.id,"stage":node.stage,"episode":node.episode,"range":node.range})),
        "domain":workflow.domain,"error":workflow.error,"cancelRequested":workflow.cancel_requested,
        "pauseRequested":workflow.pause_requested,"musicDeferred":workflow.music_deferred}),
    )
}
