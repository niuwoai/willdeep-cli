//! Explicit, credential-free launch configuration. No executable discovery.
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
pub const MAX_TIMEOUT_SECONDS: u64 = 60;
pub(super) const MAX_ORIGINS: usize = 32;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserConfig {
    pub enabled: bool,
    pub node_path: Option<PathBuf>,
    pub chrome_path: Option<PathBuf>,
    pub allowed_origins: Vec<String>,
    pub artifact_dir: Option<PathBuf>,
    pub timeout_seconds: u64,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            node_path: None,
            chrome_path: None,
            allowed_origins: Vec::new(),
            artifact_dir: None,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        }
    }
}

impl BrowserConfig {
    /// Validate syntax without running a program or creating a directory.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_TIMEOUT_SECONDS).contains(&self.timeout_seconds) {
            return Err("browser.timeout_seconds must be between 1 and 60".into());
        }
        for (name, path) in [
            ("node_path", &self.node_path),
            ("chrome_path", &self.chrome_path),
            ("artifact_dir", &self.artifact_dir),
        ] {
            if let Some(path) = path {
                if !path.is_absolute()
                    || path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return Err(format!(
                        "browser.{name} must be an absolute path without '..'"
                    ));
                }
            } else if self.enabled {
                return Err(format!("browser.{name} is required when enabled"));
            }
        }
        if self.allowed_origins.len() > MAX_ORIGINS
            || (self.enabled && self.allowed_origins.is_empty())
        {
            return Err(
                "browser.allowed_origins must contain 1 to 32 exact HTTP(S) origins when enabled"
                    .into(),
            );
        }
        let mut unique = std::collections::BTreeSet::new();
        for origin in &self.allowed_origins {
            let url = checked_url(origin)?;
            if url.origin().ascii_serialization() != *origin {
                return Err("browser.allowed_origins entries must be canonical origins without paths, trailing '/', query, fragment or credentials".into());
            }
            if !unique.insert(origin) {
                return Err("browser.allowed_origins contains a duplicate origin".into());
            }
        }
        Ok(())
    }

    pub(super) fn dependencies(&self) -> Result<(), String> {
        self.validate()?;
        if !self.enabled {
            return Err("browser is disabled; explicitly configure and enable [browser]".into());
        }
        for (name, path) in [("Node 24", &self.node_path), ("Chrome", &self.chrome_path)] {
            let path = path
                .as_ref()
                .ok_or_else(|| format!("{name} path is missing"))?;
            if !executable(path) {
                return Err(format!(
                    "configured {name} executable is missing or not executable; install it yourself and configure its absolute path"
                ));
            }
        }
        let directory = self
            .artifact_dir
            .as_ref()
            .ok_or("browser.artifact_dir is missing")?;
        let metadata = std::fs::symlink_metadata(directory)
            .map_err(|e| format!("browser artifact directory must already exist: {e}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(
                "browser artifact directory must be an existing non-symlink directory".into(),
            );
        }
        Ok(())
    }

    pub(super) fn checked_navigation(&self, raw: &str) -> Result<String, String> {
        let url = checked_url(raw)?;
        if !self
            .allowed_origins
            .contains(&url.origin().ascii_serialization())
        {
            return Err("browser navigation origin is not in allowed_origins".into());
        }
        Ok(url.to_string())
    }
}

fn checked_url(raw: &str) -> Result<reqwest::Url, String> {
    if raw.len() > 8192 || raw.chars().any(char::is_control) || raw.contains('\\') {
        return Err(
            "browser URL exceeds its limit or contains control characters/backslashes".into(),
        );
    }
    let url = reqwest::Url::parse(raw).map_err(|_| "browser URL is invalid")?;
    if url.host_str().is_some_and(|host| {
        host.is_empty()
            || host.len() > 255
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    }) {
        return Err("browser URL host must be a literal domain or IP, without wildcards or resolver directives".into());
    }
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url
            .as_str()
            .split('/')
            .nth(2)
            .is_some_and(|authority| authority.contains('@'))
    {
        return Err("browser accepts only absolute HTTP(S) URLs without credentials".into());
    }
    Ok(url)
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
