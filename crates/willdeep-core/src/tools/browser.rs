//! Fixed CDP actions behind the shared Runtime's execute/approval path.
//! Origin interception is navigation/request control, not an OS network sandbox.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use super::{ApprovalMode, ToolDefinition, ToolError, ToolRegistry};
use artifact::Artifacts;
use transport::Bridge;

mod artifact;
mod cleanup;
mod config;
#[cfg(test)]
mod tests;
mod transport;
pub use config::BrowserConfig;

const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_SNAPSHOT_NODES: usize = 200;

pub(super) struct BrowserManager {
    config: BrowserConfig,
    session: Arc<Mutex<Option<Session>>>,
}

impl Default for BrowserManager {
    fn default() -> Self {
        Self::new(BrowserConfig::default())
    }
}

impl BrowserManager {
    pub(super) fn new(config: BrowserConfig) -> Self {
        Self {
            config,
            session: Arc::new(Mutex::new(None)),
        }
    }
}

struct Session {
    bridge: Bridge,
    artifacts: Artifacts,
    refs: BTreeMap<String, Reference>,
    epoch: u64,
}

#[derive(Clone)]
struct Reference {
    epoch: u64,
    backend_node_id: u64,
    label: String,
}

impl Session {
    async fn start(config: &BrowserConfig) -> Result<Self, String> {
        // Create the process guard before awaiting CDP, so cancellation owns cleanup.
        let artifacts = Artifacts::new(
            config
                .artifact_dir
                .as_deref()
                .ok_or("browser.artifact_dir missing")?,
        )?;
        let bridge = Bridge::start(config).await?;
        Ok(Self {
            bridge,
            artifacts,
            refs: BTreeMap::new(),
            epoch: 0,
        })
    }

    async fn status(&mut self) -> Result<Value, String> {
        let result = self.bridge.request(json!({"action":"status"})).await?;
        let epoch = result["epoch"]
            .as_u64()
            .ok_or("UNKNOWN: bridge omitted page epoch")?;
        if self.epoch != epoch {
            self.refs.clear();
        }
        self.epoch = epoch;
        Ok(result)
    }

    fn reference(&self, key: &str) -> Result<Reference, String> {
        lookup_reference(&self.refs, self.epoch, key)
    }

    fn snapshot(&mut self, result: Value) -> Result<Value, String> {
        bind_snapshot(&mut self.refs, &mut self.epoch, result)
    }
}

fn lookup_reference(
    refs: &BTreeMap<String, Reference>,
    epoch: u64,
    key: &str,
) -> Result<Reference, String> {
    let reference = refs
        .get(key)
        .ok_or("snapshot ref is unknown or stale; request a new snapshot")?;
    if reference.epoch != epoch {
        return Err("snapshot ref belongs to an earlier page epoch".into());
    }
    Ok(reference.clone())
}

