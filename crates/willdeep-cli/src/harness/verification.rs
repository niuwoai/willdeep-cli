use std::path::Path;
use std::process::Command;

const MAX_DISCOVERY_ERROR_CHARS: usize = 2_048;

/// Absence of a Git repository is a supported workspace mode. Every other
/// discovery or snapshot error must remain distinguishable from that mode.
pub(super) fn snapshot(workspace: &Path) -> Result<Option<String>, String> {
    let discovery = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .env("LC_ALL", "C")
        .current_dir(workspace)
        .output()
        .map_err(|error| {
            let diagnostic = discovery_diagnostic(&format!("{}: {error}", workspace.display()));
            format!("Git workspace discovery could not be started: {diagnostic}")
        })?;
    if !discovery.status.success() {
        let stderr = String::from_utf8_lossy(&discovery.stderr);
        if repository_is_absent(&stderr) {
            return Ok(None);
        }
        let diagnostic = discovery_diagnostic(&format!("{}: {stderr}", workspace.display()));
        return Err(format!(
            "Git workspace discovery failed ({}): {diagnostic}",
            discovery.status
        ));
    }
    crate::daemon::diff_review::snapshot(workspace)
        .map(|snapshot| Some(snapshot.id))
        .map_err(|error| {
            let diagnostic = discovery_diagnostic(&format!("{}: {error:#}", workspace.display()));
            format!("Git verification snapshot could not be captured: {diagnostic}")
        })
}

fn repository_is_absent(stderr: &str) -> bool {
    // Git stops discovery at a filesystem boundary unless explicitly configured
    // otherwise. Both diagnostics mean absence; a broken gitfile is still an error.
    stderr.starts_with("fatal: not a git repository (or any of the parent directories): .git")
        || stderr.starts_with("fatal: not a git repository (or any parent up to mount point ")
}

fn discovery_diagnostic(stderr: &str) -> String {
    // Git can echo a configured gitdir verbatim, including URL credentials.
    let without_urls = stderr
        .split_whitespace()
        .map(|word| {
            if word.contains("://") {
                "[REDACTED_URL]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    willdeep_core::judge::redact_credentials(&without_urls)
        .chars()
        .take(MAX_DISCOVERY_ERROR_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_diagnostics_redact_before_truncating() {
        let stderr = format!(
            "fatal: not a git repository: 'https://fixture:fixture-password@example.invalid/repo?token=fixture-query' api_key=fixture-api-key {}",
            "界".repeat(MAX_DISCOVERY_ERROR_CHARS)
        );
        let diagnostic = discovery_diagnostic(&stderr);
        assert!(diagnostic.contains("[REDACTED_URL]"));
        assert!(diagnostic.contains("api_key=[REDACTED]"));
        for secret in ["fixture-password", "fixture-query", "fixture-api-key"] {
            assert!(!diagnostic.contains(secret), "{diagnostic}");
        }
        assert_eq!(diagnostic.chars().count(), MAX_DISCOVERY_ERROR_CHARS);
        assert!(diagnostic.ends_with('界'));
    }

    #[test]
    fn repository_absence_requires_a_discovery_diagnostic() {
        for stderr in [
            "fatal: not a git repository (or any of the parent directories): .git\n",
            "fatal: not a git repository (or any parent up to mount point /)\nStopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).\n",
        ] {
            assert!(repository_is_absent(stderr), "{stderr}");
        }
        for stderr in [
            "fatal: not a git repository: '/tmp/project/missing-directory'\n",
            "fatal: invalid gitfile format: /tmp/project/.git\n",
            "fatal: gitfile does not point to a valid repository: /tmp/project/.git\n",
            "fatal: detected dubious ownership in repository at '/tmp/project'\n",
            "fatal: failed to stat '/tmp/project/.git': Permission denied\n",
            "",
        ] {
            assert!(!repository_is_absent(stderr), "{stderr}");
        }
    }

    #[test]
    #[ignore = "requires an explicit WILLDEEP_VERIFICATION_DIAGNOSTIC_WORKSPACE"]
    fn diagnostic_snapshot_and_capture_for_explicit_workspace() {
        let workspace = std::path::PathBuf::from(
            std::env::var_os("WILLDEEP_VERIFICATION_DIAGNOSTIC_WORKSPACE")
                .expect("set WILLDEEP_VERIFICATION_DIAGNOSTIC_WORKSPACE explicitly"),
        );
        let baseline = snapshot(&workspace)
            .expect("Harness verification snapshot")
            .expect("diagnostic workspace must be a Git repository");
        let view = crate::daemon::diff_review::snapshot(&workspace).expect("Diff snapshot");
        let _capture = crate::daemon::diff_review::capture(&workspace).expect("Diff capture");
        println!(
            "WILLDEEP_VERIFICATION_DIAGNOSTIC {}",
            serde_json::json!({
                "snapshot_id": baseline,
                "diff_files": view.files.len(),
                "capture_succeeded": true,
            })
        );
    }

    #[test]
    fn snapshot_failure_preserves_the_input_path_and_reason() {
        let root =
            std::env::temp_dir().join(format!("willdeep-snapshot-error-{}", uuid::Uuid::new_v4()));
        let nested = root.join("broken-nested-input");
        std::fs::create_dir_all(&nested).unwrap();
        let init = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(init.status.success());
        std::fs::write(nested.join(".git"), "gitdir: missing-directory\n").unwrap();
        std::fs::write(nested.join("input.txt"), "fixture\n").unwrap();
        let error = snapshot(&root).unwrap_err();
        assert!(error.contains("broken-nested-input"), "{error}");
        assert!(error.contains("not a git repository"), "{error}");
        assert!(error.starts_with("Git verification snapshot could not be captured:"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshots_distinguish_plain_workspaces_repositories_and_broken_git_metadata() {
        let root =
            std::env::temp_dir().join(format!("willdeep-snapshot-mode-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(snapshot(&root).unwrap(), None);
        let init = Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(init.status.success());
        assert!(snapshot(&root).unwrap().is_some());
        std::fs::rename(root.join(".git"), root.join("saved-git")).unwrap();
        std::fs::write(root.join(".git"), "gitdir: missing-directory\n").unwrap();
        let broken = snapshot(&root).unwrap_err();
        assert!(
            broken.contains("Git workspace discovery failed"),
            "{broken}"
        );
        assert!(broken.contains("fatal:"), "{broken}");
        std::fs::remove_dir_all(&root).unwrap();
        let missing = snapshot(&root).unwrap_err();
        assert!(
            missing.contains("Git workspace discovery could not be started"),
            "{missing}"
        );
    }
}
