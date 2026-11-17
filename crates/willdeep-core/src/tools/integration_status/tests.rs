use super::*;
use crate::hooks::{Hook, HookFailure, HookRegistry};
use crate::mcp::{LazyToolSource, McpError, McpServerConfig};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("integration-status-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn tools(&self, mode: ApprovalMode) -> ToolRegistry {
        ToolRegistry::new(&self.0, mode).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

async fn status(tools: &ToolRegistry, name: Option<&str>) -> serde_json::Value {
    let call = ToolCall {
        id: "status".into(),
        name: "tool_status".into(),
        arguments: name.map_or_else(|| "{}".into(), |name| json!({"tool":name}).to_string()),
    };
    serde_json::from_str(&tools.execute(&call).await.unwrap()).unwrap()
}

#[tokio::test]
async fn public_status_survives_read_only_and_cannot_forward_actions() {
    let fixture = Fixture::new();
    let tools = fixture
        .tools(ApprovalMode::ReadOnly)
        .with_delegation_read_only();
    assert!(
        tools
            .definitions()
            .iter()
            .any(|tool| tool.name == "tool_status")
    );
    let value = status(&tools, None).await;
    assert_eq!(value["processes_started"], false);
    assert_eq!(value["integrations"].as_array().unwrap().len(), 5);
    for probe in value["integrations"].as_array().unwrap() {
        assert_ne!(probe["configuration"]["status"], "invalid_status_response");
        assert_ne!(probe["configuration"]["status"], "unavailable");
        assert!(probe["configuration"].is_object());
        assert_eq!(probe["permission"]["state"], "blocked");
        assert_eq!(probe["permission"]["reason"], "delegation_read_only");
    }
    for arguments in [
        json!({"tool":"browser","operation":"open","url":"https://example.org"}),
        json!({"tool":"run_command"}),
        json!({"tool":"mcp","name":"anything","arguments":{}}),
    ] {
        let call = ToolCall {
            id: "invalid".into(),
            name: "tool_status".into(),
            arguments: arguments.to_string(),
        };
        assert!(tools.execute(&call).await.is_err());
    }
}

#[tokio::test]
async fn permissions_follow_live_modes_and_do_not_grant_pending_approval() {
    let fixture = Fixture::new();
    let tools = fixture.tools(ApprovalMode::Strict);
    let handle = tools.approval_mode_handle();
    for (mode, tool, state, reason) in [
        (
            ApprovalMode::Strict,
            "lsp",
            "runtime_check_required",
            "command_approval_not_evaluated",
        ),
        (
            ApprovalMode::Strict,
            "ast_grep",
            "runtime_check_required",
            "command_approval_not_evaluated",
        ),
        (
            ApprovalMode::Strict,
            "browser",
            "approval_required",
            "action_approval_not_requested",
        ),
        (
            ApprovalMode::Strict,
            "mcp",
            "approval_required",
            "action_approval_not_requested",
        ),
        (
            ApprovalMode::Strict,
            "web_search",
            "approval_required",
            "action_approval_not_requested",
        ),
        (ApprovalMode::ReadOnly, "lsp", "blocked", "read_only_policy"),
        (ApprovalMode::ReadOnly, "mcp", "blocked", "read_only_policy"),
        (
            ApprovalMode::ReadOnly,
            "web_search",
            "allowed_by_policy",
            "network_read_policy",
        ),
        (
            ApprovalMode::FullAccess,
            "browser",
            "allowed_by_policy",
            "full_access_policy",
        ),
    ] {
        handle.set(mode);
        let value = status(&tools, Some(tool)).await;
        let permission = &value["integrations"][0]["permission"];
        assert_eq!(permission["state"], state, "{tool} {}", mode.as_str());
        assert_eq!(permission["reason"], reason);
        assert_eq!(permission["allowed"], state == "allowed_by_policy");
        assert_eq!(permission["action_approval"], "not_requested");
        assert_eq!(
            permission,
            &value["integrations"][0]["configuration"]["permission"]
        );
    }
}

#[tokio::test]
async fn delegation_allowlists_and_sandboxes_are_reported_independently() {
    let fixture = Fixture::new();
    let parent = SharedApprovalMode::new(ApprovalMode::FullAccess);
    let delegated = fixture
        .tools(ApprovalMode::Strict)
        .with_parent_approval_mode(parent)
        .with_delegation_read_only();
    let value = status(&delegated, Some("mcp")).await;
    assert_eq!(
        value["integrations"][0]["permission"]["reason"],
        "delegation_read_only"
    );
    let parent_only = fixture
        .tools(ApprovalMode::FullAccess)
        .with_parent_approval_mode(SharedApprovalMode::new(ApprovalMode::FullAccess));
    assert_eq!(
        status(&parent_only, Some("browser")).await["integrations"][0]["permission"]["reason"],
        "parent_only_tool"
    );
    let limited = fixture
        .tools(ApprovalMode::FullAccess)
        .with_allowed_tools(["tool_status".to_owned()]);
    for name in INTEGRATIONS {
        assert_eq!(
            status(&limited, Some(name)).await["integrations"][0]["permission"]["reason"],
            "tool_not_in_allowlist"
        );
    }
    let fenced = fixture.tools(ApprovalMode::Smart).with_sandbox(
        SandboxSpec::new(SandboxPolicy::WorkspaceWrite, [fixture.0.clone()])
            .with_network(crate::sandbox::NetworkPolicy::Deny),
    );
    let browser = status(&fenced, Some("browser")).await;
    assert_eq!(
        browser["integrations"][0]["permission"]["reason"],
        "browser_sandbox_unsupported"
    );
    let ast = status(&fenced, Some("ast_grep")).await;
    assert_eq!(
        ast["integrations"][0]["permission"]["sandbox"]["network_allowed_for_subprocesses"],
        false
    );
    assert_eq!(
        ast["integrations"][0]["permission"]["sandbox"]["backend"],
        "not_checked"
    );
    let search = status(&fenced, Some("web_search")).await;
    assert_eq!(
        search["integrations"][0]["permission"]["state"],
        "allowed_by_policy"
    );
    assert_eq!(
        search["integrations"][0]["permission"]["sandbox"]["boundary"],
        "host_http_client_not_command_sandboxed"
    );
}

struct UnusedLazySource(AtomicUsize);

#[async_trait::async_trait]
impl LazyToolSource for UnusedLazySource {
    fn definitions(&self) -> Vec<ToolDefinition> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("status must not read a lazy catalog")
    }
    fn available(&self) -> bool {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("status must not probe a lazy source")
    }
    async fn refresh_missing(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("status must not start a lazy MCP source")
    }
    async fn call(&self, _: &str, _: serde_json::Value) -> Result<String, McpError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("status must not invoke MCP")
    }
}

