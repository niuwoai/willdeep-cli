//! Applicability of workspace verification, separate from strict evidence rules.
use super::*;

impl ToolRegistry {
    /// No shell parsing or command-name inference: opaque tools are bracketed
    /// with snapshots, and any difference remains unattributed at this layer.
    pub(crate) fn completion_snapshot_before(
        &self,
        call: &ToolCall,
    ) -> Option<Result<Option<String>, String>> {
        let observation = self.hooks.is_empty()
            && matches!(
                call.name.as_str(),
                "read_file"
                    | "list_directory"
                    | "search_files"
                    | "grep_files"
                    | "read_tool_output"
                    | "git_status"
                    | "git_diff"
                    | "git_log"
                    | "git_blame"
                    | "list_worktrees"
                    | "tool_status"
                    | "web_search_status"
                    | "web_search"
                    | "web_fetch"
                    | "list_skills"
                    | "read_skill"
                    | "update_plan"
                    | "ask_user"
                    | "board_read"
                    | "board_post"
            );
        (!observation).then(|| self.try_verification_baseline())
    }

    pub(crate) fn completion_may_outlive_call(&self, call: &ToolCall) -> bool {
        let Ok(args) = serde_json::from_str::<serde_json::Value>(&call.arguments) else {
            return true;
        };
        args["run_in_background"] == true
            || matches!(
                call.name.as_str(),
                "monitor"
                    | "workflow_control"
                    | "start_session"
                    | "continue_session"
                    | "retry_session_task"
            )
            || (call.name == "run_command"
                && self.command_allowlist.is_none()
                && args["command"].as_str().is_some_and(|command| {
                    !self.accepts_verification(command)
                        && command_wait::leading_sleep_seconds(command).is_some()
                }))
    }
}
