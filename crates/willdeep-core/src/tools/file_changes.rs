//! Content-versioned edits and one-shot, bounded multi-file change previews.
//!
//! The coordinator serializes cooperating registries in this process. Hash checks
//! and atomic per-file installation do NOT implement CAS against arbitrary
//! external writers, nor a filesystem-wide transaction. Commit and recovery have
//! no await points: cancelling during approval writes nothing; commit/recovery
//! finish together. A later PostTool hook or result delivery can still be
//! cancelled: a missing receipt remains an unknown effect, never a safe retry.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{ApprovalMode, EditArgs, ToolError, ToolRegistry};
use crate::types::{ToolCall, ToolDefinition};

mod disk;
#[cfg(test)]
mod tests;

pub(super) const MAX_FILES: usize = 32;
pub(super) const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 4096;
const MAX_ARGUMENT_BYTES: usize = MAX_TOTAL_BYTES * 6 + MAX_FILES * MAX_PATH_BYTES;
const MAX_PREVIEWS: usize = 8;
const MAX_CACHED_BYTES: usize = 16 * 1024 * 1024;
const MAX_BASELINES: usize = 1024;
const PREVIEW_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_DIFF_BYTES: usize = 8 * 1024;
const MAX_ALL_DIFF_BYTES: usize = 64 * 1024;
const DIFF_CONTEXT_LINES: usize = 3;

static WRITE_COORDINATOR: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Read a regular UTF-8 file in an already-authorized Workspace without
/// changing edit baselines. Never follow symlinks within the Workspace.
pub fn read_workspace_preview(workspace: &Path, requested: &Path) -> std::io::Result<String> {
    let invalid = || std::io::Error::other("not a regular workspace text file");
    let root = workspace.canonicalize()?;
    // Session roots are canonical when registered. A root replaced with a
    // symlink must not extend the Workspace allowlist.
    if root != workspace {
        return Err(invalid());
    }
    #[cfg(windows)]
    let ordinary_root = root
        .to_str()
        .and_then(|path| path.strip_prefix(r"\\?\"))
        .map(Path::new);
    #[cfg(not(windows))]
    let ordinary_root: Option<&Path> = None;
    let path = if requested.is_absolute() {
        requested
            .strip_prefix(&root)
            .ok()
            .or_else(|| ordinary_root.and_then(|root| requested.strip_prefix(root).ok()))
            .ok_or_else(invalid)?
    } else {
        requested
    };
    let mut destination = root.clone();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::Normal(name) => {
                let name = name.to_str().ok_or_else(invalid)?;
                if name.starts_with('.')
                    || name.contains([':', '\\'])
                    || name.chars().any(char::is_control)
                    || (cfg!(windows) && name.ends_with(['.', ' ']))
                {
                    return Err(invalid());
                }
                destination.push(name);
            }
            _ => return Err(invalid()),
        }
    }
    if path.file_name().is_none() {
        return Err(invalid());
    }
    let bytes = disk::read_under(&root, &destination, super::MAX_READ_BYTES)?.bytes;
    if bytes.contains(&0) {
        return Err(invalid());
    }
    String::from_utf8(bytes).map_err(|_| invalid())
}

/// Other cooperating writers (notably create_file) must take the same lock,
/// after approval and before touching the filesystem.
pub(super) async fn write_lock() -> tokio::sync::MutexGuard<'static, ()> {
    WRITE_COORDINATOR.lock().await
}

#[derive(Default)]
pub(super) struct FileChangeState {
    inner: Mutex<State>,
    #[cfg(test)]
    fault: Mutex<Option<TestFault>>,
}

/// A cached preview is unpublished until its caller finishes every fallible
/// post-check. Dropping the owning future revokes that entry synchronously.
/// This uses the same bounded cache and coordinator as ordinary file_changes.
#[must_use = "publish the preview only after all asynchronous checks succeed"]
pub(super) struct PreviewLease<'a> {
    state: &'a FileChangeState,
    id: Option<String>,
    output: String,
}

impl PreviewLease<'_> {
    pub(super) fn publish(mut self) -> String {
        self.id = None;
        std::mem::take(&mut self.output)
    }
}

impl Drop for PreviewLease<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            self.state
                .inner
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .previews
                .remove(&id);
        }
    }
}

#[derive(Default)]
struct State {
    baselines: HashMap<PathBuf, String>,
    baseline_order: VecDeque<PathBuf>,
    previews: HashMap<String, Preview>,
}

