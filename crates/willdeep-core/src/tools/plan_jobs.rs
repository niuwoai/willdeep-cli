use super::*;
use crate::conversation::{Plan, StepStatus};

impl ToolRegistry {
    pub(crate) fn plan_job_status(&self, id: &str) -> Option<StepStatus> {
        if let Some(jobs) = &self.detached_jobs
            && let Some(job) = jobs.get(id).filter(|job| jobs.owns(job))
        {
            return Some(match jobs.state(&job) {
                crate::JobState::Running => StepStatus::InProgress,
                crate::JobState::Finished { exit_code: 0 } => StepStatus::Done,
                crate::JobState::Finished { .. } => StepStatus::Failed,
                crate::JobState::Vanished => StepStatus::Blocked,
            });
        }
        let task = self
            .background
            .snapshots()
            .into_iter()
            .find(|task| task.id == id && task.kind == BackgroundTaskKind::Shell)?;
        Some(match task.status {
            BackgroundTaskStatus::Queued | BackgroundTaskStatus::Running => StepStatus::InProgress,
            BackgroundTaskStatus::Completed if task.exit_code == Some(0) => StepStatus::Done,
            BackgroundTaskStatus::Blocked | BackgroundTaskStatus::Partial => StepStatus::Blocked,
            _ => StepStatus::Failed,
        })
    }

    pub(crate) fn started_plan_job(&self, output: &str) -> Option<String> {
        let id = output
            .lines()
            .find_map(|line| {
                line.strip_prefix("Background job started: ")
                    .or_else(|| line.strip_prefix("Background task started: "))
            })?
            .split([' ', '.'])
            .next()?;
        self.plan_job_status(id).map(|_| id.to_owned())
    }

    /// Read execution evidence, not model-authored notes or notification text.
    pub(crate) fn reconcile_plan_jobs(&self, plan: &mut Option<Plan>) -> bool {
        let Some(plan) = plan else { return false };
        let mut changed = false;
        for index in 0..plan.steps.len() {
            let step = &plan.steps[index];
            if !matches!(
                step.status,
                StepStatus::Pending
                    | StepStatus::InProgress
                    | StepStatus::Done
                    | StepStatus::Failed
                    | StepStatus::Blocked
            ) {
                continue;
            }
            let Some(id) = step.job_id.as_deref() else {
                continue;
            };
            let mut status = match self.plan_job_status(id) {
                Some(status) => status,
                // Retention can remove old execution records after the durable
                // result was saved. Do not undo a verified terminal result.
                None if matches!(step.status, StepStatus::Done | StepStatus::Failed) => continue,
                None => StepStatus::Blocked,
            };
            if matches!(status, StepStatus::InProgress | StepStatus::Done)
                && !crate::planning::waiting_for(plan, step).is_empty()
            {
                status = StepStatus::Blocked;
            }
            if status != step.status {
                plan.steps[index].status = status;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_jobs_finish_independently_and_only_unlock_satisfied_dependencies() {
        let root = std::env::temp_dir().join(format!("plan-jobs-{}", uuid::Uuid::new_v4()));
        let jobs = Arc::new(crate::DetachedJobStore::new(&root).with_owner("session"));
        let write_job = |id: &str, owner: &str, pid: u32| {
            let path = jobs.directory().join(id);
            std::fs::create_dir_all(&path).unwrap();
            let job = crate::DetachedJob {
                id: id.into(),
                command: "test".into(),
                label: "test".into(),
                workspace: root.clone(),
                pid,
                started_marker: None,
                created_at: 0,
                owner: Some(owner.into()),
            };
            std::fs::write(path.join("meta.json"), serde_json::to_vec(&job).unwrap()).unwrap();
        };
        write_job("job_a", "session", std::process::id());
        write_job("job_b", "session", std::process::id());
        write_job("job_foreign", "other", std::process::id());
        write_job("job_vanished", "session", u32::MAX / 2);
        let tools = ToolRegistry::new(&root, ApprovalMode::FullAccess)
            .unwrap()
            .with_detached_jobs(jobs.clone());
        let mut plan = Some(
            serde_json::from_value::<Plan>(serde_json::json!({"steps":[
                {"id":"a","text":"A","status":"in_progress","job_id":"job_a"},
                {"id":"b","text":"B","status":"in_progress","job_id":"job_b"},
                {"id":"c","text":"C","status":"pending","depends_on":["a","b"]}
            ]}))
            .unwrap(),
        );
        assert!(!tools.reconcile_plan_jobs(&mut plan));
        std::fs::write(jobs.directory().join("job_a/exit"), "0").unwrap();
        assert!(tools.reconcile_plan_jobs(&mut plan));
        let receipt = crate::planning::Receipt::new(plan.clone().unwrap(), false);
        assert_eq!(receipt.plan.steps[0].status, StepStatus::Done);
        assert_eq!(receipt.running, ["b"]);
        assert_eq!(receipt.waiting["c"], ["b"]);
        assert!(!tools.reconcile_plan_jobs(&mut plan));
        assert_eq!(tools.plan_job_status("job_foreign"), None);
        assert_eq!(
            tools.plan_job_status("job_vanished"),
            Some(StepStatus::Blocked)
        );
        assert!(
            tools
                .started_plan_job("Background job started: job_foreign (pid 1).")
                .is_none()
        );
        assert_eq!(
            tools.started_plan_job("[background-delay]\nBackground job started: job_a (pid 1)."),
            Some("job_a".into())
        );

        std::fs::write(jobs.directory().join("job_b/exit"), "7").unwrap();
        assert!(tools.reconcile_plan_jobs(&mut plan));
        assert_eq!(plan.as_ref().unwrap().steps[1].status, StepStatus::Failed);
        assert_eq!(
            crate::planning::Receipt::new(plan.clone().unwrap(), false).waiting["c"],
            ["b"]
        );
        write_job("job_retry", "session", std::process::id());
        plan.as_mut().unwrap().steps[1].job_id = Some("job_retry".into());
        plan.as_mut().unwrap().steps[1].status = StepStatus::InProgress;
        // The old attempt now succeeds, but cannot complete the new attempt.
        std::fs::write(jobs.directory().join("job_b/exit"), "0").unwrap();
        assert!(!tools.reconcile_plan_jobs(&mut plan));
        std::fs::write(jobs.directory().join("job_retry/exit"), "0").unwrap();
        assert!(tools.reconcile_plan_jobs(&mut plan));
        assert_eq!(
            crate::planning::Receipt::new(plan.clone().unwrap(), false).ready,
            ["c"]
        );
        std::fs::remove_dir_all(jobs.directory()).unwrap();
        assert!(!tools.reconcile_plan_jobs(&mut plan));
        std::fs::remove_dir_all(root).unwrap();
    }
}
