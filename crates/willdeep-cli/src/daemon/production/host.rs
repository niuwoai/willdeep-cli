use super::*;
use async_trait::async_trait;
use serde::Deserialize;
use willdeep_core::mcp::HostRequestError;
use willdeep_core::plugin::host_requests::{AI_COMPLETE, IMAGE_GENERATE};
use willdeep_core::plugin::{PluginHostRequests, PluginPermission, PluginRequestContext};

pub(super) const AUTHORIZE: &str = "willdeep/production/authorize";
pub(super) const METER: &str = "willdeep/production/meter";
const MAX_MODEL_INPUT: usize = 256_000;
const MAX_MODEL_OUTPUT: u64 = 8_000;

pub(super) struct Host {
    pub home: PathBuf,
    pub workflow_id: Uuid,
    pub inner: crate::plugin_host_requests::HostRequests,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MeterRequest {
    #[serde(rename = "taskID", alias = "taskId")]
    task_id: Uuid,
    grant: Uuid,
    operation: String,
    arguments: Value,
}

#[async_trait]
impl PluginHostRequests for Host {
    fn methods(&self) -> Vec<String> {
        vec![
            AUTHORIZE.into(),
            METER.into(),
            AI_COMPLETE.into(),
            IMAGE_GENERATE.into(),
        ]
    }