impl FileChangeState {
    /// `path` must be the resolved canonical path whose complete bytes were
    /// actually read, even when read_file returns only a page of those bytes.
    pub(super) fn observe(&self, path: &Path, content: &[u8]) -> String {
        let version = version(content);
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state.baseline_order.retain(|entry| entry != path);
        state.baseline_order.push_back(path.to_path_buf());
        state.baselines.insert(path.to_path_buf(), version.clone());
        while state.baselines.len() > MAX_BASELINES {
            if let Some(oldest) = state.baseline_order.pop_front() {
                state.baselines.remove(&oldest);
            }
        }
        version
    }

    fn forget(&self, path: &Path) {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state.baselines.remove(path);
        state.baseline_order.retain(|entry| entry != path);
    }

    fn expected(&self, path: &Path, explicit: Option<&str>) -> Result<String, ToolError> {
        if let Some(explicit) = explicit {
            if explicit.len() != 71
                || !explicit.starts_with("sha256:")
                || !explicit[7..]
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(change_error(
                    "expected_version must be sha256: followed by 64 lowercase hex digits",
                ));
            }
            return Ok(explicit.to_owned());
        }
        self.inner
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .baselines
            .get(path)
            .cloned()
            .ok_or_else(|| ToolError::ReadRequired(path.display().to_string()))
    }

    fn store(&self, preview: Preview) -> Result<PreviewLease<'_>, ToolError> {
        let output = preview.render().to_string();
        let id = preview.id.clone();
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state
            .previews
            .retain(|_, entry| entry.created.elapsed() < PREVIEW_TTL);
        let bytes: usize = state.previews.values().map(Preview::bytes).sum();
        if state.previews.len() >= MAX_PREVIEWS || bytes + preview.bytes() > MAX_CACHED_BYTES {
            return Err(change_error(
                "preview cache is full; apply an existing preview or wait for expiration",
            ));
        }
        state.previews.insert(id.clone(), preview);
        Ok(PreviewLease {
            state: self,
            id: Some(id),
            output,
        })
    }

    fn take(&self, id: &str) -> Result<Preview, ToolError> {
        let mut state = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        state
            .previews
            .retain(|_, entry| entry.created.elapsed() < PREVIEW_TTL);
        state.previews.remove(id).ok_or_else(|| {
            change_error(
                "unknown, expired, or already consumed preview_id; preview again before retrying",
            )
        })
    }
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Preview { changes: Vec<ChangeInput> },
    Apply { preview_id: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangeInput {
    path: String,
    operation: Operation,
    content: String,
    expected_version: Option<String>,
}

/// Internal semantic-edit contract: the caller must hash a snapshot obtained
/// before requesting edits. The preview coordinator rechecks that exact version.
/// This does not dispatch a nested tool or run a second set of hooks.
pub(super) struct VersionedReplacement {
    pub(super) path: String,
    pub(super) expected_version: String,
    pub(super) content: String,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Create,
    Replace,
}

struct BoundChange {
    requested: String,
    /// Stable workspace-relative destination, independent of symlink aliases.
    reported_path: String,
    path: PathBuf,
    parent: disk::DirectoryIdentity,
    operation: Operation,
    before: Option<disk::Snapshot>,
    after: Vec<u8>,
}

impl BoundChange {
    fn bytes(&self) -> usize {
        self.after.len() + self.before.as_ref().map_or(0, |before| before.bytes.len())
    }

    fn expected(&self) -> Option<String> {
        self.before.as_ref().map(|before| version(&before.bytes))
    }
}

struct Preview {
    id: String,
    binding: String,
    created: Instant,
    changes: Vec<BoundChange>,
}

impl Preview {
    fn new(changes: Vec<BoundChange>) -> Self {
        let mut digest = Sha256::new();
        for change in &changes {
            // Length prefixes prevent ambiguous concatenations. The digest also
            // binds the canonical destination, not just the caller's spelling.
            for bytes in [
                change.requested.as_bytes(),
                change.path.as_os_str().as_encoded_bytes(),
                match change.operation {
                    Operation::Create => b"create",
                    Operation::Replace => b"replace",
                },
                change.expected().as_deref().unwrap_or("absent").as_bytes(),
                &change.after,
            ] {
                digest.update((bytes.len() as u64).to_le_bytes());
                digest.update(bytes);
            }
        }
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            binding: format!("sha256:{:x}", digest.finalize()),
            created: Instant::now(),
            changes,
        }
    }

    fn bytes(&self) -> usize {
        self.changes.iter().map(BoundChange::bytes).sum()
    }

    fn render(&self) -> serde_json::Value {
        let mut remaining = MAX_ALL_DIFF_BYTES;
        let changes: Vec<_> = self
            .changes
            .iter()
            .map(|change| {
                let before = change
                    .before
                    .as_ref()
                    .map_or(&[][..], |snapshot| snapshot.bytes.as_slice());
                let (diff, truncated) =
                    bounded_diff(before, &change.after, remaining.min(MAX_DIFF_BYTES));
                remaining = remaining.saturating_sub(diff.len());
                json!({
                    "path": change.requested,
                    "canonical_path": change.path.to_str(),
                    "canonical_path_display": change.path.to_string_lossy(),
                    "target_path": change.reported_path,
                    "operation": change.operation,
                    "expected_version": change.expected(),
                    "new_version": version(&change.after),
                    "before_bytes": before.len(),
                    "after_bytes": change.after.len(),
                    "diff": diff,
                    "diff_truncated": truncated,
                })
            })
            .collect();
        json!({
            "status": "preview",
            "files_touched": [],
            "preview_id": self.id,
            "binding": self.binding,
            "expires_in_seconds": PREVIEW_TTL.as_secs(),
            "file_count": self.changes.len(),
            "snapshot_bytes": self.bytes(),
            "changes": changes,
            "guarantee": "cooperating in-process writers are serialized; per-file atomic installation; no external-process CAS or cross-file atomicity",
        })
    }
}

