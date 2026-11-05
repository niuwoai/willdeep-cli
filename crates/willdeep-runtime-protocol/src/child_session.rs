//! Complete child conversations have independent histories; assignments track one delegated Turn.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CHILD_SESSION_SCHEMA: u32 = 2;
pub const CHILD_SESSION_DEPENDENCY_LIMIT: usize = 16;

pub fn initial_revision() -> u64 {
    1
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChildSessionControlState {
    #[default]
    Delegated,
    UserOwned,
    Detached,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChildSessionSubscription {
    None,
    Inbox,
    #[default]
    AutoWake,
}

impl ChildSessionSubscription {
    pub fn from_notify(notify: ChildSessionNotify) -> Self {
        match notify {
            ChildSessionNotify::None => Self::None,
            ChildSessionNotify::OnTaskTerminal => Self::AutoWake,
        }
    }

    pub fn legacy_notify(self) -> ChildSessionNotify {
        match self {
            Self::AutoWake => ChildSessionNotify::OnTaskTerminal,
            Self::None | Self::Inbox => ChildSessionNotify::None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GetChildSessionLinkParams {
    pub parent_session_id: Uuid,
    pub child_session_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChangeChildSessionControlParams {
    pub parent_session_id: Uuid,
    pub child_session_id: Uuid,
    pub expected_control_revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SetChildSessionSubscriptionParams {
    pub parent_session_id: Uuid,
    pub assignment_id: Uuid,
    pub expected_subscription_revision: u64,
    pub subscription: ChildSessionSubscription,
}
pub const CHILD_SESSION_RUNNING_LIMIT: usize = 4;
pub const CHILD_SESSION_QUEUE_LIMIT: usize = 16;
pub const CHILD_SESSION_BRIEF_MAX_CHARS: usize = 16_000;
pub const CHILD_SESSION_SUMMARY_MAX_CHARS: usize = 2_000;
pub const CHILD_SESSION_MODEL_MAX_CHARS: usize = 256;
pub const CHILD_SESSION_WORKTREE_BRANCH_PREFIX: &str = "willdeep/child-";

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChildSessionContext {
    #[default]
    Fresh,
    /// Inherit the parent's completed Turn history before starting the new task.
    Fork,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChildSessionWorkspaceMode {
    #[default]
    SharedReadOnly,
    /// A dedicated writable Git worktree prepared after execution admission.
    Worktree,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChildSessionWorktreeBase {
    /// Start from the committed HEAD only. Parent working-tree changes are not copied.
    #[default]
    Head,
    /// Freeze a copy of the parent's tracked diff and untracked files into the new worktree.
    Snapshot,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChildSessionNotify {
    #[default]
    OnTaskTerminal,
    None,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StartChildSessionParams {
    pub parent_session_id: Uuid,
    #[serde(default)]
    pub created_from_turn_id: Option<Uuid>,
    pub title: String,
    pub prompt: String,
    #[serde(default)]
    pub context: ChildSessionContext,
    /// Required for `context=fork`: the completed parent Turn whose history is copied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through_turn_id: Option<Uuid>,
    #[serde(default)]
    pub workspace_mode: ChildSessionWorkspaceMode,
    #[serde(default)]
    pub worktree_base: ChildSessionWorktreeBase,
    /// Optional model override resolved from the parent's Provider profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub notify: ChildSessionNotify,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<ChildSessionSubscription>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContinueChildSessionParams {
    pub parent_session_id: Uuid,
    #[serde(default)]
    pub created_from_turn_id: Option<Uuid>,
    pub child_session_id: Uuid,
    pub prompt: String,
    #[serde(default)]
    pub notify: ChildSessionNotify,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<ChildSessionSubscription>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<Uuid>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetryChildSessionTaskParams {
    pub parent_session_id: Uuid,
    #[serde(default)]
    pub created_from_turn_id: Option<Uuid>,
    pub assignment_id: Uuid,
    pub expected_result_revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GetChildSessionTaskParams {
    pub parent_session_id: Uuid,
    pub assignment_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ListChildSessionTasksParams {
    pub parent_session_id: Uuid,
    #[serde(default)]
    pub after: Option<Uuid>,
    #[serde(default = "default_page_limit")]
    pub limit: usize,
}

fn default_page_limit() -> usize {
    50
}

impl StartChildSessionParams {
    /// Cross-field checks that serde alone cannot express.
    pub fn validate(&self) -> Result<(), String> {
        match (self.context, self.through_turn_id) {
            (ChildSessionContext::Fork, None) => {
                return Err("context=fork requires through_turn_id".into());
            }
            (ChildSessionContext::Fresh, Some(_)) => {
                return Err("through_turn_id is only valid with context=fork".into());
            }
            _ => {}
        }
        if self.workspace_mode == ChildSessionWorkspaceMode::SharedReadOnly
            && self.worktree_base != ChildSessionWorktreeBase::Head
        {
            return Err("worktree_base requires workspace_mode=worktree".into());
        }
        if let Some(model) = self.model.as_deref() {
            let model = model.trim();
            if model.is_empty() || model.chars().count() > CHILD_SESSION_MODEL_MAX_CHARS {
                return Err(format!(
                    "model override must contain 1 to {CHILD_SESSION_MODEL_MAX_CHARS} characters"
                ));
            }
            if model.chars().any(char::is_control) {
                return Err("model override contains control characters".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChildSessionTaskStatus {
    Preparing,
    Queued,
    Running,
    WaitingApproval,
    WaitingAnswer,
    Completed,
    Partial,
    Failed,
    Cancelled,
    Interrupted,
    Blocked,
}

impl ChildSessionTaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Partial
                | Self::Failed
                | Self::Cancelled
                | Self::Interrupted
                | Self::Blocked
        )
    }
    pub fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::Partial | Self::Failed | Self::Interrupted | Self::Blocked
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildSessionContextSource {
    pub through_turn_id: Uuid,
    pub message_end: usize,
    pub compression_generation: u64,
    /// Stable identity of the immutable parent snapshot used for this fork.
    pub snapshot_id: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildSessionWorktree {
    /// The parent Session workspace that owns the repository.
    pub root_workspace: String,
    /// The child Session workspace: a dedicated worktree directory.
    pub path: String,
    pub branch: String,
    pub base_commit: String,
    pub base: ChildSessionWorktreeBase,
    /// Whether the parent working tree had changes that `head` mode does not copy.
    pub parent_dirty: bool,
    /// SHA-256 of the frozen tracked patch in `snapshot` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_patch_sha256: Option<String>,
    #[serde(default)]
    pub untracked_files: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildSessionLink {
    pub link_id: Uuid,
    #[serde(default)]
    pub control_state: ChildSessionControlState,
    #[serde(default = "initial_revision")]
    pub control_revision: u64,
    #[serde(default)]
    pub control_updated_at: u64,
    pub parent_session_id: Uuid,
    pub child_session_id: Uuid,
    pub created_from_turn_id: Option<Uuid>,
    pub context: ChildSessionContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_source: Option<ChildSessionContextSource>,
    pub workspace_mode: ChildSessionWorkspaceMode,
    pub workspace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<ChildSessionWorktree>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Context window resolved for `model` at creation time, never the parent's window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_context_window: Option<u64>,
    pub created_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildSessionResult {
    pub revision: u64,
    pub final_task_id: Option<Uuid>,
    pub completed_at: u64,
    pub model: Option<String>,
    pub summary: String,
    pub report: Option<String>,
    pub report_complete: bool,
    pub notification_id: Uuid,
    pub consumed_at: Option<u64>,
    pub superseded_by: Option<Uuid>,
    /// Effective child workspace when it differs from the parent's (worktree mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<ChildSessionWorktree>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildSessionTask {
    pub assignment_id: Uuid,
    pub link_id: Uuid,
    pub parent_session_id: Uuid,
    pub child_session_id: Uuid,
    pub created_from_turn_id: Option<Uuid>,
    pub request_id: Uuid,
    pub child_turn_id: Option<Uuid>,
    pub retry_of_assignment_id: Option<Uuid>,
    pub title: String,
    pub prompt: String,
    #[serde(default)]
    pub context: ChildSessionContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub through_turn_id: Option<Uuid>,
    #[serde(default)]
    pub workspace_mode: ChildSessionWorkspaceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub notify: ChildSessionNotify,
    #[serde(default)]
    pub subscription: ChildSessionSubscription,
    #[serde(default = "initial_revision")]
    pub subscription_revision: u64,
    /// Current Link projection, never a separate control authority.
    #[serde(default)]
    pub control_state: ChildSessionControlState,
    #[serde(default = "initial_revision")]
    pub control_revision: u64,
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
    #[serde(default)]
    pub waiting_on_dependencies: bool,
    pub status: ChildSessionTaskStatus,
    pub created_at: u64,
    pub accepted_at: Option<u64>,
    pub updated_at: u64,
    pub result: Option<ChildSessionResult>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChildSessionTaskPage {
    pub tasks: Vec<ChildSessionTask>,
    pub next_cursor: Option<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_defaults_and_unknown_modes_are_explicit() {
        let request = serde_json::json!({"parent_session_id": Uuid::new_v4(), "title": "Investigate", "prompt": "Read the implementation"});
        let parsed: StartChildSessionParams = serde_json::from_value(request.clone()).unwrap();
        assert_eq!(parsed.context, ChildSessionContext::Fresh);
        assert_eq!(
            parsed.workspace_mode,
            ChildSessionWorkspaceMode::SharedReadOnly
        );
        assert_eq!(parsed.worktree_base, ChildSessionWorktreeBase::Head);
        assert_eq!(parsed.notify, ChildSessionNotify::OnTaskTerminal);
        assert_eq!(parsed.model, None);
        for (key, value) in [
            ("context", "resume"),
            ("workspace_mode", "clone"),
            ("worktree_base", "dirty"),
        ] {
            let mut unsupported = request.clone();
            unsupported[key] = value.into();
            assert!(serde_json::from_value::<StartChildSessionParams>(unsupported).is_err());
        }
    }

    #[test]
    fn fork_worktree_and_model_validate_cross_field_constraints() {
        let parent = Uuid::new_v4();
        let through = Uuid::new_v4();
        let fork: StartChildSessionParams = serde_json::from_value(serde_json::json!({
            "parent_session_id": parent, "title": "Continue", "prompt": "Apply the change",
            "context": "fork", "through_turn_id": through, "workspace_mode": "worktree",
            "worktree_base": "snapshot", "model": "deepseek-v4"
        }))
        .unwrap();
        assert_eq!(fork.context, ChildSessionContext::Fork);
        assert_eq!(fork.through_turn_id, Some(through));
        assert_eq!(fork.workspace_mode, ChildSessionWorkspaceMode::Worktree);
        assert_eq!(fork.worktree_base, ChildSessionWorktreeBase::Snapshot);
        assert_eq!(fork.model.as_deref(), Some("deepseek-v4"));
        fork.validate().unwrap();

        let mut missing = fork.clone();
        missing.through_turn_id = None;
        assert!(missing.validate().is_err());
        let mut surprise = fork.clone();
        surprise.context = ChildSessionContext::Fresh;
        assert!(surprise.validate().is_err());
        let mut base = fork.clone();
        base.workspace_mode = ChildSessionWorkspaceMode::SharedReadOnly;
        assert!(base.validate().is_err());
        let mut bad_model = fork.clone();
        bad_model.model = Some("bad\nmodel".into());
        assert!(bad_model.validate().is_err());
    }

    #[test]
    fn fork_boundary_and_worktree_metadata_round_trip() {
        let parent = Uuid::new_v4();
        let child = Uuid::new_v4();
        let through = Uuid::new_v4();
        let link = ChildSessionLink {
            link_id: Uuid::new_v4(),
            control_state: ChildSessionControlState::Delegated,
            control_revision: 1,
            control_updated_at: 1,
            parent_session_id: parent,
            child_session_id: child,
            created_from_turn_id: None,
            context: ChildSessionContext::Fork,
            context_source: Some(ChildSessionContextSource {
                through_turn_id: through,
                message_end: 7,
                compression_generation: 2,
                snapshot_id: Uuid::new_v4(),
            }),
            workspace_mode: ChildSessionWorkspaceMode::Worktree,
            workspace: "/tmp/child-worktree".into(),
            worktree: Some(ChildSessionWorktree {
                root_workspace: "/tmp/parent".into(),
                path: "/tmp/child-worktree".into(),
                branch: "willdeep/child-123456789abc".into(),
                base_commit: "abc123".into(),
                base: ChildSessionWorktreeBase::Snapshot,
                parent_dirty: true,
                baseline_patch_sha256: Some("deadbeef".into()),
                untracked_files: 3,
            }),
            model: Some("deepseek-v4".into()),
            model_context_window: Some(128_000),
            created_at: 1,
        };
        let decoded: ChildSessionLink =
            serde_json::from_value(serde_json::to_value(&link).unwrap()).unwrap();
        assert_eq!(decoded, link);
    }

    #[test]
    fn complete_assignment_fixture_preserves_distinct_identities() {
        let task: ChildSessionTask =
            serde_json::from_str(include_str!("../fixtures/child-session-v1.json")).unwrap();
        assert_eq!(task.status, ChildSessionTaskStatus::Completed);
        assert_ne!(task.parent_session_id, task.child_session_id);
        assert_ne!(Some(task.assignment_id), task.child_turn_id);
        assert!(task.result.as_ref().unwrap().consumed_at.is_none());
        let decoded: ChildSessionTask =
            serde_json::from_value(serde_json::to_value(&task).unwrap()).unwrap();
        assert_eq!(decoded, task);
        let capabilities = crate::RuntimeCapabilities::current("fixture");
        assert!(
            capabilities
                .capabilities
                .contains(&crate::Capability::SessionChildren)
        );
        for operation in [
            "session.child.start",
            "session.child.continue",
            "session.child.retry",
            "session.child.task.get",
            "session.child.task.list",
            "session.child.task.stop",
        ] {
            assert!(
                capabilities
                    .operations
                    .iter()
                    .any(|candidate| candidate == operation)
            );
        }
    }

    #[test]
    fn task_parent_is_independent_of_fork_source_and_legacy_defaults() {
        let legacy = serde_json::json!({
            "id": Uuid::new_v4(), "root_agent_id": Uuid::new_v4(), "workspace": null,
            "profile": null, "model": null, "status": "idle", "active_turn_id": null,
            "created_at": 1, "updated_at": 1
        });
        let old: crate::RuntimeSession = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(old.parent_session_id, None);
        assert!(!old.delegation_read_only);
        assert!(!old.delegation_detached);
        let parent = Uuid::new_v4();
        let mut child = legacy;
        child["parent_session_id"] = parent.to_string().into();
        child["delegation_read_only"] = true.into();
        let child: crate::RuntimeSession = serde_json::from_value(child).unwrap();
        assert_eq!(child.parent_session_id, Some(parent));
        assert!(child.delegation_read_only);
        assert_eq!(child.started_by, None);
    }

    #[test]
    fn p2_fields_are_optional_and_legacy_notify_enum_is_unchanged() {
        let legacy: StartChildSessionParams = serde_json::from_value(serde_json::json!({
            "parent_session_id": Uuid::new_v4(), "title": "Read", "prompt": "Report", "notify": "none"
        })).unwrap();
        assert_eq!(legacy.subscription, None);
        assert!(legacy.depends_on.is_empty());
        let encoded = serde_json::to_value(&legacy).unwrap();
        assert!(encoded.get("subscription").is_none());
        assert!(encoded.get("depends_on").is_none());
        assert!(serde_json::from_value::<ChildSessionNotify>("inbox".into()).is_err());
        let mut modern = encoded;
        modern["subscription"] = "inbox".into();
        modern["depends_on"] = serde_json::json!([Uuid::new_v4()]);
        let modern: StartChildSessionParams = serde_json::from_value(modern).unwrap();
        assert_eq!(modern.subscription, Some(ChildSessionSubscription::Inbox));
        assert_eq!(modern.depends_on.len(), 1);
        let capabilities = crate::RuntimeCapabilities::current("p2");
        for capability in [
            crate::Capability::SessionChildrenControl,
            crate::Capability::SessionChildrenSubscriptions,
            crate::Capability::SessionChildrenDependencies,
        ] {
            assert!(capabilities.capabilities.contains(&capability));
        }
        for operation in [
            "session.child.link.get",
            "session.child.takeover",
            "session.child.detach",
            "session.child.task.subscription.set",
        ] {
            assert!(
                capabilities
                    .operations
                    .iter()
                    .any(|candidate| candidate == operation)
            );
        }
    }

    #[test]
    fn waiting_is_not_terminal_and_cancelled_is_not_retryable() {
        assert!(!ChildSessionTaskStatus::WaitingApproval.is_terminal());
        assert!(!ChildSessionTaskStatus::WaitingAnswer.is_terminal());
        assert!(ChildSessionTaskStatus::Interrupted.is_retryable());
        assert!(ChildSessionTaskStatus::Cancelled.is_terminal());
        assert!(!ChildSessionTaskStatus::Cancelled.is_retryable());
    }
}
