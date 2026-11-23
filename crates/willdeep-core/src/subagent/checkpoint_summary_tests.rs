use super::*;
use crate::checkpoint::{CheckpointStatus, SessionCheckpointSink};
use crate::provider::{Provider, ProviderError};
use crate::types::{Completion, Role, ToolCall, ToolDefinition, Usage};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

const WINDOW: u64 = 12_288;
const HISTORY_BATCHES: usize = 20;
const BATCH_BYTES: usize = 1_900;
const ORIGINAL_CANARY: &str = "WORKER_ORIGINAL_PREFIX_CANARY";

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
    home: PathBuf,
    id: uuid::Uuid,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("worker-summary-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        let home = root.join("workers");
        std::fs::create_dir_all(&workspace).unwrap();
        Self {
            root,
            workspace,
            home,
            id: uuid::Uuid::new_v4(),
        }
    }

    fn store(&self) -> SessionStore {
        SessionStore::new(&self.home)
    }

    fn seed(&self, messages: Vec<Message>) {
        let mut session = Session::new(self.workspace.clone(), Some("reader".into()), "inspect");
        session.id = self.id;
        session.messages = messages;
        self.store().save(&mut session).unwrap();
    }

    fn agent(&self, provider: Arc<dyn Provider>, compressor: Arc<dyn Provider>) -> Agent {
        Agent::new(
            provider,
            crate::ToolRegistry::new(&self.workspace, crate::ApprovalMode::ReadOnly)
                .unwrap()
                .with_allowed_tools(["read_file".to_owned()]),
            crate::AgentConfig {
                max_turns: 1,
                system_prompt: "worker summary regression".into(),
                context_window: WINDOW,
                token_budget: None,
            },
        )
        .with_compressor(compressor, false)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove temporary worker fixture");
    }
}

fn append_batches(messages: &mut Vec<Message>, prefix: &str) {
    for index in 0..HISTORY_BATCHES {
        let call = ToolCall {
            id: format!("{prefix}-{index}"),
            name: "read_file".into(),
            arguments: format!(r#"{{"path":"{prefix}-{index}.rs"}}"#),
        };
        messages.push(Message::assistant("inspect", vec![call.clone()]));
        messages.push(Message::tool(
            &call,
            format!("{prefix}-{index}: {}", "x".repeat(BATCH_BYTES)),
        ));
    }
}

fn history() -> Vec<Message> {
    let mut messages = vec![Message::user(
        "Inspect the fixture and preserve constraints",
    )];
    append_batches(&mut messages, ORIGINAL_CANARY);
    messages.push(Message::user("continue"));
    messages
}

fn completion(content: impl Into<String>) -> Completion {
    Completion {
        reasoning: None,
        content: content.into(),
        tool_calls: Vec::new(),
        finish_reason: Some("stop".into()),
        usage: Some(Usage {
            input_tokens: Some(17),
            output_tokens: Some(3),
            ..Usage::default()
        }),
    }
}

#[derive(Default)]
struct SummaryProvider(Mutex<Vec<Vec<Message>>>);

#[async_trait::async_trait]
impl Provider for SummaryProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        let mut requests = self.0.lock().unwrap();
        requests.push(messages.to_vec());
        Ok(completion(format!("WORKER_SUMMARY_{}", requests.len())))
    }
}

#[derive(Default)]
struct ReportProvider(Mutex<Vec<Vec<Message>>>);

#[async_trait::async_trait]
impl Provider for ReportProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        self.0.lock().unwrap().push(messages.to_vec());
        Ok(completion("report"))
    }
}

