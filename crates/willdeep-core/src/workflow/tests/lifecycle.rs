use super::*;

#[test]
fn stop_requires_reconciliation_of_unbound_intents_and_actual_agent_terminals() {
    let f = Fixture::new();
    let definition = spec(
        json!([
            leaf("a", &[], Value::Null),
            leaf("b", &[], Value::Null),
            leaf("c", &["a"], Value::Null)
        ]),
        Value::Null,
    );
    let run = f.start(&definition, Value::Null);
    let initial = f.store.poll(run.id).unwrap();
    let a = initial.ready.iter().find(|t| t.node_path == "a").unwrap();
    let b = initial.ready.iter().find(|t| t.node_path == "b").unwrap();
    let mut accepted = agent(a, AgentStatus::Running);
    f.store
        .record_dispatch(run.id, a.request_id, accepted.clone())
        .unwrap();
    assert_eq!(
        f.store.stop(run.id).unwrap().status,
        WorkflowRunStatus::Stopping
    );
    let stopping = f.store.poll(run.id).unwrap();
    assert_eq!(stopping.cancel_agent_run_ids, vec![accepted.id]);
    assert!(stopping.ready.is_empty());
    assert_eq!(f.store.pending(run.id).unwrap(), vec![b.clone()]);
    assert_eq!(status(&stopping.run, "c"), WorkflowNodeStatus::Cancelled);
    assert!(
        f.store
            .reject_task(run.id, a.request_id, "not dispatched")
            .is_err()
    );
    f.store
        .reject_task(run.id, b.request_id, "ledger confirms not dispatched")
        .unwrap();
    assert_eq!(
        f.store.poll(run.id).unwrap().run.status,
        WorkflowRunStatus::Stopping
    );
    accepted.status = AgentStatus::Cancelled;
    accepted.finished_at_ms = Some(9);
    f.store.fail_task(run.id, a.request_id, accepted).unwrap();
    assert_eq!(
        f.store.poll(run.id).unwrap().run.status,
        WorkflowRunStatus::Cancelled
    );
    assert!(f.store.pending(run.id).unwrap().is_empty());
}

#[test]
fn stopping_nested_scopes_eventually_settles_control_instances() {
    let f = Fixture::new();
    let definition = spec(
        json!([{"id":"each","kind":"foreach","items":[1,2],"body":{"nodes":[leaf("a", &[], Value::Null)]}}]),
        Value::Null,
    );
    let run = f.start(&definition, Value::Null);
    let initial = f.store.poll(run.id).unwrap();
    f.store.stop(run.id).unwrap();
    for task in initial.ready {
        f.store
            .reject_task(run.id, task.request_id, "not accepted")
            .unwrap();
    }
    assert_eq!(
        f.store.poll(run.id).unwrap().run.status,
        WorkflowRunStatus::Cancelled
    );
}

#[test]
fn retry_has_new_identity_and_reuses_successes_without_new_model_usage() {
    let f = Fixture::new();
    let definition = spec(
        json!([
            leaf("a", &[], json!(1)),
            leaf("b", &["a"], json!({"$ref":"node:a#"}))
        ]),
        json!({"$ref":"node:b#"}),
    );
    let old = f.start(&definition, Value::Null);
    let first = f.store.poll(old.id).unwrap().ready.remove(0);
    complete(&f.store, &first, json!(10));
    let second = f.store.poll(old.id).unwrap().ready.remove(0);
    f.store
        .fail_task(
            old.id,
            second.request_id,
            agent(&second, AgentStatus::Failed),
        )
        .unwrap();
    let failed = f.store.poll(old.id).unwrap().run;
    assert_eq!(failed.status, WorkflowRunStatus::Failed);
    let request = Uuid::new_v4();
    let retry = f.store.retry_with_request(old.id, request).unwrap();
    assert_eq!(retry.id, request);
    assert_eq!(retry.source_run_id, Some(old.id));
    assert_eq!(retry.spec, old.spec);
    assert_eq!(retry.input, old.input);
    assert_eq!(retry.limits, old.limits);
    assert_eq!(f.store.retry_with_request(old.id, request).unwrap(), retry);
    assert!(f.store.resume_with_request(old.id, request).is_err());
    assert!(
        f.store
            .start_with_request(&definition.id, Value::Null, old.limits, request)
            .is_err()
    );
    let pending = f.store.poll(retry.id).unwrap();
    assert_eq!(pending.ready.len(), 1);
    assert_eq!(pending.ready[0].node_path, "b");
    assert_eq!(pending.ready[0].input, json!(10));
    assert_ne!(pending.ready[0].request_id, second.request_id);
    let reused = pending.run.nodes.iter().find(|n| n.path == "a").unwrap();
    assert_eq!(reused.reused_from.as_ref().unwrap().run_id, old.id);
    assert_eq!(
        reused.reused_from.as_ref().unwrap().node_instance_id,
        first.node_instance_id
    );
    complete(&f.store, &pending.ready[0], json!(11));
    assert_eq!(
        f.store.poll(retry.id).unwrap().run.status,
        WorkflowRunStatus::Completed
    );
    let report = f.store.aggregate_report(retry.id).unwrap().unwrap();
    assert_eq!(report.usage.model_calls, 1);
    assert_eq!(report.usage.total_tokens, Some(7));
    assert_eq!(f.store.get(old.id).unwrap(), Some(failed));
}

