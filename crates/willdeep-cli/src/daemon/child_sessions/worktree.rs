//! Dedicated writable worktrees for delegated child conversations.
//!
//! Creation happens only after the Turn has claimed an execution slot, never
//! while it is queued. The helper records an explicit commit baseline and never
//! merges, commits, pushes or deletes anything on the parent checkout.
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

fn prepare(home: &Path, _child: Uuid, spec: &ChildSessionWorktree) -> Result<ChildSessionWorktree> {
    let runtime = home.join("runtime");
    std::fs::create_dir_all(&runtime)?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(runtime.join("child-worktrees.lock"))
        .context("open child worktree lock")?;
    FileExt::lock_exclusive(&lock).context("lock child worktree creation")?;

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
    let mut prepared = spec.clone();
    if prepared.base_commit.is_empty() {
        prepared.base_commit = git_text(path, &["rev-parse", "HEAD"])?.trim().to_owned();
    }
    let _ = root;
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
            }
            #[cfg(not(unix))]
            anyhow::bail!("snapshot worktrees do not support untracked symlinks on this platform");
            copied += 1;
            continue;
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
