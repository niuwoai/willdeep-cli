use super::*;

struct BoundClient;
#[async_trait]
impl SessionCollaboration for BoundClient {
    async fn execute(&self, _: &str, _: serde_json::Value) -> Result<String, ToolError> {
        panic!("stable tool call identity must reach the host");
    }
    async fn execute_call(&self, call: &ToolCall) -> Result<String, ToolError> {
        assert_eq!(call.id, "stable-call-id");
        Ok("accepted by host".into())
    }
}

fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "stable-call-id".into(),
        name: name.into(),
        arguments: arguments.to_string(),
    }
}

#[test]
fn child_tool_parameters_reject_spoofed_identity_and_unsupported_modes() {
    let valid =
        json!({"title":"Investigate", "prompt":"Read the implementation", "context":"fresh"});
    validate_call(&call(START, valid.clone())).unwrap();
    for (key, value) in [
        ("parent_session_id", json!(uuid::Uuid::new_v4())),
        ("created_from_turn_id", json!(uuid::Uuid::new_v4())),
        ("config", json!("other.toml")),
        ("path", json!("../other")),
        ("request_id", json!(uuid::Uuid::new_v4())),
        ("workspace_mode", json!("clone")),
        ("context", json!("resume")),
        ("worktree_base", json!("dirty")),
        ("model", json!("bad\nmodel")),
    ] {
        let mut invalid = valid.clone();
        invalid[key] = value;
        assert!(
            matches!(
                validate_call(&call(START, invalid)),
                Err(ToolError::InvalidArguments { .. })
            ),
            "accepted {key}"
        );
    }
    let through = uuid::Uuid::new_v4();
    for (options, accepted) in [
        (json!({"context":"fork","through_turn_id":through}), true),
        (json!({"context":"fresh","through_turn_id":through}), false),
        (
            json!({"workspace_mode":"worktree","worktree_base":"snapshot"}),
            true,
        ),
        (
            json!({"workspace_mode":"shared_read_only","worktree_base":"snapshot"}),
            false,
        ),
        (json!({"model":"worker-model"}), true),
    ] {
        let mut request = valid.clone();
        for (key, value) in options.as_object().unwrap() {
            request[key] = value.clone();
        }
        let result = validate_call(&call(START, request));
        assert_eq!(
            result.is_ok(),
            accepted,
            "unexpected result for {options}: {result:?}"
        );
    }
    let id = uuid::Uuid::new_v4();
    for (name, args) in [
        (
            CONTINUE,
            json!({"child_session_id":id,"prompt":"follow up"}),
        ),
        (
            RETRY,
            json!({"assignment_id":id,"expected_result_revision":1}),
        ),
        (GET, json!({"assignment_id":id})),
    ] {
        validate_call(&call(name, args.clone())).unwrap();
        let mut invalid = args;
        invalid["parent_session_id"] = json!(id);
        assert!(validate_call(&call(name, invalid)).is_err());
    }
    assert!(validate_call(&call(RETRY, json!({"assignment_id":id}))).is_err());
    assert!(validate_call(&call(GET, json!({"assignment_id":"not-a-uuid"}))).is_err());
}

#[test]
fn subscription_and_dependency_parameters_are_strict_and_only_available_on_creation() {
    let id = uuid::Uuid::new_v4();
    for (name, base) in [
        (START, json!({"title":"Read", "prompt":"Inspect it"})),
        (
            CONTINUE,
            json!({"child_session_id":id, "prompt":"Follow up"}),
        ),
    ] {
        for subscription in ["none", "inbox", "auto_wake"] {
            let mut args = base.clone();
            args["subscription"] = json!(subscription);
            args["depends_on"] = json!([id]);
            validate_call(&call(name, args)).unwrap();
        }
        for (key, value) in [
            ("subscription", json!("on_task_terminal")),
            ("subscription", json!(null)),
            ("depends_on", json!(["not-a-uuid"])),
            ("depends_on", json!([id, id])),
            ("depends_on", json!(null)),
            ("depends_on", json!([3])),
            (
                "depends_on",
                json!((0..17).map(|_| uuid::Uuid::new_v4()).collect::<Vec<_>>()),
            ),
            ("operation", json!("session.child.takeover")),
            ("expected_control_revision", json!(1)),
            ("control_state", json!("delegated")),
            ("subscription_revision", json!(1)),
            ("delegation_detached", json!(false)),
        ] {
            let mut args = base.clone();
            args[key] = value;
            assert!(
                validate_call(&call(name, args)).is_err(),
                "accepted {name}: {key}"
            );
        }
    }
    for name in [RETRY, GET] {
        let mut args = json!({"assignment_id":id,"subscription":"inbox"});
        if name == RETRY {
            args["expected_result_revision"] = json!(1);
        }
        assert!(validate_call(&call(name, args)).is_err());
    }
    let definitions = definitions();
    assert_eq!(definitions.len(), 4);
    assert!(
        definitions
            .iter()
            .all(|tool| [START, CONTINUE, RETRY, GET].contains(&tool.name.as_str()))
    );
    for name in [START, CONTINUE] {
        let schema = &definitions
            .iter()
            .find(|tool| tool.name == name)
            .unwrap()
            .parameters;
        assert_eq!(schema["properties"]["depends_on"]["maxItems"], 16);
        assert_eq!(schema["additionalProperties"], false);
    }
}

#[tokio::test]
async fn bound_child_tools_preserve_call_identity_and_enforce_read_only() {
    let root = std::env::temp_dir();
    let tools = ToolRegistry::new(&root, ApprovalMode::FullAccess)
        .unwrap()
        .with_session_collaboration(Arc::new(BoundClient));
    let definitions = tools.definitions();
    for name in [START, CONTINUE, RETRY, GET] {
        assert!(
            definitions.iter().any(|tool| tool.name == name),
            "missing {name}"
        );
    }
    let request = call(START, json!({"title":"Investigate", "prompt":"Read it"}));
    assert_eq!(tools.execute(&request).await.unwrap(), "accepted by host");
    let mut invalid = request.clone();
    invalid.arguments = json!({"title":"Investigate", "prompt":"Read it", "parent_session_id":uuid::Uuid::new_v4()}).to_string();
    assert!(matches!(
        tools.execute(&invalid).await,
        Err(ToolError::InvalidArguments { .. })
    ));
    tools.approval_mode_handle().set(ApprovalMode::ReadOnly);
    assert!(!tools.definitions().iter().any(|tool| tool.name == START));
    assert!(tools.definitions().iter().any(|tool| tool.name == GET));
    assert!(matches!(
        tools.execute(&request).await,
        Err(ToolError::ReadOnlyPolicy(_))
    ));
}

#[test]
fn unbound_and_worker_tool_surfaces_do_not_offer_child_controls() {
    let root = std::env::temp_dir();
    assert!(
        !ToolRegistry::new(&root, ApprovalMode::FullAccess)
            .unwrap()
            .definitions()
            .iter()
            .any(|tool| [START, CONTINUE, RETRY, GET].contains(&tool.name.as_str()))
    );
    for name in [START, CONTINUE, RETRY, GET] {
        assert!(agent_control::PARENT_ONLY_TOOLS.contains(&name));
    }
}
