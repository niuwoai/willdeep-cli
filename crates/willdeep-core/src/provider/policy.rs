//! Project-bound model routing. This controls these HTTP clients, not the OS network.
//!
//! `.willdeep/model-policy.json`: `{"version":1,"allow":[{"provider":
//! "openai-compatible","model":"example","base_url":"https://api.example/v1"}]}`.
//! Each rule requires exactly one of `origin` (all paths on that origin) or
//! `base_url` (that exact configured API base). No glob or implicit local exemption.

use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

use reqwest::{Client, Request, RequestBuilder, Response, Url};
use serde::Deserialize;

use super::{ProviderConfig, ProviderError, ProviderKind};

const MAX_POLICY_BYTES: u64 = 64 * 1024;
const MAX_RULES: usize = 256;

#[derive(Clone, Debug)]
pub struct ModelPolicyContext {
    root: Arc<PathBuf>,
    state: Arc<TrustedPolicy>,
    unattributed_candidates: Option<Arc<Vec<Self>>>,
}

#[derive(Debug, Default)]
struct TrustedPolicy {
    // Once enabled, deletion or an invalid file must not remove the boundary.
    required: AtomicBool,
    effective: Mutex<Option<Policy>>,
}

// Strong references intentionally survive individual harnesses and sessions.
// Rebuilding a provider after an Agent edits the file cannot reset the baseline.
static TRUSTED_PROJECTS: OnceLock<Mutex<HashMap<PathBuf, Arc<TrustedPolicy>>>> = OnceLock::new();

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    version: u32,
    allow: Vec<Rule>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    provider: PolicyProvider,
    model: String,
    origin: Option<String>,
    base_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PolicyProvider {
    OpenaiCompatible,
    SomeIm,
    Anthropic,
}

pub(crate) fn denied(reason: &'static str) -> ProviderError {
    // Fixed reason codes only: never log request bodies, URLs, credentials or
    // parser errors containing user-controlled strings.
    eprintln!(
        "{}",
        serde_json::json!({
            "event": "model_policy", "level": "warn", "outcome": "not_sent", "reason": reason,
        })
    );
    ProviderError::PolicyDenied(reason.to_owned())
}

impl ModelPolicyContext {
    /// Bind to a trusted, explicitly selected project, never to process cwd or
    /// the usage ledger's `local` inference classification.
    pub fn for_workspace(root: &Path) -> Result<Self, ProviderError> {
        if !root.is_absolute() {
            return Err(denied("workspace_must_be_absolute"));
        }
        let root = root
            .canonicalize()
            .map_err(|_| denied("workspace_unreadable"))?;
        if !root.is_dir() {
            return Err(denied("workspace_not_directory"));
        }
        let state = TRUSTED_PROJECTS
            .get_or_init(Mutex::default)
            .lock()
            .map_err(|_| denied("policy_state_unavailable"))?
            .entry(root.clone())
            .or_default()
            .clone();
        let context = Self {
            root: Arc::new(root),
            state,
            unattributed_candidates: None,
        };
        context.load()?;
        Ok(context)
    }

    pub fn workspace(&self) -> Option<&Path> {
        self.unattributed_candidates
            .is_none()
            .then_some(self.root.as_path())
    }

    /// Unknown plugin/session ownership may retain legacy behavior only when
    /// every candidate project is known to have no policy. Never pick the first
    /// project from a multi-project allowlist.
    pub fn unambiguous_workspace(
        explicit: Option<&Path>,
        candidates: &[PathBuf],
    ) -> Result<Option<Self>, ProviderError> {
        if let Some(root) = explicit {
            return Self::for_workspace(root).map(Some);
        }
        if let [root] = candidates {
            return Self::for_workspace(root).map(Some);
        }
        // There is no trustworthy project set to check at all.
        if candidates.is_empty() {
            return Err(denied("workspace_ownership_missing"));
        }
        let contexts = candidates
            .iter()
            .map(|root| Self::for_workspace(root))
            .collect::<Result<Vec<_>, _>>()?;
        let context = Self {
            root: Arc::new(PathBuf::new()),
            state: Arc::new(TrustedPolicy::default()),
            unattributed_candidates: Some(Arc::new(contexts)),
        };
        context.load()?;
        Ok(Some(context))
    }

