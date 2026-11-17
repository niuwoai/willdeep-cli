//! Read-only discovery for optional integrations. Probes never start a process.

use super::*;

const INTEGRATIONS: [&str; 5] = ["lsp", "ast_grep", "browser", "web_search", "mcp"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    tool: Option<String>,
}

pub(super) fn definition() -> ToolDefinition {
    ToolDefinition {
        name: "tool_status".into(),
        description: "Inspect optional tool dependencies, configuration, permissions and recovery instructions without starting processes or contacting external services. This is configuration status, not a connectivity or compatibility test.".into(),
        parameters: json!({
            "type":"object", "additionalProperties":false,
            "properties":{"tool":{"type":"string","enum":INTEGRATIONS}},
        }),
    }
}

impl ToolRegistry {
    pub(super) async fn integration_status(&self, call: &ToolCall) -> Result<String, ToolError> {
        let args: Arguments = parse(call)?;
        let names = match args.tool.as_deref() {
            Some(name) if INTEGRATIONS.contains(&name) => vec![name],
            Some(_) => {
                return Err(ToolError::UnknownTool(
                    "unsupported integration status".into(),
                ));
            }
            None => INTEGRATIONS.to_vec(),
        };
        let mut statuses = Vec::new();
        for name in names {
            let mut value = match name {
                "mcp" => self.mcp.configuration_status(),
                "browser" => self.browser_configuration_status(),
                _ => {
                    // Only host-owned status arguments reach private dispatch.
                    // Caller arguments can never select an optional operation.
                    let probe = ToolCall {
                        id: call.id.clone(),
                        name: if name == "web_search" {
                            "web_search_status"
                        } else {
                            name
                        }
                        .into(),
                        arguments: if name == "web_search" {
                            "{}"
                        } else {
                            "{\"operation\":\"status\"}"
                        }
                        .into(),
                    };
                    match Box::pin(self.dispatch(&probe)).await {
                        Ok(text) => serde_json::from_str::<serde_json::Value>(&text)
                            .unwrap_or_else(|_| json!({"status":"invalid_status_response"})),
                        Err(_) => json!({
                            "status":"unavailable", "reason":"configuration_inspection_failed",
                        }),
                    }
                }
            };
            let permission = self.integration_permission(name);
            // Replace legacy permission.allowed (which only checked a launch
            // fence) so the same response never implies an action was approved.
            value["permission"] = permission.clone();
            statuses.push(json!({"tool":name,"configuration":value,"permission":permission}));
        }
        Ok(json!({
            "operation":"status", "connectivity":"not_checked", "processes_started":false,
            "approval_mode":self.approval_mode().as_str(),
            "delegation_read_only":self.delegation_read_only,
            "integrations":statuses,
        })
        .to_string())
    }

    fn integration_permission(&self, name: &str) -> serde_json::Value {
        let tool = if name == "mcp" { "call_mcp_tool" } else { name };
        // These are fixed built-ins: apply their delegation/read-only/allowlist
        // gates directly without asking a lazy MCP source to resolve names.
        let sandbox = self.effective_sandbox();
        let mode = self.approval_mode();
        let (state, reason) = if self.delegation_read_only {
            ("blocked", "delegation_read_only")
        } else if name == "browser" && self.parent_approval_mode.is_some() {
            ("blocked", "parent_only_tool")
        } else if mode == ApprovalMode::ReadOnly && name != "web_search" {
            ("blocked", "read_only_policy")
        } else if self
            .allowed_tools
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(tool))
        {
            ("blocked", "tool_not_in_allowlist")
        } else if name == "browser" && sandbox.policy.is_enforcing() {
            ("blocked", "browser_sandbox_unsupported")
        } else if self.hooks.has_event(HookEvent::PreTool) {
            ("runtime_check_required", "pre_tool_hooks_not_evaluated")
        } else if name == "web_search" && mode != ApprovalMode::Strict {
            ("allowed_by_policy", "network_read_policy")
        } else if mode == ApprovalMode::FullAccess {
            ("allowed_by_policy", "full_access_policy")
        } else if matches!(name, "lsp" | "ast_grep") {
            ("runtime_check_required", "command_approval_not_evaluated")
        } else {
            ("approval_required", "action_approval_not_requested")
        };
        json!({
            "state":state,
            "allowed":state == "allowed_by_policy",
            "reason":reason,
            "action_approval":"not_requested",
            "availability":"reported_separately_in_configuration",
            "sandbox":{
                "policy":sandbox.policy,
                "network_allowed_for_subprocesses":sandbox.allows_network(),
                "backend":"not_checked",
                "boundary":match name {
                    "lsp" | "ast_grep" => "command_os_sandbox_required_when_enforcing",
                    "browser" => "enforcing_command_sandbox_unsupported",
                    "web_search" => "host_http_client_not_command_sandboxed",
                    _ => "existing_mcp_transports_not_command_sandboxed",
                },
            },
            "recovery":match state {
                "blocked" => "Ask the operator to change the session policy or delegated tool scope; requesting approval alone does not remove this restriction.",
                "allowed_by_policy" => "Policy permits the operation; configuration, dependencies and operation-specific validation still apply.",
                _ => "Invoke the concrete operation through Runtime to evaluate its exact approval and execution boundary; this status grants no permission.",
            },
        })
    }
}

#[cfg(test)]
mod tests;
