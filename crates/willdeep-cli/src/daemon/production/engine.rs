use super::*;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, sync::Arc};
use willdeep_core::plugin::PluginHost;

pub(super) const MAX_NODE_ATTEMPTS: u16 = 4;
const MAX_TEXT_NODE_ATTEMPTS: u16 = 8;
const MAX_EPISODE_TEXT_ATTEMPTS: u16 = 24;
const POLL_SECONDS: u64 = 3;

pub(super) fn node_attempt_limit(stage: ProductionStage) -> u16 {
    if matches!(
        stage,
        ProductionStage::WriteEpisode | ProductionStage::ReviewEpisode
    ) {
        MAX_EPISODE_TEXT_ATTEMPTS
    } else if validation::text_stage(stage) {
        MAX_TEXT_NODE_ATTEMPTS
    } else {
        MAX_NODE_ATTEMPTS
    }
}

pub(super) fn digest(value: &Value) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

pub(super) fn launch(home: PathBuf, id: Uuid) {
    tokio::spawn(async move {
        let store = Store::new(&home);
        let Ok(_lease) = store.lock(id, true) else {
            return;
        };
        let result = run(&home, id).await;
        // Cancellation can revoke the plugin's commit grant while a paid request
        // is still returning. Re-enter only the settlement branch, under our lease.
        let result = if result.is_err()
            && store
                .get(id)
                .is_ok_and(|workflow| workflow.cancel_requested)
        {
            run(&home, id).await
        } else {
            result
        };
        if let Err(error) = result {
            let message = willdeep_core::judge::redact_credentials(&format!("{error:#}"));
            let saved = store.update(id, |workflow| {
                if workflow.cancel_requested {
                    workflow.status = ProductionStatus::Cancelled;
                } else if message.contains("production_budget_limited") {
                    workflow.status = ProductionStatus::BudgetLimited;
                } else if message.contains("production_adapter_missing")
                    || message.contains("production_dependencies_unavailable")
                    || message.contains("production_music_deferred")
                {
                    workflow.status = ProductionStatus::WaitingDependency;
                } else {
                    workflow.status = ProductionStatus::WaitingUser;
                }
                workflow.error = Some(message.clone());
                Ok(())
            });
            if let Err(error) = saved {
                eprintln!(
                    "persist production failure: {}",
                    willdeep_core::judge::redact_credentials(&format!("{error:#}"))
                );
            }
        }
    });
}

