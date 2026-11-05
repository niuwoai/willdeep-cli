//! Dedicated writable worktrees for delegated child conversations.
//!
//! Creation normally follows an execution claim. A local user's explicit open
//! may also prepare a worktree after takeover/detach, without creating a Turn.
//! Queued delegated tasks and list reads never create a checkout. The helper
//! records an explicit baseline and never changes the parent checkout.
use super::*;
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use willdeep_runtime_protocol::child_session::{
    ChildSessionWorkspaceMode, ChildSessionWorktree, ChildSessionWorktreeBase,
};

const MAX_UNTRACKED_FILES: usize = 2_000;
const MAX_UNTRACKED_BYTES: u64 = 50 * 1024 * 1024;

/// Called by the daemon after `claim_next` and before `TaskManager::submit`.
/// A queued child still has no worktree; a claimed one gets it exactly once.
pub(in crate::daemon) async fn ensure_for_claim(
    state: &ServerState,
    request: &mut crate::daemon::SubmitTask,
) -> Result<()> {
    let Some(child) = request.session_id else {
        return Ok(());
    };
    let Some(session) = state.sessions.get(child)? else {
        return Ok(());
    };
    if session.parent_session_id.is_none() {
        return Ok(());
    }
    let link = state.sessions.children.transaction(|ledger| {
        Ok(ledger
            .links
            .values()
            .find(|link| link.child_session_id == child)
            .cloned())
    })?;
    let Some(link) = link else {
        return Ok(());
    };
    if link.workspace_mode != ChildSessionWorkspaceMode::Worktree {
        return Ok(());
    }
    let spec = link
        .worktree
        .clone()
        .context("worktree relation is missing its binding")?;
    if !spec.base_commit.is_empty() && Path::new(&spec.path).exists() {
        request.workspace = PathBuf::from(&spec.path);
        return Ok(());
    }

    let home = state.home.clone();
    let prepared = tokio::task::spawn_blocking(move || prepare(&home, child, &spec))
        .await
        .map_err(|error| anyhow::anyhow!("prepare child worktree task: {error}"))??;
    state.sessions.children.transaction(|ledger| {
        let link = ledger
            .links
            .values_mut()
            .find(|link| link.child_session_id == child)
            .context("child relation disappeared while preparing its worktree")?;
        link.worktree = Some(prepared.clone());
        link.workspace = prepared.path.clone();
        Ok(())
    })?;
    state
        .sessions
        .set_child_workspace(child, PathBuf::from(&prepared.path))?;
    request.workspace = PathBuf::from(&prepared.path);
    Ok(())
}

/// Local user entry only: opening a taken-over child may need its first checkout.
/// It never enqueues a Turn or restores parent delegation. Git runs outside the
/// ChildStore transaction, under `prepare`'s separate worktree lock.
pub(crate) async fn prepare_child_workspace_for_user_open(
    home: &Path,
    parent: Uuid,
    child: Uuid,
    workspace: &Path,
) -> Result<bool> {
    let home = home.to_path_buf();
    let workspace = workspace.to_path_buf();
    tokio::task::spawn_blocking(move || prepare_for_user_open(&home, parent, child, &workspace))
        .await
        .context("join child workspace preparation for explicit user open")?
}

