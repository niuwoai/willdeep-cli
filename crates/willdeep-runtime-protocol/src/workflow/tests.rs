use super::*;
use serde_json::json;

#[test]
fn workflow_wire_rejects_unknown_node_and_expression_fields() {
    let node = json!({"id":"a","kind":"agent","profile":"generalist","prompt":"Inspect"});
    let parsed: WorkflowNode = serde_json::from_value(node.clone()).unwrap();
    assert_eq!(parsed.id, "a");
    let mut unknown = node;
    unknown["code"] = json!("execute()");
    assert!(serde_json::from_value::<WorkflowNode>(unknown).is_err());
    let condition = json!({"op":"eq","left":1,"right":1,"eval":"script"});
    assert!(serde_json::from_value::<WorkflowCondition>(condition).is_err());
    assert!(
        serde_json::from_value::<WorkflowCondition>(json!({"op":"eval","source":"true"})).is_err()
    );
}

#[test]
fn workflow_limits_have_backward_compatible_defaults_and_reject_typos() {
    assert_eq!(
        serde_json::from_value::<WorkflowLimits>(json!({})).unwrap(),
        WorkflowLimits::default()
    );
    let lowered: WorkflowLimits = serde_json::from_value(json!({"max_concurrent":2})).unwrap();
    assert_eq!(lowered.max_concurrent, 2);
    assert_eq!(lowered.max_attempts, WORKFLOW_MAX_ATTEMPTS);
    assert!(serde_json::from_value::<WorkflowLimits>(json!({"max_attmpts":2})).is_err());
}

#[test]
fn stopping_status_is_never_a_terminal_receipt() {
    assert!(!WorkflowRunStatus::Stopping.is_terminal());
    assert!(!WorkflowNodeStatus::Stopping.is_terminal());
    assert!(!WorkflowNodeStatus::DispatchPending.is_terminal());
    assert!(WorkflowRunStatus::Interrupted.is_terminal());
    assert!(WorkflowNodeStatus::Blocked.is_terminal());
}
