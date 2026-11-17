use super::*;
use crate::provider::{Provider, ProviderError};
use crate::types::{Completion, ToolCall, ToolDefinition};
use crate::{
    Agent, AgentConfig, AgentStopReason, ApprovalMode, Session, SessionStore, ToolRegistry,
};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct SyntheticProvider {
    tools: Mutex<VecDeque<Vec<ToolCall>>>,
    requests: Mutex<Vec<Vec<Message>>>,
}

#[async_trait]
impl Provider for SyntheticProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        self.requests.lock().unwrap().push(messages.to_vec());
        Ok(Completion {
            reasoning: None,
            content: "Synthetic identity answer".into(),
            tool_calls: self.tools.lock().unwrap().pop_front().unwrap_or_default(),
            finish_reason: Some("stop".into()),
            usage: None,
        })
    }
}

struct Fixture {
    root: std::path::PathBuf,
    sink: SessionCheckpointSink,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("input-scope-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let store = SessionStore::new(&root);
        let mut session = Session::new(workspace, None, "release fixture");
        store.save(&mut session).unwrap();
        Self {
            root,
            sink: SessionCheckpointSink {
                store,
                session_id: session.id,
            },
        }
    }

    fn saved(&self) -> Session {
        self.sink.store.load(self.sink.session_id).unwrap()
    }

    fn tools(&self) -> ToolRegistry {
        let workspace = self.saved().workspace;
        let snapshot = workspace.join("result.txt");
        ToolRegistry::new(workspace, ApprovalMode::WorkspaceAccess)
            .unwrap()
            .with_verification_snapshot(move || {
                Some(std::fs::read_to_string(&snapshot).unwrap_or_else(|_| "initial".into()))
            })
            .with_verification_reporter(|_| {})
    }

    fn agent(&self, provider: Arc<SyntheticProvider>, tools: ToolRegistry) -> Agent {
        Agent::new(
            provider,
            tools,
            AgentConfig {
                max_turns: 5,
                system_prompt: "Offline input scope fixture".into(),
                context_window: 128_000,
                token_budget: None,
            },
        )
    }

