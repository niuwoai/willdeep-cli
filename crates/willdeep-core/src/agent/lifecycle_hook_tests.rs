#![cfg(unix)]

use super::*;
use crate::hooks::{Hook, HookEvent, HookRegistry};
use crate::tools::ApprovalMode;
use crate::types::{Completion, ToolDefinition};
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

struct TestProvider {
    calls: AtomicUsize,
    fail: bool,
}

#[async_trait]
impl Provider for TestProvider {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(ProviderError::MissingModel);
        }
        Ok(Completion {
            reasoning: None,
            content: "private model output".into(),
            tool_calls: Vec::new(),
            usage: Some(Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                ..Default::default()
            }),
            finish_reason: Some("stop".into()),
        })
    }
}

fn hook(event: HookEvent, command: String, blocking: bool) -> Hook {
    Hook {
        name: event.as_str().into(),
        event,
        command,
        blocking,
        timeout: Duration::from_secs(2),
        on_error: Default::default(),
    }
}

async fn exercise(block: bool, fail: bool, delegated: bool, budget: bool) {
    let root = std::env::temp_dir().join(format!("lifecycle-hook-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let events = root.join("events.jsonl");
    let capture = format!("cat >> '{}'", events.display());
    let hooks = HookRegistry::new(vec![
        hook(
            HookEvent::BeforeAgent,
            if block {
                format!("{capture}; exit 1")
            } else {
                capture.clone()
            },
            true,
        ),
        hook(HookEvent::AgentEnd, format!("{capture}; exit 9"), false),
    ]);
    let mut tools = ToolRegistry::new(&root, ApprovalMode::ReadOnly)
        .unwrap()
        .with_session_id(Some("lifecycle-session".into()))
        .with_hooks(hooks.clone());
    if delegated {
        tools = tools.with_delegation_read_only().with_hooks(hooks);
    }
    let provider = Arc::new(TestProvider {
        calls: AtomicUsize::new(0),
        fail,
    });
    let agent = Agent::new(
        provider.clone(),
        tools,
        AgentConfig {
            max_turns: 1,
            system_prompt: "private system instruction".into(),
            context_window: 128_000,
            token_budget: budget.then_some(1),
        },
    );
    let result = agent.run("private user request").await;
    if delegated {
        assert!(result.is_ok());
        assert!(!events.exists());
    } else {
        let raw = std::fs::read_to_string(&events).unwrap();
        for text in [
            "private user request",
            "private model output",
            "private system instruction",
        ] {
            assert!(!raw.contains(text));
        }
        let rows: Vec<serde_json::Value> = serde_json::Deserializer::from_str(&raw)
            .into_iter()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["event"], "before_agent");
        assert_eq!(rows[1]["event"], "agent_end");
        assert_eq!(rows[1]["session_id"], "lifecycle-session");
        assert_eq!(rows[1]["workspace"], root.to_str().unwrap());
        assert!(rows[1]["detail"].is_null());
        let expected = if block {
            assert!(matches!(
                result,
                Err(AgentError::Tool(ToolError::HookDenied(_)))
            ));
            assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
            "blocked"
        } else if fail {
            assert!(matches!(
                result,
                Err(AgentError::Provider(ProviderError::MissingModel))
            ));
            "error"
        } else if budget {
            assert!(matches!(
                result,
                Err(AgentError::TokenBudgetExceeded { .. })
            ));
            "error"
        } else {
            let outcome = result.unwrap();
            assert!(outcome.stop_reason.is_complete());
            outcome.stop_reason.as_str()
        };
        assert_eq!(rows[1]["outcome"], expected);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn lifecycle_hook_blocks_before_model_and_reports_blocked() {
    exercise(true, false, false, false).await;
}

#[tokio::test]
async fn lifecycle_hook_observes_success_without_changing_it() {
    exercise(false, false, false, false).await;
}

#[tokio::test]
async fn lifecycle_hook_observes_provider_error() {
    exercise(false, true, false, false).await;
}

#[tokio::test]
async fn lifecycle_hook_observes_budget_stop_without_claiming_success() {
    exercise(false, false, false, true).await;
}

#[tokio::test]
async fn lifecycle_hook_never_executes_in_read_only_delegation() {
    exercise(true, false, true, false).await;
}