fn user_owned_link(
    ledger: &Ledger,
    home: &Path,
    parent: Uuid,
    child: Uuid,
    workspace: &Path,
) -> Result<ChildSessionLink> {
    let link = control::link(ledger, parent, child)?;
    anyhow::ensure!(
        matches!(
            link.control_state,
            ChildSessionControlState::UserOwned | ChildSessionControlState::Detached
        ),
        "take over or detach the child before preparing its workspace for user input"
    );
    let cores = willdeep_core::SessionStore::new(home);
    let target = cores
        .load(child)
        .context("load child Core before user workspace preparation")?;
    let source = match cores.load(parent) {
        Ok(source) => Some(source),
        // The immutable Link still records the original repository after its
        // parent conversation was deleted. Corrupt/unreadable parents still fail.
        Err(willdeep_core::session::SessionError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            None
        }
        Err(error) => {
            return Err(error)
                .context("load original parent Core before user workspace preparation");
        }
    };
    let read_only = link.workspace_mode == ChildSessionWorkspaceMode::SharedReadOnly;
    anyhow::ensure!(
        child != parent
            && target.parent_session_id == Some(parent)
            && source.as_ref().is_none_or(
                |source| source.parent_session_id.is_none() && !source.delegation_read_only
            )
            && target.delegation_read_only == read_only
            && target.workspace == workspace
            && Path::new(&link.workspace) == workspace,
        "child workspace or permission binding conflicts with its original parent"
    );
    if read_only {
        anyhow::ensure!(
            link.worktree.is_none()
                && source
                    .as_ref()
                    .is_none_or(|source| workspace == source.workspace),
            "read-only child workspace binding changed"
        );
    } else {
        let spec = link
            .worktree
            .as_ref()
            .context("child worktree binding is missing")?;
        let expected = home.join("worktrees/children").join(child.to_string());
        let branch = format!(
            "{CHILD_SESSION_WORKTREE_BRANCH_PREFIX}{}",
            &child.simple().to_string()[..12]
        );
        anyhow::ensure!(
            workspace == expected
                && Path::new(&spec.path) == workspace
                && Path::new(&spec.root_workspace).is_absolute()
                && source
                    .as_ref()
                    .is_none_or(|source| Path::new(&spec.root_workspace) == source.workspace)
                && spec.branch == branch,
            "child worktree does not match its fixed path, branch or original repository"
        );
    }
    Ok(link.clone())
}

fn same_binding(left: &ChildSessionWorktree, right: &ChildSessionWorktree) -> bool {
    left.path == right.path
        && left.root_workspace == right.root_workspace
        && left.branch == right.branch
        && left.base == right.base
}

fn prepare_for_user_open(home: &Path, parent: Uuid, child: Uuid, workspace: &Path) -> Result<bool> {
    let children = ChildStore::new(home);
    let original = children.transaction(|ledger| {
        let link = control::link(ledger, parent, child)?;
        if link.control_state == ChildSessionControlState::Delegated && workspace.exists() {
            // Existing delegated conversations keep their normal navigation path.
            return Ok(None);
        }
        user_owned_link(ledger, home, parent, child, workspace).map(Some)
    })?;
    let Some(original) = original else {
        return Ok(false);
    };
    let Some(spec) = original.worktree else {
        return Ok(false);
    };
    anyhow::ensure!(
        spec.base_commit.is_empty() || workspace.exists(),
        "previously prepared child worktree is missing; recover it before opening the conversation"
    );
    if !spec.base_commit.is_empty() {
        return Ok(true);
    }
    // Keep the independent worktree lock through the preparation receipt, so a
    // concurrent open cannot adopt the path before its snapshot metadata commits.
    let _worktree_lock = lock_worktrees(home)?;
    // No ChildStore lock is held while Git creates/adopts the checkout.
    let prepared = prepare_locked(&spec)
        .with_context(|| format!("prepare child {child} worktree at {}", workspace.display()))?;
    children.transaction(|ledger| {
        let current = user_owned_link(ledger, home, parent, child, workspace)?;
        let current_spec = current
            .worktree
            .as_ref()
            .context("child worktree binding disappeared during preparation")?;
        anyhow::ensure!(
            current.link_id == original.link_id && same_binding(current_spec, &spec),
            "child worktree binding changed during user preparation"
        );
        if current_spec.base_commit.is_empty() {
            ledger
                .links
                .get_mut(&current.link_id)
                .context("child relation disappeared during user preparation")?
                .worktree = Some(prepared);
        } else {
            // Another explicit open may have committed the preparation first.
            // Retain that complete snapshot record rather than an adoption result.
            anyhow::ensure!(
                current_spec.base_commit == prepared.base_commit,
                "child worktree baseline changed during user preparation"
            );
        }
        Ok(())
    })?;
    Ok(true)
}

fn prepare(home: &Path, _child: Uuid, spec: &ChildSessionWorktree) -> Result<ChildSessionWorktree> {
    let _worktree_lock = lock_worktrees(home)?;
    prepare_locked(spec)
}

fn lock_worktrees(home: &Path) -> Result<std::fs::File> {
    let runtime = home.join("runtime");
    std::fs::create_dir_all(&runtime)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(runtime.join("child-worktrees.lock"))
        .context("open child worktree lock")?;
    FileExt::lock_exclusive(&lock).context("lock child worktree creation")?;
    Ok(lock)
}