#[derive(Serialize)]
struct Problem {
    path: String,
    reason: String,
}

#[derive(Serialize)]
struct CommitReport {
    status: &'static str,
    preview_id: String,
    binding: String,
    /// Historical writes, including those subsequently rolled back.
    applied: Vec<String>,
    /// Actually installed paths not subsequently restored. Empty on preview,
    /// preflight failure, and complete rollback; consumed by WorkerFacts.
    files_touched: Vec<String>,
    rolled_back: Vec<String>,
    conflicts: Vec<Problem>,
    recovery_failed: Vec<Problem>,
    cleanup_failed: Vec<Problem>,
    versions: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edit_display: Option<super::edit_display::EditDisplay>,
}

impl CommitReport {
    fn new(preview: &Preview) -> Self {
        Self {
            status: "preflight_failed",
            preview_id: preview.id.clone(),
            binding: preview.binding.clone(),
            applied: Vec::new(),
            files_touched: Vec::new(),
            rolled_back: Vec::new(),
            conflicts: Vec::new(),
            recovery_failed: Vec::new(),
            cleanup_failed: Vec::new(),
            versions: HashMap::new(),
            edit_display: None,
        }
    }

    fn fail(self) -> ToolError {
        ToolError::FileChange(serde_json::to_string(&self).expect("serializable commit report"))
    }
}

pub(super) fn definition() -> ToolDefinition {
    super::definition(
        "file_changes",
        "Preview or apply a bounded set of versioned file creates/replacements. Preview writes nothing and returns a one-shot preview_id binding paths, content, versions and a bounded diff. Replace requires expected_version from read_file or this registry's actual read baseline. Create requires an absent destination and existing parent directory. Apply accepts only preview_id, rechecks every path/permission/version, then obtains one approval for the bound diff. No delete/rename. Cooperating registries serialize writes; arbitrary external writers are not covered by CAS. Each file installs atomically; partial failures report applied/rolled_back/conflicts/recovery_failed. Previews expire after 10 minutes and are consumed even on denial/failure. Limits: 32 files, 4 MiB combined old/new bytes, 8 previews, 16 MiB cache.",
        json!({
            "type": "object",
            "oneOf": [
                {"properties": {
                    "operation": {"const": "preview"},
                    "changes": {"type":"array", "minItems":1, "maxItems":MAX_FILES,
                        "items":{"type":"object", "properties":{
                            "path":{"type":"string", "minLength":1, "maxLength":MAX_PATH_BYTES},
                            "operation":{"enum":["create","replace"]},
                            "content":{"type":"string", "maxLength":MAX_TOTAL_BYTES},
                            "expected_version":{"type":"string", "pattern":"^sha256:[0-9a-f]{64}$"}
                        }, "required":["path","operation","content"], "additionalProperties":false}}
                }, "required":["operation","changes"], "additionalProperties":false},
                {"properties":{"operation":{"const":"apply"}, "preview_id":{"type":"string", "format":"uuid"}},
                 "required":["operation","preview_id"], "additionalProperties":false}
            ]
        }),
    )
}