    async fn handle(
        &self,
        context: &PluginRequestContext,
        method: &str,
        params: Value,
    ) -> Result<Value, HostRequestError> {
        context.require(PluginPermission::AiChat)?;
        if context.plugin_id != PLUGIN {
            return Err(rpc("production_plugin_denied"));
        }
        match method {
            AUTHORIZE => {
                let workflow = Store::new(&self.home).get(self.workflow_id).map_err(rpc)?;
                let node = if workflow.cancel_requested {
                    cancelled_node(&workflow)
                        .map_err(rpc)?
                        .ok_or_else(|| rpc("production_no_cancelled_receipts"))?
                } else {
                    active(&workflow).map_err(rpc)?
                };
                if node.arguments != params {
                    return Err(rpc("production_task_grant_invalid"));
                }
                if workflow.cancel_requested {
                    let receipts: Vec<_> = workflow
                        .generations
                        .iter()
                        .filter(|receipt| {
                            receipt.node_id == node.id
                                && receipt.state != GenerationState::Settled
                                && matches!(
                                    receipt.kind,
                                    GenerationKind::Music | GenerationKind::Video
                                )
                        })
                        .collect();
                    return Ok(json!({"allowed":true,"settlementOnly":true,"receipts":receipts}));
                }
                let usage = budget::usage(&workflow).map_err(rpc)?;
                let conversation = if node.stage == ProductionStage::Deliver {
                    accounting::conversation(
                        &self.home,
                        &json!({"input":workflow.input,"usage":usage}),
                    )
                } else {
                    Value::Null
                };
                Ok(
                    json!({"allowed":true,"sourceScreenplay":source_screenplay(&self.home, &workflow).map_err(rpc)?,
                        "usage":usage, "conversationUsage":conversation,
                        "allSettled":historical::current(&workflow).map_err(rpc)?.is_none() && workflow.generations.iter().all(|receipt|receipt.state == GenerationState::Settled)}),
                )
            }
            METER => self.meter(context, params).map_err(rpc),
            AI_COMPLETE | IMAGE_GENERATE => self.proxy(context, method, params).await,
            _ => Err(HostRequestError::method_not_found(
                "unsupported production request",
            )),
        }
    }
}

pub(super) fn needs_reconciliation(workflow: &ProductionWorkflow) -> bool {
    workflow.cancel_requested
        && workflow.generations.iter().any(|receipt| {
            receipt.state != GenerationState::Settled
                && matches!(receipt.kind, GenerationKind::Music | GenerationKind::Video)
        })
}

pub(super) fn cancelled_node(workflow: &ProductionWorkflow) -> Result<Option<&ProductionNode>> {
    if !needs_reconciliation(workflow) {
        return Ok(None);
    }
    anyhow::ensure!(
        workflow.status == ProductionStatus::Cancelled,
        "production_cancellation_state_invalid"
    );
    let mut nodes = workflow.nodes.iter().filter(|node| {
        matches!(node.state, NodeState::Running | NodeState::WaitingExternal)
            && node.grant != Uuid::nil()
    });
    let node = nodes.next().context("production_cancelled_node_missing")?;
    anyhow::ensure!(
        nodes.next().is_none()
            && workflow
                .generations
                .iter()
                .filter(|receipt| receipt.state != GenerationState::Settled
                    && matches!(receipt.kind, GenerationKind::Music | GenerationKind::Video))
                .all(|receipt| receipt.node_id == node.id),
        "production_cancelled_node_ambiguous"
    );
    Ok(Some(node))
}

fn active(workflow: &ProductionWorkflow) -> Result<&ProductionNode> {
    anyhow::ensure!(
        workflow.status == ProductionStatus::Running && !workflow.cancel_requested,
        "production_not_running"
    );
    let mut active = workflow
        .nodes
        .iter()
        .filter(|node| matches!(node.state, NodeState::Running | NodeState::WaitingExternal));
    let node = active.next().context("production_active_node_missing")?;
    anyhow::ensure!(active.next().is_none(), "production_active_node_ambiguous");
    Ok(node)
}

fn source_screenplay(home: &Path, workflow: &ProductionWorkflow) -> Result<Option<Value>> {
    let Some(id) = workflow.input.source_screenplay_id else {
        return Ok(None);
    };
    let path = home.join("runtime/screenplays").join(format!("{id}.json"));
    let source: willdeep_runtime_protocol::screenplay::ScreenplayWorkflow =
        serde_json::from_slice(&std::fs::read(path).context("production_source_unavailable")?)?;
    anyhow::ensure!(
        source.id == id
            && source.status == "completed"
            && source.input.workspace == workflow.input.workspace
            && source.calls.iter().all(|call| call.state == "completed"),
        "production_source_not_verified"
    );
    let domain = source.domain.context("production_source_export_missing")?;
    anyhow::ensure!(
        domain["scripts"] == workflow.input.spec.episode_count
            && domain["mustFix"] == 0
            && domain["export"].is_object(),
        "production_source_spec_mismatch"
    );
    Ok(Some(json!({"id":id,"status":"completed","domain":domain})))
}

fn permission(context: &PluginRequestContext, kind: GenerationKind) -> Result<()> {
    let required = match kind {
        GenerationKind::Model => PluginPermission::AiChat,
        GenerationKind::Image => PluginPermission::AiImage,
        _ => PluginPermission::NetworkAccess,
    };
    context
        .require(required)
        .map_err(|error| anyhow::anyhow!(error.message))
}

impl Host {
    fn meter(&self, context: &PluginRequestContext, params: Value) -> Result<Value> {
        let envelope: MeterRequest = serde_json::from_value(params)?;
        let store = Store::new(&self.home);
        store.update(self.workflow_id, |workflow| {
            let node = workflow
                .nodes
                .iter()
                .find(|node| node.id == envelope.task_id)
                .context("production_node_missing")?;
            anyhow::ensure!(
                envelope.grant != Uuid::nil() && envelope.grant == node.grant,
                "production_task_grant_invalid"
            );
            if envelope.operation == "reserve" {
                let mut arguments = envelope.arguments;
                let subject = arguments
                    .as_object_mut()
                    .and_then(|value| value.remove("imageSubject"));
                let request: budget::Reservation = serde_json::from_value(arguments)?;
                anyhow::ensure!(
                    request.node_id == envelope.task_id,
                    "production_reservation_node_mismatch"
                );
                permission(context, request.kind)?;
                image_retries::bind_reservation(
                    workflow,
                    &request,
                    subject.as_ref().and_then(Value::as_str),
                )?;
                let result = budget::reserve(workflow, &request)?;
                return Ok(match result {
                    budget::Reserved::Fresh => json!({"ok":true,"fresh":true}),
                    budget::Reserved::Existing(receipt) => {
                        let mut value = json!({"ok":true,"fresh":false,"receipt":receipt});
                        if receipt.state == GenerationState::Settled {
                            let saved = store
                                .receipt(self.workflow_id, receipt.request_id)?
                                .context("production_saved_response_missing")?;
                            anyhow::ensure!(
                                receipt.evidence_digest.as_ref() == Some(&engine::digest(&saved)?),
                                "production_receipt_digest_mismatch"
                            );
                            value["response"] = saved["response"].clone();
                        }
                        value
                    }
                });
            }
            if envelope.operation == "image_status" {
                permission(context, GenerationKind::Image)?;
                return image_retries::status(
                    workflow,
                    envelope.arguments["subject"]
                        .as_str()
                        .context("production_image_subject_required")?,
                );
            }
            let id: Uuid = serde_json::from_value(envelope.arguments["requestID"].clone())?;
            let receipt = workflow
                .generations
                .iter()
                .find(|receipt| receipt.request_id == id)
                .context("production_reservation_missing")?;
            anyhow::ensure!(
                receipt.node_id == envelope.task_id,
                "production_reservation_node_mismatch"
            );
            permission(context, receipt.kind)?;
            match envelope.operation.as_str() {
                "submitted" => budget::submitted(
                    workflow,
                    id,
                    envelope.arguments["externalID"]
                        .as_str()
                        .context("production_external_id_required")?,
                )?,
                "unknown" => budget::unknown(workflow, id)?,
                "settle" => {
                    let actual: Quantities =
                        serde_json::from_value(envelope.arguments["actual"].clone())?;
                    let succeeded = envelope.arguments["succeeded"]
                        .as_bool()
                        .context("production_outcome_required")?;
                    let saved = json!({"response":envelope.arguments["response"],"actual":actual,
                        "succeeded":succeeded,"requestDigest":receipt.request_digest});
                    let hash = engine::digest(&saved)?;
                    // Never overwrite a known settlement with a conflicting provider response.
                    let previous = store.receipt(self.workflow_id, id)?;
                    if let Some(previous) = &previous {
                        anyhow::ensure!(
                            engine::digest(previous)? == hash,
                            "production_receipt_conflict"
                        );
                    }
                    budget::settle(workflow, id, actual, succeeded, hash)?;
                    if previous.is_none() {
                        store.save_receipt(self.workflow_id, id, &saved)?;
                    }
                }
                _ => anyhow::bail!("production_meter_operation_denied"),
            }
            Ok(json!({"ok":true}))
        })
    }

