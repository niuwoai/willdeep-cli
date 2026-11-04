use super::*;

#[test]
fn dsl_example_round_trips_and_unknown_fields_are_rejected() {
    let example = example_spec();
    validate_spec(&example, &WorkflowLimits::default()).unwrap();
    let encoded = serde_json::to_value(&example).unwrap();
    assert_eq!(
        serde_json::from_value::<WorkflowSpecV1>(encoded.clone()).unwrap(),
        example
    );
    for key in ["unknown", "depends_onn", "task_packet_typo"] {
        let mut bad = encoded.clone();
        bad["nodes"][0][key] = json!(true);
        assert!(
            serde_json::from_value::<WorkflowSpecV1>(bad).is_err(),
            "accepted {key}"
        );
    }
    assert_eq!(spec_schema()["properties"]["version"]["const"], json!(1));
}

#[test]
fn cycles_missing_dependencies_and_cross_branch_references_fail_early() {
    let limits = WorkflowLimits::default();
    for nodes in [
        json!([
            leaf("a", &["b"], Value::Null),
            leaf("b", &["a"], Value::Null)
        ]),
        json!([leaf("a", &["absent"], Value::Null)]),
        json!([leaf("a", &["a"], Value::Null)]),
        json!([
            leaf("a", &[], Value::Null),
            leaf("b", &[], json!({"$ref":"node:a#"}))
        ]),
        json!([leaf("a", &[], json!({"$ref":"item#"}))]),
        json!([leaf("a", &[], json!({"$ref":"previous#"}))]),
        json!([leaf("a", &[], json!({"$ref":"input#/bad~3escape"}))]),
        json!([leaf("a", &[], json!({"$ref":"input#/x","sibling":true}))]),
    ] {
        assert!(validate_spec(&spec(nodes, Value::Null), &limits).is_err());
    }
    let mut valid = spec(
        json!([
            leaf("a", &[], Value::Null),
            leaf("b", &["a"], Value::Null),
            leaf("c", &["b"], json!({"$ref":"node:a#"}))
        ]),
        Value::Null,
    );
    validate_spec(&valid, &limits).unwrap();
    valid.output = json!({"$ref":"node:missing#"});
    assert!(validate_spec(&valid, &limits).is_err());
    let cross = spec(
        json!([{
            "id":"choose","kind":"switch","condition":{"op":"eq","left":1,"right":1},
            "then_scope":{"nodes":[leaf("left", &[], Value::Null)]},
            "else_scope":{"nodes":[leaf("right", &[], json!({"$ref":"node:left#"}))]}
        }]),
        Value::Null,
    );
    assert!(validate_spec(&cross, &limits).is_err());
}

#[test]
fn finite_schema_rejects_unknown_keywords_remote_refs_and_invalid_values() {
    for bad in [
        json!({"$ref":"https://example.com/schema"}),
        json!({"$ref":"#/$defs/value"}),
        json!({"format":"email"}),
        json!({"pattern":".*"}),
        json!({"anyOf":[true,false]}),
        json!({"type":["string","null"]}),
        json!({"required":["a","a"]}),
        json!({"minItems":3,"maxItems":2}),
    ] {
        assert!(schema::validate_schema(&bad).is_err(), "accepted {bad}");
    }
    let schema = json!({"type":"object","properties":{"names":{"type":"array","items":{"type":"string","minLength":2},"minItems":1,"maxItems":2},"score":{"type":"integer","minimum":0,"maximum":10}},"required":["names","score"],"additionalProperties":false});
    schema::validate_output(&schema, &json!({"names":["张三"],"score":10})).unwrap();
    for invalid in [
        json!({"names":[],"score":1}),
        json!({"names":["a"],"score":1}),
        json!({"names":["aa"],"score":11}),
        json!({"names":["aa"],"score":1.5}),
        json!({"names":["aa"],"score":1,"extra":true}),
        json!({"names":["aa"]}),
    ] {
        assert!(schema::validate_output(&schema, &invalid).is_err());
    }
    schema::validate_output(&json!(true), &Value::Null).unwrap();
    assert!(schema::validate_output(&json!(false), &Value::Null).is_err());
    assert!(
        schema::validate_output(
            &json!({"type":"integer","maximum":9007199254740992u64}),
            &json!(9007199254740993u64)
        )
        .is_err()
    );
}