impl ToolRegistry {
    pub(super) async fn edit_file_versioned(&self, args: EditArgs) -> Result<String, ToolError> {
        self.check_tool_policy("edit_file")?;
        self.file_change_writable()?;
        if args.old_string == args.new_string {
            return Err(ToolError::IdenticalEdit);
        }
        if args.old_string.is_empty() {
            return Err(change_error("old_string must not be empty"));
        }
        let preview = {
            let _lock = write_lock().await;
            self.require_write_target(&args.path, false)?;
            validate_path(&args.path)?;
            let path = self.resolve_existing(&args.path)?;
            let parent = disk::DirectoryIdentity::capture(&path)?;
            let before = disk::read(&path, &parent, MAX_TOTAL_BYTES)?;
            let expected = self
                .file_changes
                .expected(&path, args.expected_version.as_deref())?;
            check_version(&args.path, &expected, &before.bytes)?;
            if before.permissions.readonly() {
                return Err(change_error(format!("file is read-only: {}", args.path)));
            }
            let content = std::str::from_utf8(&before.bytes).map_err(|error| {
                change_error(format!("edit {} requires UTF-8 text: {error}", args.path))
            })?;
            let old = super::match_edit_text(content, &args.old_string)
                .ok_or_else(|| ToolError::EditTextNotFound(args.path.clone()))?;
            let count = content.matches(&old).count();
            if count > 1 && !args.replace_all.unwrap_or(false) {
                return Err(ToolError::EditTextNotUnique {
                    path: args.path,
                    count,
                });
            }
            let updated = if args.replace_all.unwrap_or(false) {
                content.replace(&old, &args.new_string)
            } else {
                content.replacen(&old, &args.new_string, 1)
            };
            let change = BoundChange {
                requested: args.path,
                reported_path: self.change_receipt_path(&path)?,
                path,
                parent,
                operation: Operation::Replace,
                before: Some(before),
                after: updated.into_bytes(),
            };
            if change.bytes() > MAX_TOTAL_BYTES {
                return Err(change_error(
                    "edit old/new content exceeds the 4 MiB combined limit",
                ));
            }
            Preview::new(vec![change])
        };
        let _lock = self.approve_and_lock(&preview, "edit_file").await?;
        // Keep the explicit EditConflict type for ordinary edit_file callers.
        self.check_change(&preview.changes[0])?;
        self.commit_changes(&preview)
    }

    pub(super) async fn file_changes(&self, call: &ToolCall) -> Result<String, ToolError> {
        self.check_tool_policy("file_changes")?;
        if call.arguments.len() > MAX_ARGUMENT_BYTES {
            return Err(change_error(
                "file_changes arguments exceed the bounded request size",
            ));
        }
        match super::parse::<Request>(call)? {
            Request::Preview { changes } => self.preview_changes(changes).await,
            Request::Apply { preview_id } => {
                self.file_change_writable()?;
                // Removing before awaiting approval makes retries and concurrent
                // apply calls one-shot, including denied/cancelled attempts.
                let preview = self.file_changes.take(&preview_id)?;
                {
                    let _lock = write_lock().await;
                    self.check_all(&preview)?;
                }
                let _lock = self.approve_and_lock(&preview, "file_changes").await?;
                self.commit_changes(&preview)
            }
        }
    }

