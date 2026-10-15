//! Offline adapter exercises the production Worker and tool path without a network provider.
use crate::{
    AgentEvent, EventSink, Message,
    agent_definition::{AgentPackage, digest},
    provider::{Provider, ProviderError, ProviderIdentity},
    types::{Completion, ToolCall, ToolDefinition, Usage},
};
use anyhow::{Result, ensure};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};

struct OfflineProvider {
    marker: String,
    output: String,
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl Provider for OfflineProvider {
    fn ledger_identity(&self) -> Option<ProviderIdentity> {
        Some(ProviderIdentity {
            provider: "mock".into(),
            model: "deterministic-reviewer-v1".into(),
            local: true,
        })
    }
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> std::result::Result<Completion, ProviderError> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let usage = Some(Usage {
            input_tokens: Some(40),
            output_tokens: Some(10),
            total_tokens: Some(50),
            cache_read_tokens: Some(0),
        });
        if call == 0 {
            return Ok(Completion {
                tool_calls: vec![ToolCall {
                    id: "offline-read".into(),
                    name: "read_file".into(),
                    arguments: r#"{"path":"fixture.txt"}"#.into(),
                }],
                usage,
                ..Completion::default()
            });
        }
        ensure_tool_observed(messages, &self.marker)?;
        Ok(Completion {
            content: self.output.clone(),
            finish_reason: Some("stop".into()),
            usage,
            ..Completion::default()
        })
    }
}
fn ensure_tool_observed(
    messages: &[Message],
    marker: &str,
) -> std::result::Result<(), ProviderError> {
    if messages
        .iter()
        .any(|m| m.role == crate::Role::Tool && m.content.contains(marker))
    {
        return Ok(());
    }
    Err(ProviderError::InvalidResponse(
        "offline fixture was not read through the Worker tool".into(),
    ))
}
#[derive(Default)]
struct Events(Mutex<Vec<serde_json::Value>>);
#[async_trait::async_trait]
impl EventSink for Events {
    async fn emit(&self, event: AgentEvent) {
        let value = match event {
            AgentEvent::SubagentToolCompleted { name, is_error, .. } => {
                serde_json::json!({"kind":"tool_completed","name":name,"is_error":is_error})
            }
            AgentEvent::SubagentUsage { usage, .. } => {
                serde_json::json!({"kind":"usage","usage":usage})
            }
            AgentEvent::SubagentVerdict {
                verifier_passed, ..
            } => serde_json::json!({"kind":"worker_verdict","passed":verifier_passed}),
            _ => return,
        };
        self.0.lock().unwrap().push(value);
    }
}
pub struct OfflineObservation {
    pub context: crate::rsi::ExecutionContext,
    pub report_sha256: String,
    pub verifier_input: serde_json::Value,
    pub verifier_input_sha256: String,
    pub events: Vec<serde_json::Value>,
    pub verified: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
}
pub fn effective_tools(package: &AgentPackage) -> Result<Vec<String>> {
    let provider = Arc::new(OfflineProvider {
        marker: String::new(),
        output: String::new(),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let base = super::builtin_profiles(provider)
        .into_iter()
        .find(|p| p.id == package.definition.base_role)
        .ok_or_else(|| anyhow::anyhow!("unknown base role"))?;
    let profile = package.definition.project(&base)?;
    Ok(profile
        .tool_names
        .into_iter()
        .filter(|name| !crate::tools::PARENT_ONLY_TOOLS.contains(&name.as_str()))
        .collect())
}

/// Caller owns a fresh experiment-only home/workspace. This function cannot construct a network provider.
pub async fn run_offline(
    package: &AgentPackage,
    home: &Path,
    workspace: &Path,
    prompt: &str,
    marker: &str,
    output: &str,
    budget: &serde_json::Value,
) -> Result<OfflineObservation> {
    package.definition.validate()?;
    ensure!(
        package.sha256 == package.definition.revision()?,
        "package revision mismatch"
    );
    let provider = Arc::new(OfflineProvider {
        marker: marker.into(),
        output: output.into(),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let base = super::builtin_profiles(provider.clone())
        .into_iter()
        .find(|p| p.id == package.definition.base_role)
        .ok_or_else(|| anyhow::anyhow!("unknown base role"))?;
    let mut profile = package.definition.project(&base)?;
    // P4b's offline fixtures are read-only and do not execute user-supplied commands.
    ensure!(
        profile.tool_names.iter().any(|t| t == "read_file"),
        "offline adapter requires read_file"
    );
    ensure!(
        !package
            .definition
            .tools
            .iter()
            .any(|t| t == "workspace.edit" || t == "process.verify"),
        "offline reviewer fixtures require read-only capabilities"
    );
    profile.max_turns = budget["max_turns"].as_u64().unwrap() as usize;
    profile.token_budget = Some(budget["max_tokens"].as_u64().unwrap());
    profile.timeout_seconds = Some(budget["timeout_seconds"].as_u64().unwrap());
    let events = Arc::new(Events::default());
    let worker = uuid::Uuid::new_v4();
    let sink = crate::feedback::FeedbackSink::spawn(home.join("feedback"));
    let feedback = crate::feedback::FeedbackRecorder::new(sink.clone(), "cli", false)
        .for_run(Some(provider.ledger_identity().unwrap()));
    let run = super::runner::SubagentRun {
        isolated_evaluation: true,
        execution_deadline: None,
        workspace: workspace.into(),
        sandbox: crate::sandbox::SandboxSpec::new(crate::sandbox::SandboxPolicy::ReadOnly, []),
        profile,
        prompt: prompt.into(),
        task: None,
        approved_targets: None,
        verifier: None,
        max_attempts: 1,
        claimed_files: Arc::new(Mutex::new(BTreeSet::new())),
        skills: None,
        safety_judge: None,
        approved_command: None,
        mcp: None,
        always_allow_store: None,
        state_home: Some(home.into()),
        parent_approval_mode: None,
        usage_ledger: None,
        feedback: Some(feedback),
        board: None,
        goal: None,
    };
    let report = super::runner::run_subagent(run, events.clone(), worker, None).await?;
    ensure!(
        sink.flush(std::time::Duration::from_secs(5)),
        "offline feedback did not flush"
    );
    let mut contexts = Vec::new();
    for entry in std::fs::read_dir(home.join("agent-evidence"))? {
        let path = entry?.path();
        let bytes = std::fs::read(&path)?;
        ensure!(
            path.file_stem().and_then(|s| s.to_str()) == Some(digest(&bytes).as_str()),
            "worker event digest mismatch"
        );
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        if value["phase"] == "started" {
            contexts.push(serde_json::from_value::<crate::rsi::ExecutionContext>(
                value["context"].clone(),
            )?);
        }
    }
    ensure!(contexts.len() == 1, "missing or ambiguous target execution");
    let context = contexts.remove(0);
    ensure!(
        context.definition_revision == package.sha256,
        "actual revision differs from requested target"
    );
    let events = events.0.lock().unwrap().clone();
    let verified = report.starts_with(output)
        && events.iter().any(|e| {
            e["kind"] == "tool_completed" && e["name"] == "read_file" && e["is_error"] == false
        });
    let input_tokens = events
        .iter()
        .filter_map(|e| e["usage"]["input_tokens"].as_u64())
        .sum();
    let output_tokens = events
        .iter()
        .filter_map(|e| e["usage"]["output_tokens"].as_u64())
        .sum();
    let verifier_input = verifier_input(&report, output, &events);
    let verifier_input_sha256 = crate::rsi::contract::hash(&verifier_input)?;
    Ok(OfflineObservation {
        context,
        report_sha256: digest(report.as_bytes()),
        verifier_input,
        verifier_input_sha256,
        events,
        verified,
        input_tokens,
        output_tokens,
    })
}

/// Digest the actual inputs to the host's prefix + successful-read predicate.
/// Full report/expected text stays local; event order and all tool outcomes are bound.
fn verifier_input(report: &str, expected: &str, events: &[serde_json::Value]) -> serde_json::Value {
    serde_json::json!({"schema":"willdeep.rust-offline-verifier-input.v1",
        "report_sha256":digest(report.as_bytes()),"expected_output_sha256":digest(expected.as_bytes()),
        "tool_events":events.iter().filter(|event|event["kind"]=="tool_completed").collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verifier_fingerprint_changes_with_actual_report_expectation_and_tool_outcome() {
        let events =
            vec![serde_json::json!({"kind":"tool_completed","name":"read_file","is_error":false})];
        let value = verifier_input("actual report", "expected prefix", &events);
        let hash = crate::rsi::contract::hash(&value).unwrap();
        for changed in [
            verifier_input("changed report", "expected prefix", &events),
            verifier_input("actual report", "changed expectation", &events),
            verifier_input(
                "actual report",
                "expected prefix",
                &[serde_json::json!({"kind":"tool_completed","name":"read_file","is_error":true})],
            ),
        ] {
            assert_ne!(hash, crate::rsi::contract::hash(&changed).unwrap());
        }
        assert_eq!(value["report_sha256"], digest(b"actual report"));
    }
}
