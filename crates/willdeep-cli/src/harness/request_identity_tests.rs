use super::input_scope_tests::InputFixture;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use willdeep_core::SessionStore;
use willdeep_core::provider::{Provider, ProviderError};
use willdeep_core::types::{Completion, Message, MessageAttachment, Role, ToolDefinition};

struct VisionProbe {
    store: SessionStore,
    session: uuid::Uuid,
    turn: uuid::Uuid,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Provider for VisionProbe {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> std::result::Result<Completion, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let saved = self.store.load(self.session).unwrap();
        let input = saved.messages.last().unwrap();
        assert_eq!(input.runtime_turn_id, Some(self.turn));
        assert_eq!(input.content, "read the parser");
        assert_eq!(input.attachments.len(), 1);
        assert!(matches!(
            input.attachments[0],
            MessageAttachment::Image { .. }
        ));
        assert!(saved.execution_checkpoint.is_none());
        Ok(Completion {
            reasoning: None,
            content: "a square".into(),
            tool_calls: Vec::new(),
            finish_reason: Some("stop".into()),
            usage: None,
        })
    }
}

struct StopAfterCheckpoint {
    store: SessionStore,
    session: uuid::Uuid,
    turn: uuid::Uuid,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Provider for StopAfterCheckpoint {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> std::result::Result<Completion, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let saved = self.store.load(self.session).unwrap();
        assert_eq!(saved.messages[0].role, Role::System);
        assert!(saved.execution_checkpoint.is_some());
        let input = saved
            .messages
            .iter()
            .find(|message| message.runtime_turn_id == Some(self.turn))
            .unwrap();
        assert!(input.content.contains("<runtime-route"));
        assert!(input.attachments.is_empty());
        assert!(
            messages
                .iter()
                .any(|message| message.runtime_turn_id == Some(self.turn))
        );
        Err(ProviderError::InvalidResponse(
            "offline fixture stops after the first durable checkpoint".into(),
        ))
    }
}

#[tokio::test]
async fn runtime_request_identity_is_saved_before_vision_and_survives_real_agent_checkpoint() {
    let mut fixture = InputFixture::new();
    fixture.session.messages.clear();
    fixture.session.execution_checkpoint = None;
    fixture.store.save(&mut fixture.session).unwrap();
    let mut built = fixture.harness().await;
    let turn = uuid::Uuid::new_v4();
    built.runtime_turn_id = Some(turn);
    let vision = Arc::new(VisionProbe {
        store: fixture.store.clone(),
        session: fixture.session.id,
        turn,
        calls: AtomicUsize::new(0),
    });
    let provider = Arc::new(StopAfterCheckpoint {
        store: fixture.store.clone(),
        session: fixture.session.id,
        turn,
        calls: AtomicUsize::new(0),
    });
    built.agent = Arc::new(
        Agent::new(
            provider.clone(),
            ToolRegistry::new(&fixture.session.workspace, ApprovalMode::ReadOnly).unwrap(),
            AgentConfig {
                max_turns: 1,
                system_prompt: "fixture pin".into(),
                context_window: 128_000,
                token_budget: None,
            },
        )
        .with_subagents(built.subagents.clone())
        .with_routing_guard(Arc::new(RoutingGuard::new(RoutingPolicy {
            auto_dispatch_read_only: false,
            ..Default::default()
        })))
        .with_image_fallback(vision.clone(), "fixture vision"),
    );
    let result = execute_noninteractive(
        &built,
        &fixture.store,
        &mut fixture.session,
        "read the parser".into(),
        vec![MessageAttachment::Image {
            name: "figure.png".into(),
            media_type: "image/png".into(),
            data: "AQID".into(),
            width: 1,
            height: 1,
        }],
        Language::En,
        ExecutionOptions::default(),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(vision.calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let saved = fixture.store.load(fixture.session.id).unwrap();
    assert_eq!(
        saved
            .messages
            .iter()
            .filter(|message| message.runtime_turn_id == Some(turn))
            .count(),
        1
    );
    let input = saved
        .messages
        .iter()
        .find(|message| message.runtime_turn_id == Some(turn))
        .unwrap();
    assert!(
        input
            .content
            .contains(willdeep_core::types::IMAGE_DESCRIPTION_MARKER)
    );
    assert!(input.content.contains("<runtime-route"));
    assert!(input.attachments.is_empty());
}

#[tokio::test]
async fn replay_upgrades_legacy_input_identity_without_adding_another_request() {
    let mut fixture = InputFixture::new();
    fixture.session.execution_checkpoint = None;
    fixture.store.save(&mut fixture.session).unwrap();
    let user_count = fixture
        .session
        .messages
        .iter()
        .filter(|message| message.role == Role::User)
        .count();
    let mut built = fixture.harness().await;
    let turn = uuid::Uuid::new_v4();
    built.runtime_turn_id = Some(turn);
    execute_noninteractive(
        &built,
        &fixture.store,
        &mut fixture.session,
        "persisted recovery input".into(),
        Vec::new(),
        Language::En,
        ExecutionOptions {
            replay_existing_user_message: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let saved = fixture.store.load(fixture.session.id).unwrap();
    assert_eq!(
        saved
            .messages
            .iter()
            .filter(|message| message.role == Role::User)
            .count(),
        user_count
    );
    assert_eq!(
        saved
            .messages
            .iter()
            .filter(|message| message.runtime_turn_id == Some(turn))
            .count(),
        1
    );
}
