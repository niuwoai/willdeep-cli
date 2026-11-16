use super::*;
use crate::checkpoint::{CheckpointStatus, SessionCheckpointSink};
use crate::provider::{Provider, ProviderError};
use crate::subagent::test_support::CaptureSink;
use crate::types::{Completion, Message, ToolCall, ToolDefinition, Usage};
use crate::{Session, SessionStore};

const WINDOW: u64 = 12_288;
const ORIGINAL_CANARY: &str = "RUNNER_OLD_PREFIX_CANARY";
const HISTORY_BATCHES: usize = 20;
const BATCH_BYTES: usize = 1_900;

struct RetryProvider {
    home: PathBuf,
    id: uuid::Uuid,
    marker: PathBuf,
    summaries: Mutex<Vec<Vec<Message>>>,
    attempts: Mutex<Vec<(usize, Vec<Message>)>>,
}

#[async_trait::async_trait]
impl Provider for RetryProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        let content = if messages.len() == 1
            && messages[0]
                .content
                .starts_with("Summarize this older coding-agent conversation")
        {
            let mut summaries = self.summaries.lock().unwrap();
            summaries.push(messages.to_vec());
            format!("RUNNER_SUMMARY_{}", summaries.len())
        } else {
            let mut attempts = self.attempts.lock().unwrap();
            let calls = self.summaries.lock().unwrap().len();
            let store = SessionStore::new(&self.home);
            let saved = store.load(self.id).unwrap();
            assert_eq!(
                saved.execution_checkpoint.as_ref().unwrap().status,
                CheckpointStatus::Running
            );
            assert!(matches!(
                SessionCheckpointSink {
                    store,
                    session_id: self.id
                }
                .claim(),
                Err(crate::session::SessionError::ConcurrentUpdate(
                    "active execution"
                ))
            ));
            if !attempts.is_empty() {
                let previous = saved.context_summary.as_ref().expect(
                    "the first attempt must durably save its summary before verifier retry",
                );
                assert!(
                    messages
                        .iter()
                        .any(|message| message.content.contains(&previous.summary))
                );
                assert!(messages.iter().any(|message| {
                    message.content.contains("<verifier-failure attempt=\"1\"")
                }));
                assert_eq!(
                    calls, attempts[0].0,
                    "the actual verifier retry must reuse the stored summary"
                );
                std::fs::write(&self.marker, "fixed on retry").unwrap();
            } else {
                assert!(!self.marker.exists());
            }
            attempts.push((calls, messages.to_vec()));
            "worker report".into()
        };
        Ok(Completion {
            reasoning: None,
            content,
            tool_calls: Vec::new(),
            finish_reason: Some("stop".into()),
            usage: Some(Usage {
                input_tokens: Some(17),
                output_tokens: Some(3),
                ..Usage::default()
            }),
        })
    }
}

#[tokio::test]
async fn verifier_failure_retries_the_same_worker_with_its_persisted_summary() {
    let root = std::env::temp_dir().join(format!("runner-summary-retry-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    let home = root.join("workers");
    std::fs::create_dir_all(&workspace).unwrap();
    let id = uuid::Uuid::new_v4();
    let provider = Arc::new(RetryProvider {
        home: home.clone(),
        id,
        marker: workspace.join("ready.txt"),
        summaries: Mutex::new(Vec::new()),
        attempts: Mutex::new(Vec::new()),
    });
    let mut profile = super::super::builtin_profiles(provider.clone())
        .into_iter()
        .find(|profile| profile.id == "scout")
        .unwrap();
    profile.context_window = WINDOW;
    profile.max_turns = 1;
    profile.timeout_seconds = None;
    profile.tool_names = vec!["read_file".into()];
    let mut session = Session::new(workspace.clone(), Some(profile.id.clone()), "inspect");
    session.id = id;
    session.messages.push(Message::user("Inspect the fixture"));
    for index in 0..HISTORY_BATCHES {
        let call = ToolCall {
            id: format!("old-{index}"),
            name: "read_file".into(),
            arguments: format!(r#"{{"path":"old-{index}.rs"}}"#),
        };
        session
            .messages
            .push(Message::assistant("inspect", vec![call.clone()]));
        session.messages.push(Message::tool(
            &call,
            format!("{ORIGINAL_CANARY}-{index}: {}", "x".repeat(BATCH_BYTES)),
        ));
    }
    session.messages.push(Message::user("continue"));
    let store = SessionStore::new(&home);
    store.save(&mut session).unwrap();
    let sink = Arc::new(CaptureSink::default());
    let report = run_subagent(
        SubagentRun {
            isolated_evaluation: true,
            offline_review_json: false,
            structured_output: None,
            execution_deadline: None,
            workspace: workspace.clone(),
            sandbox: crate::sandbox::SandboxSpec::new(crate::sandbox::SandboxPolicy::Off, []),
            profile,
            prompt: "inspect and report".into(),
            task: None,
            approved_targets: None,
            verifier: Some(TaskVerifier {
                command: "test -f ready.txt".into(),
                expected_exit_code: Some(0),
            }),
            max_attempts: 2,
            claimed_files: Arc::new(Mutex::new(BTreeSet::new())),
            skills: None,
            global_rules: None,
            safety_judge: None,
            approved_command: None,
            mcp: None,
            always_allow_store: None,
            state_home: Some(home),
            parent_approval_mode: None,
            usage_ledger: None,
            feedback: None,
            board: None,
            goal: None,
        },
        sink.clone(),
        id,
        None,
    )
    .await
    .unwrap();
    assert!(report.contains("verdict=\"passed\"") && report.contains("attempts=2"));
    assert!(provider.marker.exists());
    let attempts = provider.attempts.lock().unwrap();
    assert_eq!(
        attempts.len(),
        2,
        "the real verifier must fail once and trigger exactly one retry"
    );
    assert!(
        attempts[0].0 > 0,
        "the first attempt must compress its large history"
    );
    assert_eq!(attempts[0].0, attempts[1].0);
    for (_, request) in attempts.iter() {
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
    let saved = store.load(id).unwrap();
    assert!(saved.context_summary.is_some());
    let checkpoint = saved.execution_checkpoint.unwrap();
    assert_eq!(checkpoint.status, CheckpointStatus::Completed);
    assert_eq!(checkpoint.input_tokens, 17);
    assert_eq!(checkpoint.output_tokens, 3);
    assert!(
        saved
            .messages
            .iter()
            .any(|message| message.content.contains("<verifier-failure attempt=\"1\""))
    );
    assert!(sink.0.lock().unwrap().iter().any(|event| matches!(event,
        AgentEvent::SubagentVerdict { id: child, verifier_passed: Some(true), attempts: 2, .. } if *child == id)));
    drop(
        SessionCheckpointSink {
            store,
            session_id: id,
        }
        .claim()
        .unwrap(),
    );
    std::fs::remove_dir_all(root).unwrap();
}
