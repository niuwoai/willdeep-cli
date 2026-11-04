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
