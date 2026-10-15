//! Only the local admission broker can spend money; this transport never owns a model key.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use willdeep_core::{
    Message,
    provider::{Provider, ProviderError, ProviderIdentity},
    types::{Completion, ToolCall, ToolDefinition, Usage},
};

pub(super) const MODEL: &str = "deepseek-v4-flash";
pub(super) const PROPOSAL: &str =
    "a3ae36e96d268f8aaab8a02efacea61cb68f58c1d3bf812b9a38311890736606";
const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Captured {
    pub receipts: Vec<Value>,
    pub first_request: Option<Value>,
    pub raw_model_content: Option<String>,
}
pub(super) struct BrokerProvider {
    endpoint: reqwest::Url,
    client: reqwest::Client,
    capability: String,
    trial_id: String,
    sequence: AtomicUsize,
    preview: bool,
    pub captured: Arc<Mutex<Captured>>,
}
impl BrokerProvider {
    pub fn new(origin: &str, capability: String, trial_id: String, preview: bool) -> Result<Self> {
        let origin = reqwest::Url::parse(origin)?;
        ensure!(
            origin.scheme() == "http"
                && origin.host_str() == Some("127.0.0.1")
                && origin.port().is_some()
                && origin.path() == "/"
                && origin.username().is_empty()
                && origin.password().is_none()
                && origin.query().is_none()
                && origin.fragment().is_none(),
            "native evaluation requires a credential-free 127.0.0.1 broker origin"
        );
        ensure!(!capability.trim().is_empty(), "broker capability required");
        Ok(Self {
            endpoint: origin.join("v1/chat/completions")?,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(120))
                .build()?,
            capability,
            trial_id,
            sequence: AtomicUsize::new(0),
            preview,
            captured: Arc::new(Mutex::new(Captured::default())),
        })
    }
}
fn invalid(message: &str) -> ProviderError {
    ProviderError::InvalidResponse(message.into())
}
fn body(messages: &[Message], tools: &[ToolDefinition]) -> Result<Value, ProviderError> {
    if !messages.first().is_some_and(|message| {
        message.role == willdeep_core::Role::System
            && message
                .content
                .starts_with("You are a WillDeep subagent working in ")
    }) {
        return Err(invalid(
            "native broker rejects auxiliary or context-summary requests",
        ));
    }
    let messages=messages.iter().map(|message| {
        if !message.attachments.is_empty() {return Err(invalid("native reviewer accepts text only"));}
        let mut value=json!({"role":message.role,"content":message.content});
        if let Some(reasoning)=&message.reasoning {value["reasoning_content"]=reasoning.clone().into();}
        if let Some(id)=&message.tool_call_id {value["tool_call_id"]=id.clone().into();}
        if !message.tool_calls.is_empty() {value["tool_calls"]=message.tool_calls.iter().map(|call|json!({"id":call.id,"type":"function","function":{"name":call.name,"arguments":call.normalized_arguments()}})).collect();}
        Ok(value)
    }).collect::<Result<Vec<_>,ProviderError>>()?;
    Ok(
        json!({"model":MODEL,"stream":false,"temperature":0,"max_tokens":4000,"messages":messages,
        "tools":tools.iter().map(|tool|json!({"type":"function","function":{"name":tool.name,"description":tool.description,"parameters":tool.parameters}})).collect::<Vec<_>>(),"tool_choice":"auto"}),
    )
}
#[async_trait::async_trait]
impl Provider for BrokerProvider {
    fn ledger_identity(&self) -> Option<ProviderIdentity> {
        Some(ProviderIdentity {
            provider: "some-im".into(),
            model: MODEL.into(),
            local: false,
        })
    }
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        if sequence > 4 {
            return Err(invalid("native trial request ceiling reached"));
        }
        let body = body(messages, tools)?;
        {
            let mut captured = self.captured.lock().unwrap();
            if captured.first_request.is_none() {
                captured.first_request = Some(body.clone());
            }
        }
        if self.preview {
            return Err(invalid("native preview stopped before network request"));
        }
        let bytes =
            serde_json::to_vec(&body).map_err(|_| invalid("cannot encode native request"))?;
        let request_sha = willdeep_core::agent_definition::digest(&bytes);
        let started = std::time::Instant::now();
        let response = self
            .client
            .post(self.endpoint.clone())
            .bearer_auth(&self.capability)
            .header("X-RSI-Trial-Id", &self.trial_id)
            .header("X-RSI-Request-Sequence", sequence)
            .header("X-RSI-Proposal-Sha256", PROPOSAL)
            .json(&body)
            .send()
            .await;
        let mut receipt = json!({"sequence":sequence,"request_sha256":request_sha,"request_utf8_bytes":bytes.len(),"status":null,"request_id":null,"routed_model":null,"receipt_id":null,"response_id":null,"usage":null});
        receipt["read_results_in_request"] = read_results(messages);
        let outcome = async {
            let mut response =
                response.map_err(|_| invalid("budget broker transport failed; usage unknown"))?;
            receipt["status"] = response.status().as_u16().into();
            for (header, field) in [
                ("X-Request-Id", "request_id"),
                ("X-Routed-Model", "routed_model"),
                ("X-RSI-Receipt-Id", "receipt_id"),
                ("X-RSI-Approved-Model", "approved_model"),
            ] {
                if let Some(value) = response.headers().get(header).and_then(|v| v.to_str().ok()) {
                    receipt[field] = value.into();
                }
            }
            if !response.status().is_success() {
                return Err(invalid("budget broker rejected native request"));
            }
            if response
                .content_length()
                .is_some_and(|n| n > RESPONSE_LIMIT as u64)
            {
                return Err(invalid("broker response exceeds limit"));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| invalid("broker response interrupted; usage unknown"))?
            {
                if bytes.len() + chunk.len() > RESPONSE_LIMIT {
                    return Err(invalid("broker response exceeds limit"));
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&bytes)
                .map_err(|_| invalid("invalid broker JSON response"))?;
            receipt["response_id"] = value["id"].clone();
            receipt["response_model"] = value["model"].clone();
            receipt["usage"] = value["usage"].clone();
            if receipt["approved_model"] != MODEL
                || receipt["routed_model"].as_str().is_none_or(str::is_empty)
                || receipt["request_id"].as_str().is_none_or(str::is_empty)
                || receipt["receipt_id"].as_str().is_none_or(str::is_empty)
            {
                return Err(invalid(
                    "broker route or request receipt is missing or differs from approved model",
                ));
            }
            let choice = value["choices"]
                .get(0)
                .ok_or_else(|| invalid("broker omitted completion"))?;
            let message = &choice["message"];
            receipt["finish_reason"] = choice["finish_reason"].clone();
            receipt["completion_truncated"] = matches!(
                choice["finish_reason"].as_str(),
                Some("length" | "max_tokens" | "incomplete" | "content_filter")
            )
            .into();
            let calls = message["tool_calls"]
                .as_array()
                .map(|calls| {
                    calls
                        .iter()
                        .map(|call| {
                            Ok(ToolCall {
                                id: call["id"]
                                    .as_str()
                                    .ok_or_else(|| invalid("missing tool call ID"))?
                                    .into(),
                                name: call["function"]["name"]
                                    .as_str()
                                    .ok_or_else(|| invalid("missing tool name"))?
                                    .into(),
                                arguments: call["function"]["arguments"]
                                    .as_str()
                                    .ok_or_else(|| invalid("missing tool arguments"))?
                                    .into(),
                            })
                        })
                        .collect::<Result<Vec<_>, ProviderError>>()
                })
                .transpose()?
                .unwrap_or_default();
            let content = message["content"].as_str().unwrap_or_default().to_owned();
            if calls.is_empty() {
                self.captured.lock().unwrap().raw_model_content = Some(content.clone());
            }
            let usage = value
                .get("usage")
                .filter(|v| v.is_object())
                .map(|usage| Usage {
                    input_tokens: usage["prompt_tokens"].as_u64(),
                    output_tokens: usage["completion_tokens"].as_u64(),
                    total_tokens: usage["total_tokens"].as_u64(),
                    cache_read_tokens: usage["prompt_tokens_details"]["cached_tokens"]
                        .as_u64()
                        .or_else(|| usage["prompt_cache_hit_tokens"].as_u64()),
                });
            Ok(Completion {
                content,
                reasoning: message["reasoning_content"].as_str().map(str::to_owned),
                tool_calls: calls,
                finish_reason: choice["finish_reason"].as_str().map(str::to_owned),
                usage,
            })
        }
        .await;
        receipt["duration_ms"] =
            (started.elapsed().as_millis().min(u64::MAX as u128) as u64).into();
        receipt["transport_valid"] = outcome.is_ok().into();
        self.captured.lock().unwrap().receipts.push(receipt);
        outcome
    }
}

fn read_results(messages: &[Message]) -> Value {
    messages.iter().enumerate().filter_map(|(index,message)| {
        if message.role!=willdeep_core::Role::Tool {return None;}
        let id=message.tool_call_id.as_ref()?;
        let call=messages[..index].iter().rev().flat_map(|m|m.tool_calls.iter()).find(|call|call.id==*id && call.name=="read_file")?;
        let args:Value=serde_json::from_str(&call.arguments).ok()?;
        Some(json!({"tool_call_id":id,"arguments":args,"tool_result_sha256":willdeep_core::agent_definition::digest(message.content.as_bytes())}))
    }).collect()
}
