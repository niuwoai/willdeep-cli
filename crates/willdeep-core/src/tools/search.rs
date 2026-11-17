//! Independent, approval-gated search. No DNS, probes or credential lookup in status.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ToolError, ToolRegistry, WebSearchArgs};

mod response;
mod transport;

const DEFAULT_COUNT: usize = 8;
const MAX_COUNT: usize = 20;
const MAX_QUERY_BYTES: usize = 16 * 1024;
const SEARCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchBackend {
    #[default]
    Auto,
    Disabled,
    Searxng,
}

/// Resolved runtime settings. Only the CLI resolves environment references.
/// `Auto` uses the legacy `with_web_tools` configuration; explicit SearXNG never does.
#[derive(Clone, Default)]
pub struct SearchSettings {
    pub backend: SearchBackend,
    pub api_base: Option<String>,
    pub api_key: Option<String>,
    pub allow_private_endpoint: bool,
}

impl fmt::Debug for SearchSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchSettings")
            .field("backend", &self.backend)
            .field("api_base", &self.api_base.as_ref().map(|_| "[configured]"))
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .field("allow_private_endpoint", &self.allow_private_endpoint)
            .finish()
    }
}

impl SearchSettings {
    /// Syntactic validation only. Never resolves a host or checks credentials online.
    pub fn validate(&self) -> Result<(), String> {
        if self.backend != SearchBackend::Searxng {
            if self.api_base.is_some() || self.api_key.is_some() {
                return Err(
                    "search.api_base and search.api_key_env require backend=searxng".into(),
                );
            }
            return Ok(());
        }
        let base = self
            .api_base
            .as_deref()
            .ok_or("search.api_base is required for backend=searxng")?;
        parse_endpoint(base, Backend::Searxng).map_err(|e| e.to_string())?;
        if self.api_key.as_deref().is_some_and(|key| {
            key.trim().is_empty()
                || reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")).is_err()
        }) {
            return Err(
                "search.api_key_env must resolve to a nonempty valid HTTP credential".into(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Backend {
    Searxng,
    SomeIm,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Self::Searxng => "searxng",
            Self::SomeIm => "some_im",
        }
    }
}

struct Endpoint<'a> {
    backend: Backend,
    url: reqwest::Url,
    key: Option<&'a str>,
    allow_private: bool,
}

impl ToolRegistry {
    pub fn with_search_settings(mut self, settings: SearchSettings) -> Self {
        self.search = settings;
        self
    }

    fn search_endpoint(&self) -> Result<Endpoint<'_>, ToolError> {
        self.search
            .validate()
            .map_err(|_| failure("invalid_configuration", "check the search configuration"))?;
        let (backend, base, key) = match self.search.backend {
            SearchBackend::Disabled => {
                return Err(failure("disabled", "search is disabled by configuration"));
            }
            SearchBackend::Searxng => (
                Backend::Searxng,
                self.search.api_base.as_deref().unwrap_or_default(),
                self.search.api_key.as_deref(),
            ),
            SearchBackend::Auto => {
                let config = self.web.as_ref().ok_or_else(|| {
                    failure(
                        "not_configured",
                        "configure [search] backend=searxng or a compatible SomeIm provider",
                    )
                })?;
                if config.api_key.is_empty() {
                    return Err(failure(
                        "missing_credential",
                        "configure the SomeIm credential",
                    ));
                }
                (
                    Backend::SomeIm,
                    config.some_im_base_url.as_str(),
                    Some(config.api_key.as_str()),
                )
            }
        };
        Ok(Endpoint {
            backend,
            url: parse_endpoint(base, backend)?,
            key,
            allow_private: self.search.allow_private_endpoint,
        })
    }

    pub(super) fn web_search_status(&self) -> Result<String, ToolError> {
        let status = match self.search_endpoint() {
            Ok(endpoint) => json!({
                "backend": endpoint.backend.name(),
                "state": "configured",
                "endpoint": endpoint.url.origin().ascii_serialization(),
                "authentication": if endpoint.key.is_some() { "configured" } else { "none" },
                "allow_private_endpoint": endpoint.allow_private,
                "network_checked": false,
                "message": "Configuration is present; connectivity and credentials have not been checked."
            }),
            Err(error) => json!({
                "backend": self.search.backend,
                "state": match self.search.backend {
                    SearchBackend::Disabled => "disabled",
                    SearchBackend::Auto if self.web.is_none() => "not_configured",
                    _ => "invalid_configuration",
                },
                "network_checked": false,
                "message": error.to_string(),
            }),
        };
        serde_json::to_string_pretty(&status)
            .map_err(|_| failure("encoding", "cannot encode search status"))
    }

    pub(super) async fn web_search(&self, args: WebSearchArgs) -> Result<String, ToolError> {
        let query = args.query.trim();
        if query.is_empty() || query.len() > MAX_QUERY_BYTES {
            return Err(failure(
                "invalid_query",
                "query must contain 1 to 16384 UTF-8 bytes",
            ));
        }
        let endpoint = self.search_endpoint()?;
        let count = args.count.unwrap_or(DEFAULT_COUNT).clamp(1, MAX_COUNT);
        // Approval precedes all DNS and socket work, including public-address checks.
        self.require_network_read_approval(&format!(
            "search using {} at {} for: {query}",
            endpoint.backend.name(),
            endpoint.url.origin().ascii_serialization(),
        ))
        .await?;
        let source = endpoint.url.origin().ascii_serialization();
        let bytes = transport::request(&endpoint, query, count, SEARCH_TIMEOUT).await?;
        response::format_results(&bytes, endpoint.backend, &source, count, endpoint.key)
    }
}

fn parse_endpoint(base: &str, backend: Backend) -> Result<reqwest::Url, ToolError> {
    let mut url = reqwest::Url::parse(base.trim()).map_err(|_| {
        failure(
            "invalid_endpoint",
            "search endpoint must be an absolute HTTP(S) URL",
        )
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url
            .as_str()
            .split('/')
            .nth(2)
            .is_some_and(|authority| authority.contains('@'))
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port_or_known_default() == Some(0)
    {
        return Err(failure(
            "invalid_endpoint",
            "search endpoint requires a host and cannot contain credentials, query or fragment",
        ));
    }
    match backend {
        Backend::SomeIm => url.set_path("/api/v1/customer/web-search"),
        Backend::Searxng => {
            let path = url.path().trim_end_matches('/');
            let path = if path.ends_with("/search") {
                path.to_owned()
            } else {
                format!("{path}/search")
            };
            url.set_path(&path);
        }
    }
    Ok(url)
}

fn failure(code: &str, message: &str) -> ToolError {
    ToolError::Network(format!("search[{code}]: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_debug_redacts_key_and_endpoint() {
        let settings = SearchSettings {
            backend: SearchBackend::Searxng,
            api_base: Some("https://fixture-user:fixture-secret@example.org/private".into()),
            api_key: Some("unit-fixture-key".into()),
            allow_private_endpoint: false,
        };
        let debug = format!("{settings:?}");
        assert!(!debug.contains("fixture-secret"));
        assert!(!debug.contains("unit-fixture-key"));
        assert!(!debug.contains("private\""));
    }

    #[test]
    fn legacy_someim_debug_redacts_key_and_endpoint() {
        let config = super::super::WebToolConfig {
            some_im_base_url: "https://fixture-user:p@example.org/private".into(),
            api_key: "synthetic-legacy-debug-key".into(),
        };
        let debug = format!("{config:?}");
        assert!(!debug.contains("synthetic-legacy-debug-key"));
        assert!(!debug.contains("fixture-user"));
    }

    #[test]
    fn endpoint_validation_is_syntactic_and_preserves_searxng_prefix() {
        let url = parse_endpoint("https://search.invalid/prefix/", Backend::Searxng).unwrap();
        assert_eq!(url.as_str(), "https://search.invalid/prefix/search");
        assert!(parse_endpoint("https://u:p@search.invalid", Backend::Searxng).is_err());
        assert!(parse_endpoint("https://search.invalid/?key=x", Backend::Searxng).is_err());
        assert!(parse_endpoint("https://search.invalid/#key", Backend::Searxng).is_err());
        let url = parse_endpoint("https://some.invalid/v1", Backend::SomeIm).unwrap();
        assert_eq!(url.path(), "/api/v1/customer/web-search");
    }
}
