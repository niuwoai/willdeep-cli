//! Optional terminal-scoped status reporting. No prompts, output or input capabilities cross this bridge.
mod transport;
mod view;
pub(crate) use view::ViewState;
#[cfg(test)]
mod tests;

use serde::Serialize;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use uuid::Uuid;
use willdeep_core::RuntimeStatus;

const HEARTBEAT: Duration = Duration::from_secs(30);
const RETRY: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Context {
    #[serde(rename = "sessionID")]
    session_id: Uuid,
    #[serde(rename = "taskID", skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(rename = "gateID", skip_serializing_if = "Option::is_none")]
    gate_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct State {
    activity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'static str>,
    context: Context,
}

impl State {
    fn new(
        session: Uuid,
        task: Option<String>,
        gate: Option<String>,
        status: RuntimeStatus,
    ) -> Self {
        let (activity, block_reason, result) = match status {
            RuntimeStatus::Working | RuntimeStatus::Queued => ("working", None, None),
            RuntimeStatus::WaitingApproval => ("blocked", Some("approval"), None),
            RuntimeStatus::WaitingAnswer => ("blocked", Some("question"), None),
            RuntimeStatus::Blocked => ("blocked", Some("other"), None),
            RuntimeStatus::Done => ("idle", None, Some("completed")),
            RuntimeStatus::Failed => ("idle", None, Some("failed")),
            RuntimeStatus::Cancelled | RuntimeStatus::Interrupted => {
                ("idle", None, Some("interrupted"))
            }
            RuntimeStatus::Partial => ("idle", None, Some("unknown")),
            RuntimeStatus::Unknown => ("unknown", None, None),
            RuntimeStatus::Idle => ("idle", None, None),
        };
        Self {
            activity,
            block_reason,
            result,
            context: Context {
                session_id: session,
                task_id: task,
                gate_id: gate,
            },
        }
    }
}

#[derive(Default)]
struct Shared {
    state: Option<State>,
    revision: u64,
    stop: bool,
    error: bool,
    preserve_runtime: bool,
}

/// UI calls only replace a bounded metadata snapshot and wake a dedicated worker.
/// This object belongs to a frontend, never to the shared WillDeep Runtime.
#[derive(Default)]
pub(crate) struct Reporter {
    shared: Option<Arc<(Mutex<Shared>, Condvar)>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Reporter {
    pub(crate) fn from_env() -> Self {
        let Some(path) = std::env::var_os("QTERM_AGENT_CONTEXT") else {
            return Self::default();
        };
        let Ok(endpoint) = transport::Endpoint::open(std::path::Path::new(&path)) else {
            return Self::default();
        };
        Self::start(endpoint)
    }

    fn start(endpoint: transport::Endpoint) -> Self {
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let worker = shared.clone();
        let Ok(handle) = std::thread::Builder::new()
            .name("qterm-status".into())
            .spawn(move || run_worker(endpoint, worker))
        else {
            return Self::default();
        };
        Self {
            shared: Some(shared),
            worker: Some(handle),
        }
    }

    /// Drain the latest bounded snapshot before the frontend exits. Each transport
    /// request already has a deadline; this never waits for a Runtime task to end.
    /// A headless frontend leaves the task state intact because its Runtime owns
    /// the task after the command returns (including a still-pending gate).
    pub(crate) async fn finish(mut self, preserve_runtime: bool) {
        if let Some(shared) = &self.shared {
            let mut slot = shared.0.lock().unwrap_or_else(|p| p.into_inner());
            slot.preserve_runtime = preserve_runtime;
            slot.stop = true;
            shared.1.notify_one();
        }
        if let Some(worker) = self.worker.take() {
            let _ = tokio::task::spawn_blocking(move || worker.join()).await;
        }
    }

    pub(crate) fn update(
        &self,
        session: Uuid,
        task: Option<String>,
        gate: Option<String>,
        status: RuntimeStatus,
    ) {
        let Some(shared) = &self.shared else { return };
        let mut slot = shared
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = State::new(session, task, gate, status);
        if slot.state.as_ref() == Some(&next) {
            return;
        }
        slot.state = Some(next);
        slot.revision = slot.revision.wrapping_add(1);
        shared.1.notify_one();
    }

    pub(crate) fn take_error(&self) -> bool {
        self.shared.as_ref().is_some_and(|shared| {
            let mut slot = shared
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut slot.error)
        })
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.shared.is_some()
    }

    pub(crate) fn unavailable(&self, session: Uuid) {
        let current = self.current_state(session);
        self.update(
            session,
            current.as_ref().and_then(|s| s.context.task_id.clone()),
            current.and_then(|s| s.context.gate_id),
            RuntimeStatus::Unknown,
        );
    }

    fn has_result(&self, session: Uuid) -> bool {
        self.shared.as_ref().is_some_and(|shared| {
            shared
                .0
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .state
                .as_ref()
                .is_some_and(|state| state.context.session_id == session && state.result.is_some())
        })
    }

    fn current_state(&self, session: Uuid) -> Option<State> {
        self.shared.as_ref().and_then(|shared| {
            shared
                .0
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .state
                .clone()
                .filter(|state| state.context.session_id == session)
        })
    }

    /// Ignore stale snapshots and other sessions or background child agents.
    pub(crate) fn runtime_snapshot(
        &self,
        snapshot: &crate::daemon::RuntimeSnapshot,
        session: Uuid,
        cursor: u64,
        running: bool,
    ) {
        if snapshot.event_sequence.is_none() {
            if let Some(shared) = &self.shared {
                let previous = shared
                    .0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .state
                    .clone();
                if let Some(state) = previous.filter(|state| state.context.session_id == session) {
                    self.update(
                        session,
                        state.context.task_id,
                        state.context.gate_id,
                        RuntimeStatus::Unknown,
                    );
                }
            }
            return;
        }
        if !snapshot
            .event_sequence
            .is_some_and(|sequence| sequence >= cursor)
        {
            return;
        }
        let roots = snapshot
            .agents
            .iter()
            .filter(|agent| agent.parent_id.is_none())
            .map(|agent| agent.id)
            .collect::<Vec<_>>();
        let task = main_task(&snapshot.tasks, session, &roots);
        let Some(task) = task else { return };
        use willdeep_runtime_protocol::TaskStatus;
        if running
            && matches!(
                task.status,
                TaskStatus::Completed
                    | TaskStatus::Failed
                    | TaskStatus::Partial
                    | TaskStatus::Cancelled
                    | TaskStatus::Interrupted
            )
        {
            // A queued new turn is absent from older Runtime task snapshots. Do not present
            // an old completed task as the result of that new turn.
            return;
        }
        let status = match task.status {
            TaskStatus::Running | TaskStatus::Queued | TaskStatus::Cancelling => {
                RuntimeStatus::Working
            }
            TaskStatus::WaitingApproval => RuntimeStatus::WaitingApproval,
            TaskStatus::WaitingAnswer => RuntimeStatus::WaitingAnswer,
            TaskStatus::Completed => RuntimeStatus::Done,
            TaskStatus::Failed => RuntimeStatus::Failed,
            TaskStatus::Partial => RuntimeStatus::Partial,
            TaskStatus::Cancelled => RuntimeStatus::Cancelled,
            TaskStatus::Interrupted => RuntimeStatus::Interrupted,
        };
        let gate = snapshot
            .gates
            .iter()
            .find(|gate| gate.task_id() == task.id)
            .map(|gate| gate.id().to_string());
        self.update(session, Some(task.id.to_string()), gate, status);
    }
}

fn main_task<'a>(
    tasks: &'a [crate::daemon::tui_bridge::RemoteTask],
    session: Uuid,
    roots: &[Uuid],
) -> Option<&'a crate::daemon::tui_bridge::RemoteTask> {
    use willdeep_runtime_protocol::TaskStatus;
    tasks
        .iter()
        .filter(|task| {
            task.session_id == Some(session)
                && task.agent_id.is_none_or(|agent| roots.contains(&agent))
        })
        .max_by_key(|task| {
            (
                matches!(
                    task.status,
                    TaskStatus::Queued
                        | TaskStatus::Running
                        | TaskStatus::Cancelling
                        | TaskStatus::WaitingApproval
                        | TaskStatus::WaitingAnswer
                ),
                task.created_at,
                task.id,
            )
        })
}

