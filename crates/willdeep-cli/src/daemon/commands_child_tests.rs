use super::*;
use clap::Parser;
use serde_json::json;
use willdeep_runtime_protocol::{Capability, RuntimeCapabilities};

#[derive(Parser)]
struct ChildCli {
    #[command(subcommand)]
    action: ChildSessionAction,
}

fn parse(args: Vec<String>) -> Result<(&'static str, serde_json::Value, Option<uuid::Uuid>)> {
    let cli = ChildCli::try_parse_from(std::iter::once("test".to_owned()).chain(args))?;
    child_request(cli.action)
}

#[test]
fn cli_creation_serializes_subscription_and_repeated_dependency_ids() {
    let parent = uuid::Uuid::new_v4();
    let child = uuid::Uuid::new_v4();
    let first = uuid::Uuid::new_v4();
    let second = uuid::Uuid::new_v4();
    let request = uuid::Uuid::new_v4();
    for action in ["start", "continue"] {
        let mut args = vec![action.to_owned(), "--parent".into(), parent.to_string()];
        if action == "start" {
            args.extend(["--title".into(), "Audit".into()]);
        } else {
            args.push(child.to_string());
        }
        args.extend([
            "--subscription".into(),
            "auto-wake".into(),
            "--depends-on".into(),
            first.to_string(),
            "--depends-on".into(),
            second.to_string(),
            "--request-id".into(),
            request.to_string(),
            "Inspect the implementation".into(),
        ]);
        let (operation, params, id) = parse(args).unwrap();
        assert_eq!(operation, format!("session.child.{action}"));
        assert_eq!(params["subscription"], "auto_wake");
        assert_eq!(params["depends_on"], json!([first, second]));
        assert_eq!(id, Some(request));
    }
}

#[test]
fn cli_rejects_bad_or_duplicate_dependencies_and_requires_control_confirmation() {
    let parent = uuid::Uuid::new_v4();
    let child = uuid::Uuid::new_v4();
    let base = || {
        vec![
            "continue".to_owned(),
            "--parent".into(),
            parent.to_string(),
            child.to_string(),
        ]
    };
    for dependencies in [
        vec!["invalid".to_owned()],
        vec![child.to_string(), child.to_string()],
        (0..17).map(|_| uuid::Uuid::new_v4().to_string()).collect(),
    ] {
        let mut args = base();
        for dependency in dependencies {
            args.extend(["--depends-on".into(), dependency]);
        }
        args.push("Read it".into());
        assert!(parse(args).is_err());
    }
    for action in ["takeover", "detach"] {
        let args = vec![
            action.to_owned(),
            "--parent".into(),
            parent.to_string(),
            child.to_string(),
            "--expected-control-revision".into(),
            "2".into(),
        ];
        assert!(parse(args.clone()).is_err());
        let (_, params, _) = parse(args.into_iter().chain(["--yes".into()]).collect()).unwrap();
        assert_eq!(params["expected_control_revision"], 2);
        assert!(params.get("request_id").is_none());
    }
}

#[test]
fn cli_control_and_subscription_use_exact_revisions_and_envelope_request_id() {
    let parent = uuid::Uuid::new_v4();
    let child = uuid::Uuid::new_v4();
    let request = uuid::Uuid::new_v4();
    let (operation, params, id) = parse(vec![
        "detach".into(),
        "--parent".into(),
        parent.to_string(),
        child.to_string(),
        "--expected-control-revision".into(),
        "3".into(),
        "--request-id".into(),
        request.to_string(),
        "--yes".into(),
    ])
    .unwrap();
    assert_eq!(operation, "session.child.detach");
    assert_eq!(params["child_session_id"], json!(child));
    assert_eq!(id, Some(request));
    let (operation, params, id) = parse(vec![
        "subscription".into(),
        "set".into(),
        "--parent".into(),
        parent.to_string(),
        child.to_string(),
        "--expected-subscription-revision".into(),
        "4".into(),
        "--subscription".into(),
        "inbox".into(),
        "--request-id".into(),
        request.to_string(),
    ])
    .unwrap();
    assert_eq!(operation, "session.child.task.subscription.set");
    assert_eq!(params["subscription"], "inbox");
    assert_eq!(params["expected_subscription_revision"], 4);
    assert_eq!(id, Some(request));
}

#[test]
fn unsupported_hosts_receive_no_p2_fields_and_legacy_calls_stay_compatible() {
    let mut capabilities = RuntimeCapabilities::current("fixture");
    capabilities.capabilities.retain(|capability| {
        !matches!(
            capability,
            Capability::SessionChildrenControl
                | Capability::SessionChildrenSubscriptions
                | Capability::SessionChildrenDependencies
        )
    });
    let (_, legacy, _) = parse(vec![
        "continue".into(),
        "--parent".into(),
        uuid::Uuid::new_v4().to_string(),
        uuid::Uuid::new_v4().to_string(),
        "Read it".into(),
    ])
    .unwrap();
    assert!(legacy.get("subscription").is_none());
    assert!(legacy.get("depends_on").is_none());
    validate_child_capabilities(
        &capabilities,
        "session.child.continue",
        &legacy,
        crate::i18n::Language::En,
    )
    .unwrap();
    for (operation, params) in [
        ("session.child.continue", json!({"subscription":"inbox"})),
        ("session.child.start", json!({"depends_on":[]})),
        ("session.child.link.get", json!({})),
        ("session.child.takeover", json!({})),
        ("session.child.detach", json!({})),
        (
            "session.child.task.subscription.set",
            json!({"subscription":"none"}),
        ),
    ] {
        assert!(
            validate_child_capabilities(
                &capabilities,
                operation,
                &params,
                crate::i18n::Language::En
            )
            .is_err()
        );
        validate_child_capabilities(
            &RuntimeCapabilities::current("fixture"),
            operation,
            &params,
            crate::i18n::Language::En,
        )
        .unwrap();
    }
}
