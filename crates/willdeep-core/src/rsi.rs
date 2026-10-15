//! Local execution lineage. A completion is never a verification or a promotion.
use serde::{Deserialize, Serialize};
pub mod contract;

/// Attached by the host after resolving the definition and assembling the real prompt.
/// Contains summaries only: no paths, code, credentials, or model responses.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionContext {
    pub definition_id: String,
    pub definition_revision: String,
    pub base_role: String,
    pub effective_prompt_sha256: String,
    pub skills_sha256: Vec<String>,
    pub effective_tools: Vec<String>,
    pub root_run_id: Option<String>,
    pub worker_run_id: String,
    pub attempt: usize,
    pub max_turns: usize,
    pub token_budget: Option<u64>,
    pub timeout_seconds: Option<u64>,
    pub runtime_version: String,
    pub provider: Option<String>,
    pub model: Option<String>,
}

impl ExecutionContext {
    pub fn from_profile(
        profile: &crate::SubagentProfile,
        prompt: &str,
        tools: Vec<String>,
        worker: uuid::Uuid,
        attempt: usize,
    ) -> anyhow::Result<Option<Self>> {
        let Some(definition) = &profile.definition else {
            return Ok(None);
        };
        Ok(Some(Self {
            definition_id: definition.definition_id.clone(),
            definition_revision: definition.revision()?,
            base_role: definition.base_role.clone(),
            effective_prompt_sha256: crate::agent_definition::digest(prompt.as_bytes()),
            skills_sha256: definition
                .skills
                .iter()
                .map(|skill| skill.sha256.clone())
                .collect(),
            effective_tools: tools,
            root_run_id: None,
            worker_run_id: worker.to_string(),
            attempt,
            max_turns: profile.max_turns,
            token_budget: profile.token_budget,
            timeout_seconds: profile.timeout_seconds,
            runtime_version: crate::VERSION.into(),
            provider: profile
                .provider
                .ledger_identity()
                .as_ref()
                .map(|identity| identity.provider.clone()),
            model: profile
                .provider
                .ledger_identity()
                .map(|identity| identity.model),
        }))
    }
}

pub fn record_worker_event(
    home: Option<&std::path::Path>,
    context: Option<&ExecutionContext>,
    phase: &str,
    outcome: &str,
    verifier: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    let (Some(home), Some(context)) = (home, context) else {
        return Ok(());
    };
    let value = serde_json::json!({"schema":"willdeep.worker-evidence.v1", "event_id":uuid::Uuid::new_v4(),
        "context":context, "phase":phase,"outcome":outcome,"verifier":verifier});
    let bytes = serde_json::to_vec(&value)?;
    let path = home
        .join("agent-evidence")
        .join(format!("{}.json", crate::agent_definition::digest(&bytes)));
    crate::agent_definition::store::private_write(&path, &bytes)
}

/// A timeout, cancellation, panic or dropped future leaves an explicit interrupted event.
pub(crate) struct WorkerEvidenceGuard {
    home: Option<std::path::PathBuf>,
    context: Option<ExecutionContext>,
    finished: bool,
}
impl WorkerEvidenceGuard {
    pub fn start(
        home: Option<&std::path::Path>,
        context: Option<ExecutionContext>,
    ) -> anyhow::Result<Self> {
        record_worker_event(home, context.as_ref(), "started", "unverified", None)?;
        Ok(Self {
            home: home.map(std::path::Path::to_path_buf),
            context,
            finished: false,
        })
    }
    pub fn finish(&mut self, outcome: &str) -> anyhow::Result<()> {
        record_worker_event(
            self.home.as_deref(),
            self.context.as_ref(),
            "finished",
            outcome,
            None,
        )?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for WorkerEvidenceGuard {
    fn drop(&mut self) {
        if !self.finished
            && let Err(error) = record_worker_event(
                self.home.as_deref(),
                self.context.as_ref(),
                "interrupted",
                "interrupted",
                None,
            )
        {
            eprintln!("unable to persist interrupted worker evidence: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropped_worker_keeps_interrupted_evidence_without_claiming_completion() {
        let home = std::env::temp_dir().join(format!("rsi-interrupted-{}", uuid::Uuid::new_v4()));
        {
            let _guard =
                WorkerEvidenceGuard::start(Some(&home), Some(ExecutionContext::default())).unwrap();
        }
        let events: Vec<serde_json::Value> = std::fs::read_dir(home.join("agent-evidence"))
            .unwrap()
            .map(|e| serde_json::from_slice(&std::fs::read(e.unwrap().path()).unwrap()).unwrap())
            .collect();
        assert_eq!(events.len(), 2);
        assert!(events.iter().any(|e| e["phase"] == "started"));
        assert!(
            events
                .iter()
                .any(|e| e["phase"] == "interrupted" && e["outcome"] == "interrupted")
        );
        assert!(!events.iter().any(|e| e["outcome"] == "passed"));
        std::fs::remove_dir_all(home).unwrap();
    }
}
