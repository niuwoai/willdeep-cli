use super::*;
use crate::types::Role;
use std::collections::BTreeSet;

const UNKNOWN_RESULT: &str = "[willdeep-unknown-tool-effect]";

pub(super) fn has_unknown_effect(messages: &[Message]) -> bool {
    messages
        .iter()
        .any(|message| message.role == Role::Tool && message.content.starts_with(UNKNOWN_RESULT))
}

fn key(call: &ToolCall) -> String {
    let arguments = serde_json::from_str::<serde_json::Value>(&call.arguments)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| call.arguments.clone());
    serde_json::json!([call.name, arguments]).to_string()
}

pub(super) struct UncertainCalls(BTreeSet<String>);

impl UncertainCalls {
    /// Preserve missing results as UNKNOWN, not as success or permission to retry.
    /// This also retains the original call arguments through protocol sanitation.
    pub(super) fn recover(messages: &mut Vec<Message>) -> Self {
        let original = std::mem::take(messages);
        let mut uncertain = BTreeSet::new();
        let mut index = 0;
        while index < original.len() {
            let message = &original[index];
            messages.push(message.clone());
            index += 1;
            if message.role != Role::Assistant || message.tool_calls.is_empty() {
                continue;
            }
            let start = index;
            while index < original.len() && original[index].role == Role::Tool {
                messages.push(original[index].clone());
                index += 1;
            }
            for call in &message.tool_calls {
                let result = original[start..index]
                    .iter()
                    .find(|result| result.tool_call_id.as_deref() == Some(&call.id));
                if result.is_none_or(|result| result.content.starts_with(UNKNOWN_RESULT)) {
                    uncertain.insert(key(call));
                    if result.is_none() {
                        messages.push(Message::tool(call, format!("{UNKNOWN_RESULT} The previous execution has no saved result. Its effects are unknown. Inspect current state before deciding whether to retry; this is not a success receipt. Repeating the same side-effecting call requires explicit one-time approval.")));
                    }
                }
            }
        }
        Self(uncertain)
    }