    fn load(&self) -> Result<Option<Policy>, ProviderError> {
        if let Some(candidates) = &self.unattributed_candidates {
            for context in candidates.iter() {
                if context.load()?.is_some() {
                    return Err(denied("workspace_ownership_ambiguous"));
                }
            }
            return Ok(None);
        }
        let directory = self.root.join(".willdeep");
        match std::fs::symlink_metadata(&directory) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(denied("policy_directory_invalid"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return self.missing();
            }
            Err(_) => return Err(denied("policy_directory_unreadable")),
        }
        let path = directory.join("model-policy.json");
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return self.missing(),
            Err(_) => return Err(denied("policy_unreadable")),
        };
        self.state.required.store(true, Ordering::Release);
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(denied("policy_not_regular_file"));
        }
        if metadata.len() > MAX_POLICY_BYTES {
            return Err(denied("policy_too_large"));
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x00200000;
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let file = options
            .open(path)
            .map_err(|_| denied("policy_unreadable"))?;
        let opened = file.metadata().map_err(|_| denied("policy_unreadable"))?;
        if !opened.is_file() || opened.len() > MAX_POLICY_BYTES {
            return Err(denied("policy_not_regular_or_too_large"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            if opened.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(denied("policy_not_regular_file"));
            }
        }
        let mut bytes = Vec::new();
        file.take(MAX_POLICY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| denied("policy_unreadable"))?;
        if bytes.len() as u64 > MAX_POLICY_BYTES {
            return Err(denied("policy_too_large"));
        }
        let mut policy: Policy =
            serde_json::from_slice(&bytes).map_err(|_| denied("policy_schema_invalid"))?;
        if policy.version != 1 || policy.allow.len() > MAX_RULES {
            return Err(denied("policy_version_or_rule_limit_invalid"));
        }
        for rule in &mut policy.allow {
            if rule.model.is_empty()
                || rule.model.len() > 256
                || rule.model.trim() != rule.model
                || rule.model.chars().any(char::is_control)
            {
                return Err(denied("policy_model_invalid"));
            }
            match (&rule.origin, &rule.base_url) {
                (Some(origin), None) => {
                    let url = clean_url(origin)?;
                    if url.path() != "/" {
                        return Err(denied("policy_origin_has_path"));
                    }
                    rule.origin = Some(url.origin().ascii_serialization());
                }
                (None, Some(base)) => {
                    rule.base_url =
                        Some(clean_url(base)?.as_str().trim_end_matches('/').to_owned());
                }
                _ => return Err(denied("policy_endpoint_scope_invalid")),
            }
        }
        policy.allow.sort();
        policy.allow.dedup();
        let mut effective = self
            .state
            .effective
            .lock()
            .map_err(|_| denied("policy_state_unavailable"))?;
        let policy = match effective.as_ref() {
            Some(trusted) => match intersect(trusted, &policy) {
                Ok(policy) => policy,
                Err(error) => {
                    // An over-complex valid restriction cannot be discarded in
                    // favor of the previous, broader policy on the next read.
                    *effective = Some(Policy {
                        version: 1,
                        allow: Vec::new(),
                    });
                    return Err(error);
                }
            },
            None => policy,
        };
        *effective = Some(policy.clone());
        Ok(Some(policy))
    }

    fn missing(&self) -> Result<Option<Policy>, ProviderError> {
        if self.state.required.load(Ordering::Acquire) {
            Err(denied("policy_removed"))
        } else {
            Ok(None)
        }
    }
}

