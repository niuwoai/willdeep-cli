use super::*;

struct UnusedProvider;
#[async_trait]
impl Provider for UnusedProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[crate::types::ToolDefinition],
    ) -> Result<crate::types::Completion, ProviderError> {
        panic!("policy checks must not need a model call");
    }
}

#[tokio::test]
async fn delegation_cap_guards_agent_special_paths_and_ignores_executing_extensions() {
    let root = std::env::temp_dir().join(format!("delegation-agent-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let tools = ToolRegistry::new(&root, crate::ApprovalMode::FullAccess)
        .unwrap()
        .with_delegation_read_only();
    let catalog = Arc::new(SubagentCatalog::new(
        &root,
        Vec::new(),
        Arc::new(BackgroundTaskRegistry::default()),
    ));
    let agent = Agent::new(
        Arc::new(UnusedProvider),
        tools,
        AgentConfig {
            max_turns: 2,
            system_prompt: String::new(),
            context_window: 128_000,
            token_budget: None,
        },
    )
    .with_subagents(catalog)
    .with_goal_continuation(Arc::new(GoalContinuation::new()));
    assert!(agent.subagents.is_none());
    assert!(agent.goal_continuation().is_none());
    for name in [
        "spawn_agent",
        "continue_agent",
        "resume_agent",
        "list_agent_recoveries",
        "stop_agent",
        "send_agent_message",
        "await_agents",
        "start_session",
        "run_command",
    ] {
        let call = ToolCall {
            id: "forged-special".into(),
            name: name.into(),
            arguments: "{}".into(),
        };
        assert!(
            matches!(
                agent.execute_tool(&call).await,
                Err(ToolError::ReadOnlyPolicy(_))
            ),
            "unguarded {name}"
        );
    }
    agent.prepare_background_execution().await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
