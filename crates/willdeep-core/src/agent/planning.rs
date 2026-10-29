use super::*;
use crate::conversation::Plan;

impl Agent {
    pub(super) async fn execute_planned_tool(
        &self,
        call: &ToolCall,
        plan: &mut Option<Plan>,
    ) -> Result<String, ToolError> {
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
        let next = crate::planning::apply(plan.as_ref(), &update).map_err(invalid)?;
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