    pub(super) fn needs_approval(&self, call: &ToolCall) -> bool {
        matches!(
            call.name.as_str(),
            "run_command"
                | "monitor"
                | "create_file"
                | "edit_file"
                | "create_worktree"
                | "call_mcp_tool"
                | "configuration_write"
                | "spawn_agent"
        ) && self.0.contains(&key(call))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Completion, ToolDefinition};
    use crate::{ApprovalDecision, ApprovalMode, Approver};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "create_file".into(),
            arguments: r#"{"path":"result.txt","content":"once"}"#.into(),
        }
    }

    #[test]
    fn missing_receipt_preserves_arguments_and_survives_repeated_recovery() {
        let mut history = vec![Message::assistant("working", vec![call("old")])];
        let calls = UncertainCalls::recover(&mut history);
        sanitize_tool_history(&mut history);
        assert_eq!(history[0].tool_calls.len(), 1);
        assert!(history[1].content.starts_with(UNKNOWN_RESULT));
        assert!(calls.needs_approval(&call("new")));
        let again = UncertainCalls::recover(&mut history);
        assert_eq!(history.len(), 2);
        let mut reordered = call("different-id");
        reordered.arguments = r#"{ "content": "once", "path": "result.txt" }"#.into();
        assert!(again.needs_approval(&reordered));
    }

    #[test]
    fn interrupted_configuration_apply_needs_approval_but_reads_do_not() {
        for name in ["configuration_write", "configuration_read"] {
            let original = ToolCall {
                id: "interrupted".into(),
                name: name.into(),
                arguments: if name == "configuration_write" {
                    json!({"operation":"apply","params":{"plan_id":"saved-plan"}})
                } else {
                    json!({"operation":"snapshot","params":{}})
                }
                .to_string(),
            };
            let mut history = vec![Message::assistant("working", vec![original.clone()])];
            UncertainCalls::recover(&mut history);
            sanitize_tool_history(&mut history);
            let recovered = UncertainCalls::recover(&mut history);
            let retry = ToolCall {
                id: "retry".into(),
                ..original
            };
            assert_eq!(
                recovered.needs_approval(&retry),
                name == "configuration_write"
            );
        }
    }

    struct Repeats {
        calls: AtomicUsize,
        call: ToolCall,
    }
    #[async_trait]
    impl Provider for Repeats {
        async fn complete(
            &self,
            messages: &[Message],
            _: &[ToolDefinition],
        ) -> Result<Completion, ProviderError> {
            let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
            if first {
                assert!(
                    messages
                        .iter()
                        .any(|message| message.content.starts_with(UNKNOWN_RESULT))
                );
            }
            Ok(Completion {
                reasoning: None,
                content: "reported".into(),
                tool_calls: if first {
                    vec![self.call.clone()]
                } else {
                    Vec::new()
                },
                usage: None,
                finish_reason: Some("stop".into()),
            })
        }
    }

    struct Decision {
        allow: bool,
        calls: AtomicUsize,
    }
    #[async_trait]
    impl Approver for Decision {
        async fn approve(&self, description: &str, always: bool) -> ApprovalDecision {
            assert!(description.contains("effects are unknown"));
            assert!(!always);
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.allow {
                ApprovalDecision::AllowOnce
            } else {
                ApprovalDecision::Deny
            }
        }
    }

    #[tokio::test]
    async fn uncertain_write_needs_fresh_approval_even_with_workspace_access() {
        for allow in [false, true] {
            let root = std::env::temp_dir().join(format!("uncertain-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let approval = Arc::new(Decision {
                allow,
                calls: AtomicUsize::new(0),
            });
            let tools = ToolRegistry::new(&root, ApprovalMode::WorkspaceAccess)
                .unwrap()
                .with_approver(approval.clone());
            let agent = Agent::new(
                Arc::new(Repeats {
                    calls: AtomicUsize::new(0),
                    call: call("new"),
                }),
                tools,
                AgentConfig {
                    max_turns: 2,
                    system_prompt: "test".into(),
                    context_window: 32000,
                    token_budget: None,
                },
            );
            agent
                .run_with_history(
                    vec![Message::assistant("working", vec![call("old")])],
                    "continue",
                )
                .await
                .unwrap();
            assert_eq!(approval.calls.load(Ordering::SeqCst), 1);
            assert_eq!(root.join("result.txt").exists(), allow);
            if allow {
                assert_eq!(
                    std::fs::read_to_string(root.join("result.txt")).unwrap(),
                    "once"
                );
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    struct ConfigurationWrites(AtomicUsize);

    #[async_trait]
    impl crate::ConfigurationHost for ConfigurationWrites {
        async fn execute(
            &self,
            operation: &str,
            params: serde_json::Value,
        ) -> Result<serde_json::Value, crate::tools::ToolError> {
            assert_eq!(operation, "apply");
            assert_eq!(params, json!({"plan_id":"saved-plan"}));
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"applied":true}))
        }

        fn approval_description(&self, plan_id: &str) -> Result<String, crate::tools::ToolError> {
            if plan_id != "saved-plan" {
                return Err(crate::tools::ToolError::UnknownTool(
                    "expired configuration plan".into(),
                ));
            }
            Ok("Apply saved configuration plan".into())
        }
    }

    #[tokio::test]
    async fn expired_or_read_only_configuration_replays_never_request_approval() {
        for mode in [
            ApprovalMode::ReadOnly,
            ApprovalMode::Strict,
            ApprovalMode::Smart,
            ApprovalMode::WorkspaceAccess,
            ApprovalMode::FullAccess,
        ] {
            let approval = Arc::new(Decision {
                allow: true,
                calls: AtomicUsize::new(0),
            });
            let host = Arc::new(ConfigurationWrites(AtomicUsize::new(0)));
            let tools = ToolRegistry::new(std::env::temp_dir(), mode)
                .unwrap()
                .with_approver(approval.clone())
                .with_configuration_host(host.clone());
            for plan_id in if mode == ApprovalMode::ReadOnly {
                vec!["expired", "saved-plan"]
            } else {
                vec!["expired"]
            } {
                let call = ToolCall {
                    id: "retry".into(),
                    name: "configuration_write".into(),
                    arguments: json!({"operation":"apply","params":{"plan_id":plan_id}})
                        .to_string(),
                };
                assert!(tools.approve_uncertain_replay(&call).await.is_err());
            }
            assert_eq!(approval.calls.load(Ordering::SeqCst), 0);
            assert_eq!(host.0.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn uncertain_configuration_apply_requires_one_time_approval_even_in_full_access() {
        for allow in [false, true] {
            let approval = Arc::new(Decision {
                allow,
                calls: AtomicUsize::new(0),
            });
            let host = Arc::new(ConfigurationWrites(AtomicUsize::new(0)));
            let tools = ToolRegistry::new(std::env::temp_dir(), ApprovalMode::FullAccess)
                .unwrap()
                .with_approver(approval.clone())
                .with_configuration_host(host.clone());
            let previous = ToolCall {
                id: "interrupted".into(),
                name: "configuration_write".into(),
                arguments: json!({"operation":"apply","params":{"plan_id":"saved-plan"}})
                    .to_string(),
            };
            let agent = Agent::new(
                Arc::new(Repeats {
                    calls: AtomicUsize::new(0),
                    call: ToolCall {
                        id: "retry".into(),
                        ..previous.clone()
                    },
                }),
                tools,
                AgentConfig {
                    max_turns: 2,
                    system_prompt: "test".into(),
                    context_window: 32000,
                    token_budget: None,
                },
            );
            agent
                .run_with_history(
                    vec![Message::assistant("working", vec![previous])],
                    "continue",
                )
                .await
                .unwrap();
            assert_eq!(approval.calls.load(Ordering::SeqCst), 1);
            assert_eq!(host.0.load(Ordering::SeqCst), usize::from(allow));
        }
    }
}
