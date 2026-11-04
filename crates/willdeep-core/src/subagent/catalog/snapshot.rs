//! Shared catalog replacement and credential-free execution snapshots.
use super::*;
use crate::agent_definition::store::ModelBinding;
use serde::{Deserialize, Serialize};

#[path = "workflow_snapshot.rs"]
mod workflow_snapshot;

pub(super) type BindingResolver =
    dyn Fn(&ModelBinding, Arc<dyn Provider>) -> Result<Arc<dyn Provider>, AgentError> + Send + Sync;

/// Every execution-owned policy is frozen here. Provider objects and ProviderConfig never enter it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ExecutionSnapshot {
    base_profile: String,
    tier: Option<crate::WorkerTier>,
    binding: ModelBinding,
    #[serde(default)]
    provider_identity: Option<String>,
    id: String,
    purpose: String,
    tool_names: Vec<String>,
    capability_prompt: String,
    max_turns: usize,
    context_window: u64,
    token_budget: Option<u64>,
    timeout_seconds: Option<u64>,
    max_consecutive_failures: usize,
    write_scope: SubagentWriteScope,
    dedicated: bool,
    tool_output_limit: Option<usize>,
    max_attempts: usize,
    shell: super::super::types::SubagentShell,
    hosted_job_prompt: bool,
    definition_model_bound: bool,
    definition_binding: Option<ModelBinding>,
}

impl ExecutionSnapshot {
    pub(super) fn tier(&self) -> Option<crate::WorkerTier> {
        self.tier
    }

    pub(super) fn capture(profile: &SubagentProfile, tier: Option<crate::WorkerTier>) -> Self {
        let mut binding = profile.definition_binding.clone().unwrap_or_default();
        binding.model = profile
            .model
            .clone()
            .or_else(|| crate::provider::provider_model(profile.provider.as_ref()));
        binding.context_window = Some(profile.context_window);
        binding.max_turns = Some(profile.max_turns);
        Self {
            base_profile: profile
                .definition
                .as_ref()
                .map_or(&profile.id, |d| &d.base_role)
                .clone(),
            tier,
            binding,
            provider_identity: profile
                .provider
                .ledger_identity()
                .map(|identity| identity.provider),
            id: profile.id.clone(),
            purpose: profile.purpose.clone(),
            tool_names: profile.tool_names.clone(),
            capability_prompt: profile.capability_prompt.clone(),
            max_turns: profile.max_turns,
            context_window: profile.context_window,
            token_budget: profile.token_budget,
            timeout_seconds: profile.timeout_seconds,
            max_consecutive_failures: profile.max_consecutive_failures,
            write_scope: profile.write_scope,
            dedicated: profile.worktree == SubagentWorktreePolicy::Dedicated,
            tool_output_limit: profile.tool_output_limit,
            max_attempts: profile.max_attempts,
            shell: profile.shell,
            hosted_job_prompt: profile.hosted_job_prompt,
            definition_model_bound: profile.definition_model_bound,
            definition_binding: profile.definition_binding.clone(),
        }
    }
}

