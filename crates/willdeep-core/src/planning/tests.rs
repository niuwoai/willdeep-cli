use super::*;
use serde_json::json;

fn update(value: serde_json::Value) -> Update {
    serde_json::from_value(value).unwrap()
}

fn release_plan() -> Plan {
    apply(
        None,
        &update(json!({"steps": [
            {"id":"push", "title":"Push tag", "status":"done"},
            {"id":"ci", "title":"Release build", "depends_on":["push"]},
            {"id":"install", "title":"Local install", "depends_on":["push"]},
            {"id":"brew", "title":"Update formula", "depends_on":["ci"]},
            {"id":"verify", "title":"Verify release", "depends_on":["install","brew"]}
        ]})),
    )
    .unwrap()
}

#[test]
fn independent_steps_run_together_and_only_dependents_wait() {
    let plan = release_plan();
    assert_eq!(Receipt::new(plan.clone(), true).ready, ["ci", "install"]);
    let running = apply(
        Some(&plan),
        &update(json!({"merge":true,"steps":[
            {"id":"ci","status":"in_progress"}, {"id":"install","status":"in_progress"}
        ]})),
    )
    .unwrap();
    let receipt = Receipt::new(running.clone(), false);
    assert_eq!(receipt.running, ["ci", "install"]);
    assert_eq!(receipt.waiting["brew"], ["ci"]);
    let finished = apply(
        Some(&running),
        &update(json!({"merge":true,"steps":[{"id":"ci","status":"done"}]})),
    )
    .unwrap();
    let receipt = Receipt::new(finished, false);
    assert_eq!(receipt.ready, ["brew"]);
    assert_eq!(receipt.running, ["install"]);
    assert_eq!(receipt.plan.steps[1].depends_on, ["push"]);
}

#[test]
fn invalid_graphs_and_premature_execution_are_rejected_atomically() {
    let plan = release_plan();
    for changes in [
        json!([{"id":"brew","status":"in_progress"}]),
        json!([{"id":"brew","status":"done"}]),
        json!([{"id":"ci","depends_on":["missing"]}]),
        json!([{"id":"ci","depends_on":["ci"]}]),
        json!([{"id":"ci","depends_on":["push","push"]}]),
        json!([{"id":"ci","depends_on":["brew"]}]),
        json!([{"id":"ci","status":"done"},{"id":"ci","status":"failed"}]),
        json!([{"status":"in_progress"}]),
        json!([{"id":"new"}]),
    ] {
        assert!(apply(Some(&plan), &update(json!({"merge":true,"steps":changes}))).is_err());
        assert_eq!(plan, release_plan());
    }
}

#[test]
fn failed_blocked_and_skipped_prerequisites_never_release_dependents() {
    for status in ["failed", "blocked", "skipped"] {
        let plan = apply(
            Some(&release_plan()),
            &update(json!({"merge":true,"steps":[{"id":"ci","status":status}]})),
        )
        .unwrap();
        let receipt = Receipt::new(plan.clone(), false);
        assert_eq!(receipt.ready, ["install"]);
        assert_eq!(receipt.waiting["brew"], ["ci"]);
        assert!(
            apply(
                Some(&plan),
                &update(json!({"merge":true,"steps":[{"id":"brew","status":"in_progress"}]}))
            )
            .is_err()
        );
    }
}

#[test]
fn legacy_steps_and_persisted_plans_remain_readable() {
    let plan = apply(
        None,
        &update(json!({"steps":[{"title":"Legacy step", "status":"in_progress"}]})),
    )
    .unwrap();
    assert_eq!(plan.steps[0].id, "1");
    let old: Plan =
        serde_json::from_value(json!({"steps":[{"id":"a","text":"Old", "status":"pending"}]}))
            .unwrap();
    assert!(old.steps[0].depends_on.is_empty());
    let restored: Plan =
        serde_json::from_str(&serde_json::to_string(&release_plan()).unwrap()).unwrap();
    assert_eq!(restored, release_plan());
}

#[test]
fn full_snapshots_update_the_same_card_until_a_different_plan_starts() {
    let current = release_plan();
    let replace =
        update(json!({"steps":[{"id":"ci","title":"Release build","status":"in_progress"}]}));
    let next = apply(Some(&current), &replace).unwrap();
    assert!(!starts_new_plan(Some(&current), &next, &replace));
    let replace = update(json!({"steps":[{"id":"other","title":"Another task"}]}));
    let next = apply(Some(&current), &replace).unwrap();
    assert!(starts_new_plan(Some(&current), &next, &replace));
}

#[test]
fn history_uses_successful_receipts_and_ignores_failed_tool_outputs() {
    use crate::types::{Message, ToolCall};
    let call = ToolCall {
        id: "plan-1".into(),
        name: "update_plan".into(),
        arguments: "{}".into(),
    };
    let initial = Receipt::new(release_plan(), true);
    let mut running = initial.plan.clone();
    running.steps[1].status = StepStatus::InProgress;
    running.steps[2].status = StepStatus::InProgress;
    let next = Receipt::new(running.clone(), false);
    let messages = [
        Message::assistant("", vec![call.clone()]),
        Message::tool(&call, serde_json::to_string(&initial).unwrap()),
        Message::tool(&call, serde_json::to_string(&next).unwrap()),
        Message::tool(&call, "tool error: dependencies not done"),
    ];
    let plans: Vec<_> = crate::conversation::project(&messages, None)
        .into_iter()
        .filter_map(|item| item.plan)
        .collect();
    assert_eq!(plans, [running]);
}
