use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use willdeep_core::provider::{Provider, ProviderError};
use willdeep_core::{
    Message,
    types::{Completion, ToolDefinition, Usage},
};

#[derive(Default)]
struct Worker {
    calls: AtomicUsize,
    histories: Mutex<Vec<Vec<Message>>>,
}

#[async_trait::async_trait]
impl Provider for Worker {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        assert!(
            tools
                .iter()
                .all(|tool| !["spawn_agent", "continue_agent"].contains(&tool.name.as_str()))
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.histories.lock().unwrap().push(messages.to_vec());
        Ok(Completion {
            reasoning: None,
            content: "worker report".into(),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".into()),
            usage: Some(Usage::default()),
        })
    }
}

#[tokio::test]
async fn idle_team_control_continues_child_and_commits_parent_receipts_without_parent_model() {
    let root = std::env::temp_dir().join(format!("team-control-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let parent_store = willdeep_core::SessionStore::new(&root);
    let mut parent = willdeep_core::Session::new(workspace.clone(), None, "parent");
    parent_store.save(&mut parent).unwrap();
    let runs = willdeep_core::subagent::AgentRunStore::new(root.join("workers"), parent.id);
    let background = Arc::new(BackgroundTaskRegistry::default().with_agent_run_store(runs.clone()));
    let worker = Arc::new(Worker::default());
    let catalog = SubagentCatalog::new(
        &workspace,
        willdeep_core::subagent::builtin_profiles(worker.clone()),
        background.clone(),
    )
    .with_parent_session(parent.id)
    .with_state_home(root.join("workers"));
    let goal =
        willdeep_core::GoalContinuation::with_default_budget(willdeep_core::GoalBudget::default());
    let child = uuid::Uuid::new_v4();
    let mut events = background.subscribe();
    catalog
        .spawn_external_read_only(
            child,
            "initial inspection".into(),
            None,
            Some("scout".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
        .await
        .unwrap()
        .unwrap();
    let request_id = uuid::Uuid::new_v4();
    let control = AgentTeamControl::Continue {
        id: child,
        message: "followup inspection".into(),
        request_id,
    };
    let first = execute_team(
        TeamRuntime {
            subagents: &catalog,
            background: &background,
            goal: &goal,
        },
        &parent_store,
        &mut parent,
        control.clone(),
        Language::ZhCn,
    )
    .await
    .unwrap();
    assert_eq!(first.turns, 0);
    assert_eq!(worker.calls.load(Ordering::SeqCst), 2);
    assert!(first.final_text.contains("worker report"));
    let run = runs.get(request_id).unwrap().unwrap();
    assert!(run.delivered);
    let committed = parent_store.load(parent.id).unwrap();
    assert!(committed.agent_reports_received.contains(&request_id));
    assert!(committed.agent_control_requests.contains(&request_id));
    assert_eq!(
        worker.histories.lock().unwrap()[1]
            .iter()
            .filter(|message| message.content.contains("initial inspection"))
            .count(),
        1
    );
    let count = parent.messages.len();
    execute_team(
        TeamRuntime {
            subagents: &catalog,
            background: &background,
            goal: &goal,
        },
        &parent_store,
        &mut parent,
        control,
        Language::ZhCn,
    )
    .await
    .unwrap();
    assert_eq!(worker.calls.load(Ordering::SeqCst), 2);
    assert_eq!(parent.messages.len(), count);
    std::fs::remove_dir_all(root).unwrap();
}
