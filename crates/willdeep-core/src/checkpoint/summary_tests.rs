use super::*;
use crate::context_summary::ContextSummary;
use crate::provider::{Provider, ProviderError};
use crate::types::{Completion, ToolCall, ToolDefinition, Usage};
use crate::{Agent, AgentConfig, ApprovalMode, Session, SessionStore, ToolRegistry};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const HISTORY_BATCHES: usize = 20;
const TOOL_OUTPUT_BYTES: usize = 1_900;
const SUMMARY_TEXT: &str =
    "OBJECTIVE: inspect files. COMPLETED: read src/file.rs. REMAINING: report findings.";
const CURRENT_QUERY: &str = "Report findings now";

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    session_id: uuid::Uuid,
    history: Vec<Message>,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("summary-checkpoint-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(workspace.join("src")).unwrap();
        std::fs::write(workspace.join("AGENTS.md"), "Report findings accurately.").unwrap();
        std::fs::write(
            workspace.join("src/AGENTS.md"),
            "Scope: src\nParent: ../AGENTS.md\nScoped rules: preserve protocol IDs.",
        )
        .unwrap();
        std::fs::write(workspace.join("src/file.rs"), "fn example() {}\n").unwrap();
        let mut history = vec![Message::user("Inspect the files and report")];
        for index in 0..HISTORY_BATCHES {
            let call = ToolCall {
                id: format!("read-{index}"),
                name: "read_file".into(),
                arguments: r#"{"path":"src/file.rs"}"#.into(),
            };
            history.push(Message::assistant("inspect", vec![call.clone()]));
            history.push(Message::tool(
                &call,
                format!("file-{index}: {}", "x".repeat(TOOL_OUTPUT_BYTES)),
            ));
        }
        let store = SessionStore::new(root.join("sessions"));
        let mut session = Session::new(workspace.clone(), None, "report findings");
        session.messages = history.clone();
        store.save(&mut session).unwrap();
        Self {
            root,
            workspace,
            session_id: session.id,
            history,
        }
    }

    fn sink(&self) -> SessionCheckpointSink {
        SessionCheckpointSink {
            store: SessionStore::new(self.root.join("sessions")),
            session_id: self.session_id,
        }
    }

    fn saved(&self) -> Session {
        self.sink().store.load(self.session_id).unwrap()
    }

    fn agent(&self, foreground: Arc<Foreground>, compressor: Arc<Summarizer>) -> Agent {
        Agent::new(
            foreground,
            ToolRegistry::new(&self.workspace, ApprovalMode::ReadOnly)
                .unwrap()
                .with_allowed_tools(["read_file".to_owned()]),
            AgentConfig {
                max_turns: 1,
                system_prompt: "Inspect files".into(),
                context_window: 12_288,
                token_budget: None,
            },
        )
        .with_compressors(vec![(compressor, false)])
    }

    fn foreground(&self, behavior: Behavior) -> Arc<Foreground> {
        Arc::new(Foreground {
            sink: self.sink(),
            behavior,
            calls: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[derive(Default)]
struct Summarizer {
    calls: AtomicUsize,
    add_notices: Option<SessionCheckpointSink>,
}

#[async_trait]
impl Provider for Summarizer {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0
            && let Some(sink) = &self.add_notices
        {
            // Another writer adds display rows after the executor loaded history.
            sink.store
                .update(sink.session_id, |session| {
                    let mut notice = Message::system("peer replied");
                    notice.source = Some(crate::types::MessageSource::FederationNotice);
                    session.messages.insert(1, notice.clone());
                    session.messages.insert(2, notice);
                })
                .unwrap();
        }
        Ok(Completion {
            reasoning: None,
            content: SUMMARY_TEXT.into(),
            tool_calls: Vec::new(),
            usage: Some(Usage {
                input_tokens: Some(2),
                output_tokens: Some(1),
                ..Default::default()
            }),
            finish_reason: Some("stop".into()),
        })
    }
}

#[derive(Clone, Copy)]
enum Behavior {
    Fail,
    Hang,
    Succeed,
}

struct Foreground {
    sink: SessionCheckpointSink,
    behavior: Behavior,
    calls: AtomicUsize,
    entered: tokio::sync::Notify,
}

#[async_trait]
impl Provider for Foreground {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(
            messages
                .iter()
                .any(|message| message.content.contains("<context-summary"))
        );
        assert!(messages[0].content.contains("Scoped rules:"));
        let saved = self.sink.store.load(self.sink.session_id).unwrap();
        assert_saved_summary(&saved);
        self.entered.notify_one();
        match self.behavior {
            Behavior::Fail => Err(ProviderError::InvalidResponse("foreground failed".into())),
            Behavior::Hang => std::future::pending().await,
            Behavior::Succeed => Ok(Completion {
                reasoning: None,
                content: "ready".into(),
                tool_calls: Vec::new(),
                usage: Some(Usage {
                    input_tokens: Some(2),
                    output_tokens: Some(1),
                    ..Default::default()
                }),
                finish_reason: Some("stop".into()),
            }),
        }
    }
}

#[tokio::test]
async fn completed_summary_survives_foreground_model_error_in_session_store() {
    let fixture = Fixture::new();
    let foreground = fixture.foreground(Behavior::Fail);
    let compressor = Arc::new(Summarizer::default());
    let agent = fixture.agent(foreground.clone(), compressor.clone());
    let sink = fixture.sink();
    let result = agent
        .run_checkpointed_with_summary(
            fixture.history.clone(),
            Message::user(CURRENT_QUERY),
            Some(&sink),
            None,
        )
        .await;
    assert!(
        matches!(result, Err(AgentError::Provider(ProviderError::InvalidResponse(message))) if message == "foreground failed")
    );
    assert!(compressor.calls.load(Ordering::SeqCst) > 0);
    assert_eq!(foreground.calls.load(Ordering::SeqCst), 1);
    let saved = fixture.saved();
    assert_saved_summary(&saved);
    assert_eq!(
        saved.execution_checkpoint.unwrap().status,
        CheckpointStatus::Failed
    );
}

#[tokio::test]
async fn cancelled_foreground_restores_summary_and_scoped_rules_without_recompression() {
    let fixture = Fixture::new();
    let foreground = fixture.foreground(Behavior::Hang);
    let compressor = Arc::new(Summarizer {
        add_notices: Some(fixture.sink()),
        ..Default::default()
    });
    let agent = fixture.agent(foreground.clone(), compressor.clone());
    let sink = fixture.sink();
    let mut execution = Box::pin(agent.run_checkpointed_with_summary(
        fixture.history.clone(),
        Message::user(CURRENT_QUERY),
        Some(&sink),
        None,
    ));
    tokio::select! {
        _ = &mut execution => panic!("foreground must remain pending"),
        result = tokio::time::timeout(std::time::Duration::from_secs(10), foreground.entered.notified()) => result.unwrap(),
    }
    assert!(sink.store.acquire_execution(fixture.session_id).is_err());
    let before_cancel = fixture.saved();
    let original = assert_saved_summary(&before_cancel).clone();
    assert_eq!(
        before_cancel
            .messages
            .iter()
            .filter(|message| is_federation_notice(message))
            .count(),
        2
    );
    drop(execution);
    drop(agent);
    drop(sink);
    assert!(
        fixture
            .sink()
            .store
            .acquire_execution(fixture.session_id)
            .is_ok()
    );
    let saved = fixture.saved();
    assert_eq!(assert_saved_summary(&saved), &original);
    assert!(compressor.calls.load(Ordering::SeqCst) > 0);

    // Recreate every executor/store/cache from disk. Only the new provider mocks
    // remain in memory; no SummaryCache from the cancelled run is reused.
    let resumed_compressor = Arc::new(Summarizer::default());
    let resumed_foreground = fixture.foreground(Behavior::Succeed);
    let resumed = fixture.agent(resumed_foreground.clone(), resumed_compressor.clone());
    let resumed_sink = fixture.sink();
    let outcome = resumed
        .run_checkpointed_with_summary(
            saved.messages,
            Message::user("Continue the report"),
            Some(&resumed_sink),
            saved.context_summary,
        )
        .await
        .unwrap();
    assert_eq!(outcome.final_text, "ready");
    assert_eq!(resumed_foreground.calls.load(Ordering::SeqCst), 1);
    assert_eq!(resumed_compressor.calls.load(Ordering::SeqCst), 0);
    assert_eq!(assert_saved_summary(&fixture.saved()), &original);
    drop(resumed);
    drop(resumed_sink);

    // The scope must be restored from historical tool paths, then refreshed from
    // today's file. A rule edit changes the system key and invalidates the cache.
    std::fs::write(
        fixture.workspace.join("src/AGENTS.md"),
        "Scope: src\nParent: ../AGENTS.md\nScoped rules: include evidence for every finding.",
    )
    .unwrap();
    let saved = fixture.saved();
    let updated_compressor = Arc::new(Summarizer::default());
    let updated = fixture.agent(
        fixture.foreground(Behavior::Succeed),
        updated_compressor.clone(),
    );
    let updated_sink = fixture.sink();
    updated
        .run_checkpointed_with_summary(
            saved.messages,
            Message::user("Review findings under current rules"),
            Some(&updated_sink),
            saved.context_summary,
        )
        .await
        .unwrap();
    assert!(updated_compressor.calls.load(Ordering::SeqCst) > 0);
    assert_ne!(
        assert_saved_summary(&fixture.saved()).system_fingerprint,
        original.system_fingerprint
    );
}

struct RejectSummary {
    inner: SessionCheckpointSink,
    rejected: AtomicUsize,
    save_before_error: bool,
}

impl CheckpointSink for RejectSummary {
    fn acquire_run(&self) -> Result<Option<Box<dyn Send + Sync>>, String> {
        self.inner.acquire_run()
    }

    fn save(&self, checkpoint: &RunCheckpoint) -> Result<(), String> {
        if checkpoint.context_summary.is_some() {
            self.rejected.fetch_add(1, Ordering::SeqCst);
            if self.save_before_error {
                self.inner.save(checkpoint)?;
            }
            return Err("summary persistence failed".into());
        }
        self.inner.save(checkpoint)
    }
}

#[tokio::test]
async fn summary_checkpoint_failure_prevents_foreground_model_request() {
    for save_before_error in [false, true] {
        let fixture = Fixture::new();
        let foreground = fixture.foreground(Behavior::Succeed);
        let compressor = Arc::new(Summarizer::default());
        let agent = fixture.agent(foreground.clone(), compressor.clone());
        let sink = RejectSummary {
            inner: fixture.sink(),
            rejected: AtomicUsize::new(0),
            save_before_error,
        };
        let result = agent
            .run_checkpointed_with_summary(
                fixture.history.clone(),
                Message::user(CURRENT_QUERY),
                Some(&sink),
                None,
            )
            .await;
        assert!(
            matches!(result, Err(AgentError::Checkpoint(message)) if message == "summary persistence failed")
        );
        assert!(compressor.calls.load(Ordering::SeqCst) > 0);
        assert_eq!(foreground.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            sink.rejected.load(Ordering::SeqCst),
            1,
            "finish/Drop must not retry an uncertain save"
        );
        let saved = fixture.saved();
        assert_eq!(saved.context_summary.is_some(), save_before_error);
        if save_before_error {
            assert_saved_summary(&saved);
        }
        assert!(
            fixture
                .sink()
                .store
                .acquire_execution(fixture.session_id)
                .is_ok()
        );
    }
}

#[tokio::test]
async fn previous_summary_is_preserved_when_request_preparation_exceeds_capacity() {
    let fixture = Fixture::new();
    let seed = fixture.agent(
        fixture.foreground(Behavior::Fail),
        Arc::new(Summarizer::default()),
    );
    let sink = fixture.sink();
    let _ = seed
        .run_checkpointed_with_summary(
            fixture.history.clone(),
            Message::user(CURRENT_QUERY),
            Some(&sink),
            None,
        )
        .await;
    let saved = fixture.saved();
    let previous = assert_saved_summary(&saved).clone();
    let foreground = fixture.foreground(Behavior::Succeed);
    let compressor = Arc::new(Summarizer::default());
    let agent = fixture.agent(foreground.clone(), compressor);
    // User input is irreducible. Preparation cannot commit its candidate cache,
    // but the already successful summary must survive with the new full history.
    let oversized_query = "Report findings now ".repeat(5_000);
    let result = agent
        .run_checkpointed_with_summary(
            saved.messages,
            Message::user(&oversized_query),
            Some(&sink),
            saved.context_summary,
        )
        .await;
    assert!(matches!(result, Err(AgentError::ContextCapacity { .. })));
    assert_eq!(foreground.calls.load(Ordering::SeqCst), 0);
    let saved = fixture.saved();
    assert_eq!(assert_saved_summary(&saved), &previous);
    assert_eq!(saved.messages.last().unwrap().content, oversized_query);
}

#[tokio::test]
async fn checkpoint_without_summary_retains_fresh_summary_across_system_row_changes() {
    let fixture = Fixture::new();
    let foreground = fixture.foreground(Behavior::Fail);
    let agent = fixture.agent(foreground, Arc::new(Summarizer::default()));
    let sink = fixture.sink();
    let _ = agent
        .run_checkpointed_with_summary(
            fixture.history.clone(),
            Message::user(CURRENT_QUERY),
            Some(&sink),
            None,
        )
        .await;
    let saved = fixture.saved();
    let original = assert_saved_summary(&saved).clone();
    let mut messages = saved.messages;
    // A legacy caller's None must not erase an independently saved summary.
    // Removing the pin changes durable positions without changing the prefix.
    messages.retain(|message| message.role != Role::System);
    let checkpoint = RunCheckpoint {
        plan: None,
        metadata: CheckpointMetadata::default(),
        messages,
        context_summary: None,
        goal: None,
    };
    let ownership = sink.claim().unwrap();
    ownership.save(&checkpoint).unwrap();
    let retained = fixture.saved();
    let summary = assert_saved_summary(&retained);
    assert_eq!(summary.summary, original.summary);
    assert_eq!(summary.fingerprint, original.fingerprint);
    assert_eq!(summary.through + 1, original.through);
}

#[tokio::test]
async fn edited_durable_prefix_is_recompressed_after_summary_index_remapping() {
    let fixture = Fixture::new();
    let foreground = fixture.foreground(Behavior::Fail);
    let agent = fixture.agent(foreground, Arc::new(Summarizer::default()));
    let sink = fixture.sink();
    let _ = agent
        .run_checkpointed_with_summary(
            fixture.history.clone(),
            Message::user(CURRENT_QUERY),
            Some(&sink),
            None,
        )
        .await;
    let mut saved = fixture.saved();
    let original = assert_saved_summary(&saved).clone();
    let prefix_result = saved.messages[..original.through]
        .iter_mut()
        .find(|message| message.role == Role::Tool)
        .unwrap();
    prefix_result.content.push_str(" changed durable result");
    let compressor = Arc::new(Summarizer::default());
    let resumed = fixture.agent(fixture.foreground(Behavior::Succeed), compressor.clone());
    let outcome = resumed
        .run_checkpointed_with_summary(
            saved.messages,
            Message::user("Report the edited result"),
            Some(&sink),
            saved.context_summary,
        )
        .await
        .unwrap();
    assert_eq!(outcome.final_text, "ready");
    assert!(compressor.calls.load(Ordering::SeqCst) > 0);
    assert_ne!(
        assert_saved_summary(&fixture.saved()).fingerprint,
        original.fingerprint
    );
}

fn assert_saved_summary(session: &Session) -> &ContextSummary {
    let summary = session
        .context_summary
        .as_ref()
        .expect("summary must be durable before foreground wait");
    assert!(summary.through <= session.messages.len());
    assert!(!summary.summary.is_empty());
    let mut hasher = Sha256::new();
    for message in session.messages[..summary.through]
        .iter()
        .filter(|message| message.role != Role::System)
    {
        hasher.update(serde_json::to_vec(message).unwrap());
    }
    assert_eq!(summary.fingerprint, format!("{:x}", hasher.finalize()));
    assert_eq!(
        session
            .messages
            .iter()
            .filter(|message| message.role == Role::Tool)
            .count(),
        HISTORY_BATCHES
    );
    assert!(
        session
            .messages
            .iter()
            .any(|message| message.content == CURRENT_QUERY)
    );
    assert!(
        session
            .messages
            .iter()
            .filter(|message| message.role == Role::Tool)
            .all(|message| message.content.len() > TOOL_OUTPUT_BYTES)
    );
    summary
}
