use super::*;
use std::sync::Mutex;
use willdeep_core::checkpoint::{ClaimedSessionCheckpointSink, SessionCheckpointSink};
use willdeep_core::context_summary::ContextSummary;
use willdeep_core::provider::{Provider, ProviderError};
use willdeep_core::types::{Completion, MessageSource, Role, ToolCall, ToolDefinition};
use willdeep_core::{AgentConfig, AgentOutcome, ApprovalMode, ToolRegistry};

const WINDOW: u64 = 12_288;
const HISTORY_BATCHES: usize = 20;
const TOOL_OUTPUT_BYTES: usize = 1_900;
const FINISHED_TIMEOUT: Duration = Duration::from_secs(10);
const REWRITTEN_PREFIX: &str = "REWRITTEN_PREFIX_CANARY";

enum ReplyKind {
    Main,
    Summary,
}

struct RecordingProvider {
    kind: ReplyKind,
    requests: Mutex<Vec<Vec<Message>>>,
}

impl RecordingProvider {
    fn new(kind: ReplyKind) -> Arc<Self> {
        Arc::new(Self {
            kind,
            requests: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[async_trait]
impl Provider for RecordingProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> std::result::Result<Completion, ProviderError> {
        let call = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(messages.to_vec());
            requests.len()
        };
        let content = match self.kind {
            ReplyKind::Main => "done".to_owned(),
            ReplyKind::Summary => format!(
                "OBJECTIVE: inspect fixture. CONSTRAINTS: preserve input. COMPLETED: inspected. REMAINING: validate. SUMMARY_{call}"
            ),
        };
        Ok(Completion {
            reasoning: None,
            content,
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_owned()),
            usage: None,
        })
    }
}

fn long_history() -> Vec<Message> {
    let mut messages = vec![
        Message::system("fixture rules"),
        Message::user("Inspect the fixture and preserve my changes"),
    ];
    for index in 0..HISTORY_BATCHES {
        let call = ToolCall {
            id: format!("history-read-{index}"),
            name: "read_file".to_owned(),
            arguments: format!(r#"{{"path":"file-{index}.rs"}}"#),
        };
        messages.push(Message::assistant("inspect", vec![call.clone()]));
        messages.push(Message::tool(
            &call,
            format!("file-{index}: {}", "x".repeat(TOOL_OUTPUT_BYTES)),
        ));
    }
    messages.push(Message::user("continue with the original constraints"));
    messages
}

struct Fixture {
    root: PathBuf,
    session: Session,
    store: SessionStore,
    app: App,
    runtime: TuiRuntime,
    agent: Arc<Agent>,
    main: Arc<RecordingProvider>,
    compressor: Arc<RecordingProvider>,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "willdeep-tui-local-summary-{}",
            uuid::Uuid::new_v4()
        ));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let store = SessionStore::new(&root);
        let mut session = Session::new(workspace.clone(), None, "fixture");
        session.messages = long_history();
        store.save(&mut session).unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        let main = RecordingProvider::new(ReplyKind::Main);
        let compressor = RecordingProvider::new(ReplyKind::Summary);
        let tools = ToolRegistry::new(&workspace, ApprovalMode::Strict)
            .unwrap()
            .with_allowed_tools(["read_file".to_owned()]);
        let agent = Arc::new(
            Agent::new(
                main.clone(),
                tools,
                AgentConfig {
                    max_turns: 3,
                    system_prompt: "fixture rules".to_owned(),
                    context_window: WINDOW,
                    token_budget: None,
                },
            )
            .with_compressor(compressor.clone(), false)
            .with_compressor_window(WINDOW)
            .with_event_sink(Arc::new(TuiSink { ui: tx.clone() })),
        );
        let runtime = TuiRuntime {
            home: root.clone(),
            notifier: crate::notify::Notifier::disabled(),
            skills: Arc::new(SkillCatalog::default()),
            kernel: willdeep_core::EventKernel::new(),
            kernel_store: willdeep_core::kernel_store::KernelStore::new(&root),
            detached_jobs: willdeep_core::DetachedJobStore::new(&root),
            context_window: WINDOW,
            background_tasks: Arc::new(BackgroundTaskRegistry::default()),
            runtime_submit: crate::daemon::RuntimeSubmitOptions {
                workspace: workspace.clone(),
                profile: None,
                model: None,
                // Explicit missing fixture path keeps goal-budget fallback local.
                config: Some(root.join("unused-config.toml")),
            },
            // dispatch_prompt uses the mock Agent above; this is never requested.
            provider_config: willdeep_core::provider::ProviderConfig::new(
                willdeep_core::provider::ProviderKind::OpenAiCompatible,
                willdeep_core::provider::ApiDialect::ChatCompletions,
                "http://127.0.0.1:1/v1",
                "test-only",
                "fixture-model",
            ),
            reasoning_level: None,
            local_workspace: workspace,
            tx,
            rx,
        };
        Self {
            root,
            session,
            store,
            app: App::new(Vec::new(), Language::En),
            runtime,
            agent,
            main,
            compressor,
        }
    }

    fn dispatch_local(&mut self, text: &str) {
        let PromptExecution::Local(prompt) = prompt_execution(&format!("/local {text}")) else {
            panic!("explicit /local must reach dispatch_prompt");
        };
        dispatch_prompt(
            &mut self.app,
            &mut self.session,
            &self.store,
            &self.runtime,
            &self.agent,
            &self.runtime.tx,
            prompt,
        )
        .unwrap();
    }

    fn dispatch_notice(&mut self, text: &str) {
        dispatch::dispatch_notification(
            &mut self.app,
            &mut self.session,
            &self.store,
            &self.agent,
            &self.runtime.tx,
            text.to_owned(),
        )
        .unwrap();
    }

    async fn receive_finished(&mut self) -> (AgentOutcome, ClaimedSessionCheckpointSink) {
        let result = tokio::time::timeout(FINISHED_TIMEOUT, async {
            loop {
                match self.runtime.rx.recv().await.expect("Finished channel open") {
                    UiMessage::Finished(result, ownership) => return (result.unwrap(), ownership),
                    UiMessage::Agent(_) => {}
                    _ => panic!("unexpected callback during local fixture execution"),
                }
            }
        })
        .await
        .expect("mock execution must finish without model requests");
        self.app.local_turn.take().unwrap().await.unwrap();
        result
    }

    fn assert_owned(&self) {
        assert!(matches!(
            (SessionCheckpointSink {
                store: self.store.clone(),
                session_id: self.session.id,
            })
            .claim(),
            Err(willdeep_core::session::SessionError::ConcurrentUpdate(
                "active execution"
            ))
        ));
    }

    fn persist_received(
        &mut self,
        outcome: AgentOutcome,
        ownership: ClaimedSessionCheckpointSink,
    ) -> Option<ContextSummary> {
        let expected_messages = serde_json::to_value(&outcome.messages).unwrap();
        let summary = outcome.context_summary.clone();
        self.assert_owned();
        persist_finished_outcome(&mut self.session, &self.store, outcome).unwrap();
        self.assert_owned();
        let saved = self.store.load(self.session.id).unwrap();
        assert_eq!(
            serde_json::to_value(&saved.messages).unwrap(),
            expected_messages
        );
        assert_eq!(
            serde_json::to_value(&self.session.messages).unwrap(),
            expected_messages
        );
        assert_eq!(saved.context_summary, summary);
        assert_eq!(self.session.context_summary, summary);
        assert_eq!(
            saved.execution_checkpoint.as_ref().unwrap().status,
            willdeep_core::checkpoint::CheckpointStatus::Completed,
            "refresh_execution must retain the real run checkpoint before adopting outcome"
        );
        drop(ownership);
        self.app.finish_turn();
        summary
    }

    async fn finish(&mut self) -> Option<ContextSummary> {
        let (outcome, ownership) = self.receive_finished().await;
        self.persist_received(outcome, ownership)
    }

    async fn seed_real_summary(&mut self) -> ContextSummary {
        let outcome = self
            .agent
            .run_checkpointed_with_summary(
                self.session.messages.clone(),
                Message::user("seed the persisted summary"),
                None,
                None,
            )
            .await
            .unwrap();
        let summary = outcome.context_summary.clone().expect("fixture compresses");
        persist_finished_outcome(&mut self.session, &self.store, outcome).unwrap();
        // Drop the seed run's Agent events; dispatches below must produce their own Finished.
        while self.runtime.rx.try_recv().is_ok() {}
        summary
    }

    fn rewrite_covered_prefix(&mut self, summary: &ContextSummary) {
        let message = self.session.messages[..summary.through]
            .iter_mut()
            .find(|message| message.role == Role::Assistant)
            .expect("summary covers assistant history");
        message.content = REWRITTEN_PREFIX.to_owned();
        self.store.save(&mut self.session).unwrap();
    }

    fn assert_last_request_uses(&self, summary: &ContextSummary) {
        assert!(
            self.main
                .requests
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .iter()
                .any(|message| {
                    message.content.contains("<context-summary")
                        && message.content.contains(&summary.summary)
                })
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove isolated TUI summary fixture");
    }
}