#[test]
fn scalar_conditions_preserve_large_integer_precision_and_reject_hidden_errors() {
    use expression::{Context, evaluate};
    let context = Context::default();
    let gt = WorkflowCondition::Gt {
        left: json!(9007199254740993u64),
        right: json!(9007199254740992.0),
    };
    assert!(evaluate(&gt, &context).unwrap());
    assert!(
        evaluate(
            &WorkflowCondition::Lt {
                left: json!(u64::MAX),
                right: json!(18446744073709551616.0)
            },
            &context
        )
        .unwrap()
    );
    assert!(
        evaluate(
            &WorkflowCondition::Lt {
                left: json!(-2),
                right: json!(-1.5)
            },
            &context
        )
        .unwrap()
    );
    assert!(
        evaluate(
            &WorkflowCondition::Gt {
                left: json!(-1),
                right: json!(-1.5)
            },
            &context
        )
        .unwrap()
    );
    assert!(
        evaluate(
            &WorkflowCondition::Eq {
                left: json!(1),
                right: json!("1")
            },
            &context
        )
        .is_err()
    );
    assert!(
        evaluate(
            &WorkflowCondition::Lt {
                left: Value::Null,
                right: Value::Null
            },
            &context
        )
        .is_err()
    );
    let hidden = WorkflowCondition::And {
        conditions: vec![
            WorkflowCondition::Eq {
                left: json!(1),
                right: json!(2),
            },
            WorkflowCondition::Eq {
                left: json!({"$ref":"input#/missing"}),
                right: json!(true),
            },
        ],
    };
    assert!(evaluate(&hidden, &context).is_err());
}

#[test]
fn missing_runtime_pointer_is_durable_and_names_are_never_truncated() {
    let f = Fixture::new();
    let definition = spec(
        json!([leaf("a", &[], json!({"$ref":"input#/missing"}))]),
        Value::Null,
    );
    let run = f.start(&definition, json!({}));
    let failed = f.store.poll(run.id).unwrap().run;
    assert_eq!(failed.status, WorkflowRunStatus::Failed);
    assert!(failed.error.unwrap().contains("pointer does not exist"));
    let mut bad = example_spec();
    bad.id = "x".repeat(129);
    assert!(f.store.save_definition(&bad).is_err());
    assert!(f.store.get_definition("../escape").is_err());
    let mut good = example_spec();
    good.id = "x".repeat(128);
    assert_eq!(f.store.save_definition(&good).unwrap().id.len(), 128);
}

#[test]
fn concurrency_and_attempt_ceilings_are_hard_and_never_drop_tasks() {
    let f = Fixture::new();
    let definition = spec(
        json!([
            leaf("a", &[], Value::Null),
            leaf("b", &[], Value::Null),
            leaf("c", &[], Value::Null)
        ]),
        Value::Null,
    );
    f.store.save_definition(&definition).unwrap();
    let limits = WorkflowLimits {
        max_concurrent: 1,
        max_attempts: 2,
        ..Default::default()
    };
    let run = f.store.start(&definition.id, Value::Null, limits).unwrap();
    for _ in 0..2 {
        let poll = f.store.poll(run.id).unwrap();
        assert_eq!(poll.ready.len(), 1);
        complete(&f.store, &poll.ready[0], Value::Null);
    }
    let failed = f.store.poll(run.id).unwrap();
    assert_eq!(failed.run.status, WorkflowRunStatus::Failed);
    assert_eq!(failed.run.nodes.len(), 3);
    assert_eq!(status(&failed.run, "c"), WorkflowNodeStatus::Failed);
    assert!(failed.run.error.unwrap().contains("attempt count"));
    assert!(failed.ready.is_empty());
}

