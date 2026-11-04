//! Local specialist management for a live parent Agent.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use serde_json::{Value, json};
use willdeep_core::agent_definition::AgentDefinition;
use willdeep_core::provider::ProviderConfig;
use willdeep_core::tools::{AgentDefinitionHost, ToolError};
use willdeep_core::{SubagentCatalog, SubagentProfile};

#[cfg(test)]
mod tests;

struct ObservedInstallation {
    scope: String,
    revision: u64,
}

#[derive(Default)]
struct LibraryState {
    observed: BTreeMap<String, ObservedInstallation>,
    // Keep the first injected target stable across retries, even after a successful
    // write or a subsequent read advances the observation for the same definition.
    requests: BTreeMap<String, (String, Value)>,
}

impl LibraryState {
    fn prepare(&mut self, operation: &str, params: &mut Value) -> anyhow::Result<()> {
        let request_id = params
            .get("request_id")
            .and_then(Value::as_str)
            .context("request_id required")?
            .to_owned();
        if let Some((previous_operation, previous_params)) = self.requests.get(&request_id) {
            if params.get("target_scope").is_none() {
                params["target_scope"] = previous_params["target_scope"].clone();
            }
            anyhow::ensure!(
                previous_operation == operation && previous_params == &*params,
                "agent_request_conflict: request_id reused with different payload"
            );
            return Ok(());
        }
        let scope = if operation == "create" {
            // New definitions always start locally and need no prior observation.
            "local"
        } else {
            let id = if operation == "update" {
                let definition = match params.get("definition_text").and_then(Value::as_str) {
                    Some(text) => AgentDefinition::parse(text.as_bytes())?,
                    None => AgentDefinition::parse(&serde_json::to_vec(
                        params
                            .get("definition")
                            .context("agent definition required")?,
                    )?)?,
                };
                definition.definition_id
            } else {
                // For fork this is the source, never the new destination id.
                params
                    .get("id")
                    .and_then(Value::as_str)
                    .context("agent id required")?
                    .to_owned()
            };
            let observed = self.observed.get(&id).context(
                "agent_target_required: read the current definition with list/get before managing it",
            )?;
            let expected = params
                .get("expected_revision")
                .and_then(Value::as_u64)
                .context(
                    "agent_revision_required: expected_revision must be an installation integer",
                )?;
            anyhow::ensure!(
                expected == observed.revision,
                "agent_revision_conflict: expected_revision differs from the host observation; reload before retrying"
            );
            observed.scope.as_str()
        };
        if let Some(target) = params.get("target_scope") {
            anyhow::ensure!(
                target.as_str() == Some(scope),
                "agent_scope_conflict: target_scope differs from the host observation; reload before retrying"
            );
        }
        params["target_scope"] = scope.into();
        self.requests
            .insert(request_id, (operation.to_owned(), params.clone()));
        Ok(())
    }

    fn observe(&mut self, operation: &str, receipt: &Value) -> anyhow::Result<()> {
        if operation == "list" {
            let mut observed = BTreeMap::new();
            for item in receipt["items"]
                .as_array()
                .context("agent list items missing")?
            {
                let (id, target) = Self::observation(item, "scope")?;
                observed.insert(id, target);
            }
            self.observed = observed;
        } else if operation == "get" || receipt["replayed"] == false {
            // Historical replay receipts must not roll back a newer observation.
            let (id, target) = Self::observation(receipt, "target_scope")?;
            self.observed.insert(id, target);
        }
        Ok(())
    }

    fn observation(
        value: &Value,
        scope_field: &str,
    ) -> anyhow::Result<(String, ObservedInstallation)> {
        let id = value
            .get("definition_id")
            .or_else(|| value.pointer("/installation/definition_id"))
            .and_then(Value::as_str)
            .context("agent receipt definition id missing")?;
        let scope = value[scope_field]
            .as_str()
            .filter(|scope| matches!(*scope, "local" | "private"))
            .context("agent receipt target scope missing")?;
        let revision = value["installation_revision"]
            .as_u64()
            .context("agent receipt installation revision missing")?;
        Ok((
            id.to_owned(),
            ObservedInstallation {
                scope: scope.to_owned(),
                revision,
            },
        ))
    }
}

