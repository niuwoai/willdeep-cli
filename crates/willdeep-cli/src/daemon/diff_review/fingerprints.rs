use super::*;

pub(super) const MAX_SUBMODULE_DEPTH: usize = 32;

/// A missing branch is an unborn HEAD; other Git failures must remain errors.
pub(super) fn repository_head(workspace: &Path) -> Result<Option<String>> {
    let output = git_output(workspace, &["rev-parse", "--verify", "--quiet", "HEAD"])?;
    if output.status.success() {
        return Ok(Some(String::from_utf8(output.stdout)?.trim().to_owned()));
    }
    anyhow::ensure!(
        output.status.code() == Some(1) && output.stderr.is_empty(),
        "Cannot resolve Git HEAD in {} ({}): {}",
        workspace.display(),
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let reference = git(workspace, &["symbolic-ref", "--quiet", "HEAD"])?;
    let reference = std::str::from_utf8(&reference)?.trim();
    let exists = git_output(workspace, &["show-ref", "--verify", "--quiet", reference])?;
    anyhow::ensure!(
        exists.status.code() == Some(1) && exists.stderr.is_empty(),
        "Cannot resolve Git HEAD reference {reference} in {} ({}): {}",
        workspace.display(),
        exists.status,
        String::from_utf8_lossy(&exists.stderr).trim()
    );
    Ok(None)
}

pub(super) fn snapshot_id(workspace: &Path, status: &[u8], head: Option<&str>) -> Result<String> {
    snapshot_id_at_depth(workspace, status, head, 0)
}

/// Keep broken Git metadata distinguishable from an uninitialized gitlink.
fn has_git_marker(path: &Path) -> Result<bool> {
    let marker = path.join(".git");
    let metadata = match std::fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("Cannot inspect {}", marker.display()));
        }
    };
    // Git metadata links are resolved by Git, like linked-worktree gitfiles.
    // This does not change the no-follow policy for workspace file contents.
    let metadata = if metadata.file_type().is_symlink() {
        std::fs::metadata(&marker)
            .with_context(|| format!("Cannot inspect Git metadata {}", marker.display()))?
    } else {
        metadata
    };
    anyhow::ensure!(
        metadata.is_file() || metadata.is_dir(),
        "Unsupported Git metadata type at {}",
        marker.display()
    );
    Ok(true)
}

pub(super) fn submodule_fingerprint(
    path: &Path,
    hasher: &mut DefaultHasher,
    depth: usize,
) -> Result<()> {
    repository_fingerprint(path, hasher, depth, true)
}

fn repository_fingerprint(
    path: &Path,
    hasher: &mut DefaultHasher,
    depth: usize,
    allow_uninitialized: bool,
) -> Result<()> {
    (|| {
        anyhow::ensure!(
            depth < MAX_SUBMODULE_DEPTH,
            "repository nesting exceeds snapshot limit ({MAX_SUBMODULE_DEPTH})"
        );
        if !has_git_marker(path)? {
            // An uninitialized gitlink must not discover the parent repository.
            anyhow::ensure!(allow_uninitialized, "nested Git metadata is missing");
            "uninitialized-submodule".hash(hasher);
            return Ok(());
        }
        let status = git(
            path,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ],
        )?;
        let head = repository_head(path)?;
        snapshot_id_at_depth(path, &status, head.as_deref(), depth + 1)?.hash(hasher);
        Ok(())
    })()
    .with_context(|| format!("Cannot fingerprint nested repository {}", path.display()))
}

pub(super) fn untracked_fingerprint(
    path: &Path,
    hasher: &mut DefaultHasher,
    depth: usize,
) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("Cannot inspect verification input {}", path.display()))?;
    if metadata.is_dir() {
        "nested-repository".hash(hasher);
        repository_fingerprint(path, hasher, depth, false)
    } else {
        content_fingerprint(path, hasher)
    }
}

/// Git may expand a directory with a broken gitfile into ordinary untracked
/// paths. Validate their ancestors so the metadata failure cannot be hashed as
/// a normal file, even if Git did not report the directory as a repository.
fn validate_untracked_ancestors(
    workspace: &Path,
    path: &Path,
    checked: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for ancestor in path
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|ancestor| *ancestor != workspace && ancestor.starts_with(workspace))
    {
        if checked.insert(ancestor.to_path_buf()) && has_git_marker(ancestor)? {
            git(ancestor, &["rev-parse", "--show-prefix"])?;
        }
    }
    Ok(())
}

fn snapshot_id_at_depth(
    workspace: &Path,
    status: &[u8],
    head: Option<&str>,
    depth: usize,
) -> Result<String> {
    let mut hasher = DefaultHasher::new();
    // Two clean checkouts can contain entirely different code.
    head.hash(&mut hasher);
    status.hash(&mut hasher);
    git(workspace, &["diff", "--binary", "--no-ext-diff"])?.hash(&mut hasher);
    git(
        workspace,
        &["diff", "--cached", "--binary", "--no-ext-diff"],
    )?
    .hash(&mut hasher);
    // Include gitlinks even when user configuration hides their dirty status.
    let index = git(workspace, &["ls-files", "--stage", "-z"])?;
    for entry in index
        .split(|byte| *byte == 0)
        .filter(|entry| entry.starts_with(b"160000 "))
    {
        let split = entry
            .iter()
            .position(|byte| *byte == b'\t')
            .context("invalid gitlink index entry")?;
        let relative = std::str::from_utf8(&entry[split + 1..]).context("non-UTF8 gitlink path")?;
        relative.hash(&mut hasher);
        let path = workspace.join(relative);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => submodule_fingerprint(&path, &mut hasher, depth)?,
            Ok(_) => content_fingerprint(&path, &mut hasher)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                "missing-submodule".hash(&mut hasher)
            }
            Err(error) => {
                return Err(error).with_context(|| format!("Cannot inspect {}", path.display()));
            }
        }
    }
    let mut checked = BTreeSet::new();
    for file in parse_status(status)
        .into_values()
        .filter(|file| file.kind == DiffFileKind::Untracked)
    {
        file.path.hash(&mut hasher);
        // A trailing slash must not cause symlink_metadata to follow a link.
        let path = workspace.join(file.path.trim_end_matches('/'));
        validate_untracked_ancestors(workspace, &path, &mut checked)?;
        untracked_fingerprint(&path, &mut hasher, depth)?;
    }
    Ok(format!("diff-{:016x}", hasher.finish()))
}

pub(super) fn content_fingerprint(path: &Path, hasher: &mut DefaultHasher) -> Result<()> {
    fingerprint_file(path, hasher)
        .with_context(|| format!("Cannot fingerprint verification input {}", path.display()))
}

fn fingerprint_file(path: &Path, hasher: &mut DefaultHasher) -> Result<()> {
    use std::io::Read;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        "symlink".hash(hasher);
        std::fs::read_link(path)?.hash(hasher);
        return Ok(());
    }
    anyhow::ensure!(metadata.is_file(), "unsupported verification input type");
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let mut file = options.open(path)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "verification input changed type"
    );
    "file".hash(hasher);
    metadata.len().hash(hasher);
    metadata.permissions().readonly().hash(hasher);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode().hash(hasher);
    }
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.write(&buffer[..read]);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
