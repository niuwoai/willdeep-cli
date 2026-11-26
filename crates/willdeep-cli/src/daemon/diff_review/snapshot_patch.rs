use super::*;

pub(super) fn content(
    workspace: &Path,
    snapshot: &DiffSnapshot,
    path: &str,
    area: DiffArea,
) -> Result<String, StatusCode> {
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .ok_or(StatusCode::NOT_FOUND)?;
    let patch = patch(workspace, file, area);
    exact_snapshot(workspace, &snapshot.id)?;
    patch.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

fn patch(workspace: &Path, file: &DiffFile, area: DiffArea) -> Result<String> {
    let mut bytes = Vec::new();
    if file.staged && matches!(area, DiffArea::Staged | DiffArea::Combined) {
        bytes.extend(tracked_patch(workspace, file, true)?);
    }
    if file.unstaged && matches!(area, DiffArea::Unstaged | DiffArea::Combined) {
        let unstaged = tracked_patch(workspace, file, false)?;
        if unstaged.is_empty() {
            // A staged deletion and a replacement outside the index can share a path.
            // Ask Git which paths are untracked instead of treating an empty patch as a new file.
            let untracked = git(
                workspace,
                &[
                    "--literal-pathspecs",
                    "ls-files",
                    "--others",
                    "--exclude-standard",
                    "-z",
                    "--",
                    &file.path,
                ],
            )?;
            if untracked.strip_suffix(&[0]) == Some(file.path.as_bytes()) {
                bytes.extend(untracked_patch(workspace, &file.path)?);
            }
        } else {
            bytes.extend(unstaged);
        }
    }
    let truncated = bytes.len() > MAX_DIFF_BYTES;
    bytes.truncate(MAX_DIFF_BYTES);
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        text.push_str("\n[diff truncated]");
    }
    Ok(text)
}

const GIT_PATCH_OPTIONS: &[&str] = &["--no-ext-diff", "--no-textconv", "--no-color"];

fn tracked_patch(workspace: &Path, file: &DiffFile, staged: bool) -> Result<Vec<u8>> {
    let mut args = vec!["--literal-pathspecs", "diff", "--no-relative"];
    args.extend_from_slice(GIT_PATCH_OPTIONS);
    if staged {
        args.push("--cached");
    }
    if let Some(source) = file.old_path.as_deref() {
        // The source can have an independent patch in either area. Use it only
        // when Git identifies this exact source/destination pair for this area.
        let pair_options = ["-C", "--diff-filter=CR"];
        let paths = ["--", file.path.as_str(), source];
        let mut status_args = args.clone();
        status_args.extend(pair_options);
        status_args.extend(["--name-status", "-z"]);
        status_args.extend(paths);
        let status = git(workspace, &status_args)?;
        let mut fields = status.split(|byte| *byte == 0);
        if fields
            .next()
            .is_some_and(|kind| matches!(kind.first(), Some(b'C' | b'R')))
            && fields.next() == Some(source.as_bytes())
            && fields.next() == Some(file.path.as_bytes())
            && fields.next() == Some(&[])
            && fields.next().is_none()
        {
            args.extend(pair_options);
            args.extend(paths);
            return git(workspace, &args);
        }
    }
    args.extend(["--", file.path.as_str()]);
    git(workspace, &args)
}

