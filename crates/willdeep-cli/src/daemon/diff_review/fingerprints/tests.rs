use super::super::{capture, snapshot};
use super::*;

const STATUS_ARGS: &[&str] = &[
    "status",
    "--porcelain=v1",
    "-z",
    "--untracked-files=all",
    "--ignore-submodules=none",
];

struct Fixture {
    root: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("willdeep-nested-snapshot-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("parent");
        init(&workspace);
        git(
            &workspace,
            &["commit", "--quiet", "--allow-empty", "-m", "parent"],
        )
        .unwrap();
        Self { root, workspace }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.root) {
            eprintln!(
                "Cannot remove test fixture {}: {error}",
                self.root.display()
            );
        }
    }
}

fn init(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "--quiet"]).unwrap();
    for (key, value) in [
        ("user.name", "WillDeep Test"),
        ("user.email", "test@willdeep.invalid"),
        ("commit.gpgsign", "false"),
        ("core.hooksPath", ".git/no-test-hooks"),
    ] {
        git(path, &["config", key, value]).unwrap();
    }
}

fn seed(path: &Path) {
    init(path);
    std::fs::write(path.join("tracked.txt"), "base\n").unwrap();
    git(path, &["add", "tracked.txt"]).unwrap();
    git(path, &["commit", "--quiet", "-m", "seed"]).unwrap();
}

fn assert_changes_invalidate_parent(fixture: &Fixture, child: &Path, relative: &str) {
    let parent = &fixture.workspace;
    let status = git(parent, STATUS_ARGS).unwrap();
    let initial = capture(parent).expect("capture must accept nested repositories");
    assert!(initial.fingerprints.contains_key(relative));
    assert_eq!(initial.snapshot.id, snapshot(parent).unwrap().id);

    // An empty commit changes only HEAD: the parent's ?? entry stays identical.
    git(
        child,
        &["commit", "--quiet", "--allow-empty", "-m", "new head"],
    )
    .unwrap();
    let head_changed = capture(parent).unwrap();
    assert_ne!(initial.snapshot.id, head_changed.snapshot.id);
    assert_ne!(
        initial.fingerprints[relative],
        head_changed.fingerprints[relative]
    );

    std::fs::write(child.join("tracked.txt"), "one!\n").unwrap();
    let first = capture(parent).unwrap();
    let child_status = git(child, STATUS_ARGS).unwrap();
    std::fs::write(child.join("tracked.txt"), "two!\n").unwrap();
    let second = capture(parent).unwrap();
    assert_eq!(child_status, git(child, STATUS_ARGS).unwrap());
    assert_ne!(first.snapshot.id, second.snapshot.id);
    assert_ne!(first.fingerprints[relative], second.fingerprints[relative]);

    git(child, &["add", "tracked.txt"]).unwrap();
    let staged = capture(parent).unwrap();
    assert_ne!(second.snapshot.id, staged.snapshot.id);
    assert_ne!(second.fingerprints[relative], staged.fingerprints[relative]);

    std::fs::write(child.join("scratch.txt"), "one\n").unwrap();
    let untracked = capture(parent).unwrap();
    let child_status = git(child, STATUS_ARGS).unwrap();
    std::fs::write(child.join("scratch.txt"), "two\n").unwrap();
    let changed = capture(parent).unwrap();
    assert_eq!(child_status, git(child, STATUS_ARGS).unwrap());
    assert_ne!(untracked.snapshot.id, changed.snapshot.id);
    assert_ne!(
        untracked.fingerprints[relative],
        changed.fingerprints[relative]
    );
    assert_eq!(status, git(parent, STATUS_ARGS).unwrap());
    assert_eq!(changed.snapshot.id, snapshot(parent).unwrap().id);
}

#[test]
fn untracked_nested_repository_content_head_and_index_invalidate_parent() {
    let fixture = Fixture::new();
    let child = fixture.workspace.join("nested");
    seed(&child);
    assert_changes_invalidate_parent(&fixture, &child, "nested/");
}

#[test]
fn untracked_linked_worktree_content_head_and_index_invalidate_parent() {
    let fixture = Fixture::new();
    let seed_repo = fixture.root.join("seed");
    seed(&seed_repo);
    let child = fixture.workspace.join(".claude/worktrees/linked");
    std::fs::create_dir_all(child.parent().unwrap()).unwrap();
    git(
        &seed_repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            child.to_str().unwrap(),
            "HEAD",
        ],
    )
    .unwrap();
    assert!(child.join(".git").is_file());
    assert_changes_invalidate_parent(&fixture, &child, ".claude/worktrees/linked/");
}

