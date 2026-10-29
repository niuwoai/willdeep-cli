//! Dependency-aware session plans. Display state is never execution evidence.
use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::conversation::{Plan, PlanStep, StepStatus};

const MAX_STEPS: usize = 24;
const MAX_TEXT: usize = 500;

pub const SCHEDULING_CONTRACT: &str = r#"Session planning:
- For multi-step work use update_plan with stable ids and explicit depends_on ids. List order is not execution order. Omitted dependencies on a new step mean independent work. When the user changes the objective, revise or replace the plan before dispatching; an old ready entry alone is not authorization to resume unrelated work.
- The tool returns ready, running and waiting. Advance ALL independent ready steps when resources and permissions allow: dispatch bounded workers with spawn_agent(run_in_background=true), or batch eligible read-only tools. Keep doing ready work while external jobs run; join workers only when their results are needed or no independent work remains.
- Dependencies must be done before starting or completing a dependent step. Failed, blocked and skipped prerequisites do not satisfy dependencies: repair/retry them or explicitly revise the plan. Serialize writes to shared files, Git/index/version metadata, deployment targets and other shared resources; add dependencies for these conflicts. Parallelism never changes approval, tool allowlists or budgets.
- Mark a step in_progress only when actually starting its work, and update all concurrently running steps using merge=true. Keep unrelated running steps unchanged. After each result record done/failed/blocked and a concise note, then start newly ready work. A dispatched worker or running CI job is not done; verify its result first.
- When a background command implements an entire plan step, pass its stable plan_step_id to run_command (with run_in_background=true and an explicit timeout sufficient for the work). The runtime binds the actual job attempt and updates that step immediately on exit, independently of model replies or other jobs. For an already started shell job, bind its exact job_id using update_plan. Do not use a note as a job binding. Bind only the final command that proves the step complete; intermediate commands do not complete a larger step. Never overwrite a terminal job result with stale in_progress state. After a failure retry with a new job attempt; old completion notices must not complete the retry.
- On resume, reconcile running steps with existing jobs/reports; never blindly relaunch side effects. Keep the plan current before the final reply. Plan state is progress reporting, not proof of success. Do not emit a second prose/fenced plan when update_plan is available."#;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Update {
    #[serde(default)]
    pub merge: bool,
    pub steps: Option<Vec<StepUpdate>>,
    pub criteria: Option<Vec<String>>,
    #[serde(default)]
    pub checklist: Vec<crate::goal::ChecklistUpdate>,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StepUpdate {
    pub id: Option<String>,
    pub title: Option<String>,
    pub status: Option<StepStatus>,
    pub note: Option<String>,
    pub depends_on: Option<Vec<String>>,
    pub job_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub plan: Plan,
    pub replaced: bool,
    pub ready: Vec<String>,
    pub running: Vec<String>,
    pub waiting: BTreeMap<String, Vec<String>>,
}

impl Receipt {
    pub fn new(plan: Plan, replaced: bool) -> Self {
        let mut result = Self {
            plan,
            replaced,
            ready: Vec::new(),
            running: Vec::new(),
            waiting: BTreeMap::new(),
        };
        for step in &result.plan.steps {
            if step.status == StepStatus::InProgress {
                result.running.push(step.id.clone());
            } else if step.status == StepStatus::Pending {
                let waiting = waiting_for(&result.plan, step);
                if waiting.is_empty() {
                    result.ready.push(step.id.clone());
                } else {
                    result.waiting.insert(step.id.clone(), waiting);
                }
            }
        }
        result
    }
}

pub fn waiting_for(plan: &Plan, step: &PlanStep) -> Vec<String> {
    step.depends_on
        .iter()
        .filter(|id| {
            !plan
                .steps
                .iter()
                .any(|other| &other.id == *id && other.status == StepStatus::Done)
        })
        .cloned()
        .collect()
}

