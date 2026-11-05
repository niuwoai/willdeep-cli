use super::*;

fn request(command: &str) -> Request {
    parse(command, uuid::Uuid::nil(), Language::En)
        .unwrap()
        .unwrap()
}

#[test]
fn creation_options_use_protocol_fields_and_leave_prompts_intact() {
    let dependency = uuid::Uuid::new_v4();
    let child = uuid::Uuid::new_v4();
    let id = uuid::Uuid::new_v4();
    let started = request(&format!(
        "start --subscription inbox --depends-on {dependency} --request-id {id} Audit | Read the files | retain this separator"
    ));
    assert_eq!(started.operation, "session.child.start");
    assert_eq!(started.params["subscription"], "inbox");
    assert_eq!(started.params["depends_on"], json!([dependency]));
    assert_eq!(
        started.params["prompt"],
        "Read the files | retain this separator"
    );
    assert_eq!(started.request_id, Some(id));
    let continued = request(&format!(
        "continue {child} --subscription auto-wake --depends-on {dependency} Continue the audit"
    ));
    assert_eq!(continued.params["subscription"], "auto_wake");
    assert_eq!(continued.params["prompt"], "Continue the audit");
    let legacy = request("start Audit | Read it");
    assert!(legacy.params.get("subscription").is_none());
    assert!(legacy.params.get("depends_on").is_none());
}

#[test]
fn invalid_modes_revisions_dependencies_and_host_fields_are_rejected() {
    let child = uuid::Uuid::new_v4();
    let many = (0..17)
        .map(|_| format!("--depends-on {}", uuid::Uuid::new_v4()))
        .collect::<Vec<_>>()
        .join(" ");
    for command in [
        "start --subscription on_task_terminal Audit | Read".to_owned(),
        "start --depends-on nope Audit | Read".into(),
        format!("start --depends-on {child} --depends-on {child} Audit | Read"),
        format!("start {many} Audit | Read"),
        "start --snapshot Audit | Read".into(),
        "start --fork Audit | Read".into(),
        "start --parent-session-id injected Audit | Read".into(),
        "start --subscription inbox --subscription none Audit | Read".into(),
        format!("takeover {child} --yes"),
        format!("detach {child} --expected-control-revision 0 --yes"),
        format!("detach {child} --expected-control-revision 1 --yes unexpected"),
        format!("continue {child} --worktree Read"),
        format!("subscription set {child} --subscription inbox"),
        format!(
            "subscription set {child} --expected-subscription-revision 1 --subscription invalid"
        ),
    ] {
        assert!(
            parse(&command, uuid::Uuid::nil(), Language::En).is_err(),
            "accepted {command}"
        );
    }
}

#[test]
fn user_controls_require_confirmation_and_preserve_concurrency_keys() {
    let child = uuid::Uuid::new_v4();
    let id = uuid::Uuid::new_v4();
    for action in ["takeover", "detach"] {
        let command = format!("{action} {child} --expected-control-revision 3 --request-id {id}");
        assert!(request(&command).needs_confirmation);
        let confirmed = request(&format!("{command} --yes"));
        assert!(!confirmed.needs_confirmation);
        assert_eq!(confirmed.request_id, Some(id));
        assert_eq!(confirmed.params["expected_control_revision"], 3);
        assert!(confirmed.params.get("request_id").is_none());
        assert!(confirmed.params.get("yes").is_none());
    }
    let configured = request(&format!(
        "subscription set {child} --subscription inbox --expected-subscription-revision 4 --request-id {id}"
    ));
    assert_eq!(configured.operation, "session.child.task.subscription.set");
    assert_eq!(configured.params["expected_subscription_revision"], 4);
    assert_eq!(configured.request_id, Some(id));
    assert!(!configured.needs_confirmation);
    assert_eq!(
        request(&format!("link {child}")).operation,
        "session.child.link.get"
    );
}