#[tokio::test]
async fn local_prompt_finished_persists_summary_and_reuses_it_on_next_turn() {
    let mut fixture = Fixture::new();
    assert!(matches!(
        prompt_execution("ordinary input"),
        PromptExecution::Runtime(_)
    ));
    assert!(fixture.session.context_summary.is_none());
    fixture.dispatch_local("continue");
    let first = fixture
        .finish()
        .await
        .expect("first local run creates summary");
    let calls = fixture.compressor.calls();
    assert!(calls > 0);
    fixture.assert_last_request_uses(&first);

    fixture.session = fixture.store.load_baselined(fixture.session.id).unwrap();
    fixture.dispatch_local("continue again");
    let second = fixture.finish().await.unwrap();
    assert_eq!(second, first);
    assert_eq!(fixture.main.calls(), 2);
    assert_eq!(
        fixture.compressor.calls(),
        calls,
        "second local turn must reuse the disk summary"
    );
    fixture.assert_last_request_uses(&second);
}

#[tokio::test]
async fn notification_finished_persists_summary_for_the_next_local_prompt() {
    let mut fixture = Fixture::new();
    let notice = "background fixture finished";
    fixture.dispatch_notice(notice);
    let first = fixture
        .finish()
        .await
        .expect("notification creates summary");
    let calls = fixture.compressor.calls();
    assert!(calls > 0);
    assert!(fixture.session.messages.iter().any(|message| {
        message.source == Some(MessageSource::HostInstruction) && message.content == notice
    }));

    fixture.session = fixture.store.load_baselined(fixture.session.id).unwrap();
    fixture.dispatch_local("continue after the notification");
    let second = fixture.finish().await.unwrap();
    assert_eq!(second, first);
    assert_eq!(fixture.compressor.calls(), calls);
    fixture.assert_last_request_uses(&second);
}

