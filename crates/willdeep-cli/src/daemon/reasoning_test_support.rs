//! Exercise real Runtime claim snapshots from TUI tests without widening the
//! production visibility of the session store or starting a daemon process.
use super::session_store::{CreateRuntimeSession, CreateRuntimeTurn, RuntimeSessionStore};
use anyhow::{Context, Result};
use std::path::Path;

pub(crate) struct ReasoningSessionFixture {
    store: RuntimeSessionStore,
    pub id: uuid::Uuid,
}

impl ReasoningSessionFixture {
    pub fn new(root: &Path) -> Result<Self> {
        let store = RuntimeSessionStore::open(root.join("runtime-sessions.json"), root)?;
        let session = store.create(CreateRuntimeSession {
            id: None,
            workspace: root.to_owned(),
            profile: Some("preview".into()),
            model: Some("test-model".into()),
            config: None,
            title: None,
        })?;
        Ok(Self {
            store,
            id: session.id,
        })
    }

    pub fn set(&self, level: Option<&str>) -> Result<()> {
        self.store
            .update_reasoning_level(self.id, level.map(str::to_owned))?;
        Ok(())
    }

    pub fn level(&self) -> Result<Option<String>> {
        Ok(self
            .store
            .get(self.id)?
            .context("test session missing")?
            .reasoning_level)
    }

    pub fn enqueue(&self, prompt: &str) -> Result<uuid::Uuid> {
        let (turn, _) = self.store.enqueue_turn(
            self.id,
            CreateRuntimeTurn {
                team_control: None,
                request_id: uuid::Uuid::new_v4(),
                prompt: prompt.to_owned(),
                attachments: Vec::new(),
                origin_client: None,
                defer_until_released: false,
            },
        )?;
        Ok(turn.id)
    }

    pub fn claim(&self) -> Result<(uuid::Uuid, Option<String>)> {
        let claimed = self
            .store
            .claim_next(self.id)?
            .context("test turn not claimable")?;
        Ok((claimed.metadata.id, claimed.request.reasoning_level))
    }

    pub fn cancel(&self, id: uuid::Uuid) -> Result<()> {
        self.store.request_cancel(id)?;
        Ok(())
    }
}
