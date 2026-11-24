use super::*;

pub(crate) struct ObserveInputs {
    pub running: bool,
    pub runtime_turn: bool,
    pub approval: bool,
    pub question: bool,
    pub generation: u64,
}

#[derive(Default)]
pub(crate) struct ViewState {
    pub reporter: Reporter,
    session: Option<Uuid>,
    session_title: Option<String>,
    task: Option<String>,
    running: bool,
    gate: Option<(u64, RuntimeStatus)>,
    local_mode: bool,
}

impl ViewState {
    pub(crate) fn from_env() -> Self {
        Self::new(Reporter::from_env())
    }

    pub(super) fn new(reporter: Reporter) -> Self {
        Self {
            reporter,
            ..Self::default()
        }
    }

    pub(crate) fn observe(&mut self, session: Uuid, session_title: &str, input: ObserveInputs) {
        self.session_title = session_report_title(session_title);
        if self.session != Some(session) {
            self.session = Some(session);
            self.task = None;
            self.running = false;
            self.gate = None;
            self.local_mode = false;
            self.reporter.update(
                session,
                None,
                None,
                RuntimeStatus::Idle,
                self.session_title.clone(),
            );
        }
        if input.running && (!self.running || input.runtime_turn == self.local_mode) {
            self.local_mode = !input.runtime_turn;
            if let Some(current) = self.reporter.current_state(session).filter(|state| {
                input.runtime_turn && matches!(state.activity, "working" | "blocked")
            }) {
                self.task = current.context.task_id;
            } else {
                self.task = Some(Uuid::new_v4().to_string());
                self.reporter.update(
                    session,
                    self.task.clone(),
                    None,
                    RuntimeStatus::Working,
                    self.session_title.clone(),
                );
            }
        }
        let gate = if input.approval {
            Some((input.generation, RuntimeStatus::WaitingApproval))
        } else if input.question {
            Some((input.generation, RuntimeStatus::WaitingAnswer))
        } else {
            None
        };
        if self.local_mode
            && gate != self.gate
            && (input.running || !self.reporter.has_result(session))
        {
            let status = gate.map(|(_, status)| status).unwrap_or(if input.running {
                RuntimeStatus::Working
            } else {
                RuntimeStatus::Idle
            });
            self.reporter.update(
                session,
                self.task.clone(),
                gate.map(|(id, _)| format!("local:{id}")),
                status,
                self.session_title.clone(),
            );
        }
        if self.local_mode && self.running && !input.running && !self.reporter.has_result(session) {
            self.reporter.update(
                session,
                self.task.clone(),
                None,
                RuntimeStatus::Interrupted,
                self.session_title.clone(),
            );
        }
        self.running = input.running;
        self.gate = gate;
    }

    pub(crate) fn finished(&self, session: Uuid, status: RuntimeStatus) {
        self.reporter.update(
            session,
            self.task.clone(),
            None,
            status,
            self.session_title.clone(),
        );
    }

    pub(crate) fn runtime_snapshot(
        &self,
        snapshot: &crate::daemon::RuntimeSnapshot,
        session: Uuid,
        cursor: u64,
        running: bool,
    ) {
        if !self.local_mode {
            self.reporter.runtime_snapshot(
                snapshot,
                session,
                cursor,
                running,
                self.session_title.clone(),
            );
        }
    }
}
