use super::*;
use crate::conversation::Plan;

impl Agent {
    pub fn reconcile_plan_jobs(&self, plan: &mut Option<Plan>) -> bool {
        self.tools.reconcile_plan_jobs(plan)
    }

    async fn execute_plan_command(
        &self,
        call: &ToolCall,
        plan: &mut Option<Plan>,
    ) -> Result<String, ToolError> {
        let args: serde_json::Value = serde_json::from_str(&call.arguments).map_err(|source| {
            ToolError::InvalidArguments {
                tool: call.name.clone(),
                source,
            }
        })?;
        let Some(id) = args.get("plan_step_id").and_then(|id| id.as_str()) else {
            return self.execute_tool(call).await;
        };
        let invalid = |message: &str| ToolError::InvalidArguments {
            tool: call.name.clone(),
            source: <serde_json::Error as serde::de::Error>::custom(message),
        };
        let current = plan
            .as_ref()
            .ok_or_else(|| invalid("plan_step_id requires an existing plan"))?;
        let step = current
            .steps
            .iter()
            .find(|step| step.id == id)
            .ok_or_else(|| invalid("plan_step_id does not exist"))?;
        if args
            .get("run_in_background")
            .and_then(|value| value.as_bool())
            != Some(true)
            || step.status == crate::conversation::StepStatus::Done
            || !crate::planning::waiting_for(current, step).is_empty()
            || step.job_id.as_deref().is_some_and(|job| {
                self.tools.plan_job_status(job) == Some(crate::conversation::StepStatus::InProgress)
            })
        {
            return Err(invalid(
                "planned command requires background execution, completed dependencies and no running attempt for this step",
            ));
        }
        let output = self.execute_tool(call).await?;
        let job = self.tools.started_plan_job(&output).ok_or_else(|| {
            invalid(
                "command returned no owned background job; reconcile its result before retrying",
            )
        })?;
        let step = plan
            .as_mut()
            .unwrap()
            .steps
            .iter_mut()
            .find(|step| step.id == id)
            .unwrap();
        step.job_id = Some(job);
        step.status = crate::conversation::StepStatus::InProgress;
        self.tools.reconcile_plan_jobs(plan);
        Ok(output)
    }

    pub(super) async fn execute_planned_tool(
        &self,
        call: &ToolCall,
        plan: &mut Option<Plan>,
    ) -> Result<String, ToolError> {
        if call.name == "run_command" {
            return self.execute_plan_command(call, plan).await;
        }
        if call.name != "update_plan" {
            return self.execute_tool(call).await;
        }
        if !self.tools.allows_agent_tool("update_plan") {
            return Err(ToolError::UnknownTool(call.name.clone()));
        }
        let invalid = |message: String| ToolError::InvalidArguments {
            tool: call.name.clone(),
            source: <serde_json::Error as serde::de::Error>::custom(message),
        };
        let update: crate::planning::Update =
            serde_json::from_str(&call.arguments).map_err(|error| invalid(error.to_string()))?;
        let mut next = crate::planning::apply(plan.as_ref(), &update).map_err(&invalid)?;
        // A full model-authored snapshot must not silently erase runtime bindings.
        if !update.merge
            && let Some(previous) = plan.as_ref()
        {
            for step in &mut next.steps {
                let explicitly_bound = update.steps.as_ref().is_some_and(|steps| {
                    steps.iter().any(|change| {
                        change.id.as_deref() == Some(&step.id) && change.job_id.is_some()
                    })
                });
                if !explicitly_bound
                    && let Some(old) = previous
                        .steps
                        .iter()
                        .find(|old| old.id == step.id && old.text == step.text)
                {
                    step.job_id = old.job_id.clone();
                }
            }
        }
        for step in &next.steps {
            if let Some(id) = &step.job_id
                && self.tools.plan_job_status(id).is_none()
            {
                return Err(invalid(format!("plan job {id} is not an owned shell job")));
            }
        }
        let mut reconciled = Some(next);
        self.tools.reconcile_plan_jobs(&mut reconciled);
        next = reconciled.unwrap();
        crate::planning::validate(&next).map_err(&invalid)?;
        let replaced = crate::planning::starts_new_plan(plan.as_ref(), &next, &update);
        let active = self
            .goal_continuation
            .as_ref()
            .filter(|goal| goal.is_active());
        if let Some(goal) = active {
            let steps = update.steps.as_ref().map(|_| {
                next.steps
                    .iter()
                    .map(|step| crate::goal::GoalStep {
                        title: step.text.clone(),
                        status: match step.status {
                            crate::conversation::StepStatus::Pending => {
                                crate::goal::StepStatus::Pending
                            }
                            crate::conversation::StepStatus::InProgress => {
                                crate::goal::StepStatus::InProgress
                            }
                            crate::conversation::StepStatus::Done => crate::goal::StepStatus::Done,
                            _ => crate::goal::StepStatus::Blocked,
                        },
                    })
                    .collect()
            });
            goal.update_plan(crate::goal::PlanUpdate {
                criteria: update.criteria,
                checklist: update.checklist,
                steps,
                blocked_reason: update.blocked_reason,
            })
            .map_err(invalid)?;
        } else if update.criteria.is_some()
            || !update.checklist.is_empty()
            || update.blocked_reason.is_some()
        {
            return Err(invalid("criteria, checklist and blocked_reason require an active /goal; use step status/note for session plans".into()));
        }
        let receipt = crate::planning::Receipt::new(next.clone(), replaced);
        *plan = Some(next);
        serde_json::to_string(&receipt).map_err(|error| invalid(error.to_string()))
    }
}
