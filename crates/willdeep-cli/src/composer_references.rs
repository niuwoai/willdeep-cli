//! Shared, credential-free @ candidates for the terminal and Web composer.
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;

const MAX_FILES: usize = 20_000;
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", ".willdeep"];

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Reference {
    pub kind: &'static str,
    pub name: String,
    pub value: String,
}

pub(crate) fn collect(
    root: &Path,
    home: &Path,
    config: &crate::config::ConfigFile,
    allowed_mcp: &[String],
    query: &str,
) -> anyhow::Result<Vec<Reference>> {
    let mut entries = Vec::new();
    for (name, server) in &config.mcp_servers {
        if server.enabled && (allowed_mcp.is_empty() || allowed_mcp.contains(name)) {
            entries.push(Reference {
                kind: "mcp",
                name: name.clone(),
                value: format!("@mcp:{name}"),
            });
        }
    }
    if allowed_mcp.is_empty() {
        use willdeep_core::mcp::LazyToolSource;
        for tool in willdeep_core::plugin::PluginChatTools::new(home).definitions() {
            if let Some(name) = tool
                .name
                .strip_prefix("mcp__")
                .and_then(|name| name.split("__").next())
                && !entries.iter().any(|entry| entry.name == name)
            {
                entries.push(Reference {
                    kind: "mcp",
                    name: name.to_owned(),
                    value: format!("@mcp:{name}"),
                });
            }
        }
    }
    for id in willdeep_core::subagent::PUBLIC_SUBAGENT_IDS {
        entries.push(Reference {
            kind: "agent",
            value: format!("@agent:{id}"),
            name: id.to_owned(),
        });
    }
    entries.extend(installed_agents(home)?);
    for name in files(root) {
        entries.push(Reference {
            kind: "file",
            value: format!("@file:{}", serde_json::to_string(&name).unwrap_or_default()),
            name,
        });
    }
    let query = query.to_lowercase();
    let (kind, search) = match query.split_once(':') {
        Some((kind @ ("mcp" | "agent" | "file"), search)) => (Some(kind), search),
        _ => (None, query.as_str()),
    };
    entries.retain(|entry| {
        kind.is_none_or(|kind| entry.kind == kind)
            && (entry.name.to_lowercase().contains(search)
                || entry.value.to_lowercase().contains(search))
    });
    Ok(entries)
}

fn installed_agents(home: &Path) -> anyhow::Result<Vec<Reference>> {
    use willdeep_core::agent_definition::store::AgentLibrary;
    let mut libraries = vec![AgentLibrary::new(home)];
    if let Ok(session) = crate::agent_registry::service::CustomerSession::load(home) {
        libraries.push(session.library(home));
    }
    let mut agents = BTreeMap::new();
    for library in libraries {
        for item in library
            .installations()?
            .values()
            .filter(|item| item.enabled)
        {
            let package = library.package(&item.sha256)?;
            if crate::agent_registry::compatibility(&package.definition).is_err() {
                continue;
            }
            let definition = package.definition;
            let label = definition
                .name
                .get("zh-Hans")
                .or_else(|| definition.name.get("en"));
            let id = definition.definition_id;
            let name = label
                .map(|label| format!("{id} · {label}"))
                .unwrap_or_else(|| id.clone());
            agents.insert(
                id.clone(),
                Reference {
                    kind: "agent",
                    name,
                    value: format!("@agent:{id}"),
                },
            );
        }
    }
    Ok(agents.into_values().collect())
}

fn files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut git_listing = false;
    if let Ok(mut child) = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(stdout) = child.stdout.take() {
            for bytes in BufReader::new(stdout).split(0).take(MAX_FILES).flatten() {
                if let Ok(name) = String::from_utf8(bytes) {
                    files.push(name);
                }
            }
        }
        // Also reap a truncated listing rather than leaving a blocked subprocess.
        if files.len() >= MAX_FILES {
            let _ = child.kill();
            git_listing = true;
        }
        if let Ok(status) = child.wait() {
            git_listing |= status.success();
        }
    }
    if !git_listing {
        let mut remaining = MAX_FILES;
        walk(root, root, &mut files, &mut remaining);
    }
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    files.retain(|name| {
        !name.chars().any(char::is_control)
            && root
                .join(name)
                .symlink_metadata()
                .is_ok_and(|meta| meta.is_file())
            && root
                .join(name)
                .canonicalize()
                .is_ok_and(|path| path.starts_with(&canonical_root))
    });
    files.sort();
    files.dedup();
    files
}

