//! A persisted delegation cap is independent of mutable approval preferences.
use super::*;

fn child_mutation(name: &str) -> bool {
    matches!(
        name,
        "start_session" | "continue_session" | "retry_session_task"
    )
}

fn child_read_tool(name: &str) -> bool {
    matches!(
        name,
        "read_tool_output"
            | "list_skills"
            | "read_skill"
            | "search_files"
            | "grep_files"
            | "read_file"
            | "list_directory"
            | "get_job_output"
            | "ask_user"
            | "update_plan"
    )
}

impl ToolRegistry {
    /// This ceiling survives changing either shared approval handle to FullAccess.
    pub fn with_delegation_read_only(mut self) -> Self {
        self.delegation_read_only = true;
        self.delegation_hints = false;
        self.hooks = HookRegistry::default();
        self.workflow_host = None;
        self.agent_definition_host = None;
        self
    }

    pub(crate) fn delegation_read_only(&self) -> bool {
        self.delegation_read_only
    }

    pub(crate) fn policy_allows_tool(&self, name: &str) -> bool {
        (!self.delegation_read_only || child_read_tool(name))
            && !(self.approval_mode() == ApprovalMode::ReadOnly
                && (child_mutation(name)
                    || matches!(
                        name,
                        "run_command"
                            | "monitor"
                            | "create_file"
                            | "edit_file"
                            | "create_worktree"
                            | "call_mcp_tool"
                            | "agent_library_manage"
                            | "workflow_control"
                    )
                    || self.mcp.handles(name)))
    }

    pub(crate) fn check_tool_policy(&self, name: &str) -> Result<(), ToolError> {
        if !self.policy_allows_tool(name) {
            return Err(ToolError::ReadOnlyPolicy(name.to_owned()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
