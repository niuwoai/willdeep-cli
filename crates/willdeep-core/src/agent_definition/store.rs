use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::{AgentPackage, digest, valid_hash};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Local,
    Official,
    Private,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installation {
    pub definition_id: String,
    pub sha256: String,
    pub origin: Origin,
    pub enabled: bool,
    pub pinned: bool,
    pub remote_etag: Option<String>,
    pub dirty: bool,
    #[serde(default)]
    pub binding: ModelBinding,
}

/// Local inference configuration; never included in portable packages.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelBinding {
    pub provider_profile: Option<String>,
    pub model: Option<String>,
    pub context_window: Option<u64>,
    pub max_turns: Option<usize>,
}

impl ModelBinding {
    pub const MAX_CONTEXT_WINDOW: u64 = 2_000_000;
    pub const MAX_TURNS: usize = 1000;

    pub fn apply_limits(&self, profile: &mut crate::SubagentProfile) {
        if let Some(window) = self.context_window {
            profile.context_window = window;
        }
        if let Some(turns) = self.max_turns {
            profile.max_turns = turns;
        }
    }
}

#[derive(Clone, Debug)]
pub struct AgentLibrary {
    root: PathBuf,
}

impl AgentLibrary {
    pub fn new(home: &Path) -> Self {
        Self {
            root: home.join("agents"),
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn installations(&self) -> Result<BTreeMap<String, Installation>> {
        match fs::read(self.root.join("installations.json")) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn package(&self, revision: &str) -> Result<AgentPackage> {
        ensure!(valid_hash(revision), "invalid revision");
        let package = AgentPackage::parse(&fs::read(
            self.root.join("revisions").join(format!("{revision}.json")),
        )?)?;
        ensure!(package.sha256 == revision, "stored revision mismatch");
        Ok(package)
    }
    pub fn install(
        &self,
        package: &AgentPackage,
        origin: Origin,
        keys: &BTreeMap<String, String>,
    ) -> Result<()> {
        self.install_synced(package, origin, keys, None)
    }
    pub fn install_synced(
        &self,
        package: &AgentPackage,
        origin: Origin,
        keys: &BTreeMap<String, String>,
        etag: Option<String>,
    ) -> Result<()> {
        self.install_with_options(package, origin, keys, etag, None)
    }

    pub fn create_bound(&self, package: &AgentPackage, binding: ModelBinding) -> Result<()> {
        Self::validate_binding(&binding)?;
        self.install_with_options(
            package,
            Origin::Local,
            &BTreeMap::new(),
            None,
            Some(binding),
        )
    }

    fn install_with_options(
        &self,
        package: &AgentPackage,
        origin: Origin,
        keys: &BTreeMap<String, String>,
        etag: Option<String>,
        new_binding: Option<ModelBinding>,
    ) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(package)?;
        AgentPackage::parse(&bytes)?;
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
                "local definitions require user namespace and no signature"
            );
        }
        let _lock = self.lock()?;
        let mut items = self.installations()?;
        let id = &package.definition.definition_id;
        ensure!(
            new_binding.is_none() || !items.contains_key(id),
            "agent id already installed"
        );
        for previous in self.history(id)? {
            ensure!(
                previous.definition.version != package.definition.version
                    || previous.sha256 == package.sha256,
                "same agent version has different content"
            );
        }
        if let Some(old) = items.get(id) {
            ensure!(
                etag.is_none() || !old.dirty || old.sha256 == package.sha256,
                "local agent changed during remote synchronization"
            );
            ensure!(
                !old.pinned || old.sha256 == package.sha256,
                "agent revision is pinned"
            );
            let previous = self.package(&old.sha256)?;
            ensure!(
                previous.definition.version != package.definition.version
                    || old.sha256 == package.sha256,
                "same agent version has different content"
            );
        }
        let path = self
            .root
            .join("revisions")
            .join(format!("{}.json", package.sha256));
        if path.exists() {
            let previous = self.package(&package.sha256)?;
            ensure!(
                previous.definition == package.definition,
                "immutable package mismatch"
            );
            if previous.signature != package.signature {
                private_write(&path, &bytes)?;
            }
        } else {
            private_write(&path, &bytes)?;
        }
        let pinned = items.get(id).is_some_and(|item| item.pinned);
        let enabled = items.get(id).is_none_or(|item| item.enabled);
        let binding = new_binding.unwrap_or_else(|| {
            items
                .get(id)
                .map(|item| item.binding.clone())
                .unwrap_or_default()
        });
        let dirty = origin != Origin::Official && etag.is_none();
        items.insert(
            id.clone(),
            Installation {
                definition_id: id.clone(),
                sha256: package.sha256.clone(),
                origin,
                enabled,
                pinned,
                dirty,
                binding,
                remote_etag: etag,
            },
        );
        self.save(&items)
    }
    pub fn select(&self, id: &str, revision: &str) -> Result<()> {
        let package = self.package(revision)?;
        ensure!(
            package.definition.definition_id == id,
            "revision belongs to another agent"
        );
        self.update(id, |item| {
            item.sha256 = revision.into();
            item.pinned = true;
            item.dirty = true;
        })
    }
    pub fn set_enabled(&self, id: &str, value: bool) -> Result<()> {
        self.update(id, |item| item.enabled = value)
    }
    pub fn set_pinned(&self, id: &str, value: bool) -> Result<()> {
        self.update(id, |item| item.pinned = value)
    }
    pub fn set_binding(&self, id: &str, binding: ModelBinding) -> Result<()> {
        Self::validate_binding(&binding)?;
        self.update(id, |item| item.binding = binding)
    }

