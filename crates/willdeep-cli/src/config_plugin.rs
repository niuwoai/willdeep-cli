//! Pinned first-party configuration plugin; installation never grants permissions.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rust_embed::RustEmbed;
#[cfg(test)]
use willdeep_core::plugin::{PluginHost, installed_versions};

#[cfg(test)]
pub(crate) const ID: &str = "willdeep-config";
pub(crate) const VERSION: &str = "0.6.0-rc3";

#[derive(RustEmbed)]
#[folder = "assets/willdeep-config"]
struct ConfigPluginAssets;

#[cfg(test)]
pub(crate) async fn ensure_installed(home: &Path) -> Result<()> {
    if installed_versions(&PluginHost::shared_root(home), ID)
        .iter()
        .any(|version| version == VERSION)
    {
        return Ok(());
    }
    let staging = std::env::temp_dir().join(format!("willdeep-config-{}", uuid::Uuid::new_v4()));
    let result = async {
        write_package(&staging)?;
        crate::plugin_cmd::install_one(home, &staging)?;
        Ok(())
    }
    .await;
    if let Err(error) = std::fs::remove_dir_all(&staging) {
        eprintln!("warning: config_plugin_staging_cleanup_failed error={error}");
    }
    result
}

/// Prepare the configuration service for the built-in Web settings page without
/// registering or enabling it as a user-facing plugin.
pub(crate) fn prepare_builtin_runtime(home: &Path) -> Result<PathBuf> {
    let root = home
        .join("plugin-data/willdeep-config/runtime")
        .join(VERSION);
    write_package(&root)?;
    Ok(root)
}

fn write_package(directory: &Path) -> Result<()> {
    for name in ConfigPluginAssets::iter() {
        let path = directory.join(name.as_ref());
        std::fs::create_dir_all(path.parent().context("plugin asset has no parent")?)?;
        let asset = ConfigPluginAssets::get(&name).context("missing bundled config asset")?;
        std::fs::write(path, asset.data)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn upgrading_config_plugin_keeps_old_package_config_and_approval_boundary() {
        let home = std::env::temp_dir().join(format!("config-upgrade-{}", uuid::Uuid::new_v4()));
        let source = home.join("old-package");
        write_package(&source).unwrap();
        let manifest = source.join(".codex-plugin/plugin.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        value["version"] = serde_json::json!("0.4.0-rc1");
        std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        crate::plugin_cmd::install_one(&home, &source).unwrap();
        PluginHost::discover(&home)
            .unwrap()
            .approve(ID, 1)
            .await
            .unwrap();
        let config = home.join("config.toml");
        std::fs::write(&config, "version = 1\n# retained configuration\n").unwrap();
        ensure_installed(&home).await.unwrap();
        let host = PluginHost::discover(&home).unwrap();
        assert_eq!(host.package(ID).unwrap().version, VERSION);
        assert!(host.approval_gap(ID).await.unwrap().is_some());
        assert!(
            home.join("plugins/willdeep-config/0.4.0-rc1/.codex-plugin/plugin.json")
                .is_file()
        );
        assert_eq!(
            std::fs::read_to_string(config).unwrap(),
            "version = 1\n# retained configuration\n"
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn fresh_install_requires_approval_and_repeated_install_preserves_it() {
        let home =
            std::env::temp_dir().join(format!("config-plugin-test-{}", uuid::Uuid::new_v4()));
        ensure_installed(&home).await.unwrap();
        let host = PluginHost::discover(&home).unwrap();
        assert!(host.package(ID).is_ok());
        assert!(!host.is_enabled(ID).await);
        assert!(host.approval_gap(ID).await.unwrap().is_some());
        host.approve(ID, 1).await.unwrap();
        ensure_installed(&home).await.unwrap();
        let host = PluginHost::discover(&home).unwrap();
        assert!(host.approval_gap(ID).await.unwrap().is_none());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn configuration_plugin_reads_the_hosts_custom_configuration() {
        let home =
            std::env::temp_dir().join(format!("config-plugin-path-{}", uuid::Uuid::new_v4()));
        ensure_installed(&home).await.unwrap();
        let path = home.join("selected.toml");
        std::fs::write(&path, "version = 1\n").unwrap();
        let host = PluginHost::discover(&home).unwrap();
        host.set_config_path(path.clone());
        host.approve(ID, 1).await.unwrap();
        host.set_enabled(ID, true).await.unwrap().unwrap();
        let result = host
            .execute_command(ID, "config.snapshot", serde_json::json!({}))
            .await
            .unwrap();
        let willdeep_core::plugin::CommandOutcome::Tool(value) = result else {
            panic!("expected configuration snapshot");
        };
        let text = value["content"][0]["text"].as_str().unwrap();
        let snapshot: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(snapshot["path"].as_str(), path.to_str());
        assert!(snapshot["exists"].as_bool().unwrap());
        drop(host);
        std::fs::remove_dir_all(home).unwrap();
    }
}