async fn run(home: &Path, id: Uuid) -> Result<()> {
    host::reconcile(home, id)?;
    let workflow = Store::new(home).get(id)?;
    if workflow.cancel_requested && host::cancelled_node(&workflow)?.is_none() {
        return Ok(());
    }
    anyhow::ensure!(
        workflow.cancel_requested || budget::can_reconcile(&workflow)?,
        "production_uncertain_generation"
    );
    let plugin = PluginHost::discover(home)?;
    anyhow::ensure!(
        plugin.approval_gap(PLUGIN).await?.is_none(),
        "production_plugin_approval_required"
    );
    plugin.set_host_requests(Arc::new(host::Host {
        home: home.to_path_buf(),
        workflow_id: id,
        inner: crate::plugin_host_requests::HostRequests::new(
            home.to_path_buf(),
            home.join("config.toml"),
            Arc::new(std::sync::RwLock::new(vec![
                workflow.input.workspace.clone(),
            ])),
        )
        .with_usage_context(id, workflow.input.workspace.clone())
        .with_production_prompt(),
    }));
    let mcp = plugin.mcp(PLUGIN).await?;
    let tools = mcp.request("video-studio", "tools/list", json!({})).await?;
    anyhow::ensure!(
        tools["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "production.step")),
        "production_adapter_missing: production.step"
    );
    loop {
        let workflow = Store::new(home).get(id)?;
        if workflow.cancel_requested {
            let Some(node) = host::cancelled_node(&workflow)? else {
                return Ok(());
            };
            let response = mcp
                .request(
                    "video-studio",
                    "tools/call",
                    json!({"name":"production.step","arguments":node.arguments}),
                )
                .await?;
            let result = tool_result(response)?;
            anyhow::ensure!(
                result["ok"] == true
                    && result["settlementOnly"] == true
                    && result["workflowID"] == json!(id)
                    && result["inputDigest"] == node.input_digest,
                "production_cancelled_reconciliation_failed"
            );
            if result["pending"] != true {
                if result["unresolvedRequests"]
                    .as_array()
                    .is_some_and(|items| !items.is_empty())
                {
                    Store::new(home).update(id, |saved| {
                        saved.error = Some("production_cancelled_unsettled".into());
                        Ok(())
                    })?;
                }
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_secs(POLL_SECONDS)).await;
            continue;
        }
        if workflow.status != ProductionStatus::Running || workflow.cancel_requested {
            return Ok(());
        }
        if workflow.pause_requested
            && Store::new(home).update(id, |saved| Ok(scheduling::pause_if_ready(saved)))?
        {
            return Ok(());
        }
        let node = prepare(home, id)?;
        let response = mcp
            .request(
                "video-studio",
                "tools/call",
                json!({"name":"production.step", "arguments":node.arguments}),
            )
            .await?;
        let result = tool_result(response)?;
        if result["ok"] != true
            && Store::new(home)
                .update(id, |workflow| validation::retry(workflow, node.id, &result))?
        {
            continue;
        }
        anyhow::ensure!(
            result["ok"] == true,
            "production_step_failed: {}",
            result["error"]
        );
        let waiting = Store::new(home).update(id, |workflow| finish(workflow, node.id, result))?;
        if waiting {
            tokio::time::sleep(std::time::Duration::from_secs(POLL_SECONDS)).await;
        }
    }
}

fn tool_result(response: Value) -> Result<Value> {
    if let Some(value) = response.get("structuredContent") {
        return Ok(value.clone());
    }
    let text = response["content"]
        .as_array()
        .and_then(|items| items.iter().find_map(|item| item["text"].as_str()))
        .context("production_missing_plugin_result")?;
    serde_json::from_str(text).context("production_invalid_plugin_result")
}

pub(super) fn prepare(home: &Path, id: Uuid) -> Result<ProductionNode> {
    Store::new(home).update(id, |workflow| {
        anyhow::ensure!(
            workflow.status == ProductionStatus::Running && !workflow.cancel_requested,
            "production_not_running"
        );
        if let Some(node) = workflow
            .nodes
            .iter_mut()
            .find(|node| matches!(node.state, NodeState::Running | NodeState::WaitingExternal))
        {
            node.state = NodeState::Running;
            return Ok(node.clone());
        }
        let completed: HashSet<_> = workflow
            .nodes
            .iter()
            .filter(|node| node.state == NodeState::Completed)
            .map(|node| node.id)
            .collect();
        let index = workflow
            .nodes
            .iter()
            .position(|node| {
                node.state == NodeState::Pending
                    && !scheduling::deferred(workflow, node.stage)
                    && node
                        .dependencies
                        .iter()
                        .all(|dependency| completed.contains(dependency))
            })
            .with_context(|| if workflow.music_deferred && workflow.nodes.iter().any(|node|
                node.state != NodeState::Completed && scheduling::deferred(workflow, node.stage)) {
                "production_music_deferred"
            } else { "production_has_no_runnable_node" })?;
        let dependencies: Vec<_> = workflow
            .nodes
            .iter()
            .filter(|node| workflow.nodes[index].dependencies.contains(&node.id))
            .map(|node| {
                json!({"id":node.id,"stage":node.stage,"episode":node.episode,
                "inputDigest":node.input_digest,"result":node.result})
            })
            .collect();
        let expected_coverage = coverage(workflow, &workflow.nodes[index]);
        let repair_targets: Vec<_> = workflow.nodes.iter().filter(|target|
            repairable(workflow.nodes[index].stage, target.stage) && target.episode
                .is_none_or(|episode|expected_coverage.contains(&episode))
                && target.range.is_none_or(|range|expected_coverage.contains(&range.first) && expected_coverage.contains(&range.last)))
            .map(|target|json!({"id":target.id,"stage":target.stage,"episodeOrder":target.episode,"range":target.range})).collect();
        let node = &mut workflow.nodes[index];
        anyhow::ensure!(
            node.attempt < node_attempt_limit(node.stage),
            "production_repair_limit: {}",
            node.id
        );
        node.attempt += 1;
        node.grant = Uuid::new_v4();
        let material = json!({"spec":workflow.input.spec,"brief":workflow.input.brief,
            "sourceScreenplayID":workflow.input.source_screenplay_id,"dependencies":dependencies,
            "stage":node.stage,"episodeOrder":node.episode,"range":node.range,
            "attempt":node.attempt,"previousAttempts":node.history,"repairTargets":repair_targets});
        node.input_digest = digest(&material)?;
        node.arguments = json!({"workflowID":id,"taskID":node.id,"grant":node.grant,
            "inputDigest":node.input_digest,"input":material});
        node.state = NodeState::Running;
        Ok(node.clone())
    })
}

pub(super) fn is_review(stage: ProductionStage) -> bool {
    use ProductionStage::*;
    matches!(
        stage,
        ReviewSeriesPlan
            | ReviewOutlineBatch
            | ReviewEpisode
            | ReviewScripts
            | ReviewStoryboard
            | ReviewAssets
            | ReviewFrames
            | ReviewDialogue
            | ReviewMusic
            | ReviewVideo
            | ReviewFilm
            | ReviewSeries
    )
}

fn coverage(workflow: &ProductionWorkflow, node: &ProductionNode) -> Vec<u16> {
    if let Some(episode) = node.episode {
        return vec![episode];
    }
    if let Some(range) = node.range {
        return (range.first..=range.last).collect();
    }
    if node.stage == ProductionStage::ReviewSeriesPlan {
        return vec![0];
    }
    (1..=workflow.input.spec.episode_count).collect()
}

pub(super) fn finish(
    workflow: &mut ProductionWorkflow,
    node_id: Uuid,
    result: Value,
) -> Result<bool> {
    let index = workflow
        .nodes
        .iter()
        .position(|node| node.id == node_id)
        .context("production_node_missing")?;
    let node = &workflow.nodes[index];
    anyhow::ensure!(
        node.state == NodeState::Running && result["inputDigest"] == node.input_digest,
        "production_result_basis_mismatch"
    );
    if workflow.cancel_requested {
        return Ok(false);
    }
    if result["state"] == "waiting_external" {
        workflow.nodes[index].state = NodeState::WaitingExternal;
        workflow.nodes[index].result = Some(result);
        return Ok(true);
    }
    anyhow::ensure!(
        result["state"] == "completed",
        "production_result_state_invalid"
    );
    anyhow::ensure!(
        workflow
            .generations
            .iter()
            .filter(|receipt| receipt.node_id == node_id)
            .all(|receipt| receipt.state == GenerationState::Settled
                || recovery::retained_unknown(workflow, receipt)),
        "production_unsettled_generation"
    );
    if is_review(node.stage) {
        let review = &result["review"];
        anyhow::ensure!(
            review["inputDigest"] == node.input_digest
                && review["coverage"] == json!(coverage(workflow, node)),
            "production_review_coverage_mismatch"
        );
        let mandatory = review["mustFix"]
            .as_u64()
            .context("production_review_must_fix_required")?;
        if mandatory > 0 {
            rework(workflow, index, result)?;
            return Ok(false);
        }
        anyhow::ensure!(
            matches!(review["verdict"].as_str(), Some("pass" | "warn")),
            "production_review_not_passed"
        );
    }
    if node.stage == ProductionStage::Deliver {
        delivery(workflow, &result)?;
    }
    workflow.nodes[index].state = NodeState::Completed;
    workflow.nodes[index].result = Some(result.clone());
    workflow.domain = Some(result);
    if workflow.nodes[index].stage == ProductionStage::Deliver {
        workflow.status = ProductionStatus::Completed;
    }
    Ok(false)
}

fn repairable(review: ProductionStage, target: ProductionStage) -> bool {
    use ProductionStage::*;
    matches!(
        (review, target),
        (ReviewSeriesPlan, SeriesPlan)
            | (ReviewOutlineBatch, OutlineBatch)
            | (ReviewEpisode, WriteEpisode)
            | (ReviewScripts, WriteEpisode)
            | (ReviewStoryboard, Storyboard)
            | (ReviewAssets, GenerateAssets)
            | (ReviewFrames, GenerateFrames)
            | (ReviewDialogue, GenerateDialogue)
            | (ReviewMusic, GenerateMusic)
            | (ReviewVideo, GenerateVideo)
            | (ReviewFilm, ComposeEpisode)
            | (ReviewFilm, GenerateVideo)
            | (ReviewSeries, WriteEpisode)
            | (ReviewSeries, ComposeEpisode)
            | (ReviewSeries, GenerateVideo)
    )
}

fn rework(workflow: &mut ProductionWorkflow, review_index: usize, result: Value) -> Result<()> {
    let review = &workflow.nodes[review_index];
    let target_id: Uuid = serde_json::from_value(result["reworkTarget"].clone())?;
    let target = workflow
        .nodes
        .iter()
        .find(|node| node.id == target_id)
        .context("production_repair_target_missing")?;
    anyhow::ensure!(
        repairable(review.stage, target.stage)
            && target
                .episode
                .is_none_or(|episode| coverage(workflow, review).contains(&episode))
            && target
                .range
                .is_none_or(|range| coverage(workflow, review).contains(&range.first)
                    && coverage(workflow, review).contains(&range.last)),
        "production_repair_target_denied"
    );
    anyhow::ensure!(
        target.attempt < node_attempt_limit(target.stage),
        "production_repair_limit"
    );
    let mut affected = HashSet::from([target_id]);
    for node in &workflow.nodes {
        if node
            .dependencies
            .iter()
            .any(|dependency| affected.contains(dependency))
        {
            affected.insert(node.id);
        }
    }
    anyhow::ensure!(
        affected.contains(&review.id),
        "production_repair_target_not_ancestor"
    );
    anyhow::ensure!(
        !workflow
            .generations
            .iter()
            .any(|receipt| affected.contains(&receipt.node_id)
                && receipt.state != GenerationState::Settled
                && !recovery::retained_unknown(workflow, receipt)),
        "production_repair_has_inflight_generation"
    );
    for node in &mut workflow.nodes {
        if !affected.contains(&node.id) {
            continue;
        }
        if node.attempt > 0 {
            node.history.push(
                json!({"attempt":node.attempt,"inputDigest":node.input_digest,
                "result":node.result,"trigger":result["review"],"target":target_id}),
            );
        }
        node.state = NodeState::Pending;
        node.result = None;
        node.grant = Uuid::nil();
        node.arguments = json!({});
    }
    Ok(())
}

fn delivery(workflow: &ProductionWorkflow, result: &Value) -> Result<()> {
    anyhow::ensure!(
        historical::current(workflow)?.is_none(),
        "production_historical_hold_unresolved"
    );
    anyhow::ensure!(
        workflow
            .nodes
            .iter()
            .filter(|node| node.stage != ProductionStage::Deliver)
            .all(|node| node.state == NodeState::Completed),
        "production_all_stages_required"
    );
    anyhow::ensure!(
        workflow
            .generations
            .iter()
            .all(|receipt| receipt.state == GenerationState::Settled),
        "production_unsettled_delivery"
    );
    let episodes = result["delivery"]["episodes"]
        .as_array()
        .context("production_delivery_episodes_required")?;
    anyhow::ensure!(
        episodes.len() == usize::from(workflow.input.spec.episode_count),
        "production_delivery_incomplete"
    );
    for (index, file) in episodes.iter().enumerate() {
        let order = index as u16 + 1;
        let sha = file["sha256"]
            .as_str()
            .context("production_delivery_digest_required")?;
        anyhow::ensure!(
            file["episodeOrder"] == order
                && budget::is_digest(sha)
                && file["bytes"].as_u64().is_some_and(|bytes| bytes > 0)
                && file["durationMs"]
                    .as_u64()
                    .is_some_and(|duration| duration > 0),
            "production_delivery_file_invalid"
        );
        let compose = workflow
            .nodes
            .iter()
            .find(|node| {
                node.stage == ProductionStage::ComposeEpisode && node.episode == Some(order)
            })
            .context("production_composition_missing")?;
        anyhow::ensure!(
            compose
                .result
                .as_ref()
                .is_some_and(|result| result["artifact"]["sha256"] == sha
                    && result["artifact"]["bytes"] == file["bytes"]
                    && result["artifact"]["durationMs"] == file["durationMs"]
                    && result["artifact"]["sourceDigest"] == file["sourceDigest"]),
            "production_delivery_composition_mismatch"
        );
        anyhow::ensure!(
            file["sourceDigest"].as_str().is_some_and(budget::is_digest),
            "production_delivery_source_digest_required"
        );
        anyhow::ensure!(
            file["filename"] == format!("episode-{order:03}-{}.mp4", &sha[..16]),
            "production_delivery_filename_invalid"
        );
    }
    anyhow::ensure!(
        result["delivery"]["manifestSHA256"]
            .as_str()
            .is_some_and(budget::is_digest),
        "production_delivery_manifest_required"
    );
    let manifest_sha = result["delivery"]["manifestSHA256"]
        .as_str()
        .unwrap_or_default();
    anyhow::ensure!(
        result["delivery"]["manifestBytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0)
            && result["delivery"]["manifestFilename"]
                == format!("manifest-{}.json", &manifest_sha[..24]),
        "production_delivery_manifest_invalid"
    );
    let records = &result["delivery"]["records"];
    let records_sha = records["sha256"]
        .as_str()
        .filter(|sha| budget::is_digest(sha))
        .context("production_delivery_records_required")?;
    anyhow::ensure!(
        records["bytes"].as_u64().is_some_and(|bytes| bytes > 0)
            && records["filename"] == format!("records-{}.json", &records_sha[..24]),
        "production_delivery_records_invalid"
    );
    Ok(())
}