    fn validate_binding(binding: &ModelBinding) -> Result<()> {
        ensure!(
            binding
                .context_window
                .is_none_or(|value| (1024..=ModelBinding::MAX_CONTEXT_WINDOW).contains(&value)),
            "agent context_window must be between 1024 and 2000000 tokens"
        );
        ensure!(
            binding
                .max_turns
                .is_none_or(|value| (1..=ModelBinding::MAX_TURNS).contains(&value)),
            "agent max_turns must be between 1 and 1000"
        );
        ensure!(
            binding
                .provider_profile
                .as_ref()
                .is_none_or(|value| !value.trim().is_empty()
                    && value.len() <= 128
                    && !value.chars().any(char::is_control)),
            "invalid local Provider profile name"
        );
        ensure!(
            binding
                .model
                .as_ref()
                .is_none_or(|value| !value.trim().is_empty()
                    && value.len() <= 256
                    && !value.chars().any(char::is_control)),
            "invalid local model name"
        );
        Ok(())
    }
    pub fn remove(&self, id: &str, revision: &str) -> Result<()> {
        let _lock = self.lock()?;
        let mut items = self.installations()?;
        let item = items
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("agent not installed"))?;
        ensure!(
            item.sha256 == revision,
            "local revision changed during deletion"
        );
        items.remove(id);
        self.save(&items)
    }
    pub fn history(&self, id: &str) -> Result<Vec<AgentPackage>> {
        let directory = self.root.join("revisions");
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut packages = Vec::new();
        for path in fs::read_dir(directory)? {
            let path = path?.path();
            if path.extension().is_some_and(|value| value == "json") {
                let package = AgentPackage::parse(&fs::read(path)?)?;
                if package.definition.definition_id == id {
                    packages.push(package);
                }
            }
        }
        packages.sort_by(|a, b| a.definition.version.cmp(&b.definition.version));
        Ok(packages)
    }
    pub fn mark_synced(&self, id: &str, revision: &str, etag: String) -> Result<()> {
        let _lock = self.lock()?;
        let mut items = self.installations()?;
        let item = items
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("agent not installed"))?;
        ensure!(
            item.sha256 == revision,
            "local revision changed during sync"
        );
        item.remote_etag = Some(etag);
        item.dirty = false;
        self.save(&items)
    }
    /// An explicit conflict choice may replace a pinned revision; both branches remain in history.
    /// Unlike ordinary sync this accepts divergent branches bearing the same semantic version.
    pub fn resolve_private(
        &self,
        id: &str,
        expected_revision: &str,
        package: Option<&AgentPackage>,
        etag: &str,
        compatible: bool,
    ) -> Result<()> {
        let _lock = self.lock()?;
        let mut items = self.installations()?;
        let current = items
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("agent not installed"))?;
        ensure!(
            current.sha256 == expected_revision && current.origin == Origin::Private,
            "local revision changed; synchronize again"
        );
        if let Some(package) = package {
            self.archive_private_locked(id, package)?;
            let item = items.get_mut(id).expect("checked installation");
            item.sha256 = package.sha256.clone();
            item.remote_etag = Some(etag.into());
            item.dirty = false;
            item.enabled &= compatible;
        } else {
            items.remove(id);
        }
        self.save(&items)
    }
    pub fn archive_private(&self, id: &str, package: &AgentPackage) -> Result<()> {
        let _lock = self.lock()?;
        self.archive_private_locked(id, package)
    }
    fn archive_private_locked(&self, id: &str, package: &AgentPackage) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(package)?;
        AgentPackage::parse(&bytes)?;
        ensure!(
            package.definition.definition_id == id
                && id.starts_with("user/")
                && package.signature.is_none(),
            "invalid private conflict revision"
        );
        let path = self
            .root
            .join("revisions")
            .join(format!("{}.json", package.sha256));
        if path.exists() {
            ensure!(
                self.package(&package.sha256)? == *package,
                "immutable package mismatch"
            );
            return Ok(());
        }
        private_write(&path, &bytes)
    }
    pub fn set_remote_base(&self, id: &str, etag: String) -> Result<()> {
        self.update(id, |item| {
            item.remote_etag = Some(etag);
            item.dirty = true;
        })
    }
    fn update(&self, id: &str, change: impl FnOnce(&mut Installation)) -> Result<()> {
        let _lock = self.lock()?;
        let mut items = self.installations()?;
        change(
            items
                .get_mut(id)
                .ok_or_else(|| anyhow::anyhow!("agent not installed"))?,
        );
        self.save(&items)
    }
    fn save(&self, items: &BTreeMap<String, Installation>) -> Result<()> {
        private_write(
            &self.root.join("installations.json"),
            &serde_json::to_vec_pretty(items)?,
        )
    }
    fn lock(&self) -> Result<File> {
        fs::create_dir_all(self.root.join("revisions"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(".lock"))?;
        file.lock()?;
        Ok(file)
    }
    pub fn profiles(
        &self,
        bases: &[crate::SubagentProfile],
    ) -> Result<Vec<crate::SubagentProfile>> {
        self.installations()?
            .values()
            .filter(|item| item.enabled)
            .map(|item| {
                let package = self.package(&item.sha256)?;
                let base = bases
                    .iter()
                    .find(|p| p.id == package.definition.base_role)
                    .ok_or_else(|| anyhow::anyhow!("base role unavailable"))?;
                package.definition.project(base)
            })
            .collect()
    }
    pub fn account_scope(&self, identity: &str) -> Self {
        Self {
            root: self.root.join("accounts").join(digest(identity.as_bytes())),
        }
    }
}

pub fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("invalid store path"))?,
    )?;
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