pub(super) struct LibraryHost {
    home: PathBuf,
    scope: Value,
    state: tokio::sync::Mutex<LibraryState>,
    catalog: Arc<SubagentCatalog>,
    base_profiles: Vec<SubagentProfile>,
    file: crate::config::ConfigFile,
    parent: ProviderConfig,
}

impl LibraryHost {
    pub(super) fn new(
        home: &Path,
        catalog: Arc<SubagentCatalog>,
        base_profiles: Vec<SubagentProfile>,
        file: crate::config::ConfigFile,
        parent: ProviderConfig,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            home: home.to_path_buf(),
            scope: crate::agent_registry::capture_scope(home)?,
            state: tokio::sync::Mutex::new(LibraryState::default()),
            catalog,
            base_profiles,
            file,
            parent,
        })
    }

    fn refresh(&self) -> anyhow::Result<()> {
        let _scope_lock =
            willdeep_core::agent_definition::store::AgentLibrary::new(&self.home).lock_scope()?;
        anyhow::ensure!(
            crate::agent_registry::capture_scope(&self.home)? == self.scope,
            "agent_scope_conflict: account changed before catalog refresh"
        );
        let profiles = crate::agent_registry::runtime_profiles(
            &self.home,
            &self.base_profiles,
            &self.file,
            &self.parent,
        )?;
        self.catalog.replace_specialist_profiles(profiles)?;
        Ok(())
    }
}

#[async_trait]
impl AgentDefinitionHost for LibraryHost {
    async fn execute(&self, operation: &str, mut params: Value) -> Result<Value, ToolError> {
        let mutation = matches!(operation, "create" | "update" | "fork" | "bind" | "enable");
        if !mutation && !matches!(operation, "list" | "get" | "history") {
            return Err(ToolError::Network(
                "unsupported local Agent library operation".into(),
            ));
        }
        // Serialize this host's reads, writes and observation updates. Storage CAS
        // still handles other hosts and processes under the shared library lock.
        let mut state = self.state.lock().await;
        let error = |error| ToolError::Network(format!("Agent library {operation}: {error:#}"));
        if mutation {
            let arguments = params.as_object_mut().ok_or_else(|| {
                ToolError::Network("Agent library parameters must be an object".into())
            })?;
            // Account identity and the observed target are enforced by the host.
            arguments.insert("expected_scope".into(), self.scope.clone());
            state.prepare(operation, &mut params).map_err(error)?;
        }
        let mut receipt = crate::agent_registry::operate(&self.home, operation, params)
            .await
            .map_err(error)?;
        if mutation || matches!(operation, "list" | "get") {
            if receipt["scope"] != self.scope {
                return Err(ToolError::Network(
                    "agent_scope_conflict: account or library changed; reload before retrying"
                        .into(),
                ));
            }
            // A committed write is observed even when catalog projection fails.
            state.observe(operation, &receipt).map_err(error)?;
        }
        if mutation {
            let refresh = self.refresh().map_err(|error| {
                // The write is already committed. Stop new dispatches from using
                // stale specialist definitions until a later refresh succeeds.
                match self.catalog.replace_specialist_profiles(Vec::new()) {
                    Ok(()) => error,
                    Err(clear_error) => error.context(format!(
                        "stale specialist catalog could not be cleared: {clear_error}"
                    )),
                }
            });
            if let Some(object) = receipt.as_object_mut() {
                object.insert("applied".into(), json!(refresh.is_ok()));
                object.insert("catalog_refreshed".into(), json!(refresh.is_ok()));
                if let Err(error) = refresh {
                    object.insert("status".into(), json!("saved_not_applied"));
                    object.insert("refresh_error".into(), json!(format!("{error:#}")));
                }
            }
        }
        Ok(receipt)
    }
}
