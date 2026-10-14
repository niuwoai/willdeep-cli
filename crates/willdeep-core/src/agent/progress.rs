use crate::types::ToolCall;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

const REPEATED_FAILURE_LIMIT: usize = 3;

#[derive(Default)]
pub(super) struct BlockedToolTracker {
    blocked: Option<(String, usize)>,
}

impl BlockedToolTracker {
    fn key(call: &ToolCall) -> String {
        format!(
            "{}:{}",
            call.name,
            call.parsed_arguments()
                .map(|v| v.to_string())
                .unwrap_or_else(|_| call.arguments.clone())
        )
    }

    pub fn refuses(&self, call: &ToolCall) -> bool {
        self.blocked
            .as_ref()
            .is_some_and(|(key, count)| *count >= REPEATED_FAILURE_LIMIT && *key == Self::key(call))
    }

    pub fn observe(&mut self, call: &ToolCall, output: &str, is_error: bool, novel_progress: bool) {
        if novel_progress {
            self.blocked = None;
            return;
        }
        let failed = is_error
            || (call.name == "run_command"
                && output.starts_with("exit_code:")
                && !output.starts_with("exit_code: 0\n"));
        if !failed {
            return;
        }
        let key = Self::key(call);
        match &mut self.blocked {
            Some((previous, count)) if *previous == key => *count = count.saturating_add(1),
            _ => self.blocked = Some((key, 1)),
        }
    }
}

/// Novel successful observations are evidence; another attempt at the same error is not.
#[derive(Default)]
pub(super) struct ProgressTracker {
    seen: HashSet<[u8; 32]>,
}