#[test]
fn resume_does_not_replay_uncertain_work_until_explicitly_requested() {
    let f = Fixture::new();
    let old = f.start(
        &spec(json!([leaf("a", &[], Value::Null)]), Value::Null),
        Value::Null,
    );
    assert!(f.store.resume(old.id).is_err());
    let task = f.store.poll(old.id).unwrap().ready.remove(0);
    let mut interrupted = agent(&task, AgentStatus::Interrupted);
    interrupted.usage.uncertain_execution = true;
    f.store
        .fail_task(old.id, task.request_id, interrupted)
        .unwrap();
    assert_eq!(
        f.store.poll(old.id).unwrap().run.status,
        WorkflowRunStatus::Interrupted
    );
    assert!(f.store.poll(old.id).unwrap().ready.is_empty());
    let new = f.store.resume_with_request(old.id, Uuid::new_v4()).unwrap();
    let next = f.store.poll(new.id).unwrap().ready.remove(0);
    assert_ne!(next.request_id, task.request_id);
    assert_eq!(next.input, task.input);
    assert_eq!(
        f.store.get(old.id).unwrap().unwrap().status,
        WorkflowRunStatus::Interrupted
    );
}

#[test]
fn terminal_receipts_replay_exact_payload_and_reject_changed_results() {
    let f = Fixture::new();
    let run = f.start(
        &spec(
            json!([leaf("a", &[], Value::Null)]),
            json!({"$ref":"node:a#"}),
        ),
        Value::Null,
    );
    let task = f.store.poll(run.id).unwrap().ready.remove(0);
    let result = WorkflowTaskResult {
        agent_run: agent(&task, AgentStatus::Completed),
        output: json!(12),
    };
    f.store
        .complete_task(run.id, task.request_id, result.clone())
        .unwrap();
    let terminal = f.store.poll(run.id).unwrap().run;
    assert_eq!(
        f.store
            .complete_task(run.id, task.request_id, result.clone())
            .unwrap(),
        terminal
    );
    let mut different = result;
    different.output = json!(13);
    assert!(
        f.store
            .complete_task(run.id, task.request_id, different)
            .is_err()
    );
    assert_eq!(f.store.get(run.id).unwrap(), Some(terminal));
}

#[test]
fn result_and_report_are_separate_and_ack_is_persistent() {
    let f = Fixture::new();
    let run = f.start(&example_spec(), Value::Null);
    assert!(f.store.aggregate_report(run.id).unwrap().is_none());
    assert!(f.store.acknowledge_report(run.id).is_err());
    let task = f.store.poll(run.id).unwrap().ready.remove(0);
    complete(&f.store, &task, json!({"ok":true}));
    f.store.poll(run.id).unwrap();
    let report = f.store.aggregate_report(run.id).unwrap().unwrap();
    assert_eq!(report.output, Some(json!({"ok":true})));
    assert!(report.report.contains("Human report"));
    assert!(!report.acknowledged);
    f.store.acknowledge_report(run.id).unwrap();
    let reopened = WorkflowStore::new(&f.home, run.parent_session_id);
    assert!(
        reopened
            .aggregate_report(run.id)
            .unwrap()
            .unwrap()
            .acknowledged
    );
    assert!(reopened.acknowledge_report(run.id).unwrap().acknowledged);
}

#[test]
fn dispatch_rejects_cross_parent_request_and_owner_mixups() {
    let f = Fixture::new();
    let run = f.start(&example_spec(), Value::Null);
    let task = f.store.poll(run.id).unwrap().ready.remove(0);
    let original = agent(&task, AgentStatus::Running);
    let mut wrong = original.clone();
    wrong.parent_session_id = Uuid::new_v4();
    assert!(
        f.store
            .record_dispatch(run.id, task.request_id, wrong)
            .is_err()
    );
    let mut wrong = original.clone();
    wrong.request_id = Uuid::new_v4();
    assert!(
        f.store
            .record_dispatch(run.id, task.request_id, wrong)
            .is_err()
    );
    let mut wrong = original.clone();
    wrong.workflow_owner.as_mut().unwrap().node_instance_id = "wrong".into();
    assert!(
        f.store
            .record_dispatch(run.id, task.request_id, wrong)
            .is_err()
    );
    f.store
        .record_dispatch(run.id, task.request_id, original.clone())
        .unwrap();
    assert_eq!(
        f.store
            .record_dispatch(run.id, task.request_id, original)
            .unwrap()
            .nodes[0]
            .status,
        WorkflowNodeStatus::Running
    );
}

