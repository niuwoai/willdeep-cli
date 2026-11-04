use super::*;
use willdeep_runtime_protocol::{ApiResponse, Capability, RuntimeCapabilities};

struct Fixture {
    root: PathBuf,
    state: ServerState,
    session_id: uuid::Uuid,
    scheduled: tokio::sync::mpsc::UnboundedReceiver<uuid::Uuid>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("workflow-api-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let sessions = crate::daemon::tests::test_runtime_session_store(&root);
        let session = sessions
            .create(session_store::CreateRuntimeSession {
                id: None,
                workspace: workspace.clone(),
                profile: None,
                model: None,
                config: None,
                title: None,
            })
            .unwrap();
        let (sender, scheduled) = tokio::sync::mpsc::unbounded_channel();
        let state = crate::daemon::tests::test_server_state_with_scheduler(&root, sessions, sender);
        state.workspaces.ensure_registered(&workspace).unwrap();
        Self {
            root,
            state,
            session_id: session.id,
            scheduled,
        }
    }
    fn store(&self) -> WorkflowStore {
        WorkflowStore::new(&self.root, self.session_id)
    }
    fn request(&self, operation: &str, mut params: Value) -> ApiRequest {
        let identity = params
            .get("request_id")
            .and_then(Value::as_str)
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(uuid::Uuid::new_v4);
        params["parent_session_id"] = json!(self.session_id);
        let mut request = ApiRequest::new(format!("workflow.{operation}"), params);
        request.request_id = identity;
        request
    }
    fn save_example(&self) {
        self.store()
            .save_definition(&willdeep_core::workflow::example_spec())
            .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn data(response: super::super::control_api::UnifiedResponse) -> Value {
    match response.body {
        ApiResponse::Ok { data, .. } => data,
        ApiResponse::Error { error, .. } => panic!("unexpected API error: {error:?}"),
    }
}

#[tokio::test]
async fn workflow_api_admits_and_queues_without_model_work_and_replays_exact_ids() {
    let mut fixture = Fixture::new();
    fixture.save_example();
    let identity = uuid::Uuid::new_v4();
    let request = fixture.request(
        "run",
        json!({"definition_id":"review-v1","input":{"task":"fixture"},"request_id":identity}),
    );
    let first = tokio::time::timeout(
        Duration::from_secs(1),
        control_api::execute(&fixture.state, request.clone()),
    )
    .await
    .unwrap();
    assert_eq!(data(first)["workflow"]["id"], json!(identity));
    assert_eq!(fixture.scheduled.try_recv().unwrap(), fixture.session_id);
    let turns = fixture
        .state
        .sessions
        .list_turns(fixture.session_id)
        .unwrap();
    assert_eq!(turns.len(), 1);
    let claimed = fixture
        .state
        .sessions
        .claim_next(fixture.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.request.team_control,
        Some(AgentTeamControl::RecoverQueue)
    );
    assert!(claimed.request.prompt.is_empty());
    assert!(
        fixture
            .store()
            .get_run(identity)
            .unwrap()
            .unwrap()
            .nodes
            .is_empty()
    );
    let replay = data(control_api::execute(&fixture.state, request.clone()).await);
    assert_eq!(replay["workflow"]["id"], json!(identity));
    assert_eq!(
        fixture
            .state
            .sessions
            .list_turns(fixture.session_id)
            .unwrap()
            .len(),
        1
    );
    let mut conflict = request;
    conflict.params["input"] = json!({"task":"changed"});
    assert_eq!(
        control_api::execute(&fixture.state, conflict).await.status,
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn workflow_queries_have_stable_envelopes_and_do_not_enqueue() {
    let fixture = Fixture::new();
    let spec = willdeep_core::workflow::example_spec();
    let validated = data(
        control_api::execute(
            &fixture.state,
            fixture.request("validate", json!({"spec":spec})),
        )
        .await,
    );
    assert_eq!(validated["valid"], true);
    assert!(fixture.store().list_definitions().unwrap().is_empty());
    data(
        control_api::execute(
            &fixture.state,
            fixture.request("save", json!({"spec":spec})),
        )
        .await,
    );
    let list = data(control_api::execute(&fixture.state, fixture.request("list", json!({}))).await);
    assert_eq!(list["definitions"].as_array().unwrap().len(), 1);
    assert_eq!(list["runs"], json!([]));
    let schema =
        data(control_api::execute(&fixture.state, fixture.request("schema", json!({}))).await);
    assert!(schema["schema"].is_object());
    assert!(schema["example"].is_object());
    let run = fixture
        .store()
        .start("review-v1", Value::Null, Default::default())
        .unwrap();
    let result = data(
        control_api::execute(
            &fixture.state,
            fixture.request("get", json!({"run_id":run.id})),
        )
        .await,
    );
    assert_eq!(result["workflow"]["id"], json!(run.id));
    let history =
        data(control_api::execute(&fixture.state, fixture.request("history", json!({}))).await);
    assert_eq!(history["runs"].as_array().unwrap().len(), 1);
    assert!(
        fixture
            .state
            .sessions
            .list_turns(fixture.session_id)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn workflow_drain_and_read_only_gates_cover_new_work_but_allow_stop() {
    let fixture = Fixture::new();
    fixture.save_example();
    let run = fixture
        .store()
        .start("review-v1", Value::Null, Default::default())
        .unwrap();
    *fixture.state.work_gate.write().await = true;
    for operation in ["run", "retry", "resume"] {
        let request = fixture.request(
            operation,
            json!({"definition_id":"review-v1","run_id":run.id,"request_id":uuid::Uuid::new_v4()}),
        );
        assert_eq!(
            control_api::execute(&fixture.state, request).await.status,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
    let stopped = data(
        control_api::execute(
            &fixture.state,
            fixture.request(
                "stop",
                json!({"run_id":run.id,"request_id":uuid::Uuid::new_v4()}),
            ),
        )
        .await,
    );
    assert_eq!(stopped["workflow"]["id"], json!(run.id));
    *fixture.state.work_gate.write().await = false;
    fixture
        .state
        .sessions
        .update_approval_mode(fixture.session_id, WorkspaceAccess::ReadOnly)
        .unwrap();
    let request = fixture.request(
        "run",
        json!({"definition_id":"review-v1","request_id":uuid::Uuid::new_v4()}),
    );
    assert_eq!(
        control_api::execute(&fixture.state, request).await.status,
        StatusCode::FORBIDDEN
    );
    let request = fixture.request(
        "save",
        json!({"spec":willdeep_core::workflow::example_spec()}),
    );
    assert_eq!(
        control_api::execute(&fixture.state, request).await.status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn workflow_recovery_covers_admission_before_wake_and_does_not_duplicate_turns() {
    let mut fixture = Fixture::new();
    fixture.save_example();
    fixture
        .store()
        .start("review-v1", Value::Null, Default::default())
        .unwrap();
    assert!(
        fixture
            .state
            .sessions
            .list_turns(fixture.session_id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(recover(&fixture.state).unwrap(), 1);
    assert_eq!(fixture.scheduled.try_recv().unwrap(), fixture.session_id);
    assert_eq!(recover(&fixture.state).unwrap(), 0);
    assert_eq!(
        fixture
            .state
            .sessions
            .list_turns(fixture.session_id)
            .unwrap()
            .len(),
        1
    );
    let claimed = fixture
        .state
        .sessions
        .claim_next(fixture.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        claimed.request.team_control,
        Some(AgentTeamControl::RecoverQueue)
    );
}

#[tokio::test]
async fn workflow_retry_and_resume_return_linked_new_runs_with_stable_receipts() {
    let fixture = Fixture::new();
    fixture.save_example();
    let source = fixture
        .store()
        .start("review-v1", Value::Null, Default::default())
        .unwrap();
    fixture.store().stop(source.id).unwrap();
    let original = fixture.store().get_run(source.id).unwrap().unwrap();
    assert!(original.status.is_terminal());
    for operation in ["retry", "resume"] {
        let identity = uuid::Uuid::new_v4();
        let request = fixture.request(operation, json!({"run_id":source.id,"request_id":identity}));
        let result = data(control_api::execute(&fixture.state, request.clone()).await);
        assert_eq!(result["workflow"]["id"], json!(identity));
        assert_eq!(result["workflow"]["source_run_id"], json!(source.id));
        let replay = data(control_api::execute(&fixture.state, request).await);
        assert_eq!(replay["workflow"]["id"], json!(identity));
    }
    assert_eq!(
        fixture.store().get_run(source.id).unwrap().unwrap(),
        original
    );
    assert_eq!(fixture.store().list_runs().unwrap().len(), 3);
}

#[test]
fn workflow_capabilities_and_operations_are_registered_once() {
    let capabilities = RuntimeCapabilities::current("fixture");
    assert!(
        capabilities
            .capabilities
            .contains(&Capability::WorkflowObserve)
    );
    assert!(
        capabilities
            .capabilities
            .contains(&Capability::WorkflowControl)
    );
    for operation in [
        "schema", "list", "get", "history", "validate", "save", "run", "stop", "retry", "resume",
    ] {
        let name = format!("workflow.{operation}");
        assert_eq!(
            capabilities
                .operations
                .iter()
                .filter(|operation| **operation == name)
                .count(),
            1
        );
        assert_eq!(
            control_api::is_mutating_operation(&name),
            matches!(operation, "save" | "run" | "stop" | "retry" | "resume")
        );
    }
}
