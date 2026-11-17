use super::*;
use std::sync::Mutex;
use willdeep_core::checkpoint::{CheckpointMetadata, CheckpointStatus, VerificationEvidence};
use willdeep_core::provider::{Provider, ProviderError};
use willdeep_core::types::{Completion, Message, ToolDefinition};
use willdeep_core::{Session, SessionStore};

#[derive(Default)]
pub(crate) struct AnswerProvider {
    pub(crate) requests: Mutex<Vec<Vec<Message>>>,
}

#[async_trait::async_trait]
impl Provider for AnswerProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> std::result::Result<Completion, ProviderError> {
        self.requests.lock().unwrap().push(messages.to_vec());
        Ok(Completion {
            reasoning: None,
            content: "Offline identity answer".into(),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".into()),
            usage: None,
        })
    }
}

pub(crate) struct InputFixture {
    pub(crate) root: PathBuf,
    pub(crate) store: SessionStore,
    pub(crate) session: Session,
    pub(crate) provider: Arc<AnswerProvider>,
    pub(crate) agent: Arc<Agent>,
    goal: Arc<willdeep_core::GoalContinuation>,
}

impl InputFixture {
    pub(crate) fn new() -> Self {
        let root = std::env::temp_dir().join(format!("host-input-scope-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let store = SessionStore::new(&root);
        let mut session = Session::new(workspace.clone(), None, "release fixture");
        session.messages = vec![
            Message::user("release fixture"),
            Message::assistant("Release is partial and unverified", Vec::new()),
            Message::user("persisted recovery input"),
        ];
        session.execution_checkpoint = Some(CheckpointMetadata {
            status: CheckpointStatus::Partial,
            verification_baseline: Some("before-release".into()),
            required_verifications: vec!["rake test".into()],
            verification_evidence: vec![VerificationEvidence {
                snapshot_id: Some("current".into()),
                command: "rake test".into(),
                status: willdeep_core::tools::VerificationStatus::Failed,
            }],
            ..Default::default()
        });
        store.save(&mut session).unwrap();
        let provider = Arc::new(AnswerProvider::default());
        let goal = Arc::new(willdeep_core::GoalContinuation::new());
        let agent = Arc::new(
            Agent::new(
                provider.clone(),
                ToolRegistry::new(&workspace, ApprovalMode::ReadOnly)
                    .unwrap()
                    .with_verification_snapshot(|| Some("current".into()))
                    .with_verification_reporter(|_| {}),
                AgentConfig {
                    max_turns: 5,
                    system_prompt: "offline fixture".into(),
                    context_window: 128_000,
                    token_budget: None,
                },
            )
            .with_goal_continuation(goal.clone()),
        );
        Self {
            root,
            store,
            session,
            provider,
            agent,
            goal,
        }
    }

    async fn harness(&self) -> BuiltHarness {
        // Construct only the local host services. All model execution is replaced
        // by the synthetic provider; Anthropic construction does no discovery IO.
        let config = self.root.join("config.toml");
        std::fs::write(&config, "version = 1\n[agent]\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = false\nsafety_judge = false\n").unwrap();
        let mut cli = <Cli as clap::Parser>::try_parse_from(["willdeep"]).unwrap();
        cli.provider = Some(crate::ProviderArg::Anthropic);
        cli.api = Some(ApiArg::AnthropicMessages);
        cli.api_base = Some("http://127.0.0.1:1/v1".into());
        cli.api_key = Some("test-only".into());
        cli.model = Some("offline-fixture".into());
        cli.config = Some(config.clone());
        cli.workspace = Some(self.session.workspace.clone());
        let loaded = LoadedConfig::load(Some(&config)).unwrap();
        let (tx, _) = tokio::sync::mpsc::unbounded_channel();
        let mut built = build(
            &cli,
            &loaded,
            &self.root,
            Language::En,
            Some(&self.session),
            HarnessFrontend::Tui { tx },
        )
        .await
        .unwrap();
        built.agent = self.agent.clone();
        built.goal = self.goal.clone();
        built
    }
}

impl Drop for InputFixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[tokio::test]
async fn explicit_task_continuation_can_complete_with_current_required_evidence() {
    let mut fixture = InputFixture::new();
    fixture
        .session
        .execution_checkpoint
        .as_mut()
        .unwrap()
        .verification_evidence[0]
        .status = willdeep_core::tools::VerificationStatus::Passed;
    fixture.store.save(&mut fixture.session).unwrap();
    let built = fixture.harness().await;
    let outcome = execute_noninteractive(
        &built,
        &fixture.store,
        &mut fixture.session,
        "你是谁".into(),
        Vec::new(),
        Language::En,
        ExecutionOptions {
            continue_task: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        outcome.stop_reason,
        willdeep_core::AgentStopReason::Finished
    );
    assert_eq!(outcome.turns, 1);
    let saved = fixture.store.load(fixture.session.id).unwrap();
    let completed = saved.execution_checkpoint.unwrap();
    assert_eq!(completed.status, CheckpointStatus::Completed);
    assert_eq!(
        completed.verification_baseline.as_deref(),
        Some("before-release")
    );
    assert_eq!(completed.required_verifications, ["rake test"]);
}

#[tokio::test]
async fn operator_input_answers_once_but_replay_host_and_goal_retain_verification_debt() {
    for mode in ["operator", "continue", "replay", "host", "goal"] {
        let mut fixture = InputFixture::new();
        let mut built = fixture.harness().await;
        built.host_input = mode == "host";
        if mode == "goal" {
            built
                .goal
                .activate("release fixture", willdeep_core::GoalBudget::default());
        }
        let history_len = fixture.session.messages.len();
        let outcome = execute_noninteractive(
            &built,
            &fixture.store,
            &mut fixture.session,
            "你是谁".into(),
            Vec::new(),
            Language::En,
            ExecutionOptions {
                replay_existing_user_message: mode == "replay",
                continue_task: mode == "continue",
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let requests = fixture.provider.requests.lock().unwrap();
        if mode == "operator" {
            assert_eq!(
                outcome.stop_reason,
                willdeep_core::AgentStopReason::Finished
            );
            assert_eq!(outcome.final_text, "Offline identity answer");
            assert_eq!(outcome.turns, 1);
            assert_eq!(requests.len(), 1);
            assert!(
                requests[0]
                    .iter()
                    .any(|message| message.content.starts_with("[execution-context]"))
            );
            assert!(
                !requests[0]
                    .iter()
                    .any(|message| message.content.starts_with("[execution-recovery]"))
            );
            assert!(
                fixture.session.messages[history_len..]
                    .iter()
                    .all(|message| {
                        message.tool_calls.is_empty()
                            && message.role != willdeep_core::types::Role::Tool
                            && !message
                                .content
                                .contains("[completion-verification-required]")
                    })
            );
        } else {
            assert_eq!(
                outcome.stop_reason,
                willdeep_core::AgentStopReason::Unverified,
                "{mode}"
            );
            assert_eq!(outcome.turns, 3, "{mode}");
            assert_eq!(requests.len(), 3, "{mode}");
            if mode == "continue" {
                assert!(
                    requests[0]
                        .iter()
                        .any(|message| message.content.starts_with("[execution-recovery]"))
                );
                let input = requests[0]
                    .iter()
                    .rev()
                    .find(|message| message.content == "你是谁")
                    .unwrap();
                assert_eq!(
                    input.source,
                    Some(willdeep_core::types::MessageSource::OperatorInput)
                );
            }
        }
        let saved = fixture.store.load(fixture.session.id).unwrap();
        let retained = saved.execution_checkpoint.unwrap();
        assert_eq!(retained.status, CheckpointStatus::Partial, "{mode}");
        assert_eq!(
            retained.verification_baseline.as_deref(),
            Some("before-release"),
            "{mode}"
        );
        assert_eq!(retained.required_verifications, ["rake test"], "{mode}");
        assert_eq!(
            retained.verification_evidence[0].status,
            willdeep_core::tools::VerificationStatus::Failed,
            "{mode}"
        );
    }
}