#[test]
fn foreach_and_instance_limits_fail_without_truncating_or_exposing_uncommitted_tasks() {
    let f = Fixture::new();
    let definition = spec(
        json!([{"id":"each","kind":"foreach","items":[1,2,3],"body":{"nodes":[leaf("a", &[], Value::Null)]}}]),
        Value::Null,
    );
    f.store.save_definition(&definition).unwrap();
    let run = f
        .store
        .start(
            &definition.id,
            Value::Null,
            WorkflowLimits {
                max_foreach_items: 2,
                ..Default::default()
            },
        )
        .unwrap();
    let failed = f.store.poll(run.id).unwrap();
    assert_eq!(failed.run.status, WorkflowRunStatus::Failed);
    assert!(failed.ready.is_empty());
    let run = f
        .store
        .start(
            &definition.id,
            Value::Null,
            WorkflowLimits {
                max_instances: 2,
                ..Default::default()
            },
        )
        .unwrap();
    let poll = f.store.poll(run.id).unwrap();
    assert_eq!(poll.run.nodes.len(), 2);
    assert_eq!(status(&poll.run, "each"), WorkflowNodeStatus::Failed);
    // An earlier durable intent is still uncertain; never fabricate its terminal.
    assert_eq!(poll.ready.len(), 1);
    f.store
        .reject_task(run.id, poll.ready[0].request_id, "not accepted")
        .unwrap();
    assert_eq!(
        f.store.poll(run.id).unwrap().run.status,
        WorkflowRunStatus::Failed
    );
}

#[test]
fn lowered_output_and_report_limits_fail_and_receipts_keep_conflict_semantics() {
    let f = Fixture::new();
    let definition = spec(json!([leaf("a", &[], Value::Null)]), Value::Null);
    f.store.save_definition(&definition).unwrap();
    let run = f
        .store
        .start(
            &definition.id,
            Value::Null,
            WorkflowLimits {
                max_value_bytes: 8,
                max_report_bytes: 16,
                ..Default::default()
            },
        )
        .unwrap();
    let task = f.store.poll(run.id).unwrap().ready.remove(0);
    let result = WorkflowTaskResult {
        agent_run: agent(&task, AgentStatus::Completed),
        output: json!("this is too big"),
    };
    f.store
        .complete_task(run.id, task.request_id, result.clone())
        .unwrap();
    assert_eq!(
        f.store.poll(run.id).unwrap().run.status,
        WorkflowRunStatus::Failed
    );
    f.store
        .complete_task(run.id, task.request_id, result.clone())
        .unwrap();
    let mut different = result;
    different.output = json!(0);
    assert!(
        f.store
            .complete_task(run.id, task.request_id, different)
            .is_err()
    );
    let failed = f.store.get(run.id).unwrap().unwrap();
    assert!(failed.nodes[0].output.is_none());
    assert!(failed.nodes[0].report.is_none());
}

#[test]
fn failed_atomic_commit_cannot_return_or_persist_new_dispatch_intents() {
    let f = Fixture::new();
    let definition = spec(
        json!([leaf("a", &[], json!("x".repeat(2000)))]),
        Value::Null,
    );
    f.store.save_definition(&definition).unwrap();
    let baseline = f
        .store
        .start(&definition.id, Value::Null, WorkflowLimits::default())
        .unwrap();
    // Admission fits; adding a full resolved task plus node metadata does not.
    let bytes = serde_json::to_vec(&engine::fresh_record(baseline.clone()))
        .unwrap()
        .len();
    let run = f
        .store
        .start(
            &definition.id,
            Value::Null,
            WorkflowLimits {
                max_run_bytes: bytes + 100,
                ..Default::default()
            },
        )
        .unwrap();
    assert!(matches!(f.store.poll(run.id), Err(WorkflowError::Limit(_))));
    assert!(f.store.pending(run.id).unwrap().is_empty());
    assert!(f.store.get(run.id).unwrap().unwrap().nodes.is_empty());
}

