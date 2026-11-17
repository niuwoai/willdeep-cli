//! Host-provided evidence authority. Model-authored text never implements this contract.

/// Resolves opaque references against real, scoped records and the current workspace.
/// Implementations must fail closed for missing records/snapshots, stale results and
/// superseded attempts. This method is always called outside the Goal state lock.
pub trait GoalEvidenceResolver: Send + Sync {
    /// Select the Goal identity stamped on subsequent execution attempts.
    fn bind_goal(&self, goal_id: Option<&str>);

    /// Validate the whole set against one stable current snapshot.
    fn validate(&self, goal_id: &str, references: &[&str]) -> Result<(), String>;
}