fn prepare_locked(spec: &ChildSessionWorktree) -> Result<ChildSessionWorktree> {
    let root = PathBuf::from(&spec.root_workspace);
    let path = PathBuf::from(&spec.path);
    if path.exists() {
        return adopt_existing(&root, &path, spec);
    }

    let top = git_text(&root, &["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(top.trim());
    anyhow::ensure!(
        same_path(&top, &root),
        "worktree mode requires the Session workspace to be the Git repository root"
    );
    let head = git_text(&root, &["rev-parse", "--verify", "HEAD"])
        .context("worktree mode requires at least one commit")?;
    let base_commit = head.trim().to_owned();
    let dirty = !git_bytes(&root, &["status", "--porcelain=v1", "-uno", "-z"])?.is_empty();
    let patch = if spec.base == ChildSessionWorktreeBase::Snapshot && dirty {
        Some(git_bytes(
            &root,
            &[
                "diff",
                "--binary",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "HEAD",
            ],
        )?)
    } else {
        None
    };
    let untracked = if spec.base == ChildSessionWorktreeBase::Snapshot {
        git_bytes(&root, &["ls-files", "--others", "--exclude-standard", "-z"])?
    } else {
        Vec::new()
    };

    let branch_exists = git_bytes(
        &root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{}", spec.branch),
        ],
    )
    .map(|_| true)
    .unwrap_or(false);
    let mut add = vec!["worktree", "add"];
    if !branch_exists {
        add.extend(["-b", spec.branch.as_str()]);
    }
    add.push(spec.path.as_str());
    add.push(if branch_exists {
        spec.branch.as_str()
    } else {
        base_commit.as_str()
    });
    let output = git(&root, &add)?;
    anyhow::ensure!(
        output.status.success(),
        "git worktree add failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );

    let result = (|| -> Result<ChildSessionWorktree> {
        let mut prepared = spec.clone();
        prepared.base_commit = if branch_exists {
            git_text(&path, &["rev-parse", "HEAD"])?.trim().to_owned()
        } else {
            base_commit.clone()
        };
        prepared.parent_dirty = dirty;
        if let Some(patch) = patch {
            apply_patch(&path, &patch)?;
            prepared.baseline_patch_sha256 = Some(format!("{:x}", Sha256::digest(&patch)));
        }
        prepared.untracked_files = copy_untracked(&root, &path, &untracked)?;
        Ok(prepared)
    })();

    if result.is_err() {
        // The worktree was created moments ago and contains no user work. A
        // failed snapshot copy is only a copy of the parent's files.
        let _ = git(
            &root,
            &["worktree", "remove", "--force", spec.path.as_str()],
        );
        if !branch_exists {
            let _ = git(&root, &["branch", "-D", spec.branch.as_str()]);
        }
    }
    result
}

fn adopt_existing(
    root: &Path,
    path: &Path,
    spec: &ChildSessionWorktree,
) -> Result<ChildSessionWorktree> {
    let top = git_text(path, &["rev-parse", "--show-toplevel"])
        .context("existing child workspace is not a Git worktree")?;
    anyhow::ensure!(
        same_path(Path::new(top.trim()), path),
        "existing child workspace has a different root"
    );
    let branch = git_text(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    anyhow::ensure!(
        branch.trim() == spec.branch,
        "existing child workspace is on another branch"
    );
    let parent_git = git_text(root, &["rev-parse", "--git-common-dir"])?;
    let child_git = git_text(path, &["rev-parse", "--git-common-dir"])?;
    anyhow::ensure!(
        same_path(&root.join(parent_git.trim()), &path.join(child_git.trim())),
        "existing child worktree belongs to another repository"
    );
    let mut prepared = spec.clone();
    if prepared.base_commit.is_empty() {
        prepared.base_commit = git_text(path, &["rev-parse", "HEAD"])?.trim().to_owned();
    }
    Ok(prepared)
}

fn apply_patch(worktree: &Path, patch: &[u8]) -> Result<()> {
    let mut command = git_command(worktree, &["apply", "--binary", "--whitespace=nowarn", "-"]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("start git apply for the frozen baseline")?;
    child
        .stdin
        .take()
        .context("git apply stdin")?
        .write_all(patch)
        .context("write frozen baseline patch")?;
    let output = child
        .wait_with_output()
        .context("wait for git apply of the frozen baseline")?;
    anyhow::ensure!(
        output.status.success(),
        "frozen baseline patch did not apply: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn copy_untracked(root: &Path, worktree: &Path, listing: &[u8]) -> Result<usize> {
    let mut total = 0_u64;
    let mut copied = 0;
    for raw in listing
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty())
    {
        let relative = std::str::from_utf8(raw).context("untracked path is not UTF-8")?;
        let relative = Path::new(relative);
        anyhow::ensure!(
            relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
            "untracked path escapes the parent workspace"
        );
        let source = root.join(relative);
        let target = worktree.join(relative);
        let metadata = std::fs::symlink_metadata(&source)
            .with_context(|| format!("read untracked {}", source.display()))?;
        if metadata.file_type().is_symlink() {
            let link = std::fs::read_link(&source)?;
            let resolved = source.parent().unwrap_or(root).join(&link);
            anyhow::ensure!(
                resolved
                    .canonicalize()
                    .is_ok_and(|resolved| resolved.starts_with(root)),
                "untracked symlink escapes the parent workspace"
            );
            #[cfg(unix)]
            {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let _ = std::fs::remove_file(&target);
                std::os::unix::fs::symlink(&link, &target)?;
                copied += 1;
                continue;
            }
            #[cfg(not(unix))]
            anyhow::bail!("snapshot worktrees do not support untracked symlinks on this platform");
        }
        anyhow::ensure!(
            metadata.is_file(),
            "untracked entry is not a regular file: {}",
            source.display()
        );
        total = total.saturating_add(metadata.len());
        anyhow::ensure!(
            total <= MAX_UNTRACKED_BYTES,
            "untracked files exceed the frozen baseline size limit"
        );
        copied += 1;
        anyhow::ensure!(
            copied <= MAX_UNTRACKED_FILES,
            "untracked files exceed the frozen baseline count limit"
        );
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&source, &target)
            .with_context(|| format!("copy untracked {}", source.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode();
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(copied)
}

fn git_command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    command
}

fn git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    git_command(root, args)
        .output()
        .with_context(|| format!("run git {}", args.join(" ")))
}

fn git_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = git(root, args)?;
    anyhow::ensure!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(git_bytes(root, args)?).context("git output is not UTF-8")
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_ok(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "WillDeep Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "WillDeep Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository() -> (PathBuf, PathBuf, String) {
        let root = std::env::temp_dir().join(format!("child-worktree-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let repo = root.join("repo");
        git_ok(&repo, &["init", "-q"]);
        git_ok(&repo, &["config", "user.name", "WillDeep Test"]);
        git_ok(&repo, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(repo.join("tracked.txt"), "committed\n").unwrap();
        git_ok(&repo, &["add", "tracked.txt"]);
        git_ok(&repo, &["commit", "-q", "-m", "initial"]);
        let head = git_text(&repo, &["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_owned();
        (root.join("home"), repo, head)
    }

    fn spec(repo: &Path, root: &Path, base: ChildSessionWorktreeBase) -> ChildSessionWorktree {
        ChildSessionWorktree {
            root_workspace: repo.to_string_lossy().into_owned(),
            path: root.join("worktree").to_string_lossy().into_owned(),
            branch: format!(
                "willdeep/child-{}",
                &Uuid::new_v4().simple().to_string()[..12]
            ),
            base_commit: String::new(),
            base,
            parent_dirty: false,
            baseline_patch_sha256: None,
            untracked_files: 0,
        }
    }

    struct UserOpenFixture {
        home: PathBuf,
        repo: PathBuf,
        state: ServerState,
        parent: Uuid,
        dependency: ChildSessionTask,
        task: ChildSessionTask,
        _scheduled: tokio::sync::mpsc::UnboundedReceiver<Uuid>,
    }

    impl UserOpenFixture {
        fn new(mode: ChildSessionWorkspaceMode) -> Self {
            use willdeep_runtime_protocol::ApiRequest;
            let (home, repo, _) = repository();
            // Runtime persists canonical workspace paths, including macOS temp symlinks.
            let repo = repo.canonicalize().unwrap();
            let sessions = crate::daemon::tests::test_runtime_session_store(&home);
            let parent = sessions
                .create(session_store::CreateRuntimeSession {
                    id: None,
                    workspace: repo.clone(),
                    profile: None,
                    model: None,
                    config: None,
                    title: Some("Parent".into()),
                })
                .unwrap()
                .id;
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let state =
                crate::daemon::tests::test_server_state_with_scheduler(&home, sessions, sender);
            let dependency: ChildSessionTask = serde_json::from_value(dispatch_for_test(&state, &ApiRequest::new(
                "session.child.start",
                serde_json::json!({"parent_session_id":parent, "title":"Prerequisite", "prompt":"Read files"}),
            )).map_err(|_| "start prerequisite").unwrap()).unwrap();
            let base = if mode == ChildSessionWorkspaceMode::Worktree {
                ChildSessionWorktreeBase::Snapshot
            } else {
                ChildSessionWorktreeBase::Head
            };
            let task: ChildSessionTask = serde_json::from_value(dispatch_for_test(&state, &ApiRequest::new(
                "session.child.start",
                serde_json::json!({"parent_session_id":parent, "title":"Await dependency", "prompt":"Read after prerequisite", "workspace_mode":mode, "worktree_base":base, "depends_on":[dependency.assignment_id]}),
            )).map_err(|_| "start waiting child").unwrap()).unwrap();
            assert!(task.waiting_on_dependencies && task.child_turn_id.is_none());
            assert!(
                state
                    .sessions
                    .list_turns(task.child_session_id)
                    .unwrap()
                    .is_empty()
            );
            Self {
                home,
                repo,
                state,
                parent,
                dependency,
                task,
                _scheduled: receiver,
            }
        }

        fn link(&self) -> ChildSessionLink {
            self.state
                .sessions
                .children
                .transaction(|ledger| {
                    Ok(control::link(ledger, self.parent, self.task.child_session_id)?.clone())
                })
                .unwrap()
        }

        fn revoke(&self, action: &str) {
            let request = willdeep_runtime_protocol::ApiRequest::new(
                action,
                serde_json::json!({"parent_session_id":self.parent, "child_session_id":self.task.child_session_id, "expected_control_revision":1}),
            );
            dispatch_for_test(&self.state, &request)
                .map_err(|_| "revoke child control")
                .unwrap();
        }

        fn no_turns(&self) {
            let child = self
                .state
                .sessions
                .get(self.task.child_session_id)
                .unwrap()
                .unwrap();
            assert!(child.active_turn_id.is_none());
            assert!(self.state.sessions.list_turns(child.id).unwrap().is_empty());
            assert!(
                willdeep_core::SessionStore::new(&self.home)
                    .load(child.id)
                    .unwrap()
                    .messages
                    .is_empty()
            );
        }
    }

    impl Drop for UserOpenFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.home.parent().unwrap());
        }
    }

    #[tokio::test]
    async fn explicit_user_open_prepares_waiting_worktree_after_takeover_or_detach_without_turns() {
        for (action, expected) in [
            (
                "session.child.takeover",
                ChildSessionControlState::UserOwned,
            ),
            ("session.child.detach", ChildSessionControlState::Detached),
        ] {
            let fixture = UserOpenFixture::new(ChildSessionWorkspaceMode::Worktree);
            let original = fixture.link().worktree.unwrap();
            assert!(!Path::new(&original.path).exists());
            fixture.revoke(action);
            fixture.no_turns();
            // Taking over blocks the dependency wait; it does not prepare Git.
            assert!(!Path::new(&original.path).exists());
            let result = prepare_child_workspace_for_user_open(
                &fixture.home,
                fixture.parent,
                fixture.task.child_session_id,
                Path::new(&original.path),
            )
            .await
            .unwrap();
            assert!(result);
            fixture.no_turns();
            let link = fixture.link();
            assert_eq!(link.control_state, expected);
            let prepared = link.worktree.unwrap();
            assert_eq!(prepared.path, original.path);
            assert_eq!(prepared.branch, original.branch);
            assert!(!prepared.base_commit.is_empty());
            let path = Path::new(&prepared.path);
            assert!(path.canonicalize().is_ok());
            assert_eq!(
                git_text(path, &["rev-parse", "--abbrev-ref", "HEAD"])
                    .unwrap()
                    .trim(),
                original.branch
            );
            assert_eq!(
                std::fs::read_to_string(path.join("tracked.txt")).unwrap(),
                "committed\n"
            );
            let child = fixture
                .state
                .sessions
                .get(fixture.task.child_session_id)
                .unwrap()
                .unwrap();
            assert_eq!(child.workspace, path);
            assert!(!child.delegation_read_only);
            assert_eq!(
                child.delegation_detached,
                expected == ChildSessionControlState::Detached
            );
            let task =
                fixture
                    .state
                    .sessions
                    .children
                    .transaction(|ledger| {
                        Ok(ledger
                            .project(ledger.assignments.get(&fixture.task.assignment_id).unwrap()))
                    })
                    .unwrap();
            assert_eq!(task.status, ChildSessionTaskStatus::Blocked);
            assert!(task.child_turn_id.is_none());
        }
    }

    #[tokio::test]
    async fn concurrent_user_opens_and_first_user_claim_keep_complete_snapshot_preparation() {
        let fixture = UserOpenFixture::new(ChildSessionWorkspaceMode::Worktree);
        std::fs::write(fixture.repo.join("tracked.txt"), "frozen change\n").unwrap();
        std::fs::write(fixture.repo.join("untracked.txt"), "frozen new file\n").unwrap();
        fixture.revoke("session.child.takeover");
        let workspace = PathBuf::from(fixture.link().workspace);
        let (first, second) = tokio::join!(
            prepare_child_workspace_for_user_open(
                &fixture.home,
                fixture.parent,
                fixture.task.child_session_id,
                &workspace
            ),
            prepare_child_workspace_for_user_open(
                &fixture.home,
                fixture.parent,
                fixture.task.child_session_id,
                &workspace
            ),
        );
        assert!(first.unwrap() && second.unwrap());
        fixture.no_turns();
        let prepared = fixture.link().worktree.unwrap();
        assert!(prepared.parent_dirty && prepared.baseline_patch_sha256.is_some());
        assert_eq!(prepared.untracked_files, 1);
        assert_eq!(
            std::fs::read_to_string(workspace.join("tracked.txt")).unwrap(),
            "frozen change\n"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("untracked.txt")).unwrap(),
            "frozen new file\n"
        );
        // The earlier prerequisite owns a slot before the user's later Turn,
        // just as it would with the real scheduler driving the durable queue.
        let prerequisite = fixture
            .state
            .sessions
            .claim_next(fixture.dependency.child_session_id)
            .unwrap()
            .unwrap();
        assert!(
            fixture
                .state
                .sessions
                .bind_task(prerequisite.metadata.id, Uuid::new_v4())
                .unwrap()
        );
        // Only this explicit follow-up creates a user Turn; preparing/opening did not.
        fixture
            .state
            .sessions
            .enqueue_turn(
                fixture.task.child_session_id,
                session_store::CreateRuntimeTurn {
                    team_control: None,
                    request_id: Uuid::new_v4(),
                    prompt: "User follow-up".into(),
                    attachments: Vec::new(),
                    origin_client: Some("local-user".into()),
                    defer_until_released: false,
                },
            )
            .unwrap();
        let mut claimed = fixture
            .state
            .sessions
            .claim_next(fixture.task.child_session_id)
            .unwrap()
            .unwrap();
        ensure_for_claim(&fixture.state, &mut claimed.request)
            .await
            .unwrap();
        assert_eq!(claimed.request.workspace, workspace);
        assert_eq!(fixture.link().worktree, Some(prepared));
    }

    #[tokio::test]
    async fn user_open_preserves_shared_read_only_capability_without_creating_a_worktree() {
        for action in ["session.child.takeover", "session.child.detach"] {
            let fixture = UserOpenFixture::new(ChildSessionWorkspaceMode::SharedReadOnly);
            fixture.revoke(action);
            assert!(
                !prepare_child_workspace_for_user_open(
                    &fixture.home,
                    fixture.parent,
                    fixture.task.child_session_id,
                    &fixture.repo
                )
                .await
                .unwrap()
            );
            assert!(!fixture.home.join("worktrees").exists());
            fixture.no_turns();
            let child = fixture
                .state
                .sessions
                .get(fixture.task.child_session_id)
                .unwrap()
                .unwrap();
            let core = willdeep_core::SessionStore::new(&fixture.home)
                .load(child.id)
                .unwrap();
            assert!(child.delegation_read_only && core.delegation_read_only);
            assert_eq!(child.workspace, fixture.repo);
            assert!(fixture.link().worktree.is_none());
        }
    }

    #[tokio::test]
    async fn user_open_refuses_delegation_or_changed_identity_permission_and_fixed_binding() {
        let fixture = UserOpenFixture::new(ChildSessionWorkspaceMode::Worktree);
        let path = PathBuf::from(fixture.link().workspace);
        assert!(
            prepare_child_workspace_for_user_open(
                &fixture.home,
                fixture.parent,
                fixture.task.child_session_id,
                &path
            )
            .await
            .is_err()
        );
        fixture.revoke("session.child.takeover");
        assert!(
            prepare_child_workspace_for_user_open(
                &fixture.home,
                Uuid::new_v4(),
                fixture.task.child_session_id,
                &path
            )
            .await
            .is_err()
        );
        assert!(
            prepare_child_workspace_for_user_open(
                &fixture.home,
                fixture.parent,
                fixture.task.child_session_id,
                &fixture.home.join("unbound")
            )
            .await
            .is_err()
        );
        let cores = willdeep_core::SessionStore::new(&fixture.home);
        cores
            .update(fixture.task.child_session_id, |core| {
                core.delegation_read_only = true
            })
            .unwrap();
        assert!(
            prepare_child_workspace_for_user_open(
                &fixture.home,
                fixture.parent,
                fixture.task.child_session_id,
                &path
            )
            .await
            .is_err()
        );
        assert!(
            cores
                .load(fixture.task.child_session_id)
                .unwrap()
                .delegation_read_only
        );
        cores
            .update(fixture.task.child_session_id, |core| {
                core.delegation_read_only = false
            })
            .unwrap();
        fixture
            .state
            .sessions
            .children
            .transaction(|ledger| {
                let link = ledger.links.get_mut(&fixture.task.link_id).unwrap();
                link.worktree.as_mut().unwrap().branch = "unbound-branch".into();
                Ok(())
            })
            .unwrap();
        assert!(
            prepare_child_workspace_for_user_open(
                &fixture.home,
                fixture.parent,
                fixture.task.child_session_id,
                &path
            )
            .await
            .is_err()
        );
        assert!(!path.exists());
        assert!(!fixture.home.join("unbound").exists());
        fixture.no_turns();
    }

    #[test]
    fn head_mode_uses_the_commit_and_never_copies_dirty_parent_files() {
        let (home, repo, head) = repository();
        std::fs::write(repo.join("tracked.txt"), "dirty parent\n").unwrap();
        std::fs::write(repo.join("untracked.txt"), "not in HEAD\n").unwrap();
        let spec = spec(&repo, &home, ChildSessionWorktreeBase::Head);
        let prepared = prepare(&home, Uuid::new_v4(), &spec).unwrap();
        assert_eq!(prepared.base_commit, head);
        assert!(prepared.parent_dirty);
        assert_eq!(prepared.untracked_files, 0);
        let worktree = Path::new(&prepared.path);
        assert_eq!(
            std::fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
            "committed\n"
        );
        assert!(!worktree.join("untracked.txt").exists());
        // Re-adopting the same path is idempotent.
        let adopted = prepare(&home, Uuid::new_v4(), &prepared).unwrap();
        assert_eq!(adopted.path, prepared.path);
        assert_eq!(adopted.base_commit, prepared.base_commit);
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn snapshot_mode_freezes_tracked_changes_and_untracked_files() {
        let (home, repo, _) = repository();
        std::fs::write(repo.join("tracked.txt"), "dirty parent\n").unwrap();
        std::fs::write(repo.join("untracked.txt"), "not in HEAD\n").unwrap();
        let spec = spec(&repo, &home, ChildSessionWorktreeBase::Snapshot);
        let prepared = prepare(&home, Uuid::new_v4(), &spec).unwrap();
        assert!(prepared.parent_dirty);
        assert_eq!(prepared.untracked_files, 1);
        assert!(prepared.baseline_patch_sha256.is_some());
        let worktree = Path::new(&prepared.path);
        assert_eq!(
            std::fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
            "dirty parent\n"
        );
        assert_eq!(
            std::fs::read_to_string(worktree.join("untracked.txt")).unwrap(),
            "not in HEAD\n"
        );
        let _ = std::fs::remove_dir_all(home.parent().unwrap());
    }

    #[test]
    fn non_git_workspaces_are_rejected_before_creating_a_worktree() {
        let root = std::env::temp_dir().join(format!("child-worktree-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("plain")).unwrap();
        let spec = spec(
            &root.join("plain"),
            &root.join("home"),
            ChildSessionWorktreeBase::Head,
        );
        assert!(prepare(&root.join("home"), Uuid::new_v4(), &spec).is_err());
        assert!(!Path::new(&spec.path).exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