fn bind_snapshot(
    refs: &mut BTreeMap<String, Reference>,
    current_epoch: &mut u64,
    mut result: Value,
) -> Result<Value, String> {
    let epoch = result["epoch"].as_u64().ok_or("snapshot omitted epoch")?;
    let nodes = result["nodes"]
        .as_array_mut()
        .ok_or("snapshot omitted nodes")?;
    if nodes.len() > MAX_SNAPSHOT_NODES {
        return Err("snapshot node budget exceeded".into());
    }
    refs.clear();
    *current_epoch = epoch;
    let snapshot_id = uuid::Uuid::new_v4().to_string();
    for (index, node) in nodes.iter_mut().enumerate() {
        let id = node["backend_node_id"]
            .as_u64()
            .ok_or("snapshot node omitted identity")?;
        let key = format!("{snapshot_id}:{}", index + 1);
        let label = node["name"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(160)
            .collect();
        refs.insert(
            key.clone(),
            Reference {
                epoch,
                backend_node_id: id,
                label,
            },
        );
        node.as_object_mut()
            .ok_or("snapshot node is not an object")?
            .remove("backend_node_id");
        node["ref"] = json!(key);
    }
    result["snapshot_id"] = json!(snapshot_id);
    Ok(result)
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Action {
    Status,
    Open,
    Snapshot,
    Fill,
    Click,
    Screenshot,
    Assert,
    Close,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Assertion {
    TextContains,
    TextEquals,
    UrlEquals,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    action: Action,
    url: Option<String>,
    #[serde(rename = "ref")]
    reference: Option<String>,
    text: Option<String>,
    assertion: Option<Assertion>,
    expected: Option<String>,
    timeout_seconds: Option<u64>,
}

impl Arguments {
    fn validate(&self) -> Result<(), String> {
        if self.text.as_ref().is_some_and(|s| s.len() > MAX_TEXT_BYTES)
            || self
                .expected
                .as_ref()
                .is_some_and(|s| s.len() > MAX_TEXT_BYTES)
            || self.reference.as_ref().is_some_and(|s| s.len() > 128)
        {
            return Err("browser text, assertion or ref exceeds its byte limit".into());
        }
        let fields = [
            self.url.is_some(),
            self.reference.is_some(),
            self.text.is_some(),
            self.assertion.is_some(),
            self.expected.is_some(),
        ];
        let required = match self.action {
            Action::Open => [true, false, false, false, false],
            Action::Fill => [false, true, true, false, false],
            Action::Click => [false, true, false, false, false],
            Action::Assert if self.assertion == Some(Assertion::UrlEquals) => {
                [false, false, false, true, true]
            }
            Action::Assert => [false, true, false, true, true],
            _ => [false; 5],
        };
        if fields != required {
            return Err("browser action has missing or unrelated arguments".into());
        }
        Ok(())
    }
}

pub(super) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "browser".into(),
        description: "Controlled browser with explicitly configured Node 24 and Chrome. Disabled by default. Fixed actions only: status/open/snapshot/fill/click/screenshot/assert/close. Use fresh snapshot refs for elements; stale refs are refused. Open, fill and click require exact one-time approval except full-access. PNG paths are host generated. Isolated disposable profile; no uploads/downloads/arbitrary JS/user-profile attachment. Allowed origins control navigation and intercepted requests, not all process network traffic. UNKNOWN outcomes must not be retried automatically.".into(),
        parameters: json!({
            "type":"object", "additionalProperties":false, "required":["action"],
            "properties":{
                "action":{"type":"string","enum":["status","open","snapshot","fill","click","screenshot","assert","close"]},
                "url":{"type":"string","maxLength":8192,"description":"open only: absolute HTTP(S) URL at a configured origin."},
                "ref":{"type":"string","maxLength":128,"description":"Current host-issued snapshot ref; required for fill/click and text assertions."},
                "text":{"type":"string","maxLength":MAX_TEXT_BYTES,"description":"fill only. File inputs are refused."},
                "assertion":{"type":"string","enum":["text_contains","text_equals","url_equals"]},
                "expected":{"type":"string","maxLength":MAX_TEXT_BYTES},
                "timeout_seconds":{"type":"integer","minimum":1,"maximum":config::MAX_TIMEOUT_SECONDS}
            }
        }),
    }
}

impl ToolRegistry {
    /// Install explicit configuration. Shared registry constructors use Default.
    pub fn with_browser_config(mut self, config: BrowserConfig) -> Result<Self, ToolError> {
        config.validate().map_err(browser_error)?;
        self.browser = BrowserManager::new(config);
        Ok(self)
    }

    /// Cleanup may release only this parent registry's existing browser lease.
    /// The exception never grants a Worker access or authorizes another action.
    pub(super) fn browser_cleanup_call(&self, call: &super::ToolCall) -> bool {
        if call.name != "browser"
            || self.delegation_read_only
            || self.parent_approval_mode.is_some()
            || self
                .allowed_tools
                .as_ref()
                .is_some_and(|tools| !tools.contains("browser"))
        {
            return false;
        }
        let Ok(args) = serde_json::from_str::<Arguments>(&call.arguments) else {
            return false;
        };
        args.action == Action::Close
            && args.validate().is_ok()
            && args
                .timeout_seconds
                .is_none_or(|seconds| (1..=config::MAX_TIMEOUT_SECONDS).contains(&seconds))
    }

    /// Dispatch only from the shared execute path; Value is parsed strictly here.
    pub(super) async fn browser(&self, arguments: Value) -> Result<String, ToolError> {
        let args: Arguments =
            serde_json::from_value(arguments).map_err(|source| ToolError::InvalidArguments {
                tool: "browser".into(),
                source,
            })?;
        args.validate().map_err(browser_error)?;
        let config = &self.browser.config;
        let seconds = args.timeout_seconds.unwrap_or(config.timeout_seconds);
        if !(1..=config::MAX_TIMEOUT_SECONDS).contains(&seconds) {
            return Err(browser_error(
                "browser timeout_seconds must be between 1 and 60",
            ));
        }
        if args.action == Action::Status {
            return Ok(self.browser_status().await.to_string());
        }
        if args.action == Action::Close {
            let mut cached = self.browser.session.lock().await;
            // Drop sends SIGKILL/job termination even when the bridge is unresponsive.
            let existed = cached.take().is_some();
            return Ok(json!({"tool":"browser","action":"close","closed":existed}).to_string());
        }
        let fence = browser_fence(self).map_err(browser_error)?;
        config.dependencies().map_err(browser_error)?;
        let url = args
            .url
            .as_deref()
            .map(|u| config.checked_navigation(u))
            .transpose()
            .map_err(browser_error)?;
        if args.assertion == Some(Assertion::UrlEquals) {
            config
                .checked_navigation(
                    args.expected
                        .as_deref()
                        .ok_or_else(|| browser_error("missing expected URL"))?,
                )
                .map_err(browser_error)?;
        }
        let duration = self
            .remaining_execution_time()
            .map_or(Duration::from_secs(seconds), |remaining| {
                remaining.min(Duration::from_secs(seconds))
            });
        let outcome = tokio::time::timeout(duration, async {
            let mut cached = self.browser.session.lock().await;
            // The in-flight future owns the guard, never a detached background task.
            // Dropping it on cancel/timeout destroys Node and Chrome, including on
            // cancellation while waiting for the existing exact approval dialog.
            let mut session = cached.take();
            if session.is_none() && args.action != Action::Open {
                return Err(browser_error("browser is not started; use open first"));
            }
            let before = if let Some(active) = &mut session { Some(active.status().await.map_err(browser_error)?) } else { None };
            let epoch = session.as_ref().map_or(0, |active| active.epoch);
            let reference = if let Some(key) = &args.reference {
                Some(session.as_ref().ok_or_else(|| browser_error("no browser page"))?.reference(key).map_err(browser_error)?)
            } else { None };
            let mut payload = json!({"action":format!("{:?}", args.action).to_ascii_lowercase(), "expected_epoch":epoch});
            if let Some(url) = &url { payload["url"] = json!(url); }
            if let Some(reference) = &reference { payload["backend_node_id"] = json!(reference.backend_node_id); }
            if let Some(text) = &args.text { payload["text"] = json!(text); }
            if let Some(expected) = &args.expected { payload["expected"] = json!(expected); }
            if let Some(assertion) = args.assertion {
                payload["assertion"] = json!(match assertion { Assertion::TextContains => "text_contains", Assertion::TextEquals => "text_equals", Assertion::UrlEquals => "url_equals" });
            }
            if matches!(args.action, Action::Open | Action::Fill | Action::Click) {
                let digest = format!("sha256:{:x}", Sha256::digest(serde_json::to_vec(&payload).map_err(|e| browser_error(e.to_string()))?));
                let description = crate::judge::redact_credentials(&format!(
                    "browser {:?}\npage: {}\nepoch: {epoch}\ntarget: {}\naction: {digest}\ninput bytes: {}\nDisposable profile; navigation/request origin control only. Approval applies once to this exact action.",
                    args.action, url.as_deref().or_else(|| before.as_ref().and_then(|p| p["url"].as_str())).unwrap_or("about:blank"),
                    reference.as_ref().map_or("page", |r| r.label.as_str()), args.text.as_ref().map_or(0, String::len)
                ));
                if !self.allowed_by_full_access(&description) {
                    self.report_approval(&description, super::ApprovalSource::User, "exact browser action; no permanent tool authorization".into());
                    self.require_approval(&description, false).await?;
                }
            }
            // Revalidate live policy and page generation after approval, before any CDP input.
            if browser_fence(self).map_err(browser_error)? != fence {
                return Err(browser_error("browser permission changed while waiting; action refused"));
            }
            config.dependencies().map_err(browser_error)?;
            if let Some(active) = &mut session {
                active.status().await.map_err(browser_error)?;
                if active.epoch != epoch { return Err(browser_error("page epoch changed while waiting; request a new snapshot")); }
                if let Some(key) = &args.reference { active.reference(key).map_err(browser_error)?; }
            } else {
                let mut active = Session::start(config).await.map_err(browser_error)?;
                active.status().await.map_err(browser_error)?;
                payload["expected_epoch"] = json!(active.epoch);
                session = Some(active);
            }
            // Startup/status also await IO. A mode change during those waits
            // must not borrow the earlier approval for a later dispatch.
            if browser_fence(self).map_err(browser_error)? != fence {
                return Err(browser_error("browser permission changed before dispatch; action refused"));
            }
            let active = session.as_mut().ok_or_else(|| browser_error("browser startup failed"))?;
            let mut result = active.bridge.request(payload).await.map_err(browser_error)?;
            if browser_fence(self).map_err(browser_error)? != fence {
                return Err(browser_error("UNKNOWN: browser permission changed during action; session destroyed; no automatic retry"));
            }
            let mut pending_artifact = None;
            match args.action {
                Action::Snapshot => result = active.snapshot(result).map_err(browser_error)?,
                Action::Screenshot => {
                    let png = result["png"].as_str().ok_or_else(|| browser_error("screenshot omitted PNG data"))?.to_owned();
                    let artifact = active.artifacts.save(png).await.map_err(browser_error)?;
                    result.as_object_mut().ok_or_else(|| browser_error("invalid screenshot response"))?.remove("png");
                    result["artifact"] = artifact.metadata().clone();
                    pending_artifact = Some(artifact);
                }
                Action::Open | Action::Fill | Action::Click => active.refs.clear(),
                _ => {}
            }
            let after = active.status().await.map_err(browser_error)?;
            if args.action == Action::Snapshot && result["epoch"] != after["epoch"] {
                return Err(browser_error("page changed during snapshot; result discarded"));
            }
            if browser_fence(self).map_err(browser_error)? != fence {
                return Err(browser_error("browser permission changed while finalizing; session destroyed"));
            }
            if let Some(artifact) = pending_artifact {
                artifact.commit().map_err(browser_error)?;
            }
            result["tool"] = json!("browser");
            *cached = session;
            Ok(result.to_string())
        }).await;
        outcome.map_err(|_| browser_error("UNKNOWN: browser deadline exceeded; owned bridge/Chrome process tree destroyed; do not automatically retry"))?
    }

    /// Offline diagnostic contract for tool_status/doctor: no CDP request,
    /// process launch, network access, cache removal or liveness claim.
    pub(super) fn browser_configuration_status(&self) -> Value {
        let config = &self.browser.config;
        let dependency = config.dependencies();
        let policy = browser_fence(self);
        let cache = match self.browser.session.try_lock() {
            Ok(slot) if slot.is_some() => "present_unverified",
            Ok(_) => "absent",
            Err(_) => "busy",
        };
        json!({
            "tool":"browser", "mode":"configuration_only", "enabled":config.enabled,
            "ready":false, "dependencies":{"paths_available":dependency.is_ok(),"reason":dependency.err(),"versions_verified":false},
            "configuration":{"allowed_origins":config.allowed_origins,"artifact_dir":config.artifact_dir,"timeout_seconds":config.timeout_seconds},
            "permission":{"allowed":policy.is_ok(),"reason":policy.err()}, "cache":cache,
            "recovery":"Configure absolute Node 24/Chrome paths, an existing artifact directory and explicit origins; enable browser, then use browser status/open for online checks.",
            "network_boundary":"navigation and intercepted requests; not process network isolation"
        })
    }

    async fn browser_status(&self) -> Value {
        let config = &self.browser.config;
        let dependency = config.dependencies();
        let policy = browser_fence(self);
        let mut live = None;
        let runtime = match self.browser.session.try_lock() {
            Ok(mut slot) => {
                if dependency.is_err() || policy.is_err() {
                    let existed = slot.take().is_some();
                    if existed { "retired" } else { "not_started" }
                } else if let Some(mut active) = slot.take() {
                    // A status query never starts Chrome and never trusts a cached
                    // process handle as proof that the CDP connection is alive.
                    match tokio::time::timeout(Duration::from_secs(2), active.status()).await {
                        Ok(Ok(result)) => {
                            live = Some(result);
                            *slot = Some(active);
                            "running"
                        }
                        _ => "failed",
                    }
                } else {
                    "not_started"
                }
            }
            Err(_) => "busy",
        };
        json!({
            "tool":"browser", "action":"status", "enabled":config.enabled,
            "ready":dependency.is_ok() && policy.is_ok() && runtime == "running",
            "dependencies":{"paths_available":dependency.is_ok(),"reason":dependency.err(),"runtime_versions":"Node >=24 and Chrome CDP verified only when open succeeds"},
            "permission":{"allowed":policy.is_ok(),"reason":policy.err()},
            "runtime":runtime,"page":live,"allowed_origins":config.allowed_origins,"artifact_dir":config.artifact_dir,
            "recovery":"Configure absolute Node 24/Chrome paths, an existing artifact directory and explicit origins, then enable browser and open a page. No dependencies are downloaded.",
            "boundaries":["Fresh disposable profile; no user profile attachment, upload, download or arbitrary JavaScript action.","CDP Fetch and injected CSP control navigation and page requests; Chrome background-network flags reduce background traffic. This is not whole-process network isolation.","Cancellation/timeouts destroy the owned process tree. UNKNOWN results are never automatically retried."]
        })
    }
}

fn browser_fence(registry: &ToolRegistry) -> Result<String, String> {
    registry
        .check_tool_policy("browser")
        .map_err(|e| e.to_string())?;
    if !registry.allows_agent_tool("browser") || registry.approval_mode() == ApprovalMode::ReadOnly
    {
        return Err("current tool permissions forbid browser execution".into());
    }
    let sandbox = registry.effective_sandbox();
    // Never silently skip an enforcing OS sandbox when launching an executable.
    // This backend currently supports the explicit off policy only.
    if sandbox.policy.is_enforcing() {
        return Err("browser backend cannot preserve the active OS sandbox; launch refused".into());
    }
    serde_json::to_string(&(registry.approval_mode().as_str(), sandbox))
        .map_err(|e| format!("encode browser permission fence: {e}"))
}

fn browser_error(message: impl Into<String>) -> ToolError {
    ToolError::Network(format!("browser: {}", message.into()))
}
