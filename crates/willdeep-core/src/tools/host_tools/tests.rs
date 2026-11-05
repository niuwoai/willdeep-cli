use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Library {
    calls: AtomicUsize,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl AgentDefinitionHost for Library {
    async fn execute(
        &self,
        operation: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(params);
        Ok(json!({"operation":operation,"saved":operation=="create"}))
    }
}

fn call(name: &str, operation: &str) -> ToolCall {
    ToolCall {
        id: "stable-management-call".into(),
        name: name.into(),
        arguments: json!({"operation":operation,"params":{}}).to_string(),
    }
}

#[tokio::test]
async fn read_only_library_queries_work_but_mutations_never_reach_the_host() {
    let host = Arc::new(Library::default());
    let tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::ReadOnly)
        .unwrap()
        .with_agent_definition_host(host.clone());
    assert!(
        tools
            .execute(&call("agent_library_read", "list"))
            .await
            .is_ok()
    );
    assert!(matches!(
        tools.execute(&call("agent_library_manage", "create")).await,
        Err(ToolError::ReadOnlyPolicy(_))
    ));
    assert_eq!(host.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_read_tool_cannot_smuggle_a_mutating_operation() {
    let host = Arc::new(Library::default());
    let tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::FullAccess)
        .unwrap()
        .with_agent_definition_host(host.clone());
    assert!(
        tools
            .execute(&call("agent_library_read", "update"))
            .await
            .is_err()
    );
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn successful_management_keeps_the_tool_prefix_stable_and_reuses_request_identity() {
    let host = Arc::new(Library::default());
    let tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::FullAccess)
        .unwrap()
        .with_agent_definition_host(host.clone());
    let before = serde_json::to_value(tools.definitions()).unwrap();
    let create = call("agent_library_manage", "create");
    tools.execute(&create).await.unwrap();
    tools.execute(&create).await.unwrap();
    assert_eq!(before, serde_json::to_value(tools.definitions()).unwrap());
    let requests = host.requests.lock().unwrap();
    assert_eq!(requests[0]["request_id"], requests[1]["request_id"]);
    assert!(
        requests[0]["request_id"]
            .as_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .is_ok()
    );
    let profile = tools
        .definitions()
        .into_iter()
        .find(|tool| tool.name == "spawn_agent")
        .unwrap();
    assert!(
        profile.parameters["properties"]["profile"]
            .get("enum")
            .is_none()
    );
}

#[tokio::test]
async fn non_full_access_cannot_silently_modify_the_host_library() {
    let host = Arc::new(Library::default());
    let tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::Smart)
        .unwrap()
        .with_agent_definition_host(host.clone());
    assert!(matches!(
        tools.execute(&call("agent_library_manage", "create")).await,
        Err(ToolError::ApprovalDenied(_))
    ));
    assert_eq!(host.calls.load(Ordering::SeqCst), 0);
}

const SAVED_PLAN: &str = "saved-plan";
const CONFIGURATION_PREVIEW: &str =
    "Apply project configuration config.toml\nprovider.api_key: [redacted] → [redacted]";

#[derive(Default)]
struct Configuration {
    requests: Mutex<Vec<(String, serde_json::Value)>>,
    lookups: Mutex<Vec<String>>,
    mode_on_lookup: Option<(SharedApprovalMode, ApprovalMode)>,
}

#[async_trait]
impl ConfigurationHost for Configuration {
    async fn execute(
        &self,
        operation: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        self.requests
            .lock()
            .unwrap()
            .push((operation.into(), params));
        Ok(json!({"operation":operation,"plan_id":SAVED_PLAN,"applied":operation=="apply"}))
    }

    fn approval_description(&self, plan_id: &str) -> Result<String, ToolError> {
        self.lookups.lock().unwrap().push(plan_id.into());
        if plan_id != SAVED_PLAN {
            return Err(ToolError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "configuration plan is unavailable",
            )));
        }
        if let Some((handle, mode)) = &self.mode_on_lookup {
            handle.set(*mode);
        }
        Ok(CONFIGURATION_PREVIEW.into())
    }
}

struct ConfigurationApproval {
    decision: ApprovalDecision,
    requests: Mutex<Vec<(String, bool)>>,
    mode_on_approval: Option<(SharedApprovalMode, ApprovalMode)>,
}