#[tokio::test]
async fn finished_replaces_stale_summary_after_a_covered_prefix_changes() {
    let mut fixture = Fixture::new();
    let old = fixture.seed_real_summary().await;
    let calls = fixture.compressor.calls();
    fixture.rewrite_covered_prefix(&old);
    assert_eq!(fixture.session.context_summary.as_ref(), Some(&old));
    fixture.dispatch_local("continue after the edit");
    let new = fixture
        .finish()
        .await
        .expect("edited history is summarized again");
    assert_ne!(new.fingerprint, old.fingerprint);
    assert_ne!(new.summary, old.summary);
    assert!(fixture.compressor.calls() > calls);
    assert!(
        fixture.compressor.requests.lock().unwrap()[calls..]
            .iter()
            .flatten()
            .any(|message| message.content.contains(REWRITTEN_PREFIX))
    );

    let calls = fixture.compressor.calls();
    fixture.session = fixture.store.load_baselined(fixture.session.id).unwrap();
    fixture.dispatch_local("reuse the updated summary");
    assert_eq!(fixture.finish().await.as_ref(), Some(&new));
    assert_eq!(fixture.compressor.calls(), calls);
}

#[tokio::test]
async fn notification_reuses_summary_persisted_by_local_finished() {
    let mut fixture = Fixture::new();
    fixture.dispatch_local("continue before the background result");
    let summary = fixture.finish().await.expect("local run creates summary");
    let calls = fixture.compressor.calls();
    assert!(calls > 0);

    fixture.session = fixture.store.load_baselined(fixture.session.id).unwrap();
    fixture.dispatch_notice("background fixture finished");
    assert_eq!(fixture.finish().await.as_ref(), Some(&summary));
    assert_eq!(fixture.compressor.calls(), calls);
    fixture.assert_last_request_uses(&summary);
}

#[tokio::test]
async fn finished_clears_a_concurrent_old_summary_when_the_real_run_returns_none() {
    let mut fixture = Fixture::new();
    let old = fixture.seed_real_summary().await;
    fixture.session.messages = vec![
        Message::system("fixture rules"),
        Message::user("Continue with this short replacement history"),
    ];
    fixture.session.context_summary = None;
    fixture.store.save(&mut fixture.session).unwrap();
    let calls = fixture.compressor.calls();
    fixture.dispatch_local("continue after the history replacement");
    let (outcome, ownership) = fixture.receive_finished().await;
    assert!(outcome.context_summary.is_none());
    assert_eq!(fixture.compressor.calls(), calls);
    // A concurrent writer restores a stale cache before Finished is handled.
    // The callback must adopt the outcome's None after refreshing that snapshot.
    fixture
        .store
        .update(fixture.session.id, |latest| {
            latest.context_summary = Some(old.clone());
        })
        .unwrap();
    fixture
        .store
        .refresh_execution(&mut fixture.session)
        .unwrap();
    assert_eq!(fixture.session.context_summary.as_ref(), Some(&old));
    assert!(fixture.persist_received(outcome, ownership).is_none());
    assert!(
        fixture
            .store
            .load(fixture.session.id)
            .unwrap()
            .context_summary
            .is_none()
    );
}

