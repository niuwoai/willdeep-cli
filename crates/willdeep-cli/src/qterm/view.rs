use super::*;

#[derive(Default)]
pub(crate) struct ViewState {
    pub reporter: Reporter,
    session: Option<Uuid>,
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

    pub(crate) fn observe(
        &mut self,
        session: Uuid,
        running: bool,
        runtime_turn: bool,
        approval: bool,
        question: bool,
        generation: u64,
    ) {
        if self.session != Some(session) {
            self.session = Some(session);
            self.task = None;
            self.running = false;
            self.gate = None;
            self.local_mode = false;
            self.reporter
                .update(session, None, None, RuntimeStatus::Idle);
        }
        if running && (!self.running || runtime_turn == self.local_mode) {
            self.local_mode = !runtime_turn;
            if let Some(current) = self
                .reporter
                .current_state(session)
                .filter(|state| runtime_turn && matches!(state.activity, "working" | "blocked"))
            {
                self.task = current.context.task_id;
            } else {
                self.task = Some(Uuid::new_v4().to_string());
                self.reporter
                    .update(session, self.task.clone(), None, RuntimeStatus::Working);
            }
        }
        let gate = if approval {
            Some((generation, RuntimeStatus::WaitingApproval))
        } else if question {
            Some((generation, RuntimeStatus::WaitingAnswer))
        } else {
            None
        };
        if self.local_mode && gate != self.gate && (running || !self.reporter.has_result(session)) {
            let status = gate.map(|(_, status)| status).unwrap_or(if running {
                RuntimeStatus::Working
            } else {
                RuntimeStatus::Idle
            });
            self.reporter.update(
                session,
                self.task.clone(),
                gate.map(|(id, _)| format!("local:{id}")),
                status,
            );
        }
        if self.local_mode && self.running && !running && !self.reporter.has_result(session) {
            self.reporter
                .update(session, self.task.clone(), None, RuntimeStatus::Interrupted);
        }
        self.running = running;
        self.gate = gate;
    }

    pub(crate) fn finished(&self, session: Uuid, status: RuntimeStatus) {
        self.reporter
            .update(session, self.task.clone(), None, status);
    }

    pub(crate) fn runtime_snapshot(
        &self,
        snapshot: &crate::daemon::RuntimeSnapshot,
        session: Uuid,
        cursor: u64,
        running: bool,
    ) {
        if !self.local_mode {
            self.reporter
                .runtime_snapshot(snapshot, session, cursor, running);
        }
    }
}
