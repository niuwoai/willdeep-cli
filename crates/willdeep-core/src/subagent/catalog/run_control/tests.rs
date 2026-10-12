use super::*;
use crate::provider::{Provider, ProviderError};
use crate::types::{Completion, Message, ToolDefinition, Usage};
use crate::usage_ledger::{
    ClientKind, Execution, UsageLedgerContext, UsageLedgerScope, shared_sink,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct HistoryProvider {
    calls: AtomicUsize,
    histories: Mutex<Vec<Vec<Message>>>,
}

#[async_trait::async_trait]
impl Provider for HistoryProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        assert!(
            tools
                .iter()
                .all(|tool| !crate::tools::PARENT_ONLY_TOOLS.contains(&tool.name.as_str()))
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.histories.lock().unwrap().push(messages.to_vec());
        Ok(Completion {
            reasoning: None,
            content: "inspection complete".into(),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".into()),
            usage: Some(Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                cache_read_tokens: Some(2),
            }),
        })
    }
}

fn fixture() -> (
    SubagentCatalog,
    Arc<BackgroundTaskRegistry>,
    Arc<HistoryProvider>,
    PathBuf,
) {
    let root = std::env::temp_dir().join(format!("continue-worker-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let home = root.join("workers");
    let parent = uuid::Uuid::new_v4();
    let provider = Arc::new(HistoryProvider::default());
    let background = Arc::new(
        BackgroundTaskRegistry::default()
            .with_agent_run_store(super::super::super::runs::AgentRunStore::new(&home, parent)),
    );
    let mut context = UsageLedgerContext::new(ClientKind::Cli, Execution::InProcess);
    context.session_id = Some(parent);
    let scope = UsageLedgerScope::new(shared_sink(&root.join("usage")), context);
    let catalog = SubagentCatalog::new(
        &workspace,
        crate::subagent::builtin_profiles(provider.clone()),
        background.clone(),
    )
    .with_state_home(&home)
    .with_parent_session(parent)
    .with_usage_ledger(scope);
    (catalog, background, provider, root)
}

async fn completion(
    events: &mut tokio::sync::broadcast::Receiver<crate::background::BackgroundTaskEvent>,
) {
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        event.snapshot.status,
        BackgroundTaskStatus::Completed,
        "{}",
        event.notice
    );
}

#[tokio::test]
async fn continuation_preserves_identity_history_permissions_and_accumulated_usage() {
    let (catalog, background, provider, root) = fixture();
    let mut events = background.subscribe();
    catalog
        .run(
            SpawnAgentArgs {
                profile: Some("scout".into()),
                prompt: "first inspection".into(),
                run_in_background: Some(true),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    completion(&mut events).await;
    let agent_id = background.snapshots()[0].agent_id.unwrap();
    let first = background
        .agent_run_store()
        .unwrap()
        .latest(agent_id)
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(first.usage.accounted_tokens, 15);
    assert_eq!(first.usage.model_calls, 1);
    let request_id = uuid::Uuid::new_v4();
    let next = catalog
        .continue_background_agent(agent_id, "second inspection", request_id)
        .await
        .unwrap();
    assert_eq!(next.agent_id, agent_id);
    assert_ne!(next.id, first.id);
    completion(&mut events).await;
    let duplicate = catalog
        .continue_background_agent(agent_id, "second inspection", request_id)
        .await
        .unwrap();
    assert_eq!(duplicate.id, next.id);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert!(
        catalog
            .continue_background_agent(agent_id, "changed task", request_id)
            .await
            .is_err()
    );
    let histories = provider.histories.lock().unwrap();
    assert!(
        histories[1]
            .iter()
            .any(|message| message.content.contains("first inspection"))
    );
    assert!(
        histories[1]
            .iter()
            .any(|message| message.content.contains("second inspection"))
    );
    assert_eq!(
        histories[1]
            .iter()
            .filter(|message| message.role == crate::Role::User
                && message.content.contains("first inspection"))
            .count(),
        1
    );
    let store = background.agent_run_store().unwrap();
    assert_eq!(
        store.get(first.id).unwrap().unwrap().usage.accounted_tokens,
        15
    );
    assert_eq!(
        store.get(next.id).unwrap().unwrap().usage.accounted_tokens,
        15
    );
    assert_eq!(store.list(Some(agent_id), None, 64).unwrap().runs.len(), 2);
    drop(histories);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn continuation_admitted_by_runtime_launches_the_same_durable_run_once() {
    let (catalog, background, provider, root) = fixture();
    let mut events = background.subscribe();
    catalog
        .run(
            SpawnAgentArgs {
                profile: Some("scout".into()),
                prompt: "initial".into(),
                run_in_background: Some(true),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    completion(&mut events).await;
    let id = background.snapshots()[0].agent_id.unwrap();
    let store = background.agent_run_store().unwrap();
    let request_id = uuid::Uuid::new_v4();
    let accepted = store
        .prepare_continuation(id, "pre-admitted task", request_id)
        .unwrap();
    assert_eq!(accepted.status, AgentStatus::Queued);
    assert_eq!(
        store
            .prepare_continuation(id, "pre-admitted task", request_id)
            .unwrap()
            .id,
        accepted.id
    );
    catalog
        .continue_background_agent(id, "pre-admitted task", request_id)
        .await
        .unwrap();
    completion(&mut events).await;
    catalog
        .continue_background_agent(id, "pre-admitted task", request_id)
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(store.list(Some(id), None, 64).unwrap().runs.len(), 2);
    assert_eq!(
        store.get(request_id).unwrap().unwrap().status,
        AgentStatus::Completed
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn the_final_answer_at_the_budget_limit_is_kept_and_the_next_run_is_blocked() {
    let (catalog, background, provider, root) = fixture();
    background
        .agent_run_store()
        .unwrap()
        .configure_team(super::super::super::scheduler::AgentTeamSettings {
            token_budget: Some(15),
            ..Default::default()
        })
        .unwrap();
    let mut events = background.subscribe();
    catalog
        .run(
            SpawnAgentArgs {
                profile: Some("scout".into()),
                prompt: "use the final allocation".into(),
                run_in_background: Some(true),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    completion(&mut events).await;
    let agent = background.snapshots()[0].agent_id.unwrap();
    let next = catalog
        .continue_background_agent(agent, "new request", uuid::Uuid::new_v4())
        .await
        .unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.snapshot.status, BackgroundTaskStatus::Blocked);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        background
            .agent_run_store()
            .unwrap()
            .get(next.id)
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Blocked
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn continuation_rejects_unknown_or_foreign_agents_and_empty_requests() {
    let (catalog, _, provider, root) = fixture();
    assert!(
        catalog
            .continue_background_agent(uuid::Uuid::new_v4(), "inspect", uuid::Uuid::new_v4())
            .await
            .is_err()
    );
    assert!(
        catalog
            .continue_background_agent(uuid::Uuid::new_v4(), " ", uuid::Uuid::new_v4())
            .await
            .is_err()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn settled_worker_does_not_keep_the_team_execution_owner_alive() {
    let (catalog, background, _, root) = fixture();
    let store = background.agent_run_store().unwrap().clone();
    let mut events = background.subscribe();
    catalog
        .run(
            SpawnAgentArgs {
                profile: Some("scout".into()),
                prompt: "inspect".into(),
                run_in_background: Some(true),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    completion(&mut events).await;
    assert!(store.claim().is_err());
    let weak = Arc::downgrade(&background);
    drop(catalog);
    drop(background);
    let owner = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if weak.upgrade().is_none()
                && let Ok(owner) = store.claim()
            {
                break owner;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(owner);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn new_execution_host_restores_queue_ids_in_order_and_does_not_replay_running_record() {
    let (catalog, background, provider, root) = fixture();
    let mut events = background.subscribe();
    catalog
        .run(
            SpawnAgentArgs {
                profile: Some("scout".into()),
                prompt: "initial inspection".into(),
                run_in_background: Some(true),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    completion(&mut events).await;
    let original_id = background.snapshots()[0].agent_id.unwrap();
    let store = background.agent_run_store().unwrap().clone();
    let original = store.latest(original_id).unwrap().unwrap();
    let mut seeded = Vec::new();
    for prompt in [
        "uncertain old execution",
        "first queued inspection",
        "second queued inspection",
    ] {
        let agent_id = uuid::Uuid::new_v4();
        let mut dispatch = original.dispatch.clone();
        dispatch["id"] = serde_json::json!(agent_id);
        dispatch["args"]["prompt"] = serde_json::json!(prompt);
        let (run, _) = store
            .create(RunRequest {
                request_id: uuid::Uuid::new_v4(),
                agent_id,
                reason: AgentRunReason::Spawn,
                label: prompt.into(),
                model: original.record.model.clone(),
                max_turns: original.record.max_turns,
                token_budget: original.record.token_budget,
                timeout_seconds: original.record.timeout_seconds,
                dispatch,
            })
            .unwrap();
        seeded.push(run);
    }
    store.start(seeded[0].id).unwrap();
    let profiles = catalog.profiles.values().cloned().collect();
    let scope = catalog.usage_ledger.clone().unwrap();
    let workspace = catalog.workspace.clone();
    let home = catalog.state_home.clone().unwrap();
    drop(catalog);
    drop(background);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(owner) = store.claim() {
                drop(owner);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let restarted_background = Arc::new(
        BackgroundTaskRegistry::default()
            .with_agent_team_settings(crate::subagent::AgentTeamSettings {
                max_concurrent: 1,
                ..Default::default()
            })
            .unwrap()
            .with_agent_run_store(store.clone()),
    );
    let restarted = SubagentCatalog::new(workspace, profiles, restarted_background.clone())
        .with_state_home(home)
        .with_parent_session(original.record.parent_session_id)
        .with_usage_ledger(scope);
    let mut events = restarted_background.subscribe();
    assert_eq!(restarted.restore_queued_runs().await.unwrap(), 2);
    assert_eq!(
        store.get(seeded[0].id).unwrap().unwrap().status,
        AgentStatus::Interrupted
    );
    completion(&mut events).await;
    completion(&mut events).await;
    assert_eq!(provider.calls.load(Ordering::SeqCst), 3);
    for run in &seeded[1..] {
        assert_eq!(
            store.get(run.id).unwrap().unwrap().status,
            AgentStatus::Completed
        );
        assert_eq!(
            restarted_background
                .task_for_agent(run.agent_id)
                .unwrap()
                .run_id,
            Some(run.id)
        );
    }
    let histories = provider.histories.lock().unwrap();
    assert!(
        histories[1]
            .iter()
            .any(|message| message.content.contains("first queued inspection"))
    );
    assert!(
        histories[2]
            .iter()
            .any(|message| message.content.contains("second queued inspection"))
    );
    assert!(
        histories
            .iter()
            .flatten()
            .all(|message| !message.content.contains("uncertain old execution"))
    );
    drop(histories);
    std::fs::remove_dir_all(root).unwrap();
}
