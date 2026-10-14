use super::*;

#[test]
fn terminal_handoff_note_preserves_cancelled_and_delivered_state() {
    let (store, root) = fixture();
    let (run, _) = store.create(request(Uuid::new_v4())).unwrap();
    assert!(store.append_terminal_note(run.id, "handoff").is_err());
    store
        .finish(run.id, AgentStatus::Cancelled, "cancelled")
        .unwrap();
    store.mark_delivered(run.id).unwrap();
    store
        .append_terminal_note(run.id, "retained patch; integration pending")
        .unwrap();
    let current = store.get(run.id).unwrap().unwrap();
    assert_eq!(current.status, AgentStatus::Cancelled);
    assert!(current.delivered);
    assert!(current.report.unwrap().contains("integration pending"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn retirement_excludes_pending_runs_and_new_admissions_and_can_roll_back_a_failed_merge() {
    let (store, root) = fixture();
    let agent = Uuid::new_v4();
    let (run, _) = store.create(request(agent)).unwrap();
    assert!(store.retire_agent(agent).is_err());
    store
        .finish(run.id, AgentStatus::Completed, "done")
        .unwrap();
    assert!(store.retire_agent(agent).unwrap());
    assert!(!store.retire_agent(agent).unwrap());
    assert!(
        store
            .create(request(agent))
            .unwrap_err()
            .contains("merged or archived")
    );
    let reopened = AgentRunStore::new(&root, store.parent);
    assert!(reopened.ensure_agent_available(agent).is_err());
    reopened.restore_retired_agent(agent).unwrap();
    reopened.create(request(agent)).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn durable_admission_is_atomic_across_concurrent_clients() {
    let (store, root) = fixture();
    store
        .configure_team(super::super::scheduler::AgentTeamSettings {
            max_concurrent: 2,
            max_queued: 1,
            token_budget: None,
        })
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
    let clients: Vec<_> = (0..16)
        .map(|_| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store.create(request(Uuid::new_v4())).is_ok()
            })
        })
        .collect();
    assert_eq!(
        clients
            .into_iter()
            .map(|client| client.join().unwrap())
            .filter(|accepted| *accepted)
            .count(),
        3
    );
    assert_eq!(store.list(None, None, 64).unwrap().runs.len(), 3);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn auxiliary_calls_stop_when_usage_is_unknown_or_team_budget_is_exhausted() {
    use crate::provider::{Provider, ProviderError};
    use crate::types::{Completion, Message, ToolDefinition};
    use crate::usage_ledger::{
        ClientKind, Execution, UsageLedgerContext, UsageLedgerScope, shared_sink,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Spy {
        calls: AtomicUsize,
        known: bool,
    }
    #[async_trait::async_trait]
    impl Provider for Spy {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<Completion, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Completion {
                content: "fixture".into(),
                reasoning: None,
                tool_calls: Vec::new(),
                finish_reason: Some("stop".into()),
                usage: self.known.then_some(Usage {
                    total_tokens: Some(15),
                    ..Default::default()
                }),
            })
        }
    }
    for known in [false, true] {
        let (store, root) = fixture();
        store
            .configure_team(super::super::scheduler::AgentTeamSettings {
                token_budget: Some(20),
                ..Default::default()
            })
            .unwrap();
        let (run, _) = store.create(request(Uuid::new_v4())).unwrap();
        store.start(run.id).unwrap();
        let mut context = UsageLedgerContext::new(ClientKind::Cli, Execution::InProcess);
        context.session_id = Some(store.parent);
        let scope = UsageLedgerScope::new(shared_sink(&root.join("usage")), context)
            .with_agent_run(store.clone(), run.id);
        let spy = Arc::new(Spy {
            calls: AtomicUsize::new(0),
            known,
        });
        let provider = scope.auxiliary(spy.clone());
        provider.complete(&[], &[]).await.unwrap();
        if known {
            provider.complete(&[], &[]).await.unwrap();
        }
        assert!(provider.complete(&[], &[]).await.is_err());
        assert_eq!(spy.calls.load(Ordering::SeqCst), if known { 2 } else { 1 });
        let summary = store.team_summary(&root.join("usage")).unwrap();
        assert_eq!(
            summary.auxiliary_usage.model_calls,
            spy.calls.load(Ordering::SeqCst) as u64
        );
        assert_eq!(summary.usage.unknown_usage_calls, u64::from(!known));
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn fixture() -> (AgentRunStore, PathBuf) {
    let root = std::env::temp_dir().join(format!("agent-runs-{}", Uuid::new_v4()));
    (AgentRunStore::new(&root, Uuid::new_v4()), root)
}

fn request(agent: Uuid) -> RunRequest {
    RunRequest {
        request_id: Uuid::new_v4(),
        agent_id: agent,
        reason: AgentRunReason::Spawn,
        label: "inspect".into(),
        model: Some("fixture".into()),
        max_turns: Some(4),
        token_budget: Some(1000),
        timeout_seconds: Some(30),
        dispatch: serde_json::json!({"task":"private fixture","approved_files":["a.rs"]}),
    }
}

#[test]
fn team_policy_and_cumulative_runs_survive_reopen_and_ledger_is_deduplicated() {
    use crate::usage_ledger::{Execution, UsageKind, UsageLedgerRecord, append_record};
    let (store, root) = fixture();
    let initial = super::super::scheduler::AgentTeamSettings {
        max_concurrent: 2,
        max_queued: 3,
        token_budget: Some(100),
    };
    store.configure_team(initial.clone()).unwrap();
    assert_eq!(store.configure_team(Default::default()).unwrap(), initial);
    let agent = Uuid::new_v4();
    let (run, _) = store.create(request(agent)).unwrap();
    store.start(run.id).unwrap();
    let usage = Usage {
        input_tokens: Some(10),
        output_tokens: Some(5),
        total_tokens: Some(15),
        cache_read_tokens: Some(4),
    };
    store.usage(run.id, &usage).unwrap();
    store
        .finish(run.id, AgentStatus::Completed, "done")
        .unwrap();
    let (next, _) = store.create(request(agent)).unwrap();
    let mut record = UsageLedgerRecord::new(UsageKind::Subagent, Execution::Daemon);
    record.session_id = Some(store.parent);
    record.run_id = Some(run.id);
    record.usage = usage;
    let ledger = root.join("usage");
    append_record(&ledger, &record).unwrap();
    append_record(&ledger, &record).unwrap();
    let mut parent = UsageLedgerRecord::new(UsageKind::Main, Execution::Daemon);
    parent.session_id = Some(store.parent);
    append_record(&ledger, &parent).unwrap();
    let mut foreign = record.clone();
    foreign.id = Uuid::new_v4();
    foreign.session_id = Some(Uuid::new_v4());
    append_record(&ledger, &foreign).unwrap();
    let reopened = AgentRunStore::new(&root, store.parent);
    let summary = reopened.team_summary(&ledger).unwrap();
    assert_eq!(summary.policy.unwrap().max_concurrent, 2);
    assert_eq!(summary.usage.model_calls, 2);
    assert_eq!(summary.usage.unknown_usage_calls, 1);
    assert_eq!(summary.usage.accounted_tokens, 15);
    assert_eq!(summary.child_usage.cache_read_tokens, Some(4));
    assert_eq!(summary.parent_usage.total_tokens, None);
    assert_eq!(summary.workers.len(), 1);
    assert_eq!(summary.workers[0].run_count, 2);
    assert_eq!(summary.workers[0].latest_run.id, next.id);
    assert_eq!(summary.workers[0].queue_position, Some(1));
    assert_eq!(summary.workers[0].cumulative_usage.accounted_tokens, 15);
    assert_eq!(summary.workers[0].latest_run.usage.accounted_tokens, 0);
    assert!(!summary.ledger_incomplete);
    assert!(
        reopened
            .budget_gate(&ledger)
            .unwrap_err()
            .contains("unknown")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn team_budget_uses_all_call_kinds_and_stops_at_the_limit() {
    use crate::usage_ledger::{Execution, UsageKind, UsageLedgerRecord, append_record};
    let (store, root) = fixture();
    store
        .configure_team(super::super::scheduler::AgentTeamSettings {
            token_budget: Some(20),
            ..Default::default()
        })
        .unwrap();
    let ledger = root.join("usage");
    store.budget_gate(&ledger).unwrap();
    for kind in [
        UsageKind::Main,
        UsageKind::Subagent,
        UsageKind::Compression,
        UsageKind::Auxiliary,
    ] {
        let mut record = UsageLedgerRecord::new(kind, Execution::Daemon);
        record.session_id = Some(store.parent);
        record.usage.total_tokens = Some(5);
        append_record(&ledger, &record).unwrap();
    }
    assert!(
        store
            .budget_gate(&ledger)
            .unwrap_err()
            .contains("used 20, limit 20")
    );
    let unbounded = AgentRunStore::new(&root, Uuid::new_v4());
    assert!(!unbounded.has_budget(None).unwrap());
    unbounded.budget_gate(&ledger).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn identical_request_is_idempotent_but_changed_payload_and_duplicate_agent_are_rejected() {
    let (store, root) = fixture();
    let request = request(Uuid::new_v4());
    let (first, created) = store.create(request.clone()).unwrap();
    assert!(created);
    let (same, created) = store.create(request.clone()).unwrap();
    assert!(!created);
    assert_eq!(same, first);
    let mut changed = request.clone();
    changed.dispatch = serde_json::json!({"task":"other"});
    assert!(store.create(changed).is_err());
    let mut duplicate = request;
    duplicate.request_id = Uuid::new_v4();
    assert!(store.create(duplicate.clone()).is_err());
    store
        .finish(first.id, AgentStatus::Completed, "done")
        .unwrap();
    let (next, _) = store.create(duplicate).unwrap();
    assert_eq!(next.agent_id, first.agent_id);
    assert_ne!(next.id, first.id);
    assert!(next.sequence > first.sequence);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reopening_preserves_queue_order_and_reading_never_interrupts_running_work() {
    let (store, root) = fixture();
    let (running, _) = store.create(request(Uuid::new_v4())).unwrap();
    store.start(running.id).unwrap();
    let (a, _) = store.create(request(Uuid::new_v4())).unwrap();
    let (b, _) = store.create(request(Uuid::new_v4())).unwrap();
    let reopened = AgentRunStore::new(&root, store.parent);
    assert_eq!(
        reopened.get(running.id).unwrap().unwrap().status,
        AgentStatus::Running
    );
    assert_eq!(
        reopened
            .pending()
            .unwrap()
            .iter()
            .map(|entry| entry.record.id)
            .collect::<Vec<_>>(),
        [a.id, b.id]
    );
    assert_eq!(reopened.interrupt_running().unwrap(), 1);
    assert_eq!(
        store.get(running.id).unwrap().unwrap().status,
        AgentStatus::Interrupted
    );
    assert_eq!(
        store.get(a.id).unwrap().unwrap().status,
        AgentStatus::Queued
    );
    let page = store.list(None, None, 2).unwrap();
    assert_eq!(page.next_after_sequence, Some(a.sequence));
    assert_eq!(
        store.list(None, page.next_after_sequence, 2).unwrap().runs[0].id,
        b.id
    );
    assert!(store.start(running.id).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn usage_and_delivery_survive_new_store_and_new_runs_do_not_reset_previous_usage() {
    let (store, root) = fixture();
    let initial = request(Uuid::new_v4());
    let (run, _) = store.create(initial.clone()).unwrap();
    store.start(run.id).unwrap();
    store
        .usage(
            run.id,
            &Usage {
                input_tokens: Some(100),
                output_tokens: Some(30),
                total_tokens: Some(10),
                cache_read_tokens: Some(20),
            },
        )
        .unwrap();
    store.usage(run.id, &Usage::default()).unwrap();
    store
        .finish(run.id, AgentStatus::Completed, "evidence")
        .unwrap();
    store.mark_delivered(run.id).unwrap();
    let reopened = AgentRunStore::new(&root, store.parent);
    let saved = reopened.get(run.id).unwrap().unwrap();
    assert_eq!(saved.usage.accounted_tokens, 130);
    assert_eq!(saved.usage.model_calls, 2);
    assert_eq!(saved.usage.unknown_usage_calls, 1);
    assert_eq!(saved.usage.cache_read_tokens, Some(20));
    assert!(saved.delivered);
    let mut followup = initial;
    followup.request_id = Uuid::new_v4();
    followup.reason = AgentRunReason::Continue;
    let (next, _) = store.create(followup).unwrap();
    assert_eq!(next.agent_id, saved.agent_id);
    assert_eq!(next.usage.model_calls, 0);
    assert_eq!(
        reopened
            .get(run.id)
            .unwrap()
            .unwrap()
            .usage
            .accounted_tokens,
        130
    );
    let public = serde_json::to_string(&reopened.list(None, None, 64).unwrap()).unwrap();
    assert!(!public.contains("private fixture"));
    assert!(!public.contains("approved_files"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn read_only_empty_inventory_does_not_create_files_and_record_identity_is_checked() {
    let (store, root) = fixture();
    assert!(store.list(None, None, 64).unwrap().runs.is_empty());
    assert!(!root.exists());
    let (run, _) = store.create(request(Uuid::new_v4())).unwrap();
    let mut stored = store.load(run.id).unwrap().unwrap();
    stored.record.parent_session_id = Uuid::new_v4();
    store.save(&stored).unwrap();
    assert!(store.get(run.id).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn execution_owner_is_exclusive_and_released_when_the_host_exits() {
    let (store, root) = fixture();
    let owner = store.claim().unwrap();
    let second = AgentRunStore::new(&root, store.parent);
    assert!(second.claim().is_err());
    drop(owner);
    let owner = second.claim().unwrap();
    drop(owner);
    std::fs::remove_dir_all(root).unwrap();
}
