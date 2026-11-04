use super::*;

#[tokio::test]
async fn delegation_cap_survives_full_access_and_blocks_forged_calls() {
    let root = std::env::temp_dir().join(format!("delegation-cap-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("evidence.txt"), "readable evidence\n").unwrap();
    let shared = SharedApprovalMode::new(ApprovalMode::Strict);
    let parent = SharedApprovalMode::new(ApprovalMode::FullAccess);
    let tools = ToolRegistry::new(&root, ApprovalMode::FullAccess)
        .unwrap()
        .with_shared_approval_mode(shared.clone())
        .with_parent_approval_mode(parent)
        .with_delegation_read_only();
    shared.set(ApprovalMode::FullAccess);
    assert_eq!(tools.approval_mode(), ApprovalMode::ReadOnly);
    let forbidden = [
        "create_file",
        "edit_file",
        "run_command",
        "git_status",
        "git_diff",
        "git_log",
        "git_blame",
        "list_worktrees",
        "create_worktree",
        "monitor",
        "kill_job",
        "call_mcp_tool",
        "mcp__fixture__write",
        "plugin__fixture",
        "web_fetch",
        "board_post",
        "spawn_agent",
        "continue_agent",
        "resume_agent",
        "await_agents",
        "stop_agent",
        "send_agent_message",
        "list_agent_recoveries",
        "workflow_control",
        "workflow_read",
        "agent_library_manage",
        "agent_library_read",
        "start_session",
        "continue_session",
        "retry_session_task",
        "get_session_task",
        "send_session_message",
        "list_workspace_sessions",
    ];
    let offered = tools.definitions();
    for name in forbidden {
        assert!(
            !offered.iter().any(|tool| tool.name == name),
            "offered {name}"
        );
        let call = ToolCall {
            id: "forged".into(),
            name: name.into(),
            arguments: "{}".into(),
        };
        assert!(
            matches!(
                tools.execute(&call).await,
                Err(ToolError::ReadOnlyPolicy(_))
            ),
            "executed {name}"
        );
    }
    for name in [
        "read_file",
        "grep_files",
        "search_files",
        "list_directory",
        "ask_user",
    ] {
        assert!(
            offered.iter().any(|tool| tool.name == name),
            "missing {name}"
        );
    }
    let read = ToolCall {
        id: "read".into(),
        name: "read_file".into(),
        arguments: json!({"path":"evidence.txt"}).to_string(),
    };
    assert!(
        tools
            .execute(&read)
            .await
            .unwrap()
            .contains("readable evidence")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn delegation_hooks_cannot_be_reinstalled_after_the_cap() {
    let root = std::env::temp_dir().join(format!("delegation-hooks-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let tools = ToolRegistry::new(&root, ApprovalMode::FullAccess)
        .unwrap()
        .with_delegation_read_only()
        .with_hooks(HookRegistry::new(vec![crate::hooks::Hook {
            name: "must-not-run".into(),
            event: HookEvent::PreTool,
            command: "exit 19".into(),
            blocking: true,
            timeout: std::time::Duration::from_secs(1),
            on_error: crate::hooks::HookFailure::Deny,
        }]));
    assert!(tools.hooks.is_empty());
    let call = ToolCall {
        id: "list".into(),
        name: "list_directory".into(),
        arguments: "{}".into(),
    };
    tools.execute(&call).await.unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
