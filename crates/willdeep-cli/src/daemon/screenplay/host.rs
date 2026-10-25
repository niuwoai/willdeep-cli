use super::*;
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use willdeep_core::mcp::HostRequestError;
use willdeep_core::plugin::host_requests::AI_COMPLETE;
use willdeep_core::plugin::{PluginHostRequests, PluginRequestContext};

pub(super) const AUTHORIZE: &str = "willdeep/screenplay/authorize";

pub(super) struct Host {
    pub home: PathBuf,
    pub workflow_id: Uuid,
    pub inner: crate::plugin_host_requests::HostRequests,
}

#[async_trait]
impl PluginHostRequests for Host {
    fn methods(&self) -> Vec<String> {
        vec![AUTHORIZE.into(), AI_COMPLETE.into()]
    }

    async fn handle(
        &self,
        context: &PluginRequestContext,
        method: &str,
        params: Value,
    ) -> Result<Value, HostRequestError> {
        context.require(willdeep_core::plugin::PluginPermission::AiChat)?;
        if context.plugin_id != PLUGIN {
            return Err(HostRequestError::failed("wrong screenplay plugin"));
        }
        match method {
            AUTHORIZE => {
                let workflow = Store::new(&self.home).get(self.workflow_id).map_err(rpc)?;
                let task = workflow
                    .tasks
                    .last()
                    .ok_or_else(|| rpc("no active screenplay task"))?;
                if workflow.status != "running"
                    || workflow.cancel_requested
                    || task.state != "running"
                    || task.arguments != params
                {
                    return Err(rpc("invalid or expired screenplay task grant"));
                }
                Ok(json!({"allowed":true}))
            }
            AI_COMPLETE => self.complete(context, params).await,
            _ => Err(HostRequestError::method_not_found(
                "unsupported screenplay request",
            )),
        }
    }
}

impl Host {
    async fn complete(
        &self,
        context: &PluginRequestContext,
        mut params: Value,
    ) -> Result<Value, HostRequestError> {
        // Reserve before dispatch. An interrupted or failed call is not silently free or replayable.
        let serialized = serde_json::to_vec(&params).map_err(rpc)?;
        if serialized.len() > 180_000 {
            return Err(rpc("screenplay model input is too large"));
        }
        let output = params["max_output_tokens"]
            .as_u64()
            .unwrap_or(4_000)
            .clamp(1, 6_000);
        params["max_output_tokens"] = json!(output);
        let reservation = serialized.len() as u64 + output;
        let request_digest = format!("{:x}", Sha256::digest(&serialized));
        let call_id = Uuid::new_v4();
        let replay = Store::new(&self.home)
            .update(self.workflow_id, |workflow| {
                anyhow::ensure!(
                    workflow.status == "running" && !workflow.cancel_requested,
                    "screenplay cancelled or inactive"
                );
                let task = workflow.tasks.last().context("missing task")?;
                anyhow::ensure!(task.state == "running", "no active screenplay task");
                if let Some(index) = workflow.calls.iter().position(|call| {
                    call.task_id == task.id && call.request_digest == request_digest
                }) {
                    let response = Store::new(&self.home)
                        .response(self.workflow_id, workflow.calls[index].id)?;
                    settle(&mut workflow.calls[index], &response)?;
                    return Ok(Some(response));
                }
                let final_step = task.arguments["step"] == "final_review";
                let reserve_calls = if final_step { 0 } else { 1 };
                let reserve_tokens = if final_step { 0 } else { 20_000 };
                let charged: u64 = workflow
                    .calls
                    .iter()
                    .map(|call| call.tokens.unwrap_or(call.reservation))
                    .sum();
                anyhow::ensure!(
                    workflow.calls.len() + reserve_calls < workflow.input.max_calls as usize,
                    "budget_limited: model call limit"
                );
                anyhow::ensure!(
                    charged
                        .saturating_add(reservation)
                        .saturating_add(reserve_tokens)
                        <= workflow.input.max_tokens,
                    "budget_limited: token reservation limit"
                );
                anyhow::ensure!(
                    !workflow
                        .calls
                        .iter()
                        .any(|call| call.state == "started" || call.state == "unknown"),
                    "uncertain_model_call: reconcile before further billed calls"
                );
                workflow.calls.push(ScreenplayCall {
                    id: call_id,
                    task_id: task.id,
                    state: "started".into(),
                    reservation,
                    tokens: None,
                    model: None,
                    request_digest: request_digest.clone(),
                    response_digest: None,
                });
                Ok(None)
            })
            .map_err(rpc)?;
        if let Some(response) = replay {
            return Ok(response);
        }
        let result = self.inner.handle(context, AI_COMPLETE, params).await;
        if let Ok(response) = &result {
            Store::new(&self.home)
                .save_response(self.workflow_id, call_id, response)
                .map_err(rpc)?;
        }
        Store::new(&self.home)
            .update(self.workflow_id, |workflow| {
                let call = workflow
                    .calls
                    .iter_mut()
                    .find(|call| call.id == call_id)
                    .context("missing reserved call")?;
                match &result {
                    Ok(response) => {
                        settle(call, response)?;
                    }
                    Err(_) => call.state = "unknown".into(),
                }
                Ok(())
            })
            .map_err(rpc)?;
        result
    }
}

pub(super) fn settle(call: &mut ScreenplayCall, response: &Value) -> Result<()> {
    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(response)?));
    anyhow::ensure!(
        call.response_digest
            .as_ref()
            .is_none_or(|expected| expected == &digest),
        "saved model response digest mismatch"
    );
    let usage: willdeep_core::types::Usage =
        serde_json::from_value(response["usage"].clone()).unwrap_or_default();
    let known = usage.total_tokens.is_some()
        || (usage.input_tokens.is_some() && usage.output_tokens.is_some());
    call.tokens = known.then(|| usage.billable_tokens());
    call.state = if known { "completed" } else { "unknown" }.into();
    call.model = response["model"].as_str().map(str::to_owned);
    call.response_digest = Some(digest);
    Ok(())
}

fn rpc(error: impl std::fmt::Display) -> HostRequestError {
    HostRequestError::failed(willdeep_core::judge::redact_credentials(&error.to_string()))
}
