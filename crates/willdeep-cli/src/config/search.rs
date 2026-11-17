//! Search is configured independently of chat providers. Environment references
//! are resolved here, once, without falling back to any provider credential.

use std::fmt;

use anyhow::{Result, bail};
use serde::Deserialize;
use willdeep_core::tools::{SearchBackend, SearchSettings};

#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
    pub backend: SearchBackend,
    pub api_base: Option<String>,
    pub api_key_env: Option<String>,
    pub allow_private_endpoint: bool,
}

impl fmt::Debug for SearchConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchConfig")
            .field("backend", &self.backend)
            .field("api_base", &self.api_base.as_ref().map(|_| "[configured]"))
            .field("api_key_env", &self.api_key_env)
            .field("allow_private_endpoint", &self.allow_private_endpoint)
            .finish()
    }
}

impl SearchConfig {
    /// Validate syntax without environment access, DNS or network activity.
    pub fn validate(&self) -> Result<()> {
        if self.backend != SearchBackend::Searxng && self.api_key_env.is_some() {
            bail!("search.api_key_env requires backend=searxng");
        }
        if let Some(name) = &self.api_key_env {
            let mut bytes = name.bytes();
            if !bytes
                .next()
                .is_some_and(|value| value.is_ascii_alphabetic() || value == b'_')
                || !bytes.all(|value| value.is_ascii_alphanumeric() || value == b'_')
            {
                bail!("search.api_key_env must be an environment variable name");
            }
        }
        SearchSettings {
            backend: self.backend,
            api_base: self.api_base.clone(),
            api_key: None,
            allow_private_endpoint: self.allow_private_endpoint,
        }
        .validate()
        .map_err(anyhow::Error::msg)
    }

    pub fn resolve(&self) -> Result<SearchSettings> {
        self.resolve_with_env(|name| std::env::var(name))
    }

    fn resolve_with_env(
        &self,
        mut read_env: impl FnMut(&str) -> Result<String, std::env::VarError>,
    ) -> Result<SearchSettings> {
        self.validate()?;
        let key = match &self.api_key_env {
            Some(name) => match read_env(name) {
                Ok(value) if !value.trim().is_empty() => Some(value),
                _ => bail!(
                    "search.api_key_env references a missing, empty or non-Unicode environment variable: {name}"
                ),
            },
            None => None,
        };
        let settings = SearchSettings {
            backend: self.backend,
            api_base: self.api_base.clone(),
            api_key: key,
            allow_private_endpoint: self.allow_private_endpoint,
        };
        settings.validate().map_err(anyhow::Error::msg)?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_defaults_to_auto_without_reading_environment() {
        let config: SearchConfig = toml::from_str("").unwrap();
        let settings = config
            .resolve_with_env(|_| panic!("auto must not read an environment variable"))
            .unwrap();
        assert_eq!(settings.backend, SearchBackend::Auto);
        assert!(settings.api_key.is_none());
    }

    #[test]
    fn search_searxng_resolves_only_explicit_environment_reference() {
        let config: SearchConfig = toml::from_str(
            "backend = 'searxng'\napi_base = 'https://search.invalid'\napi_key_env = 'TEST_SEARCH_KEY'",
        ).unwrap();
        let settings = config
            .resolve_with_env(|name| {
                assert_eq!(name, "TEST_SEARCH_KEY");
                Ok("synthetic-fixture-key".into())
            })
            .unwrap();
        assert_eq!(settings.api_key.as_deref(), Some("synthetic-fixture-key"));
        assert!(!format!("{settings:?}").contains("synthetic-fixture-key"));
        assert!(
            config
                .resolve_with_env(|_| Err(std::env::VarError::NotPresent))
                .is_err()
        );
        assert!(config.resolve_with_env(|_| Ok("  ".into())).is_err());
        assert!(
            config
                .resolve_with_env(|_| Ok("bad\r\nheader".into()))
                .is_err()
        );
    }

    #[test]
    fn search_config_rejects_inline_key_and_ignored_settings() {
        assert!(
            toml::from_str::<SearchConfig>("backend = 'searxng'\napi_key = 'synthetic'").is_err()
        );
        for input in [
            "backend = 'other'",
            "backend = 'searxng'",
            "backend = 'disabled'\napi_base = 'https://search.invalid'",
            "backend = 'auto'\napi_key_env = 'TEST_SEARCH_KEY'",
            "backend = 'searxng'\napi_base = 'https://u:p@search.invalid'",
            "backend = 'searxng'\napi_base = 'https://search.invalid?key=x'",
            "backend = 'searxng'\napi_base = 'https://search.invalid'\napi_key_env = 'BAD-NAME'",
        ] {
            let rejected = match toml::from_str::<SearchConfig>(input) {
                Ok(config) => config.validate().is_err(),
                Err(_) => true,
            };
            assert!(rejected, "unexpected accepted config: {input}");
        }
    }

    #[test]
    fn search_anonymous_searxng_never_reads_chat_credentials() {
        let config: SearchConfig = toml::from_str("backend = 'searxng'\napi_base = 'http://localhost:19851'\nallow_private_endpoint = true").unwrap();
        let settings = config
            .resolve_with_env(|_| panic!("no credential reference configured"))
            .unwrap();
        assert!(settings.api_key.is_none());
        assert!(settings.allow_private_endpoint);
    }
}