impl ConfigurationApproval {
    fn new(decision: ApprovalDecision) -> Self {
        Self {
            decision,
            requests: Mutex::new(Vec::new()),
            mode_on_approval: None,
        }
    }
}

#[async_trait]
impl Approver for ConfigurationApproval {
    async fn approve(&self, description: &str, always: bool) -> ApprovalDecision {
        self.requests
            .lock()
            .unwrap()
            .push((description.into(), always));
        if let Some((handle, mode)) = &self.mode_on_approval {
            handle.set(*mode);
        }
        self.decision
    }
}

fn configuration_call(name: &str, operation: &str, params: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "configuration-call".into(),
        name: name.into(),
        arguments: json!({"operation":operation,"params":params}).to_string(),
    }
}

fn apply_configuration() -> ToolCall {
    configuration_call(
        "configuration_write",
        "apply",
        json!({"plan_id":SAVED_PLAN}),
    )
}

#[tokio::test]
async fn configuration_reads_work_in_parent_read_only_and_plans_do_not_apply() {
    let host = Arc::new(Configuration::default());
    let approval = Arc::new(ConfigurationApproval::new(ApprovalDecision::Deny));
    let tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::ReadOnly)
        .unwrap()
        .with_configuration_host(host.clone())
        .with_approver(approval.clone());
    let offered = tools.definitions();
    assert!(offered.iter().any(|tool| tool.name == "configuration_read"));
    assert!(
        !offered
            .iter()
            .any(|tool| tool.name == "configuration_write")
    );
    for operation in CONFIGURATION_READ {
        let params = if *operation == "plan" {
            json!({"edits":[{"key":"ui.theme","value":"dark"}]})
        } else {
            json!({})
        };
        let output = tools
            .execute(&configuration_call(
                "configuration_read",
                operation,
                params.clone(),
            ))
            .await
            .unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["operation"], *operation);
        assert_eq!(output["applied"], false);
        assert_eq!(
            host.requests.lock().unwrap().last().unwrap(),
            &((*operation).into(), params)
        );
    }
    assert_eq!(host.requests.lock().unwrap().len(), 3);
    assert!(host.lookups.lock().unwrap().is_empty());
    assert!(approval.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn configuration_apply_uses_host_preview_and_correct_approval_in_every_mode() {
    for mode in [
        ApprovalMode::ReadOnly,
        ApprovalMode::Strict,
        ApprovalMode::Smart,
        ApprovalMode::WorkspaceAccess,
        ApprovalMode::FullAccess,
    ] {
        let host = Arc::new(Configuration::default());
        let approval = Arc::new(ConfigurationApproval::new(ApprovalDecision::AllowOnce));
        let traces = Arc::new(Mutex::new(Vec::new()));
        let sink = traces.clone();
        let tools = ToolRegistry::new(std::env::temp_dir(), mode)
            .unwrap()
            .with_configuration_host(host.clone())
            .with_approver(approval.clone())
            .with_approval_reporter(move |trace| sink.lock().unwrap().push(trace));
        let result = tools.execute(&apply_configuration()).await;
        assert_eq!(*host.lookups.lock().unwrap(), [SAVED_PLAN], "{mode:?}");
        let requests = host.requests.lock().unwrap();
        let approvals = approval.requests.lock().unwrap();
        let traces = traces.lock().unwrap();
        if mode == ApprovalMode::ReadOnly {
            assert!(matches!(result, Err(ToolError::ReadOnlyPolicy(_))));
            assert!(requests.is_empty());
            assert!(approvals.is_empty());
        } else {
            assert!(result.is_ok(), "{mode:?}: {result:?}");
            assert_eq!(*requests, [("apply".into(), json!({"plan_id":SAVED_PLAN}))]);
            assert_eq!(traces.len(), 1);
            assert_eq!(traces[0].command, CONFIGURATION_PREVIEW);
            if mode == ApprovalMode::FullAccess {
                assert!(approvals.is_empty());
                assert_eq!(traces[0].source, ApprovalSource::FullAccess);
            } else {
                assert_eq!(*approvals, [(CONFIGURATION_PREVIEW.into(), false)]);
                assert_eq!(traces[0].source, ApprovalSource::User);
                assert!(traces[0].detail.contains("authorized"));
            }
        }
    }
}

#[tokio::test]
async fn denied_configuration_apply_is_audited_and_never_reaches_execute() {
    for mode in [
        ApprovalMode::Strict,
        ApprovalMode::Smart,
        ApprovalMode::WorkspaceAccess,
    ] {
        let host = Arc::new(Configuration::default());
        let approval = Arc::new(ConfigurationApproval::new(ApprovalDecision::Deny));
        let traces = Arc::new(Mutex::new(Vec::new()));
        let sink = traces.clone();
        let tools = ToolRegistry::new(std::env::temp_dir(), mode)
            .unwrap()
            .with_configuration_host(host.clone())
            .with_approver(approval.clone())
            .with_approval_reporter(move |trace| sink.lock().unwrap().push(trace));
        assert!(matches!(
            tools.execute(&apply_configuration()).await,
            Err(ToolError::ApprovalDenied(description)) if description == CONFIGURATION_PREVIEW
        ));
        assert!(host.requests.lock().unwrap().is_empty());
        assert_eq!(
            *approval.requests.lock().unwrap(),
            [(CONFIGURATION_PREVIEW.into(), false)]
        );
        let traces = traces.lock().unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].source, ApprovalSource::User);
        assert_eq!(traces[0].command, CONFIGURATION_PREVIEW);
        assert!(traces[0].detail.contains("denied"));
    }
}