impl SubagentCatalog {
    pub(super) fn profile_snapshot(&self) -> Arc<BTreeMap<String, SubagentProfile>> {
        self.profiles
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Replace only installed specialist definitions; built-in responsibilities remain intact.
    /// Cloned catalogs see the same replacement. Existing workers retain their owned profile.
    pub fn replace_specialist_profiles(
        &self,
        profiles: Vec<SubagentProfile>,
    ) -> Result<(), AgentError> {
        let mut specialists = BTreeMap::new();
        for profile in profiles {
            let definition = profile.definition.as_ref().ok_or_else(|| {
                AgentError::Subagent("specialist refresh requires an agent definition".into())
            })?;
            definition
                .validate()
                .map_err(|e| AgentError::Subagent(e.to_string()))?;
            if profile.id != definition.definition_id
                || PUBLIC_SUBAGENT_IDS.contains(&profile.id.as_str())
            {
                return Err(AgentError::Subagent(
                    "specialist refresh cannot replace a base responsibility".into(),
                ));
            }
            if specialists.insert(profile.id.clone(), profile).is_some() {
                return Err(AgentError::Subagent(
                    "duplicate specialist ID in catalog refresh".into(),
                ));
            }
        }
        let mut current = self
            .profiles
            .write()
            .map_err(|_| AgentError::Subagent("subagent catalog unavailable".into()))?;
        let mut next: BTreeMap<_, _> = current
            .iter()
            .filter(|(_, profile)| profile.definition.is_none())
            .map(|(id, profile)| (id.clone(), profile.clone()))
            .collect();
        if specialists.keys().any(|id| next.contains_key(id)) {
            return Err(AgentError::Subagent(
                "specialist ID collides with a base responsibility".into(),
            ));
        }
        next.extend(specialists);
        *current = Arc::new(next);
        Ok(())
    }

    /// Host resolver uses the existing model factory with current credentials and permissions.
    pub fn with_definition_bindings<F>(mut self, resolver: F) -> Self
    where
        F: Fn(&ModelBinding, Arc<dyn Provider>) -> Result<Arc<dyn Provider>, AgentError>
            + Send
            + Sync
            + 'static,
    {
        self.definition_bindings = Some(Arc::new(resolver));
        self
    }

    pub(super) fn restored_profile(
        &self,
        record: &super::super::dispatch_store::DispatchRecord,
    ) -> Result<SubagentProfile, AgentError> {
        let Some(snapshot) = &record.execution else {
            // Older built-in dispatches can still use their original model/limits. A legacy
            // specialist has no reliable record of its provider binding: never guess from latest.
            if record.definition.is_some() {
                return Err(AgentError::SubagentBlocked("legacy specialist dispatch has no execution binding snapshot; explicit redispatch is required".into()));
            }
            let mut profile = self
                .profile(record.args.profile.as_deref())
                .ok_or_else(|| {
                    AgentError::SubagentBlocked("original base profile is unavailable".into())
                })?;
            if profile.definition.is_some() {
                return Err(AgentError::SubagentBlocked(
                    "legacy dispatch resolved to a specialist without an execution snapshot".into(),
                ));
            }
            if let Some(model) = &record.model {
                if profile.model.as_ref() != Some(model) {
                    profile.provider = profile.provider.with_model(model)?;
                }
                profile.model = Some(model.clone());
            }
            return Ok(profile);
        };
        self.restore_execution(snapshot, record.definition.clone())
    }

    fn restore_execution(
        &self,
        snapshot: &ExecutionSnapshot,
        definition: Option<crate::agent_definition::AgentDefinition>,
    ) -> Result<SubagentProfile, AgentError> {
        let bases = self.profile_snapshot();
        let mut profile = bases
            .get(&snapshot.base_profile)
            .filter(|p| p.definition.is_none())
            .cloned()
            .ok_or_else(|| {
                AgentError::SubagentBlocked("original definition base role unavailable".into())
            })?;
        let mut configured_model = profile
            .model
            .clone()
            .or_else(|| crate::provider::provider_model(profile.provider.as_ref()));
        if let Some(tier) = snapshot.tier {
            let binding = self.tier_bindings.get(&tier).ok_or_else(|| {
                AgentError::SubagentBlocked("original worker tier provider is unavailable".into())
            })?;
            profile.provider = binding.provider.clone();
            configured_model = binding
                .model
                .clone()
                .or_else(|| crate::provider::provider_model(profile.provider.as_ref()));
        }
        if snapshot.binding.provider_profile.is_some() {
            let resolver = self.definition_bindings.as_ref().ok_or_else(|| {
                AgentError::SubagentBlocked(
                    "original provider profile requires the host definition binding resolver"
                        .into(),
                )
            })?;
            profile.provider =
                resolver(&snapshot.binding, profile.provider.clone()).map_err(|error| {
                    AgentError::SubagentBlocked(format!(
                        "original provider binding cannot be restored: {error}"
                    ))
                })?;
            // The host factory owns model selection when its provider cannot expose an identity.
            configured_model = crate::provider::provider_model(profile.provider.as_ref())
                .or_else(|| snapshot.binding.model.clone());
        }
        if snapshot.provider_identity.as_ref().is_some_and(|expected| {
            profile
                .provider
                .ledger_identity()
                .as_ref()
                .map(|identity| &identity.provider)
                != Some(expected)
        }) {
            return Err(AgentError::SubagentBlocked("original provider route is unavailable; explicitly redispatch with a current binding".into()));
        }
        if let Some(model) = &snapshot.binding.model
            && (configured_model.as_ref() != Some(model)
                || crate::provider::provider_model(profile.provider.as_ref()).is_some())
        {
            profile.provider = profile.provider.with_model(model)?;
        }
        let actual = profile.provider.ledger_identity();
        if snapshot.provider_identity.as_ref().is_some_and(|expected| {
            actual.as_ref().map(|identity| &identity.provider) != Some(expected)
        }) || actual.as_ref().is_some_and(|identity| {
            snapshot
                .binding
                .model
                .as_ref()
                .is_some_and(|expected| expected != &identity.model)
        }) {
            return Err(AgentError::SubagentBlocked(
                "original provider/model binding cannot be restored".into(),
            ));
        }
        profile.definition = definition;
        profile.definition_model_bound = snapshot.definition_model_bound;
        profile.definition_binding = snapshot.definition_binding.clone();
        profile.id = snapshot.id.clone();
        profile.purpose = snapshot.purpose.clone();
        profile.model = snapshot.binding.model.clone();
        profile.tool_names = snapshot.tool_names.clone();
        profile.capability_prompt = snapshot.capability_prompt.clone();
        profile.max_turns = snapshot.max_turns;
        profile.context_window = snapshot.context_window;
        profile.token_budget = snapshot.token_budget;
        profile.timeout_seconds = snapshot.timeout_seconds;
        profile.max_consecutive_failures = snapshot.max_consecutive_failures;
        profile.write_scope = snapshot.write_scope;
        profile.worktree = if snapshot.dedicated {
            SubagentWorktreePolicy::Dedicated
        } else {
            SubagentWorktreePolicy::Shared
        };
        profile.tool_output_limit = snapshot.tool_output_limit;
        profile.max_attempts = snapshot.max_attempts;
        profile.shell = snapshot.shell;
        profile.hosted_job_prompt = snapshot.hosted_job_prompt;
        Ok(profile)
    }
}
