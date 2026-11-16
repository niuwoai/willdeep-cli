use super::*;
use std::sync::Mutex;
use willdeep_core::context_summary::ContextSummary;
use willdeep_core::provider::{Provider, ProviderError};
use willdeep_core::types::{Completion, Message, Role, ToolCall, ToolDefinition};
use willdeep_core::{Session, SessionStore};

const WINDOW: u64 = 12_288;
const HISTORY_BATCHES: usize = 20;
const TOOL_OUTPUT_BYTES: usize = 1_900;
const SUMMARY: &str =
    "AUTOMATIC_SAVED_STATE: inspected old files; preserve the user's dirty changes";

pub(crate) struct RecordingProvider {
    requests: Mutex<Vec<Vec<Message>>>,
    reply: &'static str,
}

impl RecordingProvider {
    fn new(reply: &'static str) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            reply,
        })
    }

    pub(crate) fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl Provider for RecordingProvider {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> std::result::Result<Completion, ProviderError> {
        self.requests.lock().unwrap().push(messages.to_vec());
        Ok(Completion {
            reasoning: None,
            content: self.reply.to_owned(),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".to_owned()),
            usage: None,
        })
    }
}

// Shared with dispatch's test: both frontends must pass the summary actually
// produced by Agent, rather than a cache assembled with a test-only fingerprint.
pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) session: Session,
    pub(crate) store: SessionStore,
    pub(crate) agent: Arc<Agent>,
    pub(crate) main: Arc<RecordingProvider>,
    pub(crate) compressor: Arc<RecordingProvider>,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("manual-compression-host-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let config = root.join("config.toml");
        // Anthropic construction skips gateway capability discovery. Execution
        // uses only the in-memory providers below; no endpoint is requested.
        std::fs::write(
            &config,
            "version = 1\ndefault_provider = 'fixture'\n[providers.fixture]\nprovider = 'anthropic'\napi = 'anthropic-messages'\napi_base = 'http://127.0.0.1:1/v1'\napi_key = 'test-only'\nmodel = 'fixture-model'\ncontext_window = 12288\n[agent]\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = false\nsafety_judge = false\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let store = SessionStore::new(&root);
        let mut session = Session::new(workspace.clone(), None, "fixture");
        session.messages = vec![
            Message::system("fixture rules"),
            Message::user("Inspect the fixture; preserve my dirty files and do not push"),
        ];
        for index in 0..HISTORY_BATCHES {
            let call = ToolCall {
                id: format!("history-read-{index}"),
                name: "read_file".to_owned(),
                arguments: format!(r#"{{"path":"file-{index}.rs"}}"#),
            };
            session
                .messages
                .push(Message::assistant("inspect", vec![call.clone()]));
            session.messages.push(Message::tool(
                &call,
                format!("file-{index}: {}", "x".repeat(TOOL_OUTPUT_BYTES)),
            ));
        }
        session
            .messages
            .push(Message::user("continue with my original constraints"));
        store.save(&mut session).unwrap();
        let main = RecordingProvider::new("done");
        let compressor = RecordingProvider::new(SUMMARY);
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
            .with_compressor(compressor.clone(), false),
        );
        Self {
            root,
            config,
            session,
            store,
            agent,
            main,
            compressor,
        }
    }

    pub(crate) async fn seed(&mut self) -> ContextSummary {
        let outcome = self
            .agent
            .run_with_history(self.session.messages.clone(), "continue")
            .await
            .unwrap();
        let summary = outcome.context_summary.expect("the real run must compress");
        assert!(self.main.calls() > 0);
        assert!(self.compressor.calls() > 0);
        self.session.messages = outcome.messages;
        self.session.context_summary = Some(summary.clone());
        self.store.save(&mut self.session).unwrap();
        self.session = self.store.load_baselined(self.session.id).unwrap();
        summary
    }

    pub(crate) fn assert_materialized(&self, summary: &ContextSummary, previous_count: usize) {
        let saved = self.store.load(self.session.id).unwrap();
        assert!(self.session.context_summary.is_none());
        assert!(
            saved.context_summary.is_none(),
            "the old durable anchor must be discarded"
        );
        assert_eq!(saved.compression_generation, 1);
        let boundary = saved.compression_checkpoint.as_ref().unwrap();
        assert_eq!(boundary.previous_message_count, previous_count);
        assert_eq!(boundary.compressed_message_count, saved.messages.len());
        assert!(saved.messages.len() < previous_count);
        assert_eq!(
            saved
                .messages
                .iter()
                .filter(|message| message.content.contains(&summary.summary))
                .count(),
            1
        );
        assert!(
            !saved
                .messages
                .iter()
                .any(|message| message.content.contains("file-0:"))
        );
        assert!(
            !saved
                .messages
                .iter()
                .any(|message| message.content == "/compress")
        );
        assert!(
            saved
                .messages
                .iter()
                .any(|message| message.role == Role::User
                    && message.content.contains("preserve my dirty files"))
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).expect("remove isolated manual compression fixture");
    }
}

#[tokio::test]
async fn headless_compress_materializes_the_real_saved_summary_without_provider_requests() {
    let mut fixture = Fixture::new();
    let summary = fixture.seed().await;
    let previous_count = fixture.session.messages.len();
    let main_calls = fixture.main.calls();
    let compression_calls = fixture.compressor.calls();
    let mut cli = <Cli as clap::Parser>::try_parse_from(["willdeep"]).unwrap();
    // Explicit CLI values also isolate construction from connection overrides
    // in the test process environment.
    cli.provider = Some(crate::ProviderArg::Anthropic);
    cli.api = Some(ApiArg::AnthropicMessages);
    cli.api_base = Some("http://127.0.0.1:1/v1".to_owned());
    cli.api_key = Some("test-only".to_owned());
    cli.model = Some("fixture-model".to_owned());
    cli.config = Some(fixture.config.clone());
    cli.workspace = Some(fixture.session.workspace.clone());
    let loaded = LoadedConfig::load(Some(&fixture.config)).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut built = build(
        &cli,
        &loaded,
        &fixture.root,
        Language::En,
        Some(&fixture.session),
        HarnessFrontend::Tui { tx },
    )
    .await
    .unwrap();
    built.agent = fixture.agent.clone();
    let outcome = execute_noninteractive(
        &built,
        &fixture.store,
        &mut fixture.session,
        "/compress".to_owned(),
        Vec::new(),
        Language::En,
        ExecutionOptions {
            allow_compress_command: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        outcome.stop_reason,
        willdeep_core::AgentStopReason::Finished
    );
    assert_eq!(outcome.turns, 0);
    assert!(outcome.compressed);
    assert_eq!(outcome.final_text, "Context compressed");
    assert_eq!(fixture.main.calls(), main_calls);
    assert_eq!(fixture.compressor.calls(), compression_calls);
    fixture.assert_materialized(&summary, previous_count);
    drop(built);
}