    fn new_input(&self) -> ClaimedSessionCheckpointSink {
        SessionCheckpointSink {
            store: self.sink.store.clone(),
            session_id: self.sink.session_id,
        }
        .claim()
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn write_call() -> ToolCall {
    ToolCall {
        id: "scope-write".into(),
        name: "create_file".into(),
        arguments: serde_json::json!({"path": "result.txt", "content": "changed"}).to_string(),
    }
}

fn assert_answer_only(outcome: &crate::AgentOutcome, history_len: usize) {
    assert_eq!(outcome.stop_reason, AgentStopReason::Finished);
    assert_eq!(outcome.turns, 1);
    assert_eq!(outcome.final_text, "Synthetic identity answer");
    let added = &outcome.messages[history_len..];
    assert!(added.iter().all(|message| message.tool_calls.is_empty()));
    assert!(added.iter().all(|message| message.role != Role::Tool));
    assert!(added.iter().all(|message| {
        !message
            .content
            .contains("[completion-verification-required]")
    }));
}

#[tokio::test]
async fn new_input_after_persisted_partial_answers_once_without_inheriting_release_debt() {
    let fixture = Fixture::new();
    let release = Arc::new(SyntheticProvider::default());
    release.tools.lock().unwrap().push_back(vec![write_call()]);
    let partial = fixture
        .agent(release, fixture.tools())
        .run_checkpointed(
            Vec::new(),
            Message::user("release fixture"),
            Some(&fixture.sink),
        )
        .await
        .unwrap();
    assert_eq!(partial.stop_reason, AgentStopReason::Unverified);
    assert_eq!(partial.turns, 4);
    let before = fixture.saved();
    let debt = before.execution_checkpoint.as_ref().unwrap();
    assert_eq!(debt.status, CheckpointStatus::Partial);
    assert_eq!(debt.verification_baseline.as_deref(), Some("initial"));
    assert_eq!(
        std::fs::read_to_string(before.workspace.join("result.txt")).unwrap(),
        "changed"
    );

    // A new provider and registry restore only persisted history, like Runtime.
    let provider = Arc::new(SyntheticProvider::default());
    let agent = fixture.agent(provider.clone(), fixture.tools());
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let answer = agent
        .run_checkpointed(
            before.messages.clone(),
            Message::user("你是谁"),
            Some(&sink),
        )
        .await
        .unwrap();
    assert_answer_only(&answer, before.messages.len());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    let saved = fixture.saved();
    let retained = saved.execution_checkpoint.unwrap();
    assert_eq!(retained.status, CheckpointStatus::Partial);
    assert_eq!(retained.verification_baseline, debt.verification_baseline);
    assert!(retained.verification_evidence.is_empty());
    assert_eq!(
        saved.messages.last().unwrap().content,
        "Synthetic identity answer"
    );
    drop(sink);
    drop(ownership);
    let resumed = agent
        .run_checkpointed(
            saved.messages,
            Message::host_instruction("recover original task"),
            Some(&fixture.sink),
        )
        .await
        .unwrap();
    assert_eq!(resumed.stop_reason, AgentStopReason::Unverified);
    assert_eq!(resumed.turns, 3);
    assert_eq!(
        fixture
            .saved()
            .execution_checkpoint
            .unwrap()
            .verification_baseline
            .as_deref(),
        Some("initial")
    );
}

#[tokio::test]
async fn new_input_on_reused_agent_does_not_inherit_restored_failures_or_required_checks() {
    let fixture = Fixture::new();
    fixture
        .sink
        .store
        .update(fixture.sink.session_id, |session| {
            session.execution_checkpoint = Some(CheckpointMetadata {
                status: CheckpointStatus::Partial,
                verification_baseline: Some("initial".into()),
                required_verifications: vec!["rake test".into()],
                verification_evidence: vec![VerificationEvidence {
                    snapshot_id: Some("initial".into()),
                    command: "rake test".into(),
                    status: crate::tools::VerificationStatus::Failed,
                }],
                ..Default::default()
            });
        })
        .unwrap();
    let provider = Arc::new(SyntheticProvider::default());
    let agent = fixture.agent(provider.clone(), fixture.tools());
    let resumed = agent
        .run_checkpointed(
            Vec::new(),
            Message::user("resume fixture"),
            Some(&fixture.sink),
        )
        .await
        .unwrap();
    assert_eq!(resumed.stop_reason, AgentStopReason::Unverified);
    assert_eq!(resumed.turns, 3);
    let calls_before = provider.requests.lock().unwrap().len();
    let history = fixture.saved().messages;
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let answer = agent
        .run_checkpointed(
            history.clone(),
            Message::user("independent question"),
            Some(&sink),
        )
        .await
        .unwrap();
    assert_answer_only(&answer, history.len());
    assert_eq!(provider.requests.lock().unwrap().len() - calls_before, 1);
    let retained = fixture.saved().execution_checkpoint.unwrap();
    assert_eq!(retained.status, CheckpointStatus::Partial);
    assert_eq!(retained.required_verifications, ["rake test"]);
    assert!(retained.verification_evidence.iter().any(|record| {
        record.command == "rake test" && record.status == crate::tools::VerificationStatus::Failed
    }));
    drop(sink);
    drop(ownership);
    let resumed = agent
        .run_checkpointed(
            fixture.saved().messages,
            Message::host_instruction("recover original task"),
            Some(&fixture.sink),
        )
        .await
        .unwrap();
    assert_eq!(resumed.stop_reason, AgentStopReason::Unverified);
    assert!(resumed.messages.iter().any(|message| {
        message
            .content
            .contains("[completion-verification-required]")
            && message.content.contains("rake test")
    }));
}

#[tokio::test]
async fn new_input_that_changes_files_still_requires_current_verification() {
    let fixture = Fixture::new();
    let provider = Arc::new(SyntheticProvider::default());
    provider.tools.lock().unwrap().push_back(vec![write_call()]);
    let agent = fixture.agent(provider.clone(), fixture.tools());
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let outcome = agent
        .run_checkpointed(Vec::new(), Message::user("new edit fixture"), Some(&sink))
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, AgentStopReason::Unverified);
    assert_eq!(outcome.turns, 4);
    assert_eq!(provider.requests.lock().unwrap().len(), 4);
    assert_eq!(
        outcome
            .messages
            .iter()
            .filter(|message| message.role == Role::Tool)
            .count(),
        1
    );
    let checkpoint = fixture.saved().execution_checkpoint.unwrap();
    assert_eq!(checkpoint.status, CheckpointStatus::Partial);
    assert_eq!(checkpoint.verification_baseline.as_deref(), Some("initial"));
}

#[tokio::test]
async fn new_input_keeps_configured_checks_and_explicit_patch_contracts() {
    let fixture = Fixture::new();
    for patch_contract in [false, true] {
        let mut tools = fixture.tools();
        if patch_contract {
            tools = tools
                .with_completion_contract(crate::tools::CompletionContract {
                    require_changed_globs: vec!["result.txt".into()],
                    preserve_existing_globs: Vec::new(),
                })
                .unwrap();
        } else {
            tools.require_verifications(&["rake test".into()]).unwrap();
        }
        let agent = fixture.agent(Arc::new(SyntheticProvider::default()), tools);
        let ownership = fixture.new_input();
        let sink = ownership.for_new_input().unwrap();
        let outcome = agent
            .run_checkpointed(
                fixture.saved().messages,
                Message::user("independent input"),
                Some(&sink),
            )
            .await
            .unwrap();
        assert_eq!(outcome.stop_reason, AgentStopReason::Unverified);
        assert_eq!(outcome.turns, 3);
        assert!(outcome.messages.iter().any(|message| {
            message
                .content
                .contains("[completion-verification-required]")
        }));
    }
}

#[tokio::test]
async fn new_input_cannot_remove_an_unfinished_patch_contract() {
    let fixture = Fixture::new();
    let contracted = fixture
        .tools()
        .with_completion_contract(crate::tools::CompletionContract {
            require_changed_globs: vec!["result.txt".into()],
            preserve_existing_globs: Vec::new(),
        })
        .unwrap();
    let mut recorder = CheckpointRecorder::new(Some(&fixture.sink));
    recorder.initialize_patch_contract(&contracted).unwrap();
    recorder
        .record(&[Message::user("original contracted task")], 1, 0, 0)
        .unwrap();
    drop(recorder);
    let provider = Arc::new(SyntheticProvider::default());
    let agent = fixture.agent(provider.clone(), fixture.tools());
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let result = agent
        .run_checkpointed(
            fixture.saved().messages,
            Message::user("independent input"),
            Some(&sink),
        )
        .await;
    assert!(
        matches!(result, Err(AgentError::Checkpoint(message)) if message.contains("completion contract"))
    );
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn new_input_cannot_reset_an_active_goal_verification_scope() {
    let fixture = Fixture::new();
    let provider = Arc::new(SyntheticProvider::default());
    let goal = Arc::new(crate::GoalContinuation::new());
    goal.activate("unfinished goal", crate::GoalBudget::default());
    let agent = fixture
        .agent(provider.clone(), fixture.tools())
        .with_goal_continuation(goal.clone());
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let result = agent
        .run_checkpointed(Vec::new(), Message::user("independent input"), Some(&sink))
        .await;
    assert!(
        matches!(result, Err(AgentError::Checkpoint(message)) if message.contains("active Goal"))
    );
    assert!(provider.requests.lock().unwrap().is_empty());
    assert!(goal.is_active());
}

#[tokio::test]
async fn new_input_preserves_unknown_tool_effects_and_does_not_auto_resume() {
    let fixture = Fixture::new();
    let pending = write_call();
    fixture
        .sink
        .save(&RunCheckpoint {
            plan: None,
            metadata: CheckpointMetadata {
                status: CheckpointStatus::Partial,
                verification_baseline: Some("initial".into()),
                pending_call_ids: vec![pending.id.clone()],
                ..Default::default()
            },
            messages: vec![
                Message::user("earlier task"),
                Message::assistant("", vec![pending.clone()]),
            ],
            goal: None,
        })
        .unwrap();
    let provider = Arc::new(SyntheticProvider::default());
    let agent = fixture.agent(provider.clone(), fixture.tools());
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let mut history = fixture.saved().messages;
    history.push(
        fixture
            .saved()
            .execution_checkpoint
            .unwrap()
            .new_input_notice()
            .unwrap(),
    );
    let outcome = agent
        .run_checkpointed(history, Message::user("independent input"), Some(&sink))
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, AgentStopReason::Finished);
    assert_eq!(outcome.turns, 1);
    let requests = provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].iter().any(|message| {
        message
            .content
            .starts_with("[willdeep-unknown-tool-effect]")
    }));
    assert!(!requests[0].iter().any(|message| {
        message
            .content
            .contains("Continue the outstanding user objective")
    }));
    let retained = fixture.saved().execution_checkpoint.unwrap();
    assert_eq!(retained.status, CheckpointStatus::Partial);
    assert_eq!(retained.pending_call_ids, [pending.id]);
    assert!(!fixture.saved().workspace.join("result.txt").exists());
}