fn intersect(trusted: &Policy, current: &Policy) -> Result<Policy, ProviderError> {
    let mut allow = std::collections::BTreeSet::new();
    for left in &trusted.allow {
        for right in &current.allow {
            if left.provider != right.provider || left.model != right.model {
                continue;
            }
            let rule = match (&left.origin, &left.base_url, &right.origin, &right.base_url) {
                (Some(a), None, Some(b), None) if a == b => Some(left),
                (None, Some(a), None, Some(b)) if a == b => Some(left),
                (Some(origin), None, None, Some(base))
                    if clean_url(base)?.origin().ascii_serialization() == *origin =>
                {
                    Some(right)
                }
                (None, Some(base), Some(origin), None)
                    if clean_url(base)?.origin().ascii_serialization() == *origin =>
                {
                    Some(left)
                }
                _ => None,
            };
            if let Some(rule) = rule {
                allow.insert(rule.clone());
            }
            if allow.len() > MAX_RULES {
                return Err(denied("policy_intersection_too_complex"));
            }
        }
    }
    Ok(Policy {
        version: 1,
        allow: allow.into_iter().collect(),
    })
}

fn clean_url(raw: &str) -> Result<Url, ProviderError> {
    let url = Url::parse(raw).map_err(|_| denied("policy_endpoint_invalid"))?;
    if raw.trim() != raw
        || !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || raw.split_once("://").is_some_and(|(_, rest)| {
            rest.split('/')
                .next()
                .is_some_and(|authority| authority.contains('@'))
        })
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(denied("policy_endpoint_invalid"));
    }
    Ok(url)
}

fn same_base(left: &Url, right: &Url) -> bool {
    left.origin() == right.origin()
        && left.path().trim_end_matches('/') == right.path().trim_end_matches('/')
}

fn valid_route(config: &ProviderConfig, base: &Url, actual: &Url) -> bool {
    if actual.origin() != base.origin() {
        return false;
    }
    if same_base(base, actual) {
        return true;
    }
    // These are the only routes the native providers construct. In particular,
    // a base path is not a prefix allowlist for arbitrary sibling services.
    let endpoints = [
        super::common::endpoint(&config.base_url, "chat/completions"),
        super::common::endpoint(&config.base_url, "responses"),
        super::common::anthropic_endpoint(&config.base_url),
        super::common::endpoint(&config.base_url, "models"),
        super::common::endpoint(&config.base_url, "model-capabilities"),
        super::common::endpoint(&config.base_url, "images/generations"),
        super::common::endpoint(
            config
                .base_url
                .trim_end_matches('/')
                .trim_end_matches("/v1"),
            "v1/models",
        ),
    ];
    endpoints.into_iter().flatten().any(|url| url == *actual)
}

fn authorize(
    context: &ModelPolicyContext,
    config: &ProviderConfig,
    wire_model: &str,
    actual: &Url,
) -> Result<bool, ProviderError> {
    let Some(policy) = context.load()? else {
        return Ok(false);
    };
    // Query parameters can carry signed media URLs; they do not alter the
    // permitted origin/base path and are never included in policy diagnostics.
    let mut actual_scope = actual.clone();
    actual_scope.set_query(None);
    let actual = clean_url(actual_scope.as_str())?;
    let base = clean_url(&config.base_url)?;
    if !valid_route(config, &base, &actual) {
        return Err(denied("request_endpoint_mismatch"));
    }
    let provider = match config.kind {
        ProviderKind::OpenAiCompatible => PolicyProvider::OpenaiCompatible,
        ProviderKind::SomeIm => PolicyProvider::SomeIm,
        ProviderKind::Anthropic => PolicyProvider::Anthropic,
    };
    for rule in policy.allow {
        if rule.provider != provider || rule.model != wire_model {
            continue;
        }
        let matches = match (rule.origin, rule.base_url) {
            (Some(origin), None) => clean_url(&origin)?.origin() == actual.origin(),
            (None, Some(endpoint)) => same_base(&clean_url(&endpoint)?, &base),
            _ => false,
        };
        if matches {
            return Ok(true);
        }
    }
    Err(denied("route_not_allowed"))
}

