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

/// 验证失效用来收窄范围的工作区视图：当前与 HEAD 不一致的路径，以及 cargo
/// 目标名到清单所在目录的映射。任何一步读不了都给 `None`，判定退回整库比较，
/// 不拿残缺的列表当「没变化」。只读，不改索引也不改工作区。
pub(super) fn workspace_view(workspace: &Path) -> willdeep_core::VerificationWorkspaceView {
    let dirty_paths = crate::daemon::diff_review::snapshot(workspace)
        .ok()
        .map(|snapshot| {
            snapshot
                .files
                .iter()
                .map(|file| file.path.clone())
                .collect()
        });
    willdeep_core::VerificationWorkspaceView {
        dirty_paths,
        package_dirs: package_dirs(workspace),
    }
}

/// `cargo metadata` 给出的包名与目标名到清单目录的映射。`--lib` 记在 `lib` 下，
/// 指向根包的清单目录。读不了（不是 cargo 工作区、cargo 不在）时返回 `None`，
/// 点名目标的命令因此维持整库比较。
fn package_dirs(workspace: &Path) -> Option<std::collections::BTreeMap<String, String>> {
    let output = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
        ])
        .env("LC_ALL", "C")
        .current_dir(workspace)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let root = metadata.get("workspace_root")?.as_str()?;
    let mut dirs = std::collections::BTreeMap::new();
    for package in metadata.get("packages")?.as_array()? {
        let name = package.get("name")?.as_str()?;
        let manifest = package.get("manifest_path")?.as_str()?;
        let dir = std::path::Path::new(manifest).parent()?;
        let relative = dir.strip_prefix(root).ok()?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        let prefix = if relative.is_empty() {
            String::new()
        } else {
            format!("{relative}/")
        };
        dirs.insert(name.to_owned(), prefix.clone());
        if package
            .get("targets")?
            .as_array()?
            .iter()
            .any(target_is_lib)
            && !dirs.contains_key("lib")
        {
            dirs.insert("lib".to_owned(), prefix.clone());
        }
        for target in package.get("targets")?.as_array()? {
            let Some(target_name) = target.get("name").and_then(|name| name.as_str()) else {
                continue;
            };
            if target_name != name {
                dirs.insert(target_name.to_owned(), prefix.clone());
            }
        }
    }
    Some(dirs)
}

fn target_is_lib(target: &serde_json::Value) -> bool {
    target
        .get("kind")
        .and_then(|kind| kind.as_array())
        .is_some_and(|kinds| {
            kinds
                .iter()
                .any(|kind| matches!(kind.as_str(), Some("lib") | Some("proc-macro")))
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
        let probe = Command::new("git")
            .args(["rev-parse", "--show-prefix"])
            .env("LC_ALL", "C")
            .current_dir(&nested)
            .output()
            .unwrap();
        assert!(!probe.status.success());
        // Git versions use different messages for the same broken gitfile.
        // The snapshot must retain this Git's actual diagnostic, not a fixed phrase.
        let reason = discovery_diagnostic(&String::from_utf8_lossy(&probe.stderr));
        assert!(!reason.is_empty(), "Git must explain the broken metadata");
        let error = snapshot(&root).unwrap_err();
        assert!(error.contains("broken-nested-input"), "{error}");
        assert!(error.contains(&reason), "{error}");
        assert!(error.starts_with("Git verification snapshot could not be captured:"));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// 另一个会话改了命令覆盖不到的文件：工作区视图能把两边的脏路径分开，
    /// 判定据此放行。覆盖范围内的文件变了，或工作区不是 cargo 工作区时，不放行。
    #[test]
    fn a_concurrent_edit_outside_coverage_is_visible_as_such() {
        let root = std::env::temp_dir().join(format!("willdeep-coverage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("crates/cli/src")).unwrap();
        std::fs::create_dir_all(root.join("crates/core/src")).unwrap();
        // cargo metadata 拒绝没有目标的清单，所以各放一个空 lib。
        std::fs::write(root.join("crates/cli/src/lib.rs"), "").unwrap();
        std::fs::write(root.join("crates/core/src/lib.rs"), "").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/cli\", \"crates/core\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/cli/Cargo.toml"),
            "[package]\nname = \"cli\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/core/Cargo.toml"),
            "[package]\nname = \"core\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        run_git(&root, &["init"]);
        run_git(&root, &["config", "user.email", "test@willdeep.invalid"]);
        run_git(&root, &["config", "user.name", "WillDeep Test"]);
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "-m", "seed"]);

        let before = workspace_view(&root);
        assert_eq!(before.dirty_paths.as_deref(), Some(&[][..]));
        let dirs = before.package_dirs.expect("cargo metadata");
        assert_eq!(dirs.get("cli").map(String::as_str), Some("crates/cli/"));
        assert_eq!(dirs.get("core").map(String::as_str), Some("crates/core/"));

        // 另一个会话改了 core，这条命令只跑 cli。
        std::fs::write(root.join("crates/core/src/lib.rs"), "// touched\n").unwrap();
        let after = workspace_view(&root);
        assert!(
            !willdeep_core::tools::covered_paths_changed(
                "cargo test -p cli",
                before.dirty_paths.as_deref().unwrap(),
                after.dirty_paths.as_deref().unwrap(),
                Some(&dirs),
            ),
            "cli 的测试不覆盖 core 的改动"
        );
        assert!(
            willdeep_core::tools::covered_paths_changed(
                "cargo test -p core",
                before.dirty_paths.as_deref().unwrap(),
                after.dirty_paths.as_deref().unwrap(),
                Some(&dirs),
            ),
            "core 自己的改动仍算数"
        );

        // 清单读不了：映射给 None，点名目标的命令因此维持整库比较。
        std::fs::write(root.join("Cargo.toml"), "this is not toml [[[").unwrap();
        let view = workspace_view(&root);
        assert!(view.package_dirs.is_none(), "坏掉的清单给不出映射");
        std::fs::remove_dir_all(root).unwrap();
    }

    fn run_git(workspace: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(workspace)
            .status()
            .unwrap();
        assert!(status.success(), "git {} failed", args.join(" "));
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
