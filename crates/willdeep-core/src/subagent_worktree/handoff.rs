use super::*;

const MAX_PATCH_BYTES: usize = 16 * 1024 * 1024;

fn excluded_artifact(path: &std::path::Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with(".env")
        || name == "core"
        || name.starts_with("core.")
        || matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("pem" | "key")
        )
}

fn backup_new_files(
    workspace: &Path,
    destination: &Path,
    names: &str,
) -> (Vec<serde_json::Value>, Vec<String>) {
    use sha2::{Digest, Sha256};
    let mut backups = Vec::new();
    let mut omitted = Vec::new();
    let mut remaining = MAX_PATCH_BYTES;
    for name in names.split('\0').filter(|name| !name.is_empty()) {
        let source = workspace.join(name);
        let copy = (|| -> Result<(), ()> {
            if excluded_artifact(&source) {
                return Err(());
            }
            let metadata = std::fs::symlink_metadata(&source).map_err(|_| ())?;
            if !metadata.is_file() || metadata.len() > remaining as u64 {
                return Err(());
            }
            if !source
                .canonicalize()
                .map_err(|_| ())?
                .starts_with(workspace.canonicalize().map_err(|_| ())?)
            {
                return Err(());
            }
            let data = std::fs::read(&source).map_err(|_| ())?;
            let text = String::from_utf8_lossy(&data).to_ascii_lowercase();
            if [
                "api_key",
                "apikey",
                "password",
                "private key",
                "access_token",
            ]
            .iter()
            .any(|pattern| text.contains(pattern))
            {
                return Err(());
            }
            let backup = destination.join(name);
            std::fs::create_dir_all(backup.parent().ok_or(())?).map_err(|_| ())?;
            crate::detached_job::write_private_atomic(&backup, &data).map_err(|_| ())?;
            remaining = remaining.saturating_sub(data.len());
            backups.push(serde_json::json!({"path":name, "backup":backup, "sha256":format!("{:x}",Sha256::digest(&data))}));
            Ok(())
        })();
        if copy.is_err() {
            omitted.push(name.to_owned());
        }
    }
    (backups, omitted)
}