#[test]
fn nested_scope_failure_settles_orphaned_controls_after_accepted_work() {
    let f = Fixture::new();
    let definition = spec(
        json!([{
            "id":"outer","kind":"foreach","items":[1,2],"body":{"nodes":[{
                "id":"choose","kind":"switch","condition":{"op":"eq","left":true,"right":true},
                "then_scope":{"nodes":[leaf("work", &[], Value::Null)]},"else_scope":{"nodes":[]}
            }]}
        }]),
        Value::Null,
    );
    f.store.save_definition(&definition).unwrap();
    let run = f
        .store
        .start(
            &definition.id,
            Value::Null,
            WorkflowLimits {
                max_instances: 3,
                ..Default::default()
            },
        )
        .unwrap();
    let poll = f.store.poll(run.id).unwrap();
    assert_eq!(poll.ready.len(), 1);
    assert_eq!(
        status(&poll.run, "outer/0/choose"),
        WorkflowNodeStatus::Blocked
    );
    complete(&f.store, &poll.ready[0], Value::Null);
    assert_eq!(
        f.store.poll(run.id).unwrap().run.status,
        WorkflowRunStatus::Failed
    );
}

#[test]
fn empty_control_scopes_and_root_output_failures_are_finite() {
    let f = Fixture::new();
    let definition = spec(
        json!([{"id":"each","kind":"foreach","items":[],"body":{"nodes":[]}},
        {"id":"repeat","kind":"repeat","initial":7,"body":{"nodes":[]},"until":{"op":"eq","left":true,"right":true},"carry":{"$ref":"previous#"},"max_iterations":1}]),
        json!({"each":{"$ref":"node:each#"},"repeat":{"$ref":"node:repeat#"}}),
    );
    let run = f.start(&definition, Value::Null);
    let done = f.store.poll(run.id).unwrap();
    assert!(done.ready.is_empty());
    assert_eq!(done.run.output, Some(json!({"each":[],"repeat":7})));
    let mut definition = spec(json!([]), json!({"$ref":"input#/missing"}));
    definition.id = "empty-v1".into();
    let run = f.start(&definition, Value::Null);
    let failed = f.store.poll(run.id).unwrap().run;
    assert_eq!(failed.status, WorkflowRunStatus::Failed);
    assert!(failed.error.unwrap().contains("pointer does not exist"));
}

#[test]
fn host_dispatch_receives_complete_leaf_contract_without_policy_changes() {
    let f = Fixture::new();
    let mut node = leaf("safe", &[], json!({"$ref":"input#"}));
    node["worker_tier"] = json!("expert");
    node["max_turns"] = json!(7);
    node["token_budget"] = json!(1000);
    node["timeout_seconds"] = json!(30);
    node["write_targets"] = json!(["src/a.rs"]);
    node["task_packet"] = json!({"original":"packet"});
    let ticket = json!({"reason":"cross-module design","attempted_profiles":["generalist"],"context_evidence":"inspection completed","why_not_decompose":"requires a joint decision"});
    node["escalation"] = ticket.clone();
    node["target_command"] = json!("cargo test");
    node["output_schema"] = json!({"type":"object"});
    let run = f.start(&spec(json!([node]), Value::Null), json!({"payload":true}));
    let task = f.store.poll(run.id).unwrap().ready.remove(0);
    assert_eq!(task.worker_tier, Some(WorkflowWorkerTier::Expert));
    assert_eq!(task.max_turns, Some(7));
    assert_eq!(task.token_budget, Some(1000));
    assert_eq!(task.timeout_seconds, Some(30));
    assert_eq!(task.write_targets, vec!["src/a.rs"]);
    assert_eq!(task.task_packet, Some(json!({"original":"packet"})));
    assert_eq!(task.escalation, Some(ticket));
    assert_eq!(task.target_command.as_deref(), Some("cargo test"));
    assert_eq!(task.input, json!({"payload":true}));
    assert_eq!(task.output_schema, Some(json!({"type":"object"})));
}
