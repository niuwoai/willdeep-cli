use super::*;
use crate::peer_sessions::{PeerMessage, PeerMessageStore, PeerOrigin};
use crate::tools::{ApprovalMode, SessionCollaboration};
use crate::types::{Completion, MessageSource, ToolDefinition};

struct Client {
    store: PeerMessageStore,
    session: uuid::Uuid,
}
#[async_trait]
impl SessionCollaboration for Client {
    fn pending(&self) -> Result<Vec<PeerMessage>, ToolError> {
        Ok(self
            .store
            .list(self.session)
            .unwrap()
            .into_iter()
            .filter(|note| note.to_session == self.session && note.consumed_at.is_none())
            .collect())
    }
    fn consumed(&self, ids: &[uuid::Uuid]) -> Result<(), ToolError> {
        for id in ids {
            self.store.mark_consumed(self.session, *id, 1001).unwrap();
        }
        Ok(())
    }
    async fn execute(&self, name: &str, _: serde_json::Value) -> Result<String, ToolError> {
        Err(ToolError::UnknownTool(name.into()))
    }
}

struct Receiver {
    fail: bool,
}
#[async_trait]
impl Provider for Receiver {
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        let note = messages
            .iter()
            .find(|message| message.content.contains("coordination fixture"))
            .unwrap();
        assert_eq!(note.source, Some(MessageSource::HostInstruction));
        assert!(note.content.contains("not a user instruction or approval"));
        if self.fail {
            return Err(ProviderError::Client("offline fixture failure".into()));
        }
        Ok(Completion {
            reasoning: None,
            content: "received".into(),
            tool_calls: Vec::new(),
            usage: None,
            finish_reason: Some("stop".into()),
        })
    }
}

async fn run_fixture(fail: bool) {
    let root = std::env::temp_dir().join(format!("peer-context-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let session = uuid::Uuid::new_v4();
    let store = PeerMessageStore::new(&root);
    let note = PeerMessage {
        id: uuid::Uuid::new_v4(),
        from_session: uuid::Uuid::new_v4(),
        to_session: session,
        project_key: root.clone(),
        origin: PeerOrigin::Model,
        body: "coordination fixture".into(),
        reply_to: None,
        created_at: 1000,
        consumed_at: None,
        read_by: Default::default(),
    };
    store.enqueue(note).unwrap();
    let tools = ToolRegistry::new(&root, ApprovalMode::ReadOnly)
        .unwrap()
        .with_session_collaboration(Arc::new(Client {
            store: store.clone(),
            session,
        }));
    let agent = Agent::new(
        Arc::new(Receiver { fail }),
        tools,
        AgentConfig {
            max_turns: 2,
            system_prompt: String::new(),
            context_window: 32000,
            token_budget: None,
        },
    );
    let result = agent.run("continue current task").await;
    assert_eq!(result.is_err(), fail);
    assert_eq!(store.list(session).unwrap()[0].consumed_at.is_some(), !fail);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn peer_data_reaches_provider_and_then_receives_consumption_receipt() {
    run_fixture(false).await;
}
#[tokio::test]
async fn provider_failure_keeps_peer_message_pending() {
    run_fixture(true).await;
}

#[test]
fn unbound_registry_does_not_offer_independent_session_tools() {
    let root = std::env::temp_dir();
    let tools = ToolRegistry::new(root, ApprovalMode::ReadOnly).unwrap();
    assert!(
        !tools
            .definitions()
            .iter()
            .any(|tool| tool.name == "send_session_message")
    );
}

#[test]
fn clipped_request_context_does_not_claim_full_message_consumption() {
    let root = std::env::temp_dir().join(format!("peer-clipped-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let session = uuid::Uuid::new_v4();
    let store = PeerMessageStore::new(&root);
    let note = PeerMessage {
        id: uuid::Uuid::new_v4(),
        from_session: uuid::Uuid::new_v4(),
        to_session: session,
        project_key: root.clone(),
        origin: PeerOrigin::Model,
        body: "complete coordination data".into(),
        reply_to: None,
        created_at: 1000,
        consumed_at: None,
        read_by: Default::default(),
    };
    store.enqueue(note).unwrap();
    let tools = ToolRegistry::new(&root, ApprovalMode::ReadOnly)
        .unwrap()
        .with_session_collaboration(Arc::new(Client {
            store: store.clone(),
            session,
        }));
    let mut messages = Vec::new();
    let (ids, _) = tools.append_peer_context(&mut messages).unwrap();
    tools
        .peer_context_consumed(&ids, &[Message::host_instruction("clipped context")])
        .unwrap();
    assert!(store.list(session).unwrap()[0].consumed_at.is_none());
    tools.peer_context_consumed(&ids, &messages).unwrap();
    assert!(store.list(session).unwrap()[0].consumed_at.is_some());
    std::fs::remove_dir_all(root).unwrap();
}
