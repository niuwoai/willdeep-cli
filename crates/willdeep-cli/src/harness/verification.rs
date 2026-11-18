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
    // porcelain -z 的路径以 Git 根目录为基准，子目录工作区也必须在同一坐标抓取。
    let root = std::path::PathBuf::from(
        String::from_utf8(discovery.stdout)
            .map_err(|_| "Git workspace root is not UTF-8".to_owned())?
            .trim_end_matches('\n'),
    );
    crate::daemon::diff_review::snapshot(&root)
        .map(|snapshot| Some(snapshot.id))
        .map_err(|error| {
            let diagnostic = discovery_diagnostic(&format!("{}: {error:#}", workspace.display()));
            format!("Git verification snapshot could not be captured: {diagnostic}")
        })
}

/// 验证失效用的工作区视图：逐文件内容指纹与 Cargo 依赖闭包。
/// 任一步读不了都给 `None`，判定退回整库比较。
pub(super) fn workspace_view(workspace: &Path) -> willdeep_core::VerificationWorkspaceView {
    let inputs = crate::daemon::diff_review::verification_inputs(workspace).ok();
    willdeep_core::VerificationWorkspaceView {
        snapshot_id: inputs.as_ref().map(|(id, _, _)| id.clone()),
        head: inputs.as_ref().map(|(_, head, _)| head.clone()),
        dirty_fingerprints: inputs.map(|(_, _, fingerprints)| fingerprints),
        package_dirs: package_dirs(workspace),
    }
}

/// Cargo 的目标目录、工作区本地依赖闭包和共享构建输入。
/// `--no-deps` 仍含本地包的 dependency.path；任何本地路径无法映射时退回整库。
fn package_dirs(workspace: &Path) -> Option<std::collections::BTreeMap<String, Vec<String>>> {
    use std::collections::{BTreeMap, BTreeSet};
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
    let discovery = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(workspace)
        .output()
        .ok()?;
    if !discovery.status.success() {
        return None;
    }
    let git_root = std::path::PathBuf::from(
        std::str::from_utf8(&discovery.stdout)
            .ok()?
            .trim_end_matches('\n'),
    )
    .canonicalize()
    .ok()?;
    let root = Path::new(metadata.get("workspace_root")?.as_str()?)
        .canonicalize()
        .ok()?;
    let packages = metadata.get("packages")?.as_array()?;
    let prefix = |path: &Path| -> Option<String> {
        let canonical = path.canonicalize().ok()?;
        let relative = canonical
            .strip_prefix(&git_root)
            .ok()?
            .to_str()?
            .replace('\\', "/");
        Some(if relative.is_empty() {
            String::new()
        } else {
            format!("{relative}/")
        })
    };
    let mut dependencies = BTreeMap::<String, Vec<String>>::new();
    for package in packages {
        let manifest = std::path::Path::new(package.get("manifest_path")?.as_str()?);
        let own = prefix(manifest.parent()?)?;
        let local = package
            .get("dependencies")?
            .as_array()?
            .iter()
            .filter_map(|dependency| dependency.get("path").and_then(|path| path.as_str()))
            .map(|path| prefix(std::path::Path::new(path)))
            .collect::<Option<Vec<_>>>()?;
        dependencies.insert(own, local);
    }
    let mut shared_inputs = BTreeSet::new();
    // Cargo/Rustup 也读取调用目录的祖先配置；全部使用 Git 根目录相对路径。
    for ancestor in workspace
        .canonicalize()
        .ok()?
        .ancestors()
        .chain(root.ancestors())
        .filter(|path| path.starts_with(&git_root))
    {
        let base = prefix(ancestor)?;
        for input in [
            "Cargo.toml",
            "Cargo.lock",
            ".cargo/config",
            ".cargo/config.toml",
            "rust-toolchain",
            "rust-toolchain.toml",
        ] {
            shared_inputs.insert(format!("{base}{input}"));
        }
    }
    let mut dirs = BTreeMap::<String, Vec<String>>::new();
    let mut lib_roots = BTreeSet::new();
    for package in packages {
        let name = package.get("name")?.as_str()?;
        let manifest = std::path::Path::new(package.get("manifest_path")?.as_str()?);
        let mut pending = vec![prefix(manifest.parent()?)?];
        let mut covered = BTreeSet::new();
        while let Some(dir) = pending.pop() {
            if covered.insert(dir.clone()) {
                pending.extend(dependencies.get(&dir)?.iter().cloned());
            }
        }
        // 显式 target.src_path 可以放在包目录之外，本地依赖的自定义目标也要计入。
        let package_roots = covered.clone();
        for dependency in packages {
            let manifest = Path::new(dependency.get("manifest_path")?.as_str()?);
            if package_roots.contains(&prefix(manifest.parent()?)?) {
                for target in dependency.get("targets")?.as_array()? {
                    let source = Path::new(target.get("src_path")?.as_str()?);
                    covered.insert(prefix(source.parent()?)?);
                }
            }
        }
        covered.extend(shared_inputs.iter().cloned());
        let roots = covered.into_iter().collect::<Vec<_>>();
        dirs.entry(name.to_owned())
            .or_default()
            .extend(roots.iter().cloned());
        if package
            .get("targets")?
            .as_array()?
            .iter()
            .any(target_is_lib)
        {
            lib_roots.extend(roots.iter().cloned());
        }
        for target in package.get("targets")?.as_array()? {
            let target_name = target.get("name")?.as_str()?;
            if target_name != name {
                // 包名和不同包的目标名可能相同，别名必须保守合并，不能覆盖。
                dirs.entry(target_name.to_owned())
                    .or_default()
                    .extend(roots.iter().cloned());
            }
        }
    }
    if !lib_roots.is_empty() {
        dirs.entry("lib".to_owned()).or_default().extend(lib_roots);
    }
    for roots in dirs.values_mut() {
        roots.sort();
        roots.dedup();
    }
    Some(dirs)
}