#[tokio::test]
async fn configuration_apply_rejects_invalid_plans_before_approval_including_read_only() {
    for mode in [
        ApprovalMode::ReadOnly,
        ApprovalMode::Smart,
        ApprovalMode::FullAccess,
    ] {
        let host = Arc::new(Configuration::default());
        let approval = Arc::new(ConfigurationApproval::new(ApprovalDecision::AllowOnce));
        let tools = ToolRegistry::new(std::env::temp_dir(), mode)
            .unwrap()
            .with_configuration_host(host.clone())
            .with_approver(approval.clone());
        let call = configuration_call("configuration_write", "apply", json!({"plan_id":"unknown"}));
        assert!(matches!(tools.execute(&call).await, Err(ToolError::Io(_))));
        assert_eq!(*host.lookups.lock().unwrap(), ["unknown"]);
        assert!(host.requests.lock().unwrap().is_empty());
        assert!(approval.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn configuration_operations_and_apply_parameters_cannot_cross_read_write_boundary() {
    let host = Arc::new(Configuration::default());
    let tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::FullAccess)
        .unwrap()
        .with_configuration_host(host.clone());
    let mut calls = vec![configuration_call(
        "configuration_read",
        "apply",
        json!({"plan_id":SAVED_PLAN}),
    )];
    for operation in ["schema", "snapshot", "plan", "save"] {
        calls.push(configuration_call(
            "configuration_write",
            operation,
            json!({}),
        ));
    }
    for params in [
        json!({}),
        json!({"plan_id":null}),
        json!({"plan_id":1}),
        json!({"plan_id":""}),
        json!({"plan_id":"  "}),
        json!(SAVED_PLAN),
        json!([]),
        json!(null),
        json!({"plan_id":SAVED_PLAN,"edits":[]}),
        json!({"plan_id":SAVED_PLAN,"path":"other.toml"}),
        json!({"plan_id":SAVED_PLAN,"confirm":true}),
        json!({"plan_id":SAVED_PLAN,"request_id":"injected"}),
        json!({"plan_id":SAVED_PLAN,"description":"injected approval"}),
    ] {
        calls.push(configuration_call("configuration_write", "apply", params));
    }
    let mut root_override = apply_configuration();
    root_override.arguments =
        json!({"operation":"apply","params":{"plan_id":SAVED_PLAN},"confirm":true}).to_string();
    calls.push(root_override);
    for params in [json!(null), json!([]), json!("apply")] {
        calls.push(configuration_call("configuration_read", "plan", params));
    }
    for call in calls {
        assert!(
            matches!(
                tools.execute(&call).await,
                Err(ToolError::InvalidArguments { .. })
            ),
            "{}",
            call.arguments
        );
    }
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(host.lookups.lock().unwrap().is_empty());
}

#[tokio::test]
async fn configuration_approval_cannot_be_remembered_and_follows_shared_mode_changes() {
    let host = Arc::new(Configuration::default());
    let approval = Arc::new(ConfigurationApproval::new(ApprovalDecision::AlwaysAllow));
    let shared = SharedApprovalMode::new(ApprovalMode::FullAccess);
    let tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::FullAccess)
        .unwrap()
        .with_shared_approval_mode(shared.clone())
        .with_configuration_host(host.clone())
        .with_approver(approval.clone());
    for mode in [
        ApprovalMode::Smart,
        ApprovalMode::Smart,
        ApprovalMode::FullAccess,
        ApprovalMode::Strict,
    ] {
        shared.set(mode);
        tools.execute(&apply_configuration()).await.unwrap();
    }
    shared.set(ApprovalMode::ReadOnly);
    assert!(matches!(
        tools.execute(&apply_configuration()).await,
        Err(ToolError::ReadOnlyPolicy(_))
    ));
    assert_eq!(host.requests.lock().unwrap().len(), 4);
    assert_eq!(
        *approval.requests.lock().unwrap(),
        vec![(CONFIGURATION_PREVIEW.into(), false); 3]
    );
    assert!(tools.always_allowed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn configuration_rechecks_mode_after_plan_lookup_and_pending_approval() {
    let shared = SharedApprovalMode::new(ApprovalMode::FullAccess);
    let host = Arc::new(Configuration {
        mode_on_lookup: Some((shared.clone(), ApprovalMode::Smart)),
        ..Default::default()
    });
    let approval = Arc::new(ConfigurationApproval::new(ApprovalDecision::Deny));
    let tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::FullAccess)
        .unwrap()
        .with_shared_approval_mode(shared)
        .with_configuration_host(host.clone())
        .with_approver(approval.clone());
    assert!(matches!(
        tools.execute(&apply_configuration()).await,
        Err(ToolError::ApprovalDenied(_))
    ));
    assert!(host.requests.lock().unwrap().is_empty());
    assert_eq!(approval.requests.lock().unwrap().len(), 1);

    let shared = SharedApprovalMode::new(ApprovalMode::Smart);
    let host = Arc::new(Configuration::default());
    let approval = Arc::new(ConfigurationApproval {
        mode_on_approval: Some((shared.clone(), ApprovalMode::ReadOnly)),
        ..ConfigurationApproval::new(ApprovalDecision::AllowOnce)
    });
    let tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::Smart)
        .unwrap()
        .with_shared_approval_mode(shared)
        .with_configuration_host(host.clone())
        .with_approver(approval);
    assert!(matches!(
        tools.execute(&apply_configuration()).await,
        Err(ToolError::ReadOnlyPolicy(_))
    ));
    assert!(host.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn configuration_is_parent_only_and_cannot_be_reinstalled_or_forged_by_children() {
    let host = Arc::new(Configuration::default());
    let no_host = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::FullAccess).unwrap();
    let mut registries = vec![no_host];
    for attach_before_cap in [false, true] {
        let parent = SharedApprovalMode::new(ApprovalMode::Smart);
        let mut tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::FullAccess).unwrap();
        if attach_before_cap {
            tools = tools.with_configuration_host(host.clone());
        }
        tools = tools
            .with_parent_approval_mode(parent.clone())
            .with_delegation_read_only()
            .with_configuration_host(host.clone());
        parent.set(ApprovalMode::FullAccess);
        tools.approval_mode_handle().set(ApprovalMode::FullAccess);
        assert!(tools.configuration_host.is_none());
        assert_eq!(tools.approval_mode(), ApprovalMode::ReadOnly);
        registries.push(tools);
    }
    for attach_before_parent in [false, true] {
        let parent = SharedApprovalMode::new(ApprovalMode::Smart);
        let mut tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::FullAccess).unwrap();
        if attach_before_parent {
            tools = tools.with_configuration_host(host.clone());
        }
        tools = tools
            .with_parent_approval_mode(parent.clone())
            .with_configuration_host(host.clone());
        parent.set(ApprovalMode::FullAccess);
        registries.push(tools);
    }
    for tools in registries {
        let tools =
            tools.with_allowed_tools(["configuration_read".into(), "configuration_write".into()]);
        let offered = tools.definitions();
        for call in [
            apply_configuration(),
            configuration_call("configuration_read", "schema", json!({})),
        ] {
            assert!(PARENT_ONLY_TOOLS.contains(&call.name.as_str()));
            assert!(!offered.iter().any(|tool| tool.name == call.name));
            assert!(tools.execute(&call).await.is_err());
            assert!(tools.host_tool(&call).await.is_err());
        }
    }
    assert!(host.requests.lock().unwrap().is_empty());
    assert!(host.lookups.lock().unwrap().is_empty());
}
