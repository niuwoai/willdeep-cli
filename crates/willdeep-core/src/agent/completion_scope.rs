//! A workspace fingerprint is not a receipt identifying the writer.
use super::*;

pub(super) const TASK_BOUNDARY: &str = "\n[current-task-boundary] Complete the current explicit request and its necessary verification. Historical memory, plans and unfinished work are context, not authorization to resume them. Workspace changes by other actors do not add tasks. A completion or budget gate does not authorize extra work. If an action would expand the current request, ask the user and wait for explicit agreement before executing it. Isolate unrelated historical obligations so an answered query can finish; do not routinely ask to resume them.";

pub(super) const UNCERTAIN_CHANGE: &str = "[completion-verification-scope-unknown] The workspace changed during an opaque tool call, so this executor cannot attribute the change to the current request. Completion remains unverified. This is not permission to run project tests, modify those files or resume historical work. Preserve the result and existing changes; any expansion requires explicit user agreement.";

#[derive(Default)]
pub(super) struct CompletionScope {
    strict: bool,
    uncertain_change: bool,
}

impl CompletionScope {
    pub(super) fn new(resumed: bool) -> Self {
        Self {
            strict: resumed,
            uncertain_change: false,
        }
    }

    pub(super) fn observe(
        &mut self,
        call: &ToolCall,
        before: Result<Option<String>, String>,
        after: Result<Option<String>, String>,
        succeeded: bool,
        may_outlive_call: bool,
    ) {
        self.uncertain_change |= may_outlive_call;
        if matches!((&before, &after), (Ok(before), Ok(after)) if before == after) {
            return;
        }
        // Only explicit workspace-writing tools establish ownership here. Shell,
        // MCP, hooks and delegated work can overlap another writer. Even an error
        // may follow a partial write; neither failure nor output text clears it.
        let applies_files = call.name == "file_changes"
            && serde_json::from_str::<serde_json::Value>(&call.arguments)
                .ok()
                .is_some_and(|args| args["operation"] == "apply");
        if succeeded && (matches!(call.name.as_str(), "create_file" | "edit_file") || applies_files)
        {
            self.strict = true;
        } else {
            self.uncertain_change = true;
        }
    }

    pub(super) fn needs_verification(&self) -> bool {
        self.strict || self.uncertain_change
    }

    pub(super) fn must_pause(&self) -> bool {
        self.uncertain_change && !self.strict
    }

    pub(super) fn restore_uncertainty(&mut self) {
        self.uncertain_change = true;
    }
}