fn walk(root: &Path, dir: &Path, files: &mut Vec<String>, remaining: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *remaining == 0 {
            return;
        }
        *remaining -= 1;
        let path: PathBuf = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && !SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()) {
            walk(root, &path, files, remaining);
        } else if kind.is_file()
            && let Ok(name) = path.strip_prefix(root)
        {
            files.push(name.to_string_lossy().into_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_are_ordered_and_files_are_quoted_and_scoped() {
        let root = std::env::temp_dir().join(format!("composer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("中文 file.rs"), "test").unwrap();
        let mut config = crate::config::ConfigFile::default();
        config
            .mcp_servers
            .insert("search".into(), Default::default());
        let entries = collect(&root, &root, &config, &[], "").unwrap();
        assert_eq!(entries.first().unwrap().kind, "mcp");
        assert!(
            entries
                .iter()
                .any(|entry| entry.value == "@agent:generalist")
        );
        assert_eq!(entries.last().unwrap().value, "@file:\"中文 file.rs\"");
        assert!(
            !collect(&root, &root, &config, &["other".into()], "")
                .unwrap()
                .iter()
                .any(|e| e.kind == "mcp")
        );
        assert_eq!(
            collect(&root, &root, &config, &[], "中文").unwrap().len(),
            1
        );
        assert_eq!(
            collect(&root, &root, &config, &[], "file:中文")
                .unwrap()
                .len(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installed_agents_include_official_and_only_the_active_private_account() {
        use crate::agent_registry::service::CustomerSession;
        use willdeep_core::agent_definition::store::{AgentLibrary, Origin};
        use willdeep_core::agent_definition::{AgentDefinition, AgentPackage};
        let root = std::env::temp_dir().join(format!("composer-agents-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let library = AgentLibrary::new(&root);
        let mut definition = AgentDefinition::parse(include_bytes!(
            "../../../docs/examples/agent-definition.v1.json"
        ))
        .unwrap();
        definition.definition_id = "user/reference-reviewer".into();
        let package = AgentPackage::local(definition.clone()).unwrap();
        library
            .install(&package, Origin::Local, &BTreeMap::new())
            .unwrap();
        definition.definition_id = "user/disabled-reviewer".into();
        let disabled = AgentPackage::local(definition.clone()).unwrap();
        library
            .install(&disabled, Origin::Local, &BTreeMap::new())
            .unwrap();
        library
            .set_enabled(&definition.definition_id, false)
            .unwrap();
        let golden: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../docs/examples/agent-contract-golden.json"
        ))
        .unwrap();
        let official =
            AgentPackage::parse(&serde_json::to_vec(&golden["packages"][2]).unwrap()).unwrap();
        let keys = serde_json::from_value(golden["keys"].clone()).unwrap();
        library.install(&official, Origin::Official, &keys).unwrap();
        let mut session = CustomerSession {
            origin: crate::agent_registry::service::DEFAULT_ORIGIN.into(),
            token: "fixture-customer-session".into(),
            user_id: "fixture-user".into(),
            tenant_id: "fixture-tenant".into(),
        };
        definition.definition_id = "user/private-reviewer".into();
        let private = AgentPackage::local(definition).unwrap();
        session
            .library(&root)
            .install(&private, Origin::Private, &BTreeMap::new())
            .unwrap();
        let config = crate::config::ConfigFile::default();
        let values = |entries: Vec<Reference>| {
            entries
                .into_iter()
                .map(|entry| entry.value)
                .collect::<Vec<_>>()
        };
        let anonymous = values(collect(&root, &root, &config, &[], "agent:").unwrap());
        assert!(anonymous.contains(&"@agent:user/reference-reviewer".into()));
        assert!(anonymous.contains(&format!("@agent:{}", official.definition.definition_id)));
        assert!(!anonymous.contains(&"@agent:user/disabled-reviewer".into()));
        assert!(!anonymous.contains(&"@agent:user/private-reviewer".into()));
        session.save(&root).unwrap();
        assert!(
            values(collect(&root, &root, &config, &[], "agent:private-reviewer").unwrap())
                .contains(&"@agent:user/private-reviewer".into())
        );
        session.tenant_id = "other-tenant".into();
        session.save(&root).unwrap();
        assert!(
            collect(&root, &root, &config, &[], "agent:private-reviewer")
                .unwrap()
                .is_empty()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_git_listing_does_not_expose_ignored_files_or_symlinks() {
        let root = std::env::temp_dir().join(format!("composer-git-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--quiet"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(root.join(".gitignore"), "*\n").unwrap();
        std::fs::write(root.join("ignored.txt"), "private").unwrap();
        assert!(files(&root).is_empty());
        std::fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(root.join("visible.rs"), "test").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("ignored.txt"), root.join("link.rs")).unwrap();
        assert_eq!(files(&root), [".gitignore", "visible.rs"]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
