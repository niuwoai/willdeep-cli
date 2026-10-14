use super::*;
use crate::checkpoint::SessionCheckpointSink;
use crate::provider::{Provider, ProviderError};
use crate::subagent::{AgentRunStore, runs::RunRequest};
use crate::types::{Completion, ToolDefinition, Usage};
use crate::{ApprovalMode, Session, SessionStore};
use std::sync::atomic::{AtomicUsize, Ordering};
use willdeep_runtime_protocol::{AgentRunReason, AgentStatus};

#[derive(Default)]
struct ProviderSpy(AtomicUsize);

#[async_trait::async_trait]
impl Provider for ProviderSpy {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Completion {
            reasoning: None,
            content: "done".into(),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".into()),
            usage: Some(Usage::default()),
        })
    }
}

struct Fixture {
    agent: Agent,
    sink: SessionCheckpointSink,
    runs: AgentRunStore,
    provider: Arc<ProviderSpy>,
    root: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("team-owner-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let store = SessionStore::new(&root);
        let mut parent = Session::new(root.clone(), None, "parent");
        store.save(&mut parent).unwrap();
        let runs = AgentRunStore::new(root.join("workers"), parent.id);
        let background =
            Arc::new(BackgroundTaskRegistry::default().with_agent_run_store(runs.clone()));
        let provider = Arc::new(ProviderSpy::default());
        let catalog = Arc::new(
            SubagentCatalog::new(
                &root,
                crate::subagent::builtin_profiles(provider.clone()),
                background.clone(),
            )
            .with_state_home(root.join("workers"))
            .with_parent_session(parent.id),
        );
        let agent = Agent::new(
            provider.clone(),
            ToolRegistry::new(&root, ApprovalMode::WorkspaceAccess).unwrap(),
            AgentConfig {
                max_turns: 1,
                system_prompt: "test".into(),
                context_window: 32_000,
                token_budget: None,
            },
        )
        .with_subagents(catalog)
        .with_background_tasks(background);
        Self {
            agent,
            sink: SessionCheckpointSink {
                store,
                session_id: parent.id,
            },
            runs,
            provider,
            root,
        }
    }

    fn running_worker(&self) -> uuid::Uuid {
        let id = uuid::Uuid::new_v4();
        self.runs
            .create(RunRequest {
                request_id: id,
                agent_id: uuid::Uuid::new_v4(),
                reason: AgentRunReason::Spawn,
                label: "uncertain previous worker".into(),
                model: None,
                max_turns: Some(1),
                token_budget: None,
                timeout_seconds: None,
                dispatch: serde_json::Value::Null,
            })
            .unwrap();
        self.runs.start(id).unwrap();
        id
    }

    async fn execute(&self) -> Result<AgentOutcome, AgentError> {
        self.agent
            .run_checkpointed(Vec::new(), Message::user("inspect"), Some(&self.sink))
            .await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[tokio::test]
async fn rejected_parent_execution_does_not_claim_or_interrupt_its_team() {
    let f = Fixture::new();
    let worker = f.running_worker();
    // Constructing the local Agent is read-only, even for an unfinished team.
    let team_owner = f.runs.claim().unwrap();
    let parent_owner = SessionCheckpointSink {
        store: f.sink.store.clone(),
        session_id: f.sink.session_id,
    }
    .claim()
    .unwrap();
    assert!(matches!(f.execute().await, Err(AgentError::Checkpoint(_))));
    assert_eq!(
        f.runs.get(worker).unwrap().unwrap().status,
        AgentStatus::Running
    );
    assert_eq!(f.provider.0.load(Ordering::SeqCst), 0);
    drop((parent_owner, team_owner));
    assert!(f.runs.claim().is_ok());
}

#[tokio::test]
async fn execution_recovers_once_and_followups_do_not_interrupt_new_workers() {
    let f = Fixture::new();
    let previous = f.running_worker();
    f.execute().await.unwrap();
    assert_eq!(
        f.runs.get(previous).unwrap().unwrap().status,
        AgentStatus::Interrupted
    );
    assert!(
        f.runs.claim().is_err(),
        "the executor retains team ownership"
    );
    let current = f.running_worker();
    f.execute().await.unwrap();
    assert_eq!(
        f.runs.get(current).unwrap().unwrap().status,
        AgentStatus::Running
    );
    assert_eq!(f.provider.0.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn failed_team_claim_can_recover_after_the_competitor_releases_it() {
    let f = Fixture::new();
    let previous = f.running_worker();
    let competitor = f.runs.claim().unwrap();
    assert!(matches!(f.execute().await, Err(AgentError::Checkpoint(_))));
    assert_eq!(
        f.runs.get(previous).unwrap().unwrap().status,
        AgentStatus::Running
    );
    assert_eq!(f.provider.0.load(Ordering::SeqCst), 0);
    drop(competitor);
    f.execute().await.unwrap();
    assert_eq!(
        f.runs.get(previous).unwrap().unwrap().status,
        AgentStatus::Interrupted
    );
    assert_eq!(f.provider.0.load(Ordering::SeqCst), 1);
}