#[tokio::test]
async fn recreated_worker_restores_the_saved_summary_without_resummarizing() {
    let fixture = Fixture::new();
    fixture.seed(history());
    let compressor = Arc::new(SummaryProvider::default());
    let provider = Arc::new(ReportProvider::default());
    let first = run(
        &fixture.agent(provider.clone(), compressor.clone()),
        &fixture.home,
        &fixture.workspace,
        fixture.id,
        "reader",
        "inspect".into(),
    )
    .await
    .unwrap();
    let saved = fixture.store().load(fixture.id).unwrap();
    assert!(first.context_summary.is_some());
    assert_eq!(saved.context_summary, first.context_summary);
    let summary_calls = compressor.0.lock().unwrap().len();
    assert!(
        summary_calls > 0,
        "the fixture must exercise automatic compression"
    );
    assert!(
        compressor
            .0
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .any(|message| message.content.contains(ORIGINAL_CANARY))
    );

    // A new Agent and the same durable identity reproduce the checkpoint entry
    // used by a verifier retry, without claiming to exercise runner scheduling.
    let second = run(
        &fixture.agent(provider.clone(), compressor.clone()),
        &fixture.home,
        &fixture.workspace,
        fixture.id,
        "reader",
        "retry with verifier feedback".into(),
    )
    .await
    .unwrap();
    assert_eq!(compressor.0.lock().unwrap().len(), summary_calls);
    assert_eq!(second.context_summary, first.context_summary);
    let saved = fixture.store().load(fixture.id).unwrap();
    assert_eq!(saved.context_summary, second.context_summary);
    assert_eq!(
        serde_json::to_value(&saved.messages).unwrap(),
        serde_json::to_value(&second.messages).unwrap()
    );
    for request in provider.0.lock().unwrap().iter() {
        assert!(
            request
                .iter()
                .any(|message| message.content.contains("<context-summary"))
        );
        assert!(
            !request
                .iter()
                .any(|message| message.content.contains(&format!("{ORIGINAL_CANARY}-0:")))
        );
    }
    let checkpoint = saved.execution_checkpoint.unwrap();
    assert_eq!(checkpoint.status, CheckpointStatus::Completed);
    assert_eq!(checkpoint.input_tokens, second.input_tokens);
    assert_eq!(checkpoint.output_tokens, second.output_tokens);
}

#[tokio::test]
async fn worker_outcome_preserves_summary_anchor_when_notices_are_merged_back() {
    let fixture = Fixture::new();
    let mut messages = history();
    let mut notice = Message::system("external peer notice");
    notice.source = Some(crate::types::MessageSource::FederationNotice);
    messages.insert(2, notice.clone());
    messages.insert(3, notice);
    fixture.seed(messages);
    let compressor = Arc::new(SummaryProvider::default());
    let provider = Arc::new(ReportProvider::default());
    let first = run(
        &fixture.agent(provider.clone(), compressor.clone()),
        &fixture.home,
        &fixture.workspace,
        fixture.id,
        "reader",
        "inspect".into(),
    )
    .await
    .unwrap();
    let saved = fixture.store().load(fixture.id).unwrap();
    let summary = saved.context_summary.as_ref().unwrap();
    assert_eq!(
        summary.through,
        first.context_summary.as_ref().unwrap().through + 2,
        "final outcome save must retain the checkpoint's remapped boundary"
    );
    assert_eq!(
        saved
            .messages
            .iter()
            .filter(|message| crate::checkpoint::is_federation_notice(message))
            .count(),
        2
    );
    let summary_calls = compressor.0.lock().unwrap().len();
    assert!(summary_calls > 0);
    run(
        &fixture.agent(provider, compressor.clone()),
        &fixture.home,
        &fixture.workspace,
        fixture.id,
        "reader",
        "continue after restart".into(),
    )
    .await
    .unwrap();
    assert_eq!(compressor.0.lock().unwrap().len(), summary_calls);
    assert_eq!(
        fixture
            .store()
            .load(fixture.id)
            .unwrap()
            .context_summary
            .as_ref(),
        Some(summary)
    );
}