    async fn proxy(
        &self,
        context: &PluginRequestContext,
        method: &str,
        mut params: Value,
    ) -> Result<Value, HostRequestError> {
        if !params.is_object() {
            return Err(HostRequestError::invalid_params(
                "production_request_object_required",
            ));
        }
        let kind = if method == AI_COMPLETE {
            GenerationKind::Model
        } else {
            GenerationKind::Image
        };
        permission(context, kind).map_err(rpc)?;
        if kind == GenerationKind::Model {
            let output = params["max_output_tokens"]
                .as_u64()
                .unwrap_or(4_000)
                .clamp(1, MAX_MODEL_OUTPUT);
            params["max_output_tokens"] = json!(output);
            if serde_json::to_vec(&params).map_err(rpc)?.len() > MAX_MODEL_INPUT {
                return Err(rpc("production_model_input_too_large"));
            }
        }
        let request_digest =
            engine::digest(&json!({"method":method,"params":params})).map_err(rpc)?;
        let workflow = Store::new(&self.home).get(self.workflow_id).map_err(rpc)?;
        let node = active(&workflow).map_err(rpc)?;
        let id = Uuid::new_v5(
            &node.id,
            format!("{}:{}:{request_digest}", node.attempt, node.input_digest).as_bytes(),
        );
        let quantities = if kind == GenerationKind::Model {
            [
                (ProductionUnit::ModelCalls, 1),
                (
                    ProductionUnit::Tokens,
                    serde_json::to_vec(&params).map_err(rpc)?.len() as u64
                        + params["max_output_tokens"]
                            .as_u64()
                            .unwrap_or(MAX_MODEL_OUTPUT),
                ),
            ]
            .into()
        } else {
            [(ProductionUnit::Images, 1)].into()
        };
        let request = budget::Reservation {
            request_id: id,
            node_id: node.id,
            kind,
            request_digest,
            quantities,
            provider: params["provider"].as_str().unwrap_or("host-default").into(),
        };
        let subject = params["productionImageSubject"].as_str().map(str::to_owned);
        let decision = Store::new(&self.home)
            .update(self.workflow_id, |workflow| {
                image_retries::bind_reservation(workflow, &request, subject.as_deref())?;
                budget::reserve(workflow, &request)
            })
            .map_err(rpc)?;
        if let budget::Reserved::Existing(_) = decision {
            let response = Store::new(&self.home)
                .receipt(self.workflow_id, id)
                .map_err(rpc)?
                .ok_or_else(|| rpc("production_uncertain_generation: no saved response"))?;
            restore(&self.home, self.workflow_id, id, &response).map_err(rpc)?;
            return Ok(response["response"].clone());
        }
        if kind == GenerationKind::Image {
            params
                .as_object_mut()
                .unwrap()
                .remove("productionImageSubject");
        }
        match self.inner.handle(context, method, params).await {
            Ok(response) => {
                let actual = actual(kind, &response);
                let saved = json!({"response":response,"actual":actual,"succeeded":true,
                    "requestDigest":request.request_digest});
                Store::new(&self.home)
                    .save_receipt(self.workflow_id, id, &saved)
                    .map_err(rpc)?;
                restore(&self.home, self.workflow_id, id, &saved).map_err(rpc)?;
                Ok(response)
            }
            Err(error) => {
                if let Some(saved) = local_rejection(kind, &request.request_digest, &error) {
                    Store::new(&self.home)
                        .save_receipt(self.workflow_id, id, &saved)
                        .map_err(rpc)?;
                    restore(&self.home, self.workflow_id, id, &saved).map_err(rpc)?;
                } else {
                    Store::new(&self.home)
                        .update(self.workflow_id, |workflow| budget::unknown(workflow, id))
                        .map_err(rpc)?;
                }
                Err(error)
            }
        }
    }
}

// These codes are emitted by local request validation before provider dispatch.
// All other failures remain unknown, including HTTP errors and missing usage.
pub(super) fn local_rejection(
    kind: GenerationKind,
    digest: &str,
    error: &HostRequestError,
) -> Option<Value> {
    if error.code != HostRequestError::INVALID_PARAMS
        || !crate::plugin_capabilities::INVALID_REQUEST_CODES.contains(&error.message.as_str())
    {
        return None;
    }
    let actual: Quantities = kind.units().iter().map(|unit| (*unit, 0)).collect();
    Some(
        json!({"response":{"error":{"code":error.message,"localRejection":true,"dispatchStarted":false}},
        "actual":actual,"succeeded":false,"requestDigest":digest}),
    )
}

fn actual(kind: GenerationKind, response: &Value) -> Option<Quantities> {
    if kind == GenerationKind::Image {
        let path = response["filePath"].as_str()?;
        return std::fs::metadata(path)
            .ok()
            .filter(|metadata| metadata.is_file() && metadata.len() > 0)
            .map(|_| [(ProductionUnit::Images, 1)].into());
    }
    let usage: willdeep_core::types::Usage =
        serde_json::from_value(response["usage"].clone()).ok()?;
    (usage.total_tokens.is_some()
        || (usage.input_tokens.is_some() && usage.output_tokens.is_some()))
    .then(|| {
        [
            (ProductionUnit::ModelCalls, 1),
            (ProductionUnit::Tokens, usage.billable_tokens()),
        ]
        .into()
    })
}

fn restore(home: &Path, workflow_id: Uuid, request: Uuid, saved: &Value) -> Result<()> {
    Store::new(home).update(workflow_id, |workflow| {
        let receipt = workflow
            .generations
            .iter()
            .find(|receipt| receipt.request_id == request)
            .context("production_reservation_missing")?;
        anyhow::ensure!(
            saved["requestDigest"] == receipt.request_digest,
            "production_receipt_request_mismatch"
        );
        if saved["actual"].is_null() {
            budget::unknown(workflow, request)?;
            return Ok(());
        }
        let actual = serde_json::from_value(saved["actual"].clone())?;
        let succeeded = saved["succeeded"]
            .as_bool()
            .context("production_outcome_required")?;
        budget::settle(workflow, request, actual, succeeded, engine::digest(saved)?)
    })?;
    let workflow = Store::new(home).get(workflow_id)?;
    anyhow::ensure!(
        workflow
            .generations
            .iter()
            .find(|receipt| receipt.request_id == request)
            .is_some_and(|receipt| receipt.state == GenerationState::Settled),
        "production_unknown_usage"
    );
    Ok(())
}

pub(super) fn reconcile(home: &Path, id: Uuid) -> Result<()> {
    let store = Store::new(home);
    let workflow = store.get(id)?;
    for receipt in workflow
        .generations
        .iter()
        .filter(|receipt| receipt.state != GenerationState::Settled)
    {
        if let Some(saved) = store.receipt(id, receipt.request_id)? {
            restore(home, id, receipt.request_id, &saved)?;
        }
    }
    Ok(())
}

fn rpc(error: impl std::fmt::Display) -> HostRequestError {
    HostRequestError::failed(willdeep_core::judge::redact_credentials(&error.to_string()))
}