#[tokio::test]
async fn concurrent_turn_save_keeps_metadata_and_the_complete_execution_snapshot() {
    let mut fixture = Fixture::new();
    let old = fixture.seed_real_summary().await;
    let stale = fixture.session.clone();
    fixture.rewrite_covered_prefix(&old);
    fixture.dispatch_local("continue after the edit");
    let (outcome, ownership) = fixture.receive_finished().await;
    let new = outcome.context_summary.as_ref().unwrap();
    assert_ne!(new.fingerprint, old.fingerprint);

    // Reproduce a writer arriving between the callback's refresh and its save.
    // Both summaries come from real runs, bound to their respective message prefixes.
    fixture
        .store
        .refresh_execution(&mut fixture.session)
        .unwrap();
    fixture.session.messages = outcome.messages;
    fixture.session.context_summary = outcome.context_summary;
    fixture.session.compression_generation += 1;
    fixture.session.compression_checkpoint = Some(willdeep_core::session::CompressionCheckpoint {
        generation: fixture.session.compression_generation,
        previous_message_count: fixture.session.messages.len() * 2,
        compressed_message_count: fixture.session.messages.len(),
        created_at: fixture.session.updated_at,
    });
    fixture.session.current_plan = Some(willdeep_core::conversation::Plan {
        summary: "authoritative execution plan".to_owned(),
        steps: vec![willdeep_core::conversation::PlanStep {
            id: "verify-snapshot".to_owned(),
            text: "Verify the authoritative snapshot".to_owned(),
            status: willdeep_core::conversation::StepStatus::Done,
            detail: None,
            depends_on: Vec::new(),
            job_id: None,
        }],
    });
    let authoritative = fixture.session.clone();
    assert_ne!(
        authoritative.compression_generation,
        stale.compression_generation
    );
    assert_ne!(
        authoritative.compression_checkpoint,
        stale.compression_checkpoint
    );
    assert_ne!(authoritative.current_plan, stale.current_plan);
    fixture
        .store
        .update(fixture.session.id, |latest| {
            latest.adopt_execution_snapshot(&stale);
            latest.title = "concurrently renamed".to_owned();
            latest.title_source = willdeep_core::TitleSource::User;
            latest.model = Some("concurrent-model".to_owned());
            latest.profile = Some("concurrent-profile".to_owned());
            latest.pinned_at = Some(123);
            latest.runtime_managed = true;
            latest.runtime_event_cursor = 91;
            latest
                .attention_read
                .insert("concurrent-attention".to_owned());
        })
        .unwrap();
    let mut probe = fixture.session.clone();
    assert!(
        matches!(
            fixture.store.save(&mut probe),
            Err(willdeep_core::session::SessionError::ConcurrentUpdate(
                "execution"
            ))
        ),
        "fixture must reach the conflict fallback"
    );
    fixture.assert_owned();
    persist_turn_result(&mut fixture.session, &fixture.store).unwrap();
    fixture.assert_owned();
    let saved = fixture.store.load(fixture.session.id).unwrap();
    assert_eq!(
        serde_json::to_value(&saved.messages).unwrap(),
        serde_json::to_value(&authoritative.messages).unwrap()
    );
    assert_eq!(saved.context_summary, authoritative.context_summary);
    assert_ne!(saved.context_summary.as_ref(), Some(&old));
    assert_eq!(
        serde_json::to_value(&saved.execution_checkpoint).unwrap(),
        serde_json::to_value(&authoritative.execution_checkpoint).unwrap()
    );
    assert_eq!(
        saved.compression_generation,
        authoritative.compression_generation
    );
    assert_eq!(
        saved.compression_checkpoint,
        authoritative.compression_checkpoint
    );
    assert_eq!(saved.current_plan, authoritative.current_plan);
    assert_eq!(saved.title, "concurrently renamed");
    assert_eq!(saved.title_source, willdeep_core::TitleSource::User);
    assert_eq!(saved.model.as_deref(), Some("concurrent-model"));
    assert_eq!(saved.profile.as_deref(), Some("concurrent-profile"));
    assert_eq!(saved.pinned_at, Some(123));
    assert!(saved.runtime_managed);
    assert_eq!(saved.runtime_event_cursor, 91);
    assert!(saved.attention_read.contains("concurrent-attention"));
    drop(ownership);
    fixture.app.finish_turn();

    // A real follow-up validates that the stored summary still matches the messages.
    let calls = fixture.compressor.calls();
    fixture.session = fixture.store.load_baselined(fixture.session.id).unwrap();
    fixture.dispatch_local("reuse the snapshot after the conflict");
    assert_eq!(fixture.finish().await, authoritative.context_summary);
    assert_eq!(fixture.compressor.calls(), calls);
}