fn target_is_lib(target: &serde_json::Value) -> bool {
    target
        .get("kind")
        .and_then(|kind| kind.as_array())
        .is_some_and(|kinds| {
            kinds.iter().any(|kind| {
                matches!(
                    kind.as_str(),
                    Some("lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro")
                )
            })
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
mod coverage_tests;

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
        std::fs::create_dir_all(root.join("crates/other/src")).unwrap();
        // cargo metadata 拒绝没有目标的清单，所以各放一个空 lib。
        std::fs::write(root.join("crates/cli/src/lib.rs"), "").unwrap();
        std::fs::write(root.join("crates/core/src/lib.rs"), "").unwrap();
        std::fs::write(root.join("crates/other/src/lib.rs"), "").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/cli\", \"crates/core\", \"crates/other\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/cli/Cargo.toml"),
            "[package]\nname = \"cli\"\nversion = \"0.0.0\"\n[dependencies]\ncore = { path = \"../core\" }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/core/Cargo.toml"),
            "[package]\nname = \"core\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("crates/other/Cargo.toml"),
            "[package]\nname = \"other\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        run_git(&root, &["init"]);
        run_git(&root, &["config", "user.email", "test@willdeep.invalid"]);
        run_git(&root, &["config", "user.name", "WillDeep Test"]);
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "-m", "seed"]);

        let before = workspace_view(&root);
        assert!(
            before
                .dirty_fingerprints
                .as_ref()
                .is_some_and(|files| files.is_empty())
        );
        let dirs = before.package_dirs.expect("cargo metadata");
        assert!(dirs["cli"].contains(&"crates/cli/".to_owned()));
        assert!(dirs["cli"].contains(&"crates/core/".to_owned()));
        assert!(dirs["core"].contains(&"crates/core/".to_owned()));
        assert!(dirs["lib"].contains(&"crates/cli/".to_owned()));
        assert!(dirs["lib"].contains(&"crates/core/".to_owned()));
        assert!(dirs["lib"].contains(&"crates/other/".to_owned()));

        // 不相关包不影响 CLI；CLI 的依赖 core 则属于覆盖范围。
        std::fs::write(root.join("crates/other/src/lib.rs"), "// touched\n").unwrap();
        let after = workspace_view(&root);
        assert!(
            !willdeep_core::tools::covered_paths_changed(
                "cargo test -p cli",
                before.dirty_fingerprints.as_ref().unwrap(),
                after.dirty_fingerprints.as_ref().unwrap(),
                Some(&dirs),
            ),
            "cli 的测试不覆盖 core 的改动"
        );
        std::fs::write(
            root.join("crates/core/src/lib.rs"),
            "// dependency changed\n",
        )
        .unwrap();
        let dependency = workspace_view(&root);
        assert!(
            willdeep_core::tools::covered_paths_changed(
                "cargo test -p cli",
                after.dirty_fingerprints.as_ref().unwrap(),
                dependency.dirty_fingerprints.as_ref().unwrap(),
                Some(&dirs),
            ),
            "依赖包改动会影响 cli 的检查"
        );
        std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"crates/cli\", \"crates/core\", \"crates/other\"]\ndefault-members = [\"crates/cli\"]\n").unwrap();
        let manifest = workspace_view(&root);
        assert!(
            willdeep_core::tools::covered_paths_changed(
                "cargo test -p cli",
                dependency.dirty_fingerprints.as_ref().unwrap(),
                manifest.dirty_fingerprints.as_ref().unwrap(),
                Some(&dirs),
            ),
            "根清单是 cargo 的共享输入"
        );
        std::fs::write(root.join("Cargo.lock"), "# lockfile changed\n").unwrap();
        let lockfile = workspace_view(&root);
        assert!(
            willdeep_core::tools::covered_paths_changed(
                "cargo test -p cli",
                manifest.dirty_fingerprints.as_ref().unwrap(),
                lockfile.dirty_fingerprints.as_ref().unwrap(),
                Some(&dirs),
            ),
            "锁文件是 cargo 的共享输入"
        );
        std::fs::write(
            root.join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"stable\"\n",
        )
        .unwrap();
        let toolchain = workspace_view(&root);
        assert!(
            willdeep_core::tools::covered_paths_changed(
                "cargo test -p cli",
                lockfile.dirty_fingerprints.as_ref().unwrap(),
                toolchain.dirty_fingerprints.as_ref().unwrap(),
                Some(&dirs),
            ),
            "工具链文件是 cargo 的共享输入"
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