impl Drop for Reporter {
    fn drop(&mut self) {
        if let Some(shared) = &self.shared {
            shared
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .stop = true;
            shared.1.notify_one();
        }
    }
}

fn run_worker(mut endpoint: transport::Endpoint, shared: Arc<(Mutex<Shared>, Condvar)>) {
    let mut sent = None;
    let mut next = Instant::now();
    let mut disconnected = false;
    loop {
        let mut slot = shared
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while !slot.stop
            && (slot.state.is_none() || (sent == Some(slot.revision) && Instant::now() < next))
        {
            let timeout = next
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(20));
            slot = shared
                .1
                .wait_timeout(slot, timeout)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        let state = slot.state.clone();
        let revision = slot.revision;
        let stop = slot.stop;
        drop(slot);
        if stop {
            if let Some(mut state) = state {
                // Drop used to skip the final revision entirely. Deliver it before
                // retiring the frontend, even when it was queued during an ACK.
                if sent != Some(revision) && endpoint.report(&state).is_err() {
                    shared.0.lock().unwrap_or_else(|p| p.into_inner()).error = true;
                }
                if shared
                    .0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .preserve_runtime
                {
                    return;
                }
                state.activity = "exited";
                state.block_reason = None;
                if endpoint.report(&state).is_err() {
                    shared
                        .0
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .error = true;
                }
            }
            return;
        }
        let Some(state) = state else { continue };
        match endpoint.report(&state) {
            Ok(()) => {
                sent = Some(revision);
                next = Instant::now() + HEARTBEAT;
                disconnected = false;
            }
            Err(_) => {
                if !disconnected {
                    shared
                        .0
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .error = true;
                }
                disconnected = true;
                sent = Some(revision);
                next = Instant::now() + RETRY;
            }
        }
    }
}