#[tokio::test]
async fn recreated_worker_updates_the_summary_using_only_the_uncovered_tail() {
    let fixture = Fixture::new();
    fixture.seed(history());
    let compressor = Arc::new(SummaryProvider::default());
    let provider = Arc::new(ReportProvider::default());
    let first = run(
        &fixture.agent(provider.clone(), compressor.clone()),
        &fixture.home,
        &fixture.workspace,
        fixture.id,
        "reader",
        "inspect".into(),
    )
    .await
    .unwrap();
    let previous = first.context_summary.unwrap();
    let covered_outputs: Vec<_> = first.messages[..previous.through]
        .iter()
        .filter(|message| message.role == Role::Tool)
        .map(|message| message.content.clone())
        .collect();
    assert!(!covered_outputs.is_empty());
    let summary_calls = compressor.0.lock().unwrap().len();
    fixture
        .store()
        .try_update(fixture.id, |session| {
            append_batches(&mut session.messages, "NEW_TAIL")
        })
        .unwrap();
    let second = run(
        &fixture.agent(provider, compressor.clone()),
        &fixture.home,
        &fixture.workspace,
        fixture.id,
        "reader",
        "retry after more inspection".into(),
    )
    .await
    .unwrap();
    let updated = second.context_summary.as_ref().unwrap();
    assert!(updated.through > previous.through);
    assert_ne!(updated.summary, previous.summary);
    assert_eq!(
        fixture.store().load(fixture.id).unwrap().context_summary,
        second.context_summary
    );
    let requests = compressor.0.lock().unwrap();
    assert!(
        requests.len() > summary_calls,
        "the grown tail must trigger incremental compression"
    );
    let incremental = &requests[summary_calls..];
    assert!(
        incremental
            .iter()
            .flatten()
            .any(|message| message.content.contains(&previous.summary))
    );
    assert!(
        incremental
            .iter()
            .flatten()
            .any(|message| message.content.contains("NEW_TAIL"))
    );
    assert!(
        !incremental
            .iter()
            .flatten()
            .any(|message| message.content.contains(&format!("{ORIGINAL_CANARY}-0:"))),
        "already summarized original text must not re-enter the summary model"
    );
    for original in &covered_outputs {
        assert!(
            !incremental
                .iter()
                .flatten()
                .any(|message| message.content.contains(original)),
            "every tool output covered by the previous summary must be absent from incremental input"
        );
    }
    let saved = fixture.store().load(fixture.id).unwrap();
    let checkpoint = saved.execution_checkpoint.unwrap();
    assert_eq!(checkpoint.status, CheckpointStatus::Completed);
    assert_eq!(checkpoint.input_tokens, second.input_tokens);
    assert_eq!(checkpoint.output_tokens, second.output_tokens);
    assert!(
        second.input_tokens > 17,
        "final persistence must retain compression usage too"
    );
    assert!(
        saved
            .messages
            .iter()
            .any(|message| message.content.contains(&format!("{ORIGINAL_CANARY}-0:"))),
        "automatic compression must retain the durable original history"
    );
}

struct PausedPlanner {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: tokio::sync::Notify,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Provider for PausedPlanner {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        let turn = self.calls.fetch_add(1, Ordering::SeqCst);
        let started = self.started.lock().unwrap().take();
        if let Some(started) = started {
            started.send(()).unwrap();
            self.release.notified().await;
        }
        let mut response = completion("report");
        if turn == 0 {
            response.tool_calls = vec![ToolCall {
                id: "worker-plan".into(), name: "update_plan".into(),
                arguments: serde_json::json!({"steps": [{"id":"inspect", "title":"Inspect fixture", "status":"done"}]}).to_string(),
            }];
            response.finish_reason = Some("tool_calls".into());
        }
        Ok(response)
    }
}

