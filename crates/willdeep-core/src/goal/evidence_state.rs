use super::*;

impl GoalContinuation {
    /// Host API: the shared ToolRegistry authority, never a model-provided resolver.
    pub fn set_evidence_resolver(&self, resolver: Arc<dyn GoalEvidenceResolver>) {
        if let Ok(mut slots) = self.slots.lock() {
            slots.resolver = Some(resolver);
            slots.revision = slots.revision.wrapping_add(1);
            if let Some(active) = slots.active.as_mut() {
                Self::forget_verified(&mut active.state);
            }
        }
        self.sync_evidence_goal();
    }

    pub(super) fn sync_evidence_goal(&self) {
        loop {
            let binding = self.slots.lock().ok().and_then(|slots| {
                Some((
                    slots.resolver.clone()?,
                    slots.active.as_ref().map(|g| g.state.goal_id.clone()),
                ))
            });
            let Some((resolver, goal_id)) = binding else {
                return;
            };
            resolver.bind_goal(goal_id.as_deref());
            // A slower old activation must not leave the authority bound to its
            // obsolete identity after a concurrent replacement already returned.
            let stable = self
                .slots
                .lock()
                .map(|slots| {
                    slots
                        .resolver
                        .as_ref()
                        .is_some_and(|current| Arc::ptr_eq(current, &resolver))
                        && slots.active.as_ref().map(|g| &g.state.goal_id) == goal_id.as_ref()
                })
                .unwrap_or(true);
            if stable {
                return;
            }
        }
    }

    pub(super) fn forget_verified(state: &mut GoalState) {
        if state.goal_id.is_empty() {
            state.goal_id = uuid::Uuid::new_v4().to_string();
        }
        for criterion in &mut state.criteria {
            criterion.evidence_verified = false;
        }
    }

    pub(super) fn validate_evidence(
        state: &mut GoalState,
        resolver: Option<&Arc<dyn GoalEvidenceResolver>>,
    ) -> Result<(), String> {
        for criterion in &mut state.criteria {
            criterion.evidence_verified = false;
        }
        if !state.criteria.iter().any(|criterion| criterion.done) {
            return Ok(());
        }
        let resolver = resolver.ok_or(
            "goal evidence resolver is unavailable; run verification through the shared runtime",
        )?;
        let mut first_error = None;
        for criterion in &mut state.criteria {
            if !criterion.done {
                continue;
            }
            let result = match criterion.evidence_ref.as_deref().filter(|r| !r.trim().is_empty()) {
                Some(reference) => resolver.validate(&state.goal_id, &[reference]),
                None => Err("criterion cannot be marked done without evidence_ref; text evidence is explanation only".to_owned()),
            };
            match result {
                Ok(()) => criterion.evidence_verified = true,
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        // A single final validation binds the accepted subset to one current
        // snapshot; separate checks alone could accept different revisions.
        let references = state
            .criteria
            .iter()
            .filter(|criterion| criterion.satisfied())
            .filter_map(|criterion| criterion.evidence_ref.as_deref())
            .collect::<Vec<_>>();
        if !references.is_empty()
            && let Err(error) = resolver.validate(&state.goal_id, &references)
        {
            for criterion in &mut state.criteria {
                criterion.evidence_verified = false;
            }
            return Err(error);
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
