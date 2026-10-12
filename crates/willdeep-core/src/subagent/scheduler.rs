//! Atomic, session-scoped admission. All background worker attempts share it.
use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use uuid::Uuid;

pub const DEFAULT_MAX_CONCURRENT: usize = 8;
pub const DEFAULT_MAX_QUEUED: usize = 64;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentTeamSettings {
    pub max_concurrent: usize,
    pub max_queued: usize,
    pub token_budget: Option<u64>,
}

impl Default for AgentTeamSettings {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            max_queued: DEFAULT_MAX_QUEUED,
            token_budget: None,
        }
    }
}

impl AgentTeamSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=64).contains(&self.max_concurrent) {
            return Err("agent_team.max_concurrent must be between 1 and 64".into());
        }
        if self.max_queued > 1024 {
            return Err("agent_team.max_queued must be between 0 and 1024".into());
        }
        if self.token_budget == Some(0) {
            return Err("agent_team.token_budget must be greater than zero".into());
        }
        Ok(())
    }
}

struct Waiting {
    agent: Uuid,
    sender: oneshot::Sender<Permit>,
}

#[derive(Default)]
struct State {
    running: BTreeSet<Uuid>,
    waiting: VecDeque<Waiting>,
}

#[derive(Clone)]
pub(crate) struct Scheduler {
    settings: AgentTeamSettings,
    state: Arc<Mutex<State>>,
}

pub(crate) struct Ticket {
    pub queued: bool,
    agent: Uuid,
    scheduler: Scheduler,
    receiver: Option<oneshot::Receiver<Permit>>,
}

pub(crate) struct Permit {
    agent: Uuid,
    scheduler: Scheduler,
}

impl Scheduler {
    #[cfg(test)]
    pub fn state_is_idle_for_test(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.running.is_empty() && state.waiting.is_empty()
    }
    pub fn new(settings: AgentTeamSettings) -> Self {
        Self {
            settings,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    pub fn reserve(&self, agent: Uuid) -> Result<Ticket, String> {
        let (sender, receiver) = oneshot::channel();
        let mut state = self
            .state
            .lock()
            .map_err(|_| "agent scheduler unavailable")?;
        if state.running.contains(&agent) || state.waiting.iter().any(|item| item.agent == agent) {
            return Err("this Agent already has an unfinished run".into());
        }
        let queued = state.running.len() >= self.settings.max_concurrent;
        if queued && state.waiting.len() >= self.settings.max_queued {
            return Err("the Agent queue is full; wait for a worker to finish".into());
        }
        if queued {
            state.waiting.push_back(Waiting { agent, sender });
        } else {
            state.running.insert(agent);
            // The receiver is local and cannot be dropped while the state is locked.
            let _ = sender.send(Permit {
                agent,
                scheduler: self.clone(),
            });
        }
        Ok(Ticket {
            queued,
            agent,
            scheduler: self.clone(),
            receiver: Some(receiver),
        })
    }

    fn release(&self, agent: Uuid) {
        let next = {
            let mut state = self.state.lock().expect("agent scheduler");
            if !state.running.remove(&agent) {
                return;
            }
            let next = state.waiting.pop_front();
            if let Some(next) = &next {
                state.running.insert(next.agent);
            }
            next
        };
        // Sending outside the lock lets a cancelled receiver drop its permit safely.
        if let Some(next) = next {
            let _ = next.sender.send(Permit {
                agent: next.agent,
                scheduler: self.clone(),
            });
        }
    }

    fn cancel_waiting(&self, agent: Uuid) {
        self.state
            .lock()
            .expect("agent scheduler")
            .waiting
            .retain(|item| item.agent != agent);
    }
}

impl Ticket {
    pub async fn wait(mut self) -> Result<Permit, String> {
        self.receiver
            .take()
            .expect("admission receiver")
            .await
            .map_err(|_| "Agent admission cancelled".into())
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.scheduler.cancel_waiting(self.agent);
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.scheduler.release(self.agent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_capacity_and_zero_budget_are_rejected() {
        for settings in [
            AgentTeamSettings {
                max_concurrent: 0,
                ..Default::default()
            },
            AgentTeamSettings {
                max_concurrent: 65,
                ..Default::default()
            },
            AgentTeamSettings {
                max_queued: 1025,
                ..Default::default()
            },
            AgentTeamSettings {
                token_budget: Some(0),
                ..Default::default()
            },
        ] {
            assert!(settings.validate().is_err());
        }
        assert!(
            AgentTeamSettings {
                max_queued: 0,
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[tokio::test]
    async fn default_admits_eight_and_preserves_fifo() {
        let scheduler = Scheduler::new(AgentTeamSettings::default());
        let mut running = Vec::new();
        for _ in 0..8 {
            let ticket = scheduler.reserve(Uuid::new_v4()).unwrap();
            assert!(!ticket.queued);
            running.push(ticket.wait().await.unwrap());
        }
        let a = scheduler.reserve(Uuid::new_v4()).unwrap();
        let b = scheduler.reserve(Uuid::new_v4()).unwrap();
        assert!(a.queued && b.queued);
        drop(running.pop());
        let permit = a.wait().await.unwrap();
        assert_eq!(scheduler.state.lock().unwrap().waiting.len(), 1);
        drop(permit);
        let _last = b.wait().await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_queue_and_duplicate_agent_do_not_consume_capacity() {
        let scheduler = Scheduler::new(AgentTeamSettings {
            max_concurrent: 1,
            max_queued: 1,
            token_budget: None,
        });
        let id = Uuid::new_v4();
        let permit = scheduler.reserve(id).unwrap().wait().await.unwrap();
        assert!(scheduler.reserve(id).is_err());
        let queued = scheduler.reserve(Uuid::new_v4()).unwrap();
        assert!(scheduler.reserve(Uuid::new_v4()).is_err());
        drop(queued);
        let next = scheduler.reserve(Uuid::new_v4()).unwrap();
        drop(permit);
        let permit = next.wait().await.unwrap();
        drop(permit);
        assert!(scheduler.state.lock().unwrap().running.is_empty());
    }

    #[tokio::test]
    async fn dropping_granted_ticket_releases_slot() {
        let scheduler = Scheduler::new(AgentTeamSettings {
            max_concurrent: 1,
            max_queued: 1,
            token_budget: None,
        });
        let ticket = scheduler.reserve(Uuid::new_v4()).unwrap();
        drop(ticket);
        assert!(!scheduler.reserve(Uuid::new_v4()).unwrap().queued);
    }
}