fn untracked_patch(workspace: &Path, path: &str) -> Result<Vec<u8>> {
    safe_workspace_path(workspace, path)?;
    let metadata = std::fs::symlink_metadata(workspace.join(path))?;
    if !metadata.is_file() && !metadata.file_type().is_symlink() {
        return Ok(Vec::new());
    }
    let mut args = vec!["diff", "--no-index"];
    args.extend_from_slice(GIT_PATCH_OPTIONS);
    args.extend(["--", "/dev/null", path]);
    let output = git_output(workspace, &args)?;
    anyhow::ensure!(
        matches!(output.status.code(), Some(0 | 1)),
        "Cannot read untracked diff for {path}"
    );
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_destination_excludes_independent_source_edits_in_both_areas() {
        let workspace =
            std::env::temp_dir().join(format!("willdeep-copy-patch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&workspace).unwrap();
        let run = |args: &[&str]| git(&workspace, args).unwrap();
        let write = |path, text: &str| std::fs::write(workspace.join(path), text).unwrap();
        let original = "original content for similarity detection\n".repeat(20);
        run(&["init", "--quiet"]);
        run(&["config", "status.renames", "copies"]);
        run(&["config", "diff.renames", "copies"]);
        write("source.txt", &original);
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "Fixture",
        ]);
        write("copy.txt", &original);
        write(
            "source.txt",
            &format!("{original}independent staged source edit\n"),
        );
        run(&["add", "."]);
        write(
            "source.txt",
            &format!("{original}independent unstaged source edit\n"),
        );
        write(
            "copy.txt",
            &format!("{original}unstaged destination edit\n"),
        );
        let initial = snapshot(&workspace).unwrap();
        let staged = content(&workspace, &initial, "copy.txt", DiffArea::Staged).unwrap();
        assert!(
            staged.contains("copy from source.txt\ncopy to copy.txt"),
            "{staged}"
        );
        assert!(
            !staged.contains("independent staged source edit"),
            "{staged}"
        );
        let unstaged = content(&workspace, &initial, "copy.txt", DiffArea::Unstaged).unwrap();
        assert!(
            unstaged.contains("+unstaged destination edit"),
            "{unstaged}"
        );
        assert!(
            !unstaged.contains("independent unstaged source edit"),
            "{unstaged}"
        );
        let source = content(&workspace, &initial, "source.txt", DiffArea::Staged).unwrap();
        assert!(
            source.contains("+independent staged source edit"),
            "{source}"
        );
        std::fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn patch_reads_only_the_requested_area_and_literal_path_in_a_snapshot() {
        let workspace =
            std::env::temp_dir().join(format!("willdeep-snapshot-patch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&workspace).unwrap();
        let run = |args: &[&str]| git(&workspace, args).unwrap();
        let write = |path, text| std::fs::write(workspace.join(path), text).unwrap();
        run(&["init", "--quiet"]);
        write("name[1].txt", "base\n");
        write("name1.txt", "unrelated base\n");
        write("source.txt", "rename body\n");
        write("replaced.txt", "old tracked value\n");
        run(&["add", "."]);
        run(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "Fixture",
        ]);
        write("name[1].txt", "staged body\n");
        run(&["--literal-pathspecs", "add", "name[1].txt"]);
        write("name[1].txt", "working body\n");
        write("name1.txt", "unrelated edit\n");
        run(&["mv", "source.txt", "destination.txt"]);
        run(&["rm", "--cached", "replaced.txt"]);
        write("replaced.txt", "untracked replacement\n");
        write("new file.txt", "no final newline");
        let initial = snapshot(&workspace).unwrap();
        let cases = [
            (
                "name[1].txt",
                DiffArea::Staged,
                "+staged body",
                "working body",
            ),
            (
                "name[1].txt",
                DiffArea::Unstaged,
                "+working body",
                "unrelated edit",
            ),
            (
                "name[1].txt",
                DiffArea::Combined,
                "+working body",
                "unrelated edit",
            ),
            (
                "new file.txt",
                DiffArea::Unstaged,
                "\\ No newline at end of file",
                "new file.txt~",
            ),
            (
                "destination.txt",
                DiffArea::Staged,
                "rename from source.txt",
                "new file mode",
            ),
            (
                "replaced.txt",
                DiffArea::Staged,
                "-old tracked value",
                "untracked replacement",
            ),
            (
                "replaced.txt",
                DiffArea::Unstaged,
                "+untracked replacement",
                "old tracked value",
            ),
        ];
        for (path, area, contains, excludes) in cases {
            let diff = content(&workspace, &initial, path, area).unwrap();
            assert!(diff.contains(contains), "{path} {area:?}: {diff}");
            assert!(!diff.contains(excludes), "{path} {area:?}: {diff}");
        }
        assert_eq!(
            content(&workspace, &initial, "new file.txt", DiffArea::Staged).unwrap(),
            ""
        );
        assert_eq!(
            content(&workspace, &initial, "../outside.txt", DiffArea::Combined),
            Err(StatusCode::NOT_FOUND)
        );
        write("replaced.txt", "changed since snapshot\n");
        assert_eq!(
            content(&workspace, &initial, "replaced.txt", DiffArea::Unstaged),
            Err(StatusCode::CONFLICT)
        );
        std::fs::remove_dir_all(workspace).unwrap();
    }
}