#[test]
fn new_input_merges_evidence_without_certifying_or_weakening_the_failed_task() {
    use crate::tools::VerificationStatus;
    let fixture = Fixture::new();
    let previous = CheckpointMetadata {
        status: CheckpointStatus::Failed,
        verification_baseline: Some("original-baseline".into()),
        required_verifications: vec!["rake test".into()],
        verification_evidence: vec![VerificationEvidence {
            snapshot_id: Some("current".into()),
            command: "rake test".into(),
            status: VerificationStatus::Failed,
        }],
        pending_call_ids: vec!["unknown-old-effect".into()],
        ..Default::default()
    };
    fixture
        .sink
        .store
        .update(fixture.sink.session_id, |session| {
            session.execution_checkpoint = Some(previous.clone());
        })
        .unwrap();
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    for status in [
        VerificationStatus::InvocationError,
        VerificationStatus::Invalidated,
        VerificationStatus::Passed,
    ] {
        sink.save(&RunCheckpoint {
            plan: None,
            metadata: CheckpointMetadata {
                status: CheckpointStatus::Completed,
                verification_baseline: Some("new-input-baseline".into()),
                required_verifications: vec!["cargo test".into()],
                verification_evidence: vec![VerificationEvidence {
                    status,
                    ..previous.verification_evidence[0].clone()
                }],
                ..Default::default()
            },
            messages: vec![Message::user("independent input")],
            goal: None,
        })
        .unwrap();
        let retained = fixture.saved().execution_checkpoint.unwrap();
        assert_eq!(retained.status, CheckpointStatus::Failed);
        assert_eq!(
            retained.verification_baseline,
            previous.verification_baseline
        );
        assert_eq!(retained.required_verifications, ["cargo test", "rake test"]);
        assert_eq!(retained.pending_call_ids, previous.pending_call_ids);
        assert_eq!(
            retained.verification_evidence[0].status,
            if status == VerificationStatus::Passed {
                VerificationStatus::Passed
            } else {
                VerificationStatus::Failed
            }
        );
    }
}
