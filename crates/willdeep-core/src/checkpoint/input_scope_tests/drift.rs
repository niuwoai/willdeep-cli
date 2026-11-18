//! Workspace changes observed between tools are not evidence of this request's writes.
use super::*;

struct AllowFixtureCommand;

#[async_trait]
impl crate::Approver for AllowFixtureCommand {
    async fn approve(&self, _: &str, _: bool) -> crate::ApprovalDecision {
        crate::ApprovalDecision::AllowOnce
    }
}

struct DriftProvider {
    inner: Arc<SyntheticProvider>,
    changed_file: std::path::PathBuf,
}

#[async_trait]
impl Provider for DriftProvider {
    async fn complete(
        &self,
        messages: &[Message],
        definitions: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        let completion = self.inner.complete(messages, definitions).await?;
        if completion.tool_calls.is_empty() {
            // A different session writes while this provider is answering.
            std::fs::write(&self.changed_file, "external change").unwrap();
        }
        Ok(completion)
    }
}

#[tokio::test]
async fn external_drift_during_an_independent_answer_does_not_request_tests() {
    assert_external_drift_is_not_a_task(Vec::new()).await;
}

#[tokio::test]
async fn external_drift_after_a_file_query_does_not_request_tests() {
    assert_external_drift_is_not_a_task(vec![ToolCall {
        id: "query".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"path":"query.txt"}).to_string(),
    }])
    .await;
}

#[cfg(unix)]
#[tokio::test]
async fn external_drift_after_a_shell_query_does_not_request_tests() {
    assert_external_drift_is_not_a_task(vec![ToolCall {
        id: "query".into(),
        name: "run_command".into(),
        arguments: serde_json::json!({"command":"printf 'synthetic query result'"}).to_string(),
    }])
    .await;
}