#[tokio::test]
async fn status_never_runs_hooks_connects_mcp_or_refreshes_lazy_sources() {
    let fixture = Fixture::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let configs = std::collections::BTreeMap::from([(
        "local".into(),
        McpServerConfig {
            url: Some(format!("http://{}/mcp", listener.local_addr().unwrap())),
            ..McpServerConfig::default()
        },
    )]);
    let source = Arc::new(UnusedLazySource(AtomicUsize::new(0)));
    let tools = fixture
        .tools(ApprovalMode::Strict)
        .with_mcp(Arc::new(
            McpRegistry::configuration_only(&configs).with_lazy_tools(source.clone()),
        ))
        .with_hooks(HookRegistry::new(vec![Hook {
            name: "status-must-not-launch-this-hook".into(),
            event: HookEvent::PreTool,
            command: "exit 37".into(),
            blocking: true,
            timeout: std::time::Duration::from_secs(1),
            on_error: HookFailure::Deny,
        }]));
    for mode in [ApprovalMode::Strict, ApprovalMode::ReadOnly] {
        tools.approval_mode_handle().set(mode);
        let value = status(&tools, Some("mcp")).await;
        assert_eq!(
            value["integrations"][0]["configuration"]["servers"][0]["connection"],
            "not_attempted"
        );
        assert_eq!(
            value["integrations"][0]["permission"]["reason"],
            if mode == ApprovalMode::Strict {
                "pre_tool_hooks_not_evaluated"
            } else {
                "read_only_policy"
            }
        );
    }
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(source.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn lsp_status_reports_invalid_and_inherited_local_configuration() {
    let fixture = Fixture::new();
    let tools = fixture.tools(ApprovalMode::ReadOnly);
    for config in [
        "{invalid-json",
        r#"{"extends":"../external-private-config.json"}"#,
    ] {
        std::fs::write(fixture.0.join("tsconfig.json"), config).unwrap();
        let value = status(&tools, Some("lsp")).await;
        let servers = value["integrations"][0]["configuration"]["servers"]
            .as_array()
            .unwrap();
        let typescript = servers
            .iter()
            .find(|server| server["language"] == "typescript")
            .unwrap();
        assert_eq!(typescript["configuration_check"]["valid"], false);
        assert!(
            typescript["configuration_check"]["reason"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
    }
}
