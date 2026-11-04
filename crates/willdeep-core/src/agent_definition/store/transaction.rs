//! Local installation transactions. Packages remain portable v1 immutable objects.
use super::*;
use anyhow::Context;
use serde_json::Value;

const STATE_SCHEMA: &str = "willdeep.agent-installations.v2";

#[derive(Default, Serialize, Deserialize)]
pub(super) struct State {
    schema: String,
    pub(super) installations: BTreeMap<String, Installation>,
    #[serde(default)]
    pub(super) clock: u64,
    #[serde(default)]
    pub(super) receipts: BTreeMap<String, StoredReceipt>,
    #[serde(default)]
    committed_versions: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct StoredReceipt {
    fingerprint: String,
    installation: Installation,
}

impl State {
    pub(super) fn next_revision(&mut self) -> Result<u64> {
        self.clock = self
            .clock
            .checked_add(1)
            .context("installation revision exhausted")?;
        Ok(self.clock)
    }
}

/// Expected revision is the local installation generation, never a package SHA.
#[derive(Clone, Debug)]
pub struct MutationRequest {
    pub request_id: String,
    pub expected_revision: Option<u64>,
    /// Complete operation parameters. Only their canonical hash is persisted.
    pub payload: Value,
}

#[derive(Clone, Debug)]
pub struct MutationReceipt {
    pub installation: Installation,
    pub replayed: bool,
}

pub enum Mutation<'a> {
    Install {
        package: &'a AgentPackage,
        origin: Origin,
        keys: &'a BTreeMap<String, String>,
        binding: Option<ModelBinding>,
        compatible: bool,
        create_only: bool,
        allow_unchanged: bool,
    },
    Fork {
        source: &'a AgentLibrary,
        id: &'a str,
        new_id: &'a str,
    },
    Bind {
        id: &'a str,
        binding: ModelBinding,
    },
    Enable {
        id: &'a str,
        value: bool,
    },
    Pin {
        id: &'a str,
        value: bool,
    },
    Rollback {
        id: &'a str,
        revision: &'a str,
    },
}

impl Mutation<'_> {
    fn identity(&self) -> Value {
        use serde_json::json;
        match self {
            Self::Install {
                package,
                origin,
                binding,
                compatible,
                create_only,
                allow_unchanged,
                ..
            } => {
                json!({"operation":"install","package":package,"origin":origin,"binding":binding,"compatible":compatible,"create_only":create_only,"allow_unchanged":allow_unchanged})
            }
            Self::Fork { source, id, new_id } => {
                json!({"operation":"fork","source":source.root(),"id":id,"new_id":new_id})
            }
            Self::Bind { id, binding } => json!({"operation":"bind","id":id,"binding":binding}),
            Self::Enable { id, value } => json!({"operation":"enable","id":id,"value":value}),
            Self::Pin { id, value } => json!({"operation":"pin","id":id,"value":value}),
            Self::Rollback { id, revision } => {
                json!({"operation":"rollback","id":id,"revision":revision})
            }
        }
    }
}

fn sorted_value(value: Value) -> Value {
    match value {
        Value::Object(items) => Value::Object(
            items
                .into_iter()
                .map(|(key, value)| (key, sorted_value(value)))
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(sorted_value).collect()),
        value => value,
    }
}