pub(crate) fn apply(current: Option<&Plan>, update: &Update) -> Result<Plan, String> {
    let empty = Plan {
        summary: String::new(),
        steps: Vec::new(),
    };
    let Some(updates) = &update.steps else {
        return Ok(current.cloned().unwrap_or(empty));
    };
    if updates.len() > MAX_STEPS {
        return Err(format!("at most {MAX_STEPS} steps are allowed"));
    }
    let mut next = if update.merge {
        current.cloned().unwrap_or(empty)
    } else {
        empty
    };
    let mut seen = HashSet::new();
    for (index, change) in updates.iter().enumerate() {
        let id = change
            .id
            .clone()
            .or_else(|| (!update.merge).then(|| (index + 1).to_string()))
            .ok_or("merge updates require a stable step id")?;
        if id.is_empty()
            || id.len() > 64
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            || !seen.insert(id.clone())
        {
            return Err(
                "step ids must be unique, 1–64 ASCII letters, digits, hyphens or underscores"
                    .into(),
            );
        }
        let existing = next.steps.iter_mut().find(|step| step.id == id);
        if let Some(step) = existing {
            patch_step(step, change);
        } else {
            let mut step = PlanStep {
                id,
                text: String::new(),
                status: StepStatus::Pending,
                detail: None,
                depends_on: Vec::new(),
                job_id: None,
            };
            patch_step(&mut step, change);
            next.steps.push(step);
        }
    }
    validate(&next)?;
    Ok(next)
}

pub(crate) fn starts_new_plan(current: Option<&Plan>, next: &Plan, update: &Update) -> bool {
    update.steps.is_some()
        && !update.merge
        && current.is_none_or(|previous| {
            !next.steps.is_empty()
                && !previous.steps.iter().any(|old| {
                    next.steps
                        .iter()
                        .any(|step| step.id == old.id && step.text == old.text)
                })
        })
}

fn patch_step(step: &mut PlanStep, update: &StepUpdate) {
    if let Some(title) = &update.title {
        step.text = title.trim().to_owned();
    }
    if let Some(status) = &update.status {
        step.status = status.clone();
    }
    if let Some(note) = &update.note {
        step.detail = (!note.trim().is_empty()).then(|| note.trim().to_owned());
    }
    if let Some(dependencies) = &update.depends_on {
        step.depends_on = dependencies.clone();
    }
    if let Some(job_id) = &update.job_id {
        step.job_id = (!job_id.is_empty()).then(|| job_id.clone());
    }
}

pub fn validate(plan: &Plan) -> Result<(), String> {
    if plan.steps.len() > MAX_STEPS {
        return Err(format!("at most {MAX_STEPS} steps are allowed"));
    }
    let ids: HashSet<_> = plan.steps.iter().map(|step| step.id.as_str()).collect();
    let mut jobs = HashSet::new();
    if ids.len() != plan.steps.len() {
        return Err("step ids must be unique".into());
    }
    for step in &plan.steps {
        if let Some(job) = &step.job_id
            && (job.is_empty()
                || job.len() > 128
                || !job
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                || !jobs.insert(job))
        {
            return Err("each job must bind at most one step using a valid job id".into());
        }
        if step.text.trim().is_empty()
            || step.text.chars().count() > MAX_TEXT
            || step
                .detail
                .as_ref()
                .is_some_and(|note| note.chars().count() > MAX_TEXT)
        {
            return Err(format!(
                "step {} requires a title and title/note must not exceed {MAX_TEXT} characters",
                step.id
            ));
        }
        let mut seen = HashSet::new();
        for dependency in &step.depends_on {
            if dependency == &step.id
                || !ids.contains(dependency.as_str())
                || !seen.insert(dependency)
            {
                return Err(format!(
                    "step {} has a missing, duplicate or self dependency: {dependency}",
                    step.id
                ));
            }
        }
        if matches!(step.status, StepStatus::InProgress | StepStatus::Done)
            && !waiting_for(plan, step).is_empty()
        {
            return Err(format!(
                "step {} cannot start or finish until dependencies are done: {}",
                step.id,
                waiting_for(plan, step).join(", ")
            ));
        }
    }
    let mut resolved = HashSet::new();
    loop {
        let before = resolved.len();
        for step in &plan.steps {
            if step
                .depends_on
                .iter()
                .all(|id| resolved.contains(id.as_str()))
            {
                resolved.insert(step.id.as_str());
            }
        }
        if resolved.len() == plan.steps.len() {
            return Ok(());
        }
        if before == resolved.len() {
            return Err("plan dependencies contain a cycle".into());
        }
    }
}

/// Only a successful update_plan result is a structured plan receipt.
pub fn receipt(output: &str) -> Option<Receipt> {
    let receipt: Receipt = serde_json::from_str(output).ok()?;
    validate(&receipt.plan).ok()?;
    Some(receipt)
}

#[cfg(test)]
mod tests;
