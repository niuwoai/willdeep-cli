//! Credential-free [browser] settings shared with core; no duplicate defaults.
//! Integration in config.rs: `mod browser; pub use browser::BrowserSettings;`
//! ConfigFile: `#[serde(default)] pub browser: BrowserSettings,`
//! validate(): `file.browser.validate().map_err(anyhow::Error::msg)?;`
pub use willdeep_core::tools::BrowserConfig as BrowserSettings;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_defaults_disabled_and_rejects_unknown_fields() {
        let default: BrowserSettings = toml::from_str("").unwrap();
        assert!(!default.enabled);
        assert!(default.validate().is_ok());
        assert!(
            toml::from_str::<BrowserSettings>("enabled = false\nuser_profile = '/tmp/profile'")
                .is_err()
        );
        assert!(
            toml::from_str::<BrowserSettings>(
                "enabled = false\nnode_options = '--require untrusted'"
            )
            .is_err()
        );
    }

    #[test]
    fn enabled_browser_requires_absolute_paths_and_exact_origins() {
        let incomplete: BrowserSettings = toml::from_str("enabled = true").unwrap();
        assert!(incomplete.validate().is_err());
        let relative: BrowserSettings = toml::from_str("node_path = 'node'").unwrap();
        assert!(relative.validate().is_err());
        let origin: BrowserSettings =
            toml::from_str("allowed_origins = ['https://example.test/path']").unwrap();
        assert!(origin.validate().is_err());
    }
}