#[tokio::test]
async fn final_summary_save_preserves_concurrent_metadata_usage_plan_and_ownership() {
    let fixture = Fixture::new();
    fixture.seed(Vec::new());
    let store = fixture.store();
    store
        .update(fixture.id, |session| {
            session.compression_generation = 7;
            session.compression_checkpoint = Some(crate::session::CompressionCheckpoint {
                generation: 7,
                previous_message_count: 20,
                compressed_message_count: 2,
                created_at: 1,
            });
        })
        .unwrap();
    let mut stale_reader = store.load_baselined(fixture.id).unwrap();
    let (started, observed) = tokio::sync::oneshot::channel();
    let provider = Arc::new(PausedPlanner {
        started: Mutex::new(Some(started)),
        release: tokio::sync::Notify::new(),
        calls: AtomicUsize::new(0),
    });
    let agent = Agent::new(
        provider.clone(),
        crate::ToolRegistry::new(&fixture.workspace, crate::ApprovalMode::ReadOnly)
            .unwrap()
            .with_allowed_tools(["update_plan".to_owned()]),
        crate::AgentConfig {
            max_turns: 2,
            system_prompt: String::new(),
            context_window: 32_000,
            token_budget: None,
        },
    );
    let mut execution = Box::pin(run(
        &agent,
        &fixture.home,
        &fixture.workspace,
        fixture.id,
        "reader",
        "inspect".into(),
    ));
    tokio::select! {
        result = &mut execution => panic!("worker completed before release: {result:?}"),
        result = tokio::time::timeout(std::time::Duration::from_secs(3), observed) => { result.unwrap().unwrap(); }
    }
    assert!(matches!(
        store.try_update(fixture.id, |_| {}),
        Err(crate::session::SessionError::ConcurrentUpdate(
            "active execution"
        ))
    ));
    assert!(matches!(
        SessionCheckpointSink {
            store: store.clone(),
            session_id: fixture.id
        }
        .claim(),
        Err(crate::session::SessionError::ConcurrentUpdate(
            "active execution"
        ))
    ));
    let contender = run(
        &agent,
        &fixture.home,
        &fixture.workspace,
        fixture.id,
        "reader",
        "must not start".into(),
    )
    .await;
    assert!(matches!(contender, Err(AgentError::Checkpoint(_))));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    stale_reader.title = "externally renamed".into();
    stale_reader.title_source = crate::session_title::TitleSource::User;
    stale_reader.pinned_at = Some(42);
    stale_reader.runtime_event_cursor = 99;
    stale_reader.runtime_managed = true;
    stale_reader.attention_read.insert("external-read".into());
    let external_report = uuid::Uuid::new_v4();
    stale_reader.agent_reports_received.insert(external_report);
    store.save(&mut stale_reader).unwrap();
    store
        .record_compression_usage(
            fixture.id,
            &Usage {
                input_tokens: Some(101),
                output_tokens: Some(19),
                ..Usage::default()
            },
        )
        .unwrap();
    store
        .update(fixture.id, |session| {
            let mut notice = Message::system("external federation notice");
            notice.source = Some(crate::types::MessageSource::FederationNotice);
            session.messages.push(notice);
        })
        .unwrap();
    provider.release.notify_one();
    let outcome = execution.await.unwrap();
    let saved = store.load(fixture.id).unwrap();
    assert_eq!(saved.title, "externally renamed");
    assert_eq!(saved.title_source, crate::session_title::TitleSource::User);
    assert_eq!(saved.pinned_at, Some(42));
    assert_eq!(saved.runtime_event_cursor, 99);
    assert!(saved.runtime_managed);
    assert!(saved.attention_read.contains("external-read"));
    assert!(saved.agent_reports_received.contains(&external_report));
    assert_eq!(saved.manual_compression_usage.reported_calls, 1);
    assert_eq!(saved.manual_compression_usage.input_tokens, 101);
    assert_eq!(saved.manual_compression_usage.output_tokens, 19);
    assert_eq!(saved.compression_generation, 7);
    assert_eq!(saved.compression_checkpoint.unwrap().generation, 7);
    let plan = saved.current_plan.unwrap();
    assert_eq!(plan.steps[0].id, "inspect");
    assert_eq!(plan.steps[0].status, crate::conversation::StepStatus::Done);
    let checkpoint = saved.execution_checkpoint.unwrap();
    assert_eq!(checkpoint.status, CheckpointStatus::Completed);
    assert_eq!(checkpoint.turns, 2);
    assert_eq!(checkpoint.input_tokens, outcome.input_tokens);
    assert_eq!(checkpoint.output_tokens, outcome.output_tokens);
    assert_eq!(checkpoint.input_tokens, 34);
    assert_eq!(checkpoint.output_tokens, 6);
    assert!(checkpoint.pending_call_ids.is_empty());
    assert_eq!(saved.context_summary, outcome.context_summary);
    assert!(
        saved
            .messages
            .iter()
            .any(|message| message.role == Role::User && message.content == "inspect")
    );
    assert!(
        !saved
            .messages
            .iter()
            .any(|message| message.content == "must not start")
    );
    assert_eq!(
        saved
            .messages
            .iter()
            .filter(|message| crate::checkpoint::is_federation_notice(message))
            .count(),
        1
    );
    drop(
        SessionCheckpointSink {
            store: store.clone(),
            session_id: fixture.id,
        }
        .claim()
        .unwrap(),
    );
    store.try_update(fixture.id, |_| {}).unwrap();
}