impl AgentLibrary {
    pub(super) fn state(&self) -> Result<State> {
        let bytes = match fs::read(self.root.join("installations.json")) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(State::default());
            }
            Err(error) => return Err(error.into()),
        };
        let value: Value = serde_json::from_slice(&bytes)?;
        let mut state = if value.get("schema").and_then(Value::as_str).is_some() {
            let state: State = serde_json::from_value(value)?;
            ensure!(
                state.schema == STATE_SCHEMA,
                "unsupported installation state schema"
            );
            state
        } else {
            // Missing generations consistently read as 1, without mutating on a read.
            // The first successful write persists the migration under the same lock.
            State {
                installations: serde_json::from_value(value)?,
                ..State::default()
            }
        };
        if state.schema.is_empty() {
            for id in state.installations.keys() {
                state.committed_versions.insert(
                    id.clone(),
                    self.history(id)?
                        .into_iter()
                        .map(|package| (package.definition.version, package.sha256))
                        .collect(),
                );
            }
        }
        state.clock = state.clock.max(
            state
                .installations
                .values()
                .map(|item| item.installation_revision)
                .max()
                .unwrap_or(0),
        );
        Ok(state)
    }

    pub(super) fn save_state(&self, state: &State) -> Result<()> {
        let mut value = serde_json::to_value(state)?;
        for item in state.installations.values() {
            let package = self.package(&item.sha256)?;
            value["committed_versions"][&item.definition_id][&package.definition.version] =
                item.sha256.clone().into();
        }
        value["schema"] = STATE_SCHEMA.into();
        private_write(
            &self.root.join("installations.json"),
            &serde_json::to_vec_pretty(&value)?,
        )
    }

    /// Scope checks, CAS, request receipts and the pointer/state commit share .lock.
    /// A package may be archived before the commit; an interrupted transaction can
    /// leave an unselected immutable object, but cannot publish a partial installation.
    pub fn mutate(
        &self,
        request: &MutationRequest,
        check_scope: impl FnOnce() -> Result<()>,
        mutation: Mutation<'_>,
    ) -> Result<MutationReceipt> {
        ensure!(
            !request.request_id.trim().is_empty()
                && request.request_id.len() <= 128
                && !request.request_id.chars().any(char::is_control),
            "request_id required (at most 128 characters)"
        );
        // Portable v1 canonicalization deliberately rejects numbers. Installation
        // transactions contain integer revisions/limits, so use a separate encoding.
        let fingerprint = digest(
            serde_json::to_vec(&sorted_value(serde_json::json!({
                "expected_revision": request.expected_revision,
                "payload": request.payload,
                "mutation": mutation.identity()
            })))?
            .as_slice(),
        );
        let _lock = self.lock()?;
        check_scope()?;
        let mut state = self.state()?;
        if let Some(receipt) = state.receipts.get(&request.request_id) {
            ensure!(
                receipt.fingerprint == fingerprint,
                "agent_request_conflict: request_id reused with different payload"
            );
            return Ok(MutationReceipt {
                installation: receipt.installation.clone(),
                replayed: true,
            });
        }
        let mut item = match mutation {
            Mutation::Install {
                package,
                origin,
                keys,
                binding,
                compatible,
                create_only,
                allow_unchanged,
            } => self.prepare_install(
                &state,
                request.expected_revision,
                package,
                origin,
                keys,
                binding,
                compatible,
                create_only,
                allow_unchanged,
            )?,
            Mutation::Fork { source, id, new_id } => {
                ensure!(
                    source.lock_root == self.lock_root,
                    "agent_scope_conflict: fork source belongs to another library"
                );
                let source_items = source.installations()?;
                let source_item = source_items.get(id).context("agent not installed")?;
                check_revision(source_item, request.expected_revision)?;
                ensure!(
                    !state.installations.contains_key(new_id),
                    "agent_revision_conflict: agent already installed; create requires an absent id"
                );
                let mut definition = source.package(&source_item.sha256)?.definition;
                definition.forked_from = Some(source_item.sha256.clone());
                definition.definition_id = new_id.into();
                definition.version = "1.0.0".into();
                let package = AgentPackage::local(definition)?;
                self.prepare_install(
                    &state,
                    None,
                    &package,
                    Origin::Local,
                    &BTreeMap::new(),
                    None,
                    source_item.enabled,
                    true,
                    false,
                )?
            }
            mutation => {
                let id = match &mutation {
                    Mutation::Bind { id, .. }
                    | Mutation::Enable { id, .. }
                    | Mutation::Pin { id, .. }
                    | Mutation::Rollback { id, .. } => *id,
                    _ => unreachable!(),
                };
                let mut item = state
                    .installations
                    .get(id)
                    .cloned()
                    .context("agent not installed")?;
                check_revision(&item, request.expected_revision)?;
                match mutation {
                    Mutation::Bind { binding, .. } => {
                        Self::validate_binding(&binding)?;
                        item.binding = binding;
                    }
                    Mutation::Enable { value, .. } => item.enabled = value,
                    Mutation::Pin { value, .. } => item.pinned = value,
                    Mutation::Rollback { revision, .. } => {
                        let package = self.package(revision)?;
                        ensure!(
                            package.definition.definition_id == id,
                            "revision belongs to another agent"
                        );
                        item.sha256 = revision.into();
                        item.pinned = true;
                        item.dirty = true;
                    }
                    _ => unreachable!(),
                }
                item
            }
        };
        item.installation_revision = state.next_revision()?;
        state
            .installations
            .insert(item.definition_id.clone(), item.clone());
        state.receipts.insert(
            request.request_id.clone(),
            StoredReceipt {
                fingerprint,
                installation: item.clone(),
            },
        );
        self.save_state(&state)?;
        Ok(MutationReceipt {
            installation: item,
            replayed: false,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_install(
        &self,
        state: &State,
        expected_revision: Option<u64>,
        package: &AgentPackage,
        origin: Origin,
        keys: &BTreeMap<String, String>,
        binding: Option<ModelBinding>,
        compatible: bool,
        create_only: bool,
        allow_unchanged: bool,
    ) -> Result<Installation> {
        AgentPackage::parse(&serde_json::to_vec(package)?)?;
        if origin == Origin::Official {
            package.verify_official(keys)?;
            ensure!(
                package.definition.definition_id.starts_with("official/"),
                "invalid official namespace"
            );
        } else {
            ensure!(
                package.signature.is_none()
                    && package.definition.definition_id.starts_with("user/"),
                "official edits require a fork; local definitions require user namespace and no signature"
            );
        }
        let id = &package.definition.definition_id;
        let old = state.installations.get(id);
        if let Some(old) = old {
            ensure!(
                !create_only,
                "agent_revision_conflict: agent already installed; create requires an absent id"
            );
            check_revision(old, expected_revision)?;
            if origin != Origin::Official {
                ensure!(
                    old.origin != Origin::Official
                        && self.package(&old.sha256)?.signature.is_none(),
                    "official edits require a fork"
                );
            }
            if allow_unchanged && old.sha256 == package.sha256 {
                ensure!(
                    binding
                        .as_ref()
                        .is_none_or(|binding| binding == &old.binding),
                    "unchanged agent package cannot change its model binding; use bind"
                );
                return Ok(old.clone());
            }
            ensure!(!old.pinned, "agent revision is pinned");
        } else {
            ensure!(
                expected_revision.is_none(),
                "agent_revision_conflict: installation no longer exists"
            );
        }
        for previous in self.history(id)? {
            ensure!(
                previous.definition.version != package.definition.version
                    || previous.sha256 == package.sha256,
                "same agent version has different content"
            );
        }
        if old.is_some() {
            for version in state
                .committed_versions
                .get(id)
                .into_iter()
                .flat_map(|versions| versions.keys())
            {
                ensure!(
                    version_cmp(&package.definition.version, version).is_gt(),
                    "agent version must strictly increase beyond installation history; use rollback for historical revisions"
                );
            }
        }
        let binding =
            binding.unwrap_or_else(|| old.map(|item| item.binding.clone()).unwrap_or_default());
        Self::validate_binding(&binding)?;
        let path = self
            .root
            .join("revisions")
            .join(format!("{}.json", package.sha256));
        if path.exists() {
            ensure!(
                self.package(&package.sha256)?.definition == package.definition,
                "immutable package mismatch"
            );
        }
        private_write(&path, &serde_json::to_vec_pretty(package)?)?;
        Ok(Installation {
            definition_id: id.clone(),
            sha256: package.sha256.clone(),
            installation_revision: 0,
            origin: origin.clone(),
            enabled: compatible && old.is_none_or(|item| item.enabled),
            pinned: false,
            remote_etag: old.and_then(|item| item.remote_etag.clone()),
            dirty: origin != Origin::Official,
            binding,
        })
    }
}

fn check_revision(item: &Installation, expected: Option<u64>) -> Result<()> {
    ensure!(
        expected.is_some(),
        "agent_revision_required: expected_revision (installation_revision) required for updates"
    );
    ensure!(
        expected == Some(item.installation_revision),
        "agent_revision_conflict: installation changed; reload before retrying"
    );
    Ok(())
}

/// Definition v1 supports numeric X.Y.Z[-rcN]. The project treats a stable
/// version as rc0 when editing it, so 1.0.0 -> 1.0.0-rc1 is a valid local edit.
fn version_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    fn parts(version: &str) -> Vec<&str> {
        let (base, rc) = version.split_once("-rc").unwrap_or((version, "0"));
        base.split('.').chain(std::iter::once(rc)).collect()
    }
    parts(left)
        .iter()
        .zip(parts(right))
        .map(|(a, b)| a.len().cmp(&b.len()).then_with(|| a.cmp(&b)))
        .find(|order| !order.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}