impl ProgressTracker {
    pub fn observe(&mut self, call: &ToolCall, output: &str, is_error: bool) -> bool {
        if is_error {
            return false;
        }
        if call.name == "run_command"
            && output.starts_with("exit_code:")
            && !output.starts_with("exit_code: 0\n")
        {
            return false;
        }
        let arguments = call
            .parsed_arguments()
            .map(|value| value.to_string())
            .unwrap_or_else(|_| call.arguments.clone());
        let mut hash = Sha256::new();
        for part in [call.name.as_str(), arguments.as_str(), output] {
            hash.update(part.as_bytes());
            hash.update([0]);
        }
        self.seen.insert(hash.finalize().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentConfig, AgentEvent, EventSink};
    use crate::goal::{ContinuationRung, GoalBudget, GoalContinuation};
    use crate::provider::{Provider, ProviderError};
    use crate::types::{Completion, Message, ToolDefinition};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    struct FailingCommandProvider;

    struct WriteThenWaitProvider(std::sync::atomic::AtomicUsize);

    struct TestOnlyFixProvider(std::sync::atomic::AtomicUsize);
    #[async_trait::async_trait]
    impl Provider for TestOnlyFixProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<Completion, ProviderError> {
            let calls = if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                vec![ToolCall { id: "test-only".into(), name: "edit_file".into(), arguments: serde_json::json!({"path":"tests/check.rs", "old_string":"original test", "new_string":"weakened test"}).to_string() }]
            } else {
                Vec::new()
            };
            Ok(Completion {
                reasoning: None,
                content: "Finished the fix".into(),
                tool_calls: calls,
                finish_reason: Some("stop".into()),
                usage: None,
            })
        }
    }

    #[tokio::test]
    async fn test_only_change_cannot_finish_an_implementation_contract() {
        let root =
            std::env::temp_dir().join(format!("implementation-contract-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("tests/check.rs"), "original test").unwrap();
        std::fs::write(root.join("src/fix.rs"), "unchanged implementation").unwrap();
        let tools = crate::ToolRegistry::new(&root, crate::ApprovalMode::FullAccess)
            .unwrap()
            .with_completion_contract(crate::tools::CompletionContract {
                require_changed_globs: vec!["src/**".into()],
                preserve_existing_globs: vec!["tests/**".into()],
            })
            .unwrap();
        let agent = Agent::new(
            Arc::new(TestOnlyFixProvider(0.into())),
            tools,
            AgentConfig {
                max_turns: 5,
                system_prompt: "test".into(),
                context_window: 128_000,
                token_budget: None,
            },
        );
        let outcome = agent
            .run("Fix production code without changing existing tests")
            .await
            .unwrap();
        assert_eq!(
            outcome.stop_reason,
            crate::agent::AgentStopReason::Unverified
        );
        assert!(
            outcome
                .messages
                .iter()
                .any(|m| m.content.contains("Existing files protected"))
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/fix.rs")).unwrap(),
            "unchanged implementation"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[async_trait::async_trait]
    impl Provider for WriteThenWaitProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<Completion, ProviderError> {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                std::future::pending::<()>().await;
            }
            Ok(Completion {
                reasoning: None,
                content: "implementing".into(),
                tool_calls: vec![ToolCall {
                    id: "write".into(),
                    name: "create_file".into(),
                    arguments: r#"{"path":"fix.rs","content":"retained fix"}"#.into(),
                }],
                finish_reason: Some("tool_calls".into()),
                usage: None,
            })
        }
    }

    #[tokio::test]
    async fn time_budget_retains_completed_write_and_returns_partial_history() {
        let root = std::env::temp_dir().join(format!("time-budget-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let agent = Agent::new(
            Arc::new(WriteThenWaitProvider(0.into())),
            crate::ToolRegistry::new(&root, crate::ApprovalMode::FullAccess).unwrap(),
            AgentConfig {
                max_turns: 6,
                system_prompt: "test".into(),
                context_window: 128_000,
                token_budget: None,
            },
        )
        .with_time_budget(Duration::from_millis(250));
        let outcome =
            tokio::time::timeout(Duration::from_secs(5), agent.run("Fix the implementation"))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            outcome.stop_reason,
            crate::agent::AgentStopReason::BudgetLimited
        );
        assert_eq!(
            std::fs::read_to_string(root.join("fix.rs")).unwrap(),
            "retained fix"
        );
        assert!(
            outcome
                .messages
                .iter()
                .any(|m| m.tool_call_id.as_deref() == Some("write"))
        );
        assert!(!outcome.stop_reason.is_complete());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[async_trait::async_trait]
    impl Provider for FailingCommandProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<Completion, ProviderError> {
            Ok(Completion {
                reasoning: None,
                content: "retry".into(),
                tool_calls: vec![ToolCall {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: "run_command".into(),
                    arguments: r#"{"command":"false"}"#.into(),
                }],
                finish_reason: Some("tool_calls".into()),
                usage: None,
            })
        }
    }

    #[tokio::test]
    async fn ordinary_agent_does_not_execute_unchanged_failures_forever() {
        let root = std::env::temp_dir().join(format!("blocked-loop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let agent = Agent::new(
            Arc::new(FailingCommandProvider),
            crate::ToolRegistry::new(&root, crate::ApprovalMode::FullAccess).unwrap(),
            AgentConfig {
                max_turns: 6,
                system_prompt: "test".into(),
                context_window: 128_000,
                token_budget: None,
            },
        );
        let outcome = agent.run("Diagnose failures").await.unwrap();
        let outputs = outcome
            .messages
            .iter()
            .filter(|m| m.role == crate::types::Role::Tool)
            .collect::<Vec<_>>();
        assert_eq!(
            outputs
                .iter()
                .filter(|m| m.content.starts_with("exit_code: 1\n"))
                .count(),
            3
        );
        assert_eq!(
            outputs
                .iter()
                .filter(|m| m.content.contains("It was not executed again"))
                .count(),
            3
        );
        assert!(!outcome.stop_reason.is_complete());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_failed_action_requires_new_successful_evidence_before_retry() {
        let call = ToolCall {
            id: "one".into(),
            name: "run_command".into(),
            arguments: r#"{"command":"pytest"}"#.into(),
        };
        let mut tracker = BlockedToolTracker::default();
        for _ in 0..3 {
            tracker.observe(&call, "approval denied", true, false);
        }
        assert!(tracker.refuses(&call));
        let mut repeated = call.clone();
        repeated.id = "new-id".into();
        assert!(tracker.refuses(&repeated));
        tracker.observe(&call, "unchanged read", false, false);
        assert!(tracker.refuses(&call));
        tracker.observe(&call, "new fix", false, true);
        assert!(!tracker.refuses(&call));
        for _ in 0..3 {
            tracker.observe(&call, "exit_code: 1\nassertion failed", false, false);
        }
        assert!(tracker.refuses(&call));
    }

    struct RepeatingProvider {
        request: std::sync::atomic::AtomicUsize,
        path: &'static str,
    }

    #[async_trait::async_trait]
    impl Provider for RepeatingProvider {
        async fn complete(
            &self,
            _: &[Message],
            _: &[ToolDefinition],
        ) -> Result<Completion, ProviderError> {
            let request = self
                .request
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Completion {
                reasoning: None,
                content: "Work remains outstanding".into(),
                tool_calls: if request.is_multiple_of(2) {
                    vec![ToolCall {
                        id: format!("attempt-{request}"),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": self.path}).to_string(),
                    }]
                } else {
                    Vec::new()
                },
                finish_reason: Some("stop".into()),
                usage: None,
            })
        }
    }

    #[derive(Default)]
    struct ProgressEvents(Mutex<Vec<AgentEvent>>);

    #[async_trait::async_trait]
    impl EventSink for ProgressEvents {
        async fn emit(&self, event: AgentEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[tokio::test]
    async fn actual_goal_loop_escalates_repeated_errors_and_unchanged_successes() {
        for (path, is_error, reconcile_index) in
            [("missing.txt", true, 2), ("stable.txt", false, 3)]
        {
            let root = std::env::temp_dir().join(format!("progress-loop-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("stable.txt"), "unchanged evidence").unwrap();
            let goal = Arc::new(GoalContinuation::new());
            goal.activate(
                "Complete the outstanding task",
                GoalBudget {
                    wall_clock: None,
                    max_continuations: 20,
                    max_tokens: None,
                },
            );
            let events = Arc::new(ProgressEvents::default());
            let provider = Arc::new(RepeatingProvider {
                request: 0.into(),
                path,
            });
            let agent = Agent::new(
                provider,
                crate::ToolRegistry::new(&root, crate::ApprovalMode::ReadOnly).unwrap(),
                AgentConfig {
                    max_turns: 12,
                    system_prompt: "test".into(),
                    context_window: 128_000,
                    token_budget: None,
                },
            )
            .with_goal_continuation(goal)
            .with_event_sink(events.clone());
            let outcome = agent.run("Continue working").await.unwrap();
            assert!(!outcome.stop_reason.is_complete());
            let events = events.0.lock().unwrap();
            let calls: Vec<_> = events
                .iter()
                .filter_map(|event| {
                    if let AgentEvent::ToolCompleted { is_error, .. } = event {
                        Some(*is_error)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(
                calls,
                vec![is_error; 6],
                "actual tool executions must match the scenario"
            );
            let rungs: Vec<_> = events
                .iter()
                .filter_map(|event| {
                    if let AgentEvent::GoalContinuationInjected { rung } = event {
                        Some(*rung)
                    } else {
                        None
                    }
                })
                .collect();
            assert_eq!(rungs.len(), 6);
            assert_eq!(rungs[0], ContinuationRung::Guidance);
            assert_eq!(rungs[reconcile_index], ContinuationRung::Reconcile);
            assert_eq!(rungs[5], ContinuationRung::Backoff);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn errors_and_repeated_successes_do_not_count_as_progress() {
        let mut tracker = ProgressTracker::default();
        let call = ToolCall {
            id: "first".to_owned(),
            name: "read_file".to_owned(),
            arguments: r#"{"path":"a"}"#.to_owned(),
        };
        assert!(!tracker.observe(&call, "not found", true));
        assert!(tracker.observe(&call, "content", false));
        assert!(!tracker.observe(
            &ToolCall {
                id: "second".to_owned(),
                ..call.clone()
            },
            "content",
            false
        ));
        assert!(tracker.observe(&call, "changed content", false));
        let command = ToolCall {
            name: "run_command".to_owned(),
            ..call
        };
        assert!(!tracker.observe(&command, "exit_code: 1\nstdout:\n", false));
        assert!(tracker.observe(&command, "exit_code: 0\nstdout:\n", false));
    }
}