async fn assert_external_drift_is_not_a_task(calls: Vec<ToolCall>) {
    let fixture = Fixture::new();
    std::fs::write(fixture.saved().workspace.join("query.txt"), "query data").unwrap();
    let expected_turns = if calls.is_empty() { 1 } else { 2 };
    let inner = Arc::new(SyntheticProvider::default());
    if !calls.is_empty() {
        inner.tools.lock().unwrap().push_back(calls);
    }
    let agent = Agent::new(
        Arc::new(DriftProvider {
            inner: inner.clone(),
            changed_file: fixture.saved().workspace.join("result.txt"),
        }),
        fixture.tools(),
        AgentConfig {
            max_turns: 5,
            system_prompt: "Offline workspace drift fixture".into(),
            context_window: 128_000,
            token_budget: None,
        },
    );
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let outcome = agent
        .run_checkpointed(
            Vec::new(),
            Message::user("return the query result"),
            Some(&sink),
        )
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, AgentStopReason::Finished);
    assert_eq!(outcome.turns, expected_turns);
    assert_eq!(inner.requests.lock().unwrap().len(), expected_turns);
    assert_eq!(outcome.final_text, "Synthetic identity answer");
    assert!(outcome.messages.iter().all(|message| {
        !message
            .content
            .contains("[completion-verification-required]")
    }));
    assert_eq!(
        std::fs::read_to_string(fixture.saved().workspace.join("result.txt")).unwrap(),
        "external change"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_shell_write_still_requires_verification() {
    assert_shell_write_is_unverified("printf changed > result.txt").await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_shell_can_still_have_written_files() {
    assert_shell_write_is_unverified("printf changed > result.txt; exit 1").await;
}

#[cfg(unix)]
async fn assert_shell_write_is_unverified(command: &str) {
    let fixture = Fixture::new();
    let provider = Arc::new(SyntheticProvider::default());
    provider.tools.lock().unwrap().push_back(vec![ToolCall {
        id: "shell-write".into(),
        name: "run_command".into(),
        arguments: serde_json::json!({"command":command}).to_string(),
    }]);
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let outcome = fixture
        .agent(
            provider,
            fixture.tools().with_approver(Arc::new(AllowFixtureCommand)),
        )
        .run_checkpointed(Vec::new(), Message::user("update result.txt"), Some(&sink))
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, AgentStopReason::Unverified);
    assert_eq!(
        std::fs::read_to_string(fixture.saved().workspace.join("result.txt")).unwrap(),
        "changed"
    );
    assert!(outcome.messages.iter().any(|message| {
        message
            .content
            .contains("[completion-verification-scope-unknown]")
    }));
    assert_eq!(
        outcome.turns, 2,
        "opaque writes must not trigger autonomous tests"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn monitor_launch_retains_uncertainty_for_later_workspace_changes() {
    let fixture = Fixture::new();
    let kernel = crate::kernel::EventKernel::new();
    let tools = fixture
        .tools()
        .with_approver(Arc::new(AllowFixtureCommand))
        .with_background_tasks(Arc::new(crate::BackgroundTaskRegistry::default()))
        .with_monitor_events(
            kernel,
            uuid::Uuid::nil(),
            fixture.saved().workspace.join("monitors"),
        );
    let inner = Arc::new(SyntheticProvider::default());
    inner.tools.lock().unwrap().push_back(vec![ToolCall {
        id: "monitor-query".into(),
        name: "monitor".into(),
        arguments: serde_json::json!({
            "command":"printf query",
            "label":"watch the query",
            "timeout_seconds":5
        })
        .to_string(),
    }]);
    let agent = Agent::new(
        Arc::new(DriftProvider {
            inner: inner.clone(),
            changed_file: fixture.saved().workspace.join("result.txt"),
        }),
        tools,
        AgentConfig {
            max_turns: 5,
            system_prompt: "Offline monitor scope".into(),
            context_window: 128_000,
            token_budget: None,
        },
    );
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let outcome = agent
        .run_checkpointed(Vec::new(), Message::user("watch the query"), Some(&sink))
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, AgentStopReason::Unverified);
    assert_eq!(inner.requests.lock().unwrap().len(), 2);
    assert!(outcome.messages.iter().any(|message| {
        message
            .content
            .contains("[completion-verification-scope-unknown]")
    }));
}

#[cfg(unix)]
#[tokio::test]
async fn background_launch_retains_uncertainty_for_later_workspace_changes() {
    let fixture = Fixture::new();
    let inner = Arc::new(SyntheticProvider::default());
    inner.tools.lock().unwrap().push_back(vec![ToolCall {
        id: "background-query".into(),
        name: "run_command".into(),
        arguments: serde_json::json!({
            "command":"printf query", "run_in_background":true
        })
        .to_string(),
    }]);
    let agent = Agent::new(
        Arc::new(DriftProvider {
            inner: inner.clone(),
            changed_file: fixture.saved().workspace.join("result.txt"),
        }),
        fixture.tools().with_approver(Arc::new(AllowFixtureCommand)),
        AgentConfig {
            max_turns: 5,
            system_prompt: "Offline background scope".into(),
            context_window: 128_000,
            token_budget: None,
        },
    );
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let outcome = agent
        .run_checkpointed(
            Vec::new(),
            Message::user("start a background query"),
            Some(&sink),
        )
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, AgentStopReason::Unverified);
    assert_eq!(inner.requests.lock().unwrap().len(), 2);
    assert!(outcome.messages.iter().any(|message| {
        message
            .content
            .contains("[completion-verification-scope-unknown]")
    }));
}

#[tokio::test]
async fn satisfying_a_patch_contract_does_not_bypass_verification() {
    let fixture = Fixture::new();
    let agent = Agent::new(
        Arc::new(DriftProvider {
            inner: Arc::new(SyntheticProvider::default()),
            changed_file: fixture.saved().workspace.join("result.txt"),
        }),
        fixture
            .tools()
            .with_completion_contract(crate::tools::CompletionContract {
                require_changed_globs: vec!["result.txt".into()],
                preserve_existing_globs: Vec::new(),
            })
            .unwrap(),
        AgentConfig {
            max_turns: 5,
            system_prompt: "Offline patch contract".into(),
            context_window: 128_000,
            token_budget: None,
        },
    );
    let ownership = fixture.new_input();
    let sink = ownership.for_new_input().unwrap();
    let outcome = agent
        .run_checkpointed(
            Vec::new(),
            Message::user("complete the contracted change"),
            Some(&sink),
        )
        .await
        .unwrap();
    assert_eq!(outcome.stop_reason, AgentStopReason::Unverified);
    assert!(outcome.messages.iter().any(|message| {
        message
            .content
            .contains("[completion-verification-required]")
            && message.content.contains("wait for explicit agreement")
    }));
}
