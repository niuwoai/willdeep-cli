//! A run-owned catalog. Only definitions and execution policies cross the disk boundary.
use super::*;
use crate::agent_definition::AgentDefinition;
use serde_json::Value;

const WORKFLOW_SNAPSHOT_VERSION: u32 = 1;
const MAX_WORKFLOW_SNAPSHOT_BYTES: usize =
    willdeep_runtime_protocol::workflow::WORKFLOW_MAX_RUN_BYTES / 4;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkflowSnapshot {
    version: u32,
    profiles: BTreeMap<String, WorkflowProfile>,
    tiers: BTreeMap<crate::WorkerTier, WorkflowTier>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkflowProfile {
    execution: ExecutionSnapshot,
    definition: Option<AgentDefinition>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkflowTier {
    execution: ExecutionSnapshot,
    window: u64,
    hosted_job_prompt: bool,
}

fn snapshot_error(error: impl std::fmt::Display) -> AgentError {
    AgentError::SubagentBlocked(format!("workflow catalog snapshot: {error}"))
}

impl SubagentCatalog {
    /// Host-only, credential-free snapshot for one complete workflow run.
    /// Capture every responsibility and tier before any leaf is dispatched.
    pub fn export_workflow_snapshot(&self) -> Result<Value, AgentError> {
        let profiles = self.profile_snapshot();
        let mut tiers = BTreeMap::new();
        for (tier, binding) in &self.tier_bindings {
            let mut base = profiles
                .values()
                .find(|p| p.definition.is_none())
                .cloned()
                .ok_or_else(|| snapshot_error("no base responsibility for worker tiers"))?;
            base.provider = binding.provider.clone();
            base.model = binding.model.clone();
            base.definition_binding = None;
            tiers.insert(
                *tier,
                WorkflowTier {
                    execution: ExecutionSnapshot::capture(&base, Some(*tier)),
                    window: binding.window,
                    hosted_job_prompt: binding.hosted_job_prompt,
                },
            );
        }
        let snapshot = WorkflowSnapshot {
            version: WORKFLOW_SNAPSHOT_VERSION,
            profiles: profiles
                .iter()
                .map(|(id, profile)| {
                    (
                        id.clone(),
                        WorkflowProfile {
                            execution: ExecutionSnapshot::capture(profile, None),
                            definition: profile.definition.clone(),
                        },
                    )
                })
                .collect(),
            tiers,
        };
        let value = serde_json::to_value(snapshot).map_err(snapshot_error)?;
        crate::workflow::check_value(&value, MAX_WORKFLOW_SNAPSHOT_BYTES)
            .map_err(snapshot_error)?;
        Ok(value)
    }

    /// Restore the original definitions/model routes using the host's current credentials.
    /// Only profiles and tier bindings detach: registry, Scheduler, admission lock,
    /// file leases and live approval handles remain shared with the parent catalog.
    #[allow(clippy::wrong_self_convention)] // The host supplies live credentials and shared handles.
    pub fn from_workflow_snapshot(&self, value: &Value) -> Result<SubagentCatalog, AgentError> {
        crate::workflow::check_value(value, MAX_WORKFLOW_SNAPSHOT_BYTES).map_err(snapshot_error)?;
        let snapshot: WorkflowSnapshot =
            serde_json::from_value(value.clone()).map_err(snapshot_error)?;
        if snapshot.version != WORKFLOW_SNAPSHOT_VERSION {
            return Err(snapshot_error("unsupported version"));
        }
        let mut profiles = BTreeMap::new();
        for (id, saved) in snapshot.profiles {
            if id != saved.execution.id || saved.execution.tier.is_some() {
                return Err(snapshot_error("profile identity/tier mismatch"));
            }
            if let Some(definition) = &saved.definition {
                definition.validate().map_err(snapshot_error)?;
                if definition.definition_id != id
                    || definition.base_role != saved.execution.base_profile
                {
                    return Err(snapshot_error("definition identity/base role mismatch"));
                }
            } else if id != saved.execution.base_profile {
                return Err(snapshot_error("base responsibility identity mismatch"));
            }
            let profile = self.restore_execution(&saved.execution, saved.definition)?;
            profiles.insert(id, profile);
        }
        let mut tiers = BTreeMap::new();
        for (tier, saved) in snapshot.tiers {
            if saved.execution.tier != Some(tier) {
                return Err(snapshot_error("worker tier identity mismatch"));
            }
            let profile = self.restore_execution(&saved.execution, None)?;
            tiers.insert(
                tier,
                TierBinding {
                    provider: profile.provider,
                    model: profile.model,
                    window: saved.window,
                    hosted_job_prompt: saved.hosted_job_prompt,
                },
            );
        }
        let mut frozen = self.clone();
        frozen.profiles = Arc::new(std::sync::RwLock::new(Arc::new(profiles)));
        frozen.tier_bindings = tiers;
        Ok(frozen)
    }
}