fn safe_address(ip: IpAddr, explicit_local: bool) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !ip.is_unspecified()
                && !ip.is_multicast()
                && !ip.is_broadcast()
                && (explicit_local || !(ip.is_loopback() || ip.is_private() || ip.is_link_local()))
        }
        IpAddr::V6(ip) => {
            if let Some(ipv4) = ip.to_ipv4_mapped() {
                return safe_address(IpAddr::V4(ipv4), explicit_local);
            }
            !ip.is_unspecified()
                && !ip.is_multicast()
                && (explicit_local
                    || !(ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()))
        }
    }
}

async fn pinned_client(config: &ProviderConfig, url: &Url) -> Result<Client, ProviderError> {
    let host = url
        .host_str()
        .ok_or_else(|| denied("endpoint_host_missing"))?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let port = url
        .port_or_known_default()
        .ok_or_else(|| denied("endpoint_port_invalid"))?;
    let literal = host.parse::<IpAddr>().ok();
    let explicit_local = literal.is_some() || host.eq_ignore_ascii_case("localhost");
    let addresses: Vec<SocketAddr> = if let Some(ip) = literal {
        vec![SocketAddr::new(ip, port)]
    } else if host.eq_ignore_ascii_case("localhost") {
        vec![SocketAddr::from(([127, 0, 0, 1], port))]
    } else {
        tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| denied("endpoint_resolution_failed"))?
            .take(32)
            .collect()
    };
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| !safe_address(address.ip(), explicit_local))
    {
        return Err(denied("endpoint_address_not_allowed"));
    }
    Client::builder()
        .timeout(std::time::Duration::from_secs(config.request_timeout_secs))
        .user_agent(crate::CLIENT_USER_AGENT)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .resolve_to_addrs(host, &addresses)
        .build()
        .map_err(|_| denied("policy_transport_failed"))
}

/// Check generation permission before any dependent upload sends project data.
pub fn check_route(
    config: &ProviderConfig,
    wire_model: &str,
    endpoint: &Url,
) -> Result<(), ProviderError> {
    if let Some(context) = &config.policy_context {
        authorize(context, config, wire_model, endpoint)?;
    }
    Ok(())
}

/// Send an explicitly attributed non-chat request (image generation, media
/// upload/download). The caller supplies the actual model; set `base_url` to
/// this service's exact base before calling. Does not retry billable requests.
pub async fn send_bound_request(
    config: &ProviderConfig,
    request: RequestBuilder,
    wire_model: &str,
) -> Result<Response, ProviderError> {
    let (client, request) = request.build_split();
    let request = request?;
    if config.policy_context.is_some() {
        if wire_model != config.model {
            return Err(denied("wire_model_mismatch"));
        }
        if request
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(';').next() == Some("application/json"))
        {
            self::wire_model(config, &request)?;
        }
    }
    let deadline = super::common::request_deadline(config)?;
    tokio::time::timeout_at(deadline, execute(config, client, request, wire_model))
        .await
        .map_err(|_| ProviderError::DeadlineExceeded)?
}

pub(super) async fn execute(
    config: &ProviderConfig,
    client: Client,
    request: Request,
    wire_model: &str,
) -> Result<Response, ProviderError> {
    let active = match &config.policy_context {
        Some(context) => authorize(context, config, wire_model, request.url())?,
        None => false,
    };
    if active {
        let client = pinned_client(config, request.url()).await?;
        // Reload after resolution too: a policy may change while DNS waits.
        authorize(
            config.policy_context.as_ref().expect("bound context"),
            config,
            wire_model,
            request.url(),
        )?;
        Ok(client.execute(request).await?)
    } else {
        Ok(client.execute(request).await?)
    }
}

pub(super) fn wire_model(
    config: &ProviderConfig,
    request: &Request,
) -> Result<String, ProviderError> {
    if request.method() == reqwest::Method::GET {
        return Ok(config.model.clone());
    }
    let bytes = request
        .body()
        .and_then(reqwest::Body::as_bytes)
        .ok_or_else(|| denied("wire_body_not_inspectable"))?;
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| denied("wire_body_invalid"))?;
    let model = value
        .get("model")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| denied("wire_model_missing"))?;
    if model != config.model {
        return Err(denied("wire_model_mismatch"));
    }
    Ok(model.to_owned())
}
