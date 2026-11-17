//! Offline MCP discovery. Retain no endpoints, command arguments or credentials.

use std::sync::atomic::{AtomicU8, Ordering};

use super::*;

const UNOBSERVED: u8 = 0;
const SUCCEEDED: u8 = 1;
const AUTHORIZATION_REQUIRED: u8 = 2;
const FAILED: u8 = 3;

#[derive(Default)]
pub(super) struct RequestObservation(AtomicU8);

impl RequestObservation {
    pub(super) fn record<T>(&self, result: &Result<T, McpError>) {
        let state = match result {
            Ok(_) => SUCCEEDED,
            Err(McpError::Unauthorized { .. }) => AUTHORIZATION_REQUIRED,
            Err(_) => FAILED,
        };
        self.0.store(state, Ordering::Relaxed);
    }

    fn state(&self) -> u8 {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone)]
pub(super) struct ServerStatus {
    enabled: bool,
    valid: bool,
    transport: Option<McpTransportKind>,
    authentication: &'static str,
    pub(super) observation: Arc<RequestObservation>,
}

impl ServerStatus {
    fn configured(name: &str, config: &McpServerConfig) -> Self {
        let transport = config.validated_transport(name).ok();
        // Retain declarations only. Validation may resolve header environment
        // references, but no value or credential-presence claim enters status.
        // OAuth files and remote authentication are never probed here.
        let authentication = if config.oauth.is_some() {
            "oauth"
        } else if config.bearer_token_env.is_some() {
            "bearer_environment"
        } else if !config.headers.is_empty() {
            "headers"
        } else {
            "not_declared"
        };
        Self {
            enabled: config.enabled,
            valid: transport.is_some(),
            transport,
            authentication,
            observation: Arc::new(RequestObservation::default()),
        }
    }
}

impl McpRegistry {
    /// Build a configuration-only registry without starting servers, contacting
    /// services or loading OAuth credentials. Useful for doctor and read-only
    /// delegations, whose normal startup deliberately does not connect MCP.
    pub fn configuration_only(configs: &BTreeMap<String, McpServerConfig>) -> Self {
        Self {
            status: configs
                .iter()
                .map(|(name, config)| (name.clone(), ServerStatus::configured(name, config)))
                .collect(),
            ..Self::default()
        }
    }

    /// Snapshot only existing observations. This never refreshes a lazy tool
    /// catalog, probes a connection, or reads a credential file. A successful
    /// request does not establish which authentication a remote tool requires.
    pub fn configuration_status(&self) -> Value {
        let servers: Vec<Value> = self
            .status
            .iter()
            .enumerate()
            .map(|(index, (name, status))| {
                let observed = status.observation.state();
                let connection = if !status.enabled {
                    "disabled"
                } else if !status.valid {
                    "invalid_configuration"
                } else if let Some(server) = self.servers.get(name) {
                    match &server.transport {
                        Transport::Stdio(connection) if !connection.is_alive() => "exited",
                        _ => "initialized_unverified",
                    }
                } else if observed == UNOBSERVED {
                    "not_attempted"
                } else {
                    "failed"
                };
                json!({
                    // Stable ordinal within this session's configuration. Never
                    // echo arbitrary configured names into a shareable report.
                    "server_index":index + 1,
                    "enabled":status.enabled,
                    "configuration_check":{
                        "valid":status.valid,
                        "reason":if status.valid {None} else {Some("invalid_mcp_configuration")},
                    },
                    "transport":match status.transport {
                        Some(McpTransportKind::Stdio) => "stdio",
                        Some(McpTransportKind::StreamableHttp) => "streamable_http",
                        None => "unknown",
                    },
                    "connection":connection,
                    "last_request":match observed {
                        SUCCEEDED => "succeeded",
                        AUTHORIZATION_REQUIRED => "authorization_required",
                        FAILED => "failed",
                        _ => "not_observed",
                    },
                    "authentication":{
                        "mode":status.authentication,
                        "state":if observed == AUTHORIZATION_REQUIRED {"required"} else {"unknown"},
                        "credentials_checked":false,
                    },
                    "recovery":match connection {
                        "disabled" => "Enable the configured MCP server if needed, then restart the session.",
                        "invalid_configuration" => "Run willdeep config check and repair the MCP server declaration.",
                        _ if observed == AUTHORIZATION_REQUIRED && status.authentication == "oauth" =>
                            "Use willdeep mcp login with the configured server name, then reconnect explicitly.",
                        _ if observed == AUTHORIZATION_REQUIRED =>
                            "Review the declared authentication with the operator, then reconnect explicitly.",
                        "failed" | "exited" => "Review MCP configuration and reconnect explicitly; no automatic retry was performed.",
                        "not_attempted" => "MCP was not connected in this context; use an authorized session to connect explicitly.",
                        _ => "Connection is cached only; explicitly invoke an approved MCP operation to verify service access.",
                    },
                })
            })
            .collect();
        json!({
            "tool":"mcp", "mode":"configuration_only", "scope":"session_configuration",
            "configured":servers.len(),
            "enabled":self.status.values().filter(|status| status.enabled).count(),
            "servers":servers,
            "lazy_sources":if self.lazy.is_some() {"present_unprobed"} else {"absent"},
            "connectivity":"not_checked", "processes_started":false,
            "credentials_checked":false,
            "recovery":"Configure [mcp_servers.<name>], enable the required server, and connect only in an authorized session. OAuth requires explicit willdeep mcp login <name>.",
            "boundaries":[
                "This snapshot never calls tools/list or refreshes plugin catalogs.",
                "Cached initialization and last-request observations do not prove current connectivity or credential validity.",
                "Existing MCP transports do not inherit the command OS sandbox; tool calls still use Runtime approval."
            ],
        })
    }
}

#[cfg(test)]
mod tests;