/// Preserve tracked changes outside the checkout without staging or committing.
/// New files stay in the retained checkout and are explicitly listed for review.
async fn bounded_patch(workspace: &Path) -> Result<Vec<u8>, AgentError> {
    use tokio::io::AsyncReadExt;
    let mut child = Command::new("git")
        .kill_on_drop(true)
        .args([
            "diff",
            "--binary",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            "--",
            ".",
            ":(glob,exclude)**/.env*",
            ":(glob,exclude)**/core",
            ":(glob,exclude)**/core.*",
            ":(glob,exclude)**/*.pem",
            ":(glob,exclude)**/*.key",
        ])
        .current_dir(workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| AgentError::Subagent(format!("capture worker patch: {error}")))?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async move {
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .expect("piped git stdout")
            .take(MAX_PATCH_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| AgentError::Subagent(format!("read worker patch: {e}")))?;
        if bytes.len() > MAX_PATCH_BYTES {
            let _ = child.kill().await;
            return Err(AgentError::Subagent(
                "Worker patch exceeds capture limit; original workspace retained".into(),
            ));
        }
        if !child
            .wait()
            .await
            .map_err(|e| AgentError::Subagent(format!("wait for worker patch: {e}")))?
            .success()
        {
            return Err(AgentError::Subagent(
                "Worker patch unavailable; original workspace retained".into(),
            ));
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| {
        AgentError::Subagent("Worker patch capture timed out; original workspace retained".into())
    })?
}

pub(super) async fn capture(prepared: &PreparedSubagentWorkspace) -> Result<String, AgentError> {
    let patch_bytes = match bounded_patch(&prepared.workspace).await {
        Ok(bytes) => bytes,
        Err(error) => {
            return Ok(format!(
                "{error}. Workspace: {}. Review retained changes explicitly; integration remains pending.",
                prepared.workspace.display()
            ));
        }
    };
    let common = git_stdout(&prepared.workspace, &["rev-parse", "--git-common-dir"]).await?;
    let directory = prepared
        .workspace
        .join(common.trim())
        .join("willdeep-handoffs");
    std::fs::create_dir_all(&directory)
        .map_err(|error| AgentError::Subagent(format!("create handoff directory: {error}")))?;
    let id = uuid::Uuid::new_v4();
    let patch = directory.join(format!("{id}.patch"));
    crate::detached_job::write_private_atomic(&patch, &patch_bytes)
        .map_err(|error| AgentError::Subagent(format!("save worker patch: {error}")))?;
    let head = git_stdout(&prepared.workspace, &["rev-parse", "HEAD"]).await?;
    let untracked = git_stdout(
        &prepared.workspace,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )
    .await?;
    let manifest = directory.join(format!("{id}.json"));
    let (backups, omitted) = backup_new_files(
        &prepared.workspace,
        &directory.join(format!("{id}.files")),
        &untracked,
    );
    let record = serde_json::json!({
        "workspace": prepared.workspace, "parent_workspace": prepared.root_workspace,
        "branch": prepared.branch, "head": head.trim(), "tracked_patch": patch,
        "untracked_files_retained_in_workspace": untracked.split('\0').filter(|p| !p.is_empty()).collect::<Vec<_>>(),
        "untracked_file_backups": backups, "omitted_from_backup": omitted,
        "integration": "pending", "parent_verification": "required"
        , "excluded_artifacts": [".env*", "core", "core.*", "*.pem", "*.key"]
    });
    crate::detached_job::write_private_atomic(
        &manifest,
        &serde_json::to_vec_pretty(&record).expect("handoff JSON"),
    )
    .map_err(|error| AgentError::Subagent(format!("save worker manifest: {error}")))?;
    Ok(format!(
        "Tracked patch: {}\nHandoff manifest: {}\nNew-file backups and omissions are listed in the manifest; original files remain in the retained workspace. Review them separately. HEAD: {}",
        patch.display(),
        manifest.display(),
        head.trim()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn captures_staged_and_unstaged_changes_without_modifying_index() {
        let root = std::env::temp_dir().join(format!("handoff-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "base",
            ],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&root)
                    .status()
                    .await
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(root.join("fix.rs"), "staged\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "fix.rs"])
                .current_dir(&root)
                .status()
                .await
                .unwrap()
                .success()
        );
        std::fs::write(root.join("fix.rs"), "final\n").unwrap();
        std::fs::write(root.join("new.rs"), "new file\n").unwrap();
        std::fs::write(root.join(".env.secret"), "PRIVATE_NEW_VALUE\n").unwrap();
        std::fs::write(root.join("core"), "crash artifact\n").unwrap();
        std::fs::write(root.join(".env.local"), "PRIVATE_FIXTURE_VALUE\n").unwrap();
        assert!(
            Command::new("git")
                .args(["add", "core", ".env.local"])
                .current_dir(&root)
                .status()
                .await
                .unwrap()
                .success()
        );
        let before = git_stdout(&root, &["status", "--porcelain"]).await.unwrap();
        let prepared = PreparedSubagentWorkspace {
            workspace: root.clone(),
            root_workspace: root.clone(),
            branch: None,
            dedicated: true,
        };
        let note = capture(&prepared).await.unwrap();
        assert!(note.contains("Tracked patch:"));
        assert_eq!(
            before,
            git_stdout(&root, &["status", "--porcelain"]).await.unwrap()
        );
        let files = std::fs::read_dir(root.join(".git/willdeep-handoffs")).unwrap();
        let patch = files
            .map(|entry| entry.unwrap().path())
            .find(|p| p.extension().is_some_and(|e| e == "patch"))
            .unwrap();
        let contents = std::fs::read_to_string(&patch).unwrap();
        assert!(contents.contains("+final"));
        assert!(!contents.contains("crash artifact"));
        assert!(!contents.contains("PRIVATE_FIXTURE_VALUE"));
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(patch.with_extension("json")).unwrap()).unwrap();
        let backup = manifest["untracked_file_backups"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["path"] == "new.rs")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(backup["backup"].as_str().unwrap()).unwrap(),
            "new file\n"
        );
        assert!(
            manifest["omitted_from_backup"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == ".env.secret")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