#[test]
fn unborn_nested_repository_is_supported_and_tracks_new_file_bytes() {
    let fixture = Fixture::new();
    let child = fixture.workspace.join("unborn");
    init(&child);
    assert_eq!(repository_head(&child).unwrap(), None);
    std::fs::write(child.join("draft.txt"), "one\n").unwrap();
    let before = capture(&fixture.workspace).unwrap();
    std::fs::write(child.join("draft.txt"), "two\n").unwrap();
    let after = capture(&fixture.workspace).unwrap();
    assert_ne!(before.snapshot.id, after.snapshot.id);
    assert_eq!(after.snapshot.id, snapshot(&fixture.workspace).unwrap().id);
}

#[test]
fn nested_repositories_recurse_and_respect_the_shared_depth_limit() {
    let fixture = Fixture::new();
    let child = fixture.workspace.join("nested");
    seed(&child);
    let grandchild = child.join("nested-again");
    seed(&grandchild);
    let before = capture(&fixture.workspace).unwrap();
    std::fs::write(grandchild.join("tracked.txt"), "different\n").unwrap();
    let after = capture(&fixture.workspace).unwrap();
    assert_ne!(before.snapshot.id, after.snapshot.id);
    assert_ne!(
        before.fingerprints["nested/"],
        after.fingerprints["nested/"]
    );
    let status = git(&fixture.workspace, STATUS_ARGS).unwrap();
    let head = repository_head(&fixture.workspace).unwrap();
    let error = snapshot_id_at_depth(
        &fixture.workspace,
        &status,
        head.as_deref(),
        MAX_SUBMODULE_DEPTH - 1,
    )
    .unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(
        diagnostic.contains("nesting exceeds snapshot limit"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("nested-again"), "{diagnostic}");
}

#[test]
fn broken_nested_gitfile_is_an_error_with_the_failing_path() {
    let fixture = Fixture::new();
    let child = fixture.workspace.join("broken-child");
    seed(&child);
    std::fs::rename(child.join(".git"), fixture.root.join("saved-git")).unwrap();
    std::fs::write(child.join(".git"), "gitdir: missing-directory\n").unwrap();
    std::fs::write(child.join("new.txt"), "untracked\n").unwrap();
    let error = snapshot(&fixture.workspace).unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains("broken-child"), "{diagnostic}");
    assert!(capture(&fixture.workspace).is_err());
}

#[test]
fn directory_without_git_metadata_is_not_silently_skipped() {
    let fixture = Fixture::new();
    let directory = fixture.workspace.join("ordinary-directory");
    std::fs::create_dir_all(&directory).unwrap();
    let error = untracked_fingerprint(&directory, &mut DefaultHasher::new(), 0).unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains("ordinary-directory"), "{diagnostic}");
    assert!(diagnostic.contains("metadata is missing"), "{diagnostic}");
}

#[cfg(unix)]
#[test]
fn initialized_submodule_git_directory_symlink_remains_supported() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let child = fixture.workspace.join("linked-metadata");
    seed(&child);
    let metadata = fixture.root.join("submodule-git-data");
    std::fs::rename(child.join(".git"), &metadata).unwrap();
    symlink(&metadata, child.join(".git")).unwrap();
    git(&child, STATUS_ARGS).expect("Git must accept the metadata symlink");
    git(&fixture.workspace, &["add", "linked-metadata"]).unwrap();
    git(&fixture.workspace, &["commit", "--quiet", "-m", "gitlink"]).unwrap();
    let before = capture(&fixture.workspace).unwrap();
    std::fs::write(child.join("tracked.txt"), "changed\n").unwrap();
    let after = capture(&fixture.workspace).unwrap();
    assert_ne!(before.snapshot.id, after.snapshot.id);
    assert_ne!(
        before.fingerprints.get("linked-metadata"),
        after.fingerprints.get("linked-metadata")
    );
}

#[cfg(unix)]
#[test]
fn nested_symlink_fingerprints_the_link_without_reading_its_target() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let child = fixture.workspace.join("nested");
    seed(&child);
    let external = fixture.root.join("external.txt");
    std::fs::write(&external, "one\n").unwrap();
    symlink(&external, child.join("link")).unwrap();
    let before = snapshot(&fixture.workspace).unwrap();
    std::fs::write(&external, "two\n").unwrap();
    assert_eq!(before.id, snapshot(&fixture.workspace).unwrap().id);
    std::fs::remove_file(child.join("link")).unwrap();
    symlink(fixture.root.join("missing.txt"), child.join("link")).unwrap();
    assert_ne!(before.id, snapshot(&fixture.workspace).unwrap().id);
}