    pub(super) async fn preview_versioned_replacements(
        &self,
        replacements: Vec<VersionedReplacement>,
    ) -> Result<PreviewLease<'_>, ToolError> {
        self.lease_preview_changes(
            replacements
                .into_iter()
                .map(|replacement| ChangeInput {
                    path: replacement.path,
                    operation: Operation::Replace,
                    content: replacement.content,
                    expected_version: Some(replacement.expected_version),
                })
                .collect(),
        )
        .await
    }

    async fn preview_changes(&self, changes: Vec<ChangeInput>) -> Result<String, ToolError> {
        Ok(self.lease_preview_changes(changes).await?.publish())
    }

    async fn lease_preview_changes(
        &self,
        changes: Vec<ChangeInput>,
    ) -> Result<PreviewLease<'_>, ToolError> {
        let _lock = write_lock().await;
        // A semantic tool must not bypass file_changes authorization merely
        // because its own tool name is permitted. Recheck after waiting, too.
        self.check_tool_policy("file_changes")?;
        self.file_change_writable()?;
        let preview = self.prepare_changes(changes)?;
        self.file_changes.store(preview)
    }

    /// Approval never holds the write coordinator. A mode which allowed an
    /// automatic write can become Strict while this task waits for the lock;
    /// in that case release it and obtain explicit approval before proceeding.
    async fn approve_and_lock(
        &self,
        preview: &Preview,
        tool: &str,
    ) -> Result<tokio::sync::MutexGuard<'static, ()>, ToolError> {
        let description = approval_description(preview);
        loop {
            self.check_tool_policy(tool)?;
            self.file_change_writable()?;
            let explicit = match self.approval_mode() {
                ApprovalMode::ReadOnly => return Err(ToolError::ReadOnlyPolicy(tool.to_owned())),
                ApprovalMode::Strict => true,
                ApprovalMode::Smart | ApprovalMode::WorkspaceAccess | ApprovalMode::FullAccess => {
                    false
                }
            };
            if explicit {
                // `false` forces the explicit gate even if the mode changes
                // between the snapshot above and require_approval's own read.
                self.require_approval(&description, false).await?;
            }
            let lock = write_lock().await;
            self.check_tool_policy(tool)?;
            self.file_change_writable()?;
            if !explicit && self.approval_mode() == ApprovalMode::Strict {
                self.check_all(preview)?;
                drop(lock);
                continue;
            }
            return Ok(lock);
        }
    }

    fn file_change_writable(&self) -> Result<(), ToolError> {
        if self.approval_mode() == ApprovalMode::ReadOnly {
            Err(ToolError::ReadOnlyPolicy(
                "file_changes/edit_file".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    fn prepare_changes(&self, changes: Vec<ChangeInput>) -> Result<Preview, ToolError> {
        if changes.is_empty() || changes.len() > MAX_FILES {
            return Err(change_error("preview requires between 1 and 32 files"));
        }
        let mut seen = HashSet::new();
        let mut prepared = Vec::with_capacity(changes.len());
        let mut total = 0usize;
        for input in changes {
            validate_path(&input.path)?;
            let create = matches!(input.operation, Operation::Create);
            self.require_write_target(&input.path, create)?;
            let path = if create {
                if input.expected_version.is_some() {
                    return Err(change_error("create must not specify expected_version"));
                }
                self.new_change_path(&input.path)?
            } else {
                self.resolve_existing(&input.path)?
            };
            if !seen.insert(path.clone()) {
                return Err(change_error(format!(
                    "duplicate canonical destination: {}",
                    input.path
                )));
            }
            let parent = disk::DirectoryIdentity::capture(&path)?;
            let before = if create {
                disk::ensure_absent(&path)?;
                None
            } else {
                let before = disk::read(&path, &parent, MAX_TOTAL_BYTES.saturating_sub(total))?;
                std::str::from_utf8(&before.bytes).map_err(|error| {
                    change_error(format!(
                        "replace {} requires UTF-8 text: {error}",
                        input.path
                    ))
                })?;
                let expected = self
                    .file_changes
                    .expected(&path, input.expected_version.as_deref())?;
                check_version(&input.path, &expected, &before.bytes)?;
                if before.permissions.readonly() {
                    return Err(change_error(format!("file is read-only: {}", input.path)));
                }
                Some(before)
            };
            let change = BoundChange {
                requested: input.path,
                reported_path: self.change_receipt_path(&path)?,
                path,
                parent,
                operation: input.operation,
                before,
                after: input.content.into_bytes(),
            };
            total = total
                .checked_add(change.bytes())
                .ok_or_else(|| change_error("preview size overflow"))?;
            if total > MAX_TOTAL_BYTES {
                return Err(change_error(
                    "combined old/new content exceeds the 4 MiB preview limit",
                ));
            }
            prepared.push(change);
        }
        let preview = Preview::new(prepared);
        self.check_all(&preview)?;
        Ok(preview)
    }

    pub(super) fn change_receipt_path(&self, path: &Path) -> Result<String, ToolError> {
        path.strip_prefix(&self.workspace)
            .ok()
            .and_then(Path::to_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                change_error("resolved destination must have a UTF-8 workspace-relative path")
            })
    }

    pub(super) fn new_change_path(&self, requested: &str) -> Result<PathBuf, ToolError> {
        let lexical = self.resolve_new(requested)?;
        // No directory creation in preview/preflight: every destination has an
        // existing, resolved parent which can be bound and later revalidated.
        let parent = lexical
            .parent()
            .ok_or_else(|| change_error("destination needs a parent"))?
            .canonicalize()?;
        self.ensure_inside(parent.clone(), requested)?;
        let name = lexical
            .file_name()
            .ok_or_else(|| change_error("destination needs a filename"))?;
        Ok(parent.join(name))
    }

    fn check_change(&self, change: &BoundChange) -> Result<(), ToolError> {
        self.file_change_writable()?;
        self.require_write_target(
            &change.requested,
            matches!(change.operation, Operation::Create),
        )?;
        let resolved = match change.operation {
            Operation::Create => self.new_change_path(&change.requested)?,
            Operation::Replace => self.resolve_existing(&change.requested)?,
        };
        if resolved != change.path {
            return Err(change_error(format!(
                "real path changed since preview: {}",
                change.requested
            )));
        }
        change.parent.check(&change.path)?;
        match &change.before {
            Some(before) => {
                let current = disk::read(&change.path, &change.parent, MAX_TOTAL_BYTES)?;
                check_version(&change.requested, &version(&before.bytes), &current.bytes)?;
                if current.permissions.readonly() {
                    return Err(change_error(format!(
                        "file is read-only: {}",
                        change.requested
                    )));
                }
                if current.permissions != before.permissions {
                    return Err(change_error(format!(
                        "file permissions changed since preview: {}",
                        change.requested
                    )));
                }
            }
            None => disk::ensure_absent(&change.path)?,
        }
        Ok(())
    }

    fn check_all(&self, preview: &Preview) -> Result<(), ToolError> {
        let mut report = CommitReport::new(preview);
        if preview.created.elapsed() >= PREVIEW_TTL {
            report.conflicts.push(Problem {
                path: String::new(),
                reason: "preview expired".to_owned(),
            });
        }
        for change in &preview.changes {
            if let Err(error) = self.check_change(change) {
                report.conflicts.push(problem(change, error));
            }
        }
        if report.conflicts.is_empty() {
            Ok(())
        } else {
            Err(report.fail())
        }
    }

    /// Synchronous by design. No task cancellation point exists between the
    /// first installed file and the final recovery/report. All backups live in
    /// memory and staging starts only after approval.
    fn commit_changes(&self, preview: &Preview) -> Result<String, ToolError> {
        self.check_all(preview)?;
        let mut report = CommitReport::new(preview);
        let mut staged = Vec::with_capacity(preview.changes.len());
        for change in &preview.changes {
            match disk::StagedFile::new(
                &change.path,
                &change.parent,
                &change.after,
                change.before.as_ref().map(|before| &before.permissions),
            ) {
                Ok(file) => staged.push(file),
                Err(error) => {
                    let error = stage_error(error, &mut report);
                    report.conflicts.push(problem(change, error));
                    cleanup(&mut staged, &mut report);
                    return Err(report.fail());
                }
            }
        }
        // Staging may take time. Recheck the entire set before the first write.
        if let Err(error) = self.check_all(preview) {
            report.conflicts.push(Problem {
                path: String::new(),
                reason: error.to_string(),
            });
            cleanup(&mut staged, &mut report);
            return Err(report.fail());
        }
        let mut installed = 0;
        for (index, change) in preview.changes.iter().enumerate() {
            let result = self.check_change(change).and_then(|()| {
                #[cfg(test)]
                self.inject_commit_failure(index)?;
                staged[index]
                    .install(matches!(change.operation, Operation::Create))
                    .map_err(ToolError::Io)
            });
            match result {
                Ok(()) => {
                    installed += 1;
                    report.applied.push(change.requested.clone());
                    let new_version = self.file_changes.observe(&change.path, &change.after);
                    report
                        .versions
                        .insert(change.requested.clone(), new_version);
                }
                Err(error) => {
                    report.conflicts.push(problem(change, error));
                    break;
                }
            }
        }
        if installed == preview.changes.len() {
            // Detect an external writer changing an earlier file during commit.
            for change in &preview.changes {
                if let Err(error) = self.check_installed(change) {
                    report.conflicts.push(problem(change, error));
                }
            }
        }
        if !report.conflicts.is_empty() {
            for change in preview.changes[..installed].iter().rev() {
                match self.restore_change(change, &mut report) {
                    Ok(()) => {
                        report.rolled_back.push(change.requested.clone());
                        report.versions.remove(&change.requested);
                        if let Some(before) = &change.before {
                            let old_version =
                                self.file_changes.observe(&change.path, &before.bytes);
                            report
                                .versions
                                .insert(change.requested.clone(), old_version);
                        } else {
                            self.file_changes.forget(&change.path);
                        }
                    }
                    Err(error) => {
                        report.versions.remove(&change.requested);
                        self.file_changes.forget(&change.path);
                        report.recovery_failed.push(problem(change, error));
                    }
                }
            }
            report.files_touched = preview.changes[..installed]
                .iter()
                .filter(|change| !report.rolled_back.contains(&change.requested))
                .map(|change| change.reported_path.clone())
                .collect();
            report.status = if report.recovery_failed.is_empty() {
                "rolled_back"
            } else {
                "partial_failure"
            };
            cleanup(&mut staged, &mut report);
            return Err(report.fail());
        }
        report.status = "applied";
        report.files_touched = preview
            .changes
            .iter()
            .map(|change| change.reported_path.clone())
            .collect();
        #[cfg(test)]
        self.inject_cleanup_failure(&staged);
        cleanup(&mut staged, &mut report);
        if report.cleanup_failed.is_empty() {
            report.edit_display = Some(super::edit_display::from_snapshots(
                preview.changes.iter().map(|change| {
                    let before = change
                        .before
                        .as_ref()
                        .map_or(&[][..], |snapshot| snapshot.bytes.as_slice());
                    (
                        change.reported_path.as_str(),
                        before,
                        change.after.as_slice(),
                    )
                }),
            ));
            Ok(serde_json::to_string(&report).expect("serializable commit report"))
        } else {
            report.status = "applied_with_cleanup_failure";
            Err(report.fail())
        }
    }

    fn check_installed(&self, change: &BoundChange) -> Result<(), ToolError> {
        // Retain the original create-vs-replace write-set identity. New-file
        // grants may name a lexical directory alias; separately verify below
        // that its current destination is still the bound canonical target.
        self.require_write_target(
            &change.requested,
            matches!(change.operation, Operation::Create),
        )?;
        if self.resolve_existing(&change.requested)? != change.path {
            return Err(change_error("destination real path changed during commit"));
        }
        change.parent.check(&change.path)?;
        let current = disk::read(&change.path, &change.parent, MAX_TOTAL_BYTES)?;
        check_version(&change.requested, &version(&change.after), &current.bytes)?;
        if change
            .before
            .as_ref()
            .is_some_and(|before| before.permissions != current.permissions)
        {
            return Err(change_error(
                "destination permissions changed during commit",
            ));
        }
        Ok(())
    }

    fn restore_change(
        &self,
        change: &BoundChange,
        report: &mut CommitReport,
    ) -> Result<(), ToolError> {
        // Never overwrite a file which no longer contains exactly our installed
        // bytes. This protects observed external changes; it is not external CAS.
        self.file_change_writable()?;
        self.check_installed(change)?;
        #[cfg(test)]
        if self
            .file_changes
            .fault
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|fault| fault.fail_recovery)
        {
            return Err(change_error("injected recovery failure"));
        }
        if let Some(before) = &change.before {
            let mut original = disk::StagedFile::new(
                &change.path,
                &change.parent,
                &before.bytes,
                Some(&before.permissions),
            )
            .map_err(|error| stage_error(error, report))?;
            let result = self
                .check_installed(change)
                .and_then(|()| original.install(false).map_err(ToolError::Io));
            if let Err(error) = original.cleanup() {
                report.cleanup_failed.push(Problem {
                    path: original.temporary_path().display().to_string(),
                    reason: error.to_string(),
                });
            }
            // The installed target's recovery outcome and temporary-file
            // cleanup are independent facts, including when both fail.
            result?;
        } else {
            disk::remove(&change.path, &change.parent)?;
        }
        Ok(())
    }
}

fn stage_error(error: disk::StageError, report: &mut CommitReport) -> ToolError {
    if let Some(cleanup) = error.cleanup {
        report.cleanup_failed.push(Problem {
            path: cleanup.path.display().to_string(),
            reason: cleanup.error.to_string(),
        });
    }
    ToolError::Io(error.error)
}

fn cleanup(staged: &mut [disk::StagedFile], report: &mut CommitReport) {
    for file in staged {
        if let Err(error) = file.cleanup() {
            report.cleanup_failed.push(Problem {
                path: file.temporary_path().display().to_string(),
                reason: error.to_string(),
            });
        }
    }
}

fn problem(change: &BoundChange, error: impl std::fmt::Display) -> Problem {
    Problem {
        path: change.requested.clone(),
        reason: error.to_string(),
    }
}

fn validate_path(path: &str) -> Result<(), ToolError> {
    super::validate_workspace_relative(path)?;
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.contains('\0')
        || Path::new(path).file_name().is_none()
    {
        return Err(change_error(
            "file path must contain a filename and at most 4096 bytes, without NUL",
        ));
    }
    Ok(())
}

pub(super) fn version(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn check_version(path: &str, expected: &str, content: &[u8]) -> Result<(), ToolError> {
    let actual = version(content);
    if actual == expected {
        Ok(())
    } else {
        Err(ToolError::EditConflict {
            path: path.to_owned(),
            expected: expected.to_owned(),
            actual,
        })
    }
}

fn change_error(message: impl Into<String>) -> ToolError {
    ToolError::FileChange(
        json!({
            "status": "rejected",
            "files_touched": [],
            "reason": message.into(),
        })
        .to_string(),
    )
}

fn approval_description(preview: &Preview) -> String {
    format!(
        "Apply these exact file changes (preview {}, binding {}):\n{}",
        preview.id,
        preview.binding,
        preview.render()
    )
}

/// One replacement hunk, with common prefix/suffix elided and context retained.
/// O(file bytes), no quadratic diff computation; byte and line content bounded.
fn bounded_diff(before: &[u8], after: &[u8], cap: usize) -> (String, bool) {
    let old = String::from_utf8_lossy(before);
    let new = String::from_utf8_lossy(after);
    let old_lines: Vec<_> = old.split_inclusive('\n').collect();
    let new_lines: Vec<_> = new.split_inclusive('\n').collect();
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(left, right)| left == right)
        .count();
    if prefix == old_lines.len() && prefix == new_lines.len() {
        return (String::new(), false);
    }
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let start = prefix.saturating_sub(DIFF_CONTEXT_LINES);
    let old_end = (old_lines.len() - suffix + DIFF_CONTEXT_LINES).min(old_lines.len());
    let new_end = (new_lines.len() - suffix + DIFF_CONTEXT_LINES).min(new_lines.len());
    let header = format!(
        "@@ -{},{} +{},{} @@\n",
        start + 1,
        old_end - start,
        start + 1,
        new_end - start
    );
    let mut output = String::new();
    let mut truncated = false;
    let mut append = |text: &str| {
        let remaining = cap.saturating_sub(output.len());
        let mut end = text.len().min(remaining);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        output.push_str(&text[..end]);
        truncated |= end != text.len();
    };
    append(&header);
    for (marker, lines) in [
        ("-", &old_lines[start..old_end]),
        ("+", &new_lines[start..new_end]),
    ] {
        for line in lines {
            append(marker);
            append(line);
            if !line.ends_with('\n') {
                append("\n\\ No newline at end of file\n");
            }
        }
    }
    (output, truncated)
}

#[cfg(test)]
#[derive(Default)]
struct TestFault {
    fail_before: Option<usize>,
    external_write: Option<(PathBuf, Vec<u8>)>,
    #[cfg(unix)]
    external_link: Option<(PathBuf, PathBuf)>,
    fail_recovery: bool,
    fail_cleanup: bool,
}

#[cfg(test)]
impl ToolRegistry {
    fn inject_cleanup_failure(&self, staged: &[disk::StagedFile]) {
        if self
            .file_changes
            .fault
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|fault| fault.fail_cleanup)
        {
            // A successfully installed create retains its staging hard link.
            // Replace that link with a directory so the real unlink fails.
            let path = staged[0].temporary_path();
            std::fs::remove_file(&path).expect("injected cleanup staging link");
            std::fs::create_dir(path).expect("injected cleanup directory");
        }
    }

    fn inject_commit_failure(&self, index: usize) -> Result<(), ToolError> {
        let mut fault = self.file_changes.fault.lock().unwrap();
        if let Some(fault) = fault.as_mut()
            && fault.fail_before == Some(index)
        {
            if let Some((path, content)) = fault.external_write.take() {
                std::fs::write(path, content)?;
            }
            #[cfg(unix)]
            if let Some((path, target)) = fault.external_link.take() {
                std::fs::remove_file(&path)?;
                std::os::unix::fs::symlink(target, path)?;
            }
            return Err(change_error("injected commit failure"));
        }
        Ok(())
    }
}