#[test]
fn concurrent_starts_and_polls_use_one_durable_identity() {
    let f = Fixture::new();
    f.store.save_definition(&example_spec()).unwrap();
    let request = Uuid::new_v4();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let store = f.store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let run = store
                    .start_with_request(
                        "review-v1",
                        Value::Null,
                        WorkflowLimits::default(),
                        request,
                    )
                    .unwrap();
                store.poll(run.id).unwrap().ready
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(results.iter().all(|r| r == &results[0]));
    assert_eq!(results[0].len(), 1);
    assert_eq!(f.store.list_runs().unwrap().len(), 1);
}

#[test]
fn foreach_retry_retains_frozen_items_and_each_successful_stage() {
    let f = Fixture::new();
    let definition = spec(
        json!([{"id":"each","kind":"foreach","items":[1,2],"body":{"nodes":[leaf("stage", &[], json!({"$ref":"item#"}))],"output":{"$ref":"node:stage#"}}}]),
        json!({"$ref":"node:each#"}),
    );
    let run = f.start(&definition, Value::Null);
    let initial = f.store.poll(run.id).unwrap();
    complete(&f.store, &initial.ready[0], json!(10));
    f.store
        .fail_task(
            run.id,
            initial.ready[1].request_id,
            agent(&initial.ready[1], AgentStatus::Failed),
        )
        .unwrap();
    f.store.poll(run.id).unwrap();
    let retry = f.store.retry(run.id).unwrap();
    let next = f.store.poll(retry.id).unwrap();
    assert_eq!(next.ready.len(), 1);
    assert_eq!(next.ready[0].node_path, "each/1/stage");
    assert_eq!(next.ready[0].input, json!(2));
    complete(&f.store, &next.ready[0], json!(20));
    assert_eq!(
        f.store.poll(retry.id).unwrap().run.output,
        Some(json!([10, 20]))
    );
}

#[test]
fn ledger_reconciliation_is_idempotent_before_and_after_terminal_result() {
    let f = Fixture::new();
    let run = f.start(&example_spec(), Value::Null);
    let task = f.store.poll(run.id).unwrap().ready.remove(0);
    let running = agent(&task, AgentStatus::Running);
    f.store
        .record_dispatch(run.id, task.request_id, running.clone())
        .unwrap();
    let mut queued = running.clone();
    queued.status = AgentStatus::Queued;
    let snapshot = f
        .store
        .record_dispatch(run.id, task.request_id, queued)
        .unwrap();
    assert_eq!(snapshot.nodes[0].status, WorkflowNodeStatus::Running);
    let mut completed = running;
    completed.status = AgentStatus::Completed;
    completed.finished_at_ms = Some(8);
    f.store
        .record_dispatch(run.id, task.request_id, completed.clone())
        .unwrap();
    f.store
        .record_dispatch(run.id, task.request_id, completed.clone())
        .unwrap();
    assert!(f.store.poll(run.id).unwrap().ready.is_empty());
    let result = WorkflowTaskResult {
        agent_run: completed.clone(),
        output: json!({"ok":true}),
    };
    f.store
        .complete_task(run.id, task.request_id, result)
        .unwrap();
    f.store.poll(run.id).unwrap();
    completed.delivered = true;
    f.store
        .complete_task(
            run.id,
            task.request_id,
            WorkflowTaskResult {
                agent_run: completed,
                output: json!({"ok":true}),
            },
        )
        .unwrap();
    assert_eq!(
        f.store.get(run.id).unwrap().unwrap().status,
        WorkflowRunStatus::Completed
    );
}

#[test]
fn all_unknown_usage_stays_unknown_in_aggregate() {
    let f = Fixture::new();
    let run = f.start(&example_spec(), Value::Null);
    let task = f.store.poll(run.id).unwrap().ready.remove(0);
    let mut agent = agent(&task, AgentStatus::Completed);
    agent.usage = AgentUsageSummary {
        model_calls: 1,
        unknown_usage_calls: 1,
        ..Default::default()
    };
    f.store
        .complete_task(
            run.id,
            task.request_id,
            WorkflowTaskResult {
                agent_run: agent,
                output: json!({"ok":true}),
            },
        )
        .unwrap();
    f.store.poll(run.id).unwrap();
    let report = f.store.aggregate_report(run.id).unwrap().unwrap();
    assert_eq!(report.usage.total_tokens, None);
    assert_eq!(report.usage.input_tokens, None);
    assert_eq!(report.usage.unknown_usage_calls, 1);
}

#[cfg(unix)]
#[test]
fn storage_rejects_symlinked_parent_directories_and_files() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.store.save_definition(&example_spec()).unwrap();
    let other = WorkflowStore::new(&f.home, Uuid::new_v4());
    symlink(f.store.root(), other.root()).unwrap();
    assert!(other.list_definitions().is_err());
    let definition = f.store.root().join("definitions/review-v1.json");
    let link = f.store.root().join("definitions/linked.json");
    symlink(definition, link).unwrap();
    assert!(f.store.get_definition("linked").is_err());
    let permissions = std::fs::metadata(f.store.root()).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(permissions.mode() & 0o777, 0o700);
}
