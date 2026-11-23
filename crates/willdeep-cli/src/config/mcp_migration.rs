use std::path::Path;

use anyhow::Result;
use willdeep_core::McpServerConfig;

use super::ConfigFile;
use super::mcp_files;

/// 主配置里残留的 `[mcp_servers.*]` 拆出来之后的结果。
///
/// 用文档值而不是 [`ConfigFile`] 来解析：MCP 段里多一个不认识的字段时，
/// 只丢掉 MCP，providers 和其他段照常。
pub(super) enum LegacyMcp {
    /// 主配置里没有 MCP 段。
    Absent,
    /// 主配置本身就不是合法 TOML。不能安全地判断哪些行属于 MCP，
    /// 所以一行都不动，交给后面的严格解析报原因的错。
    Unreadable,
    /// 每一段都是合法服务，可以迁走并从主配置里删掉。
    Clean {
        stripped: String,
        servers: std::collections::BTreeMap<String, McpServerConfig>,
    },
    /// 至少一段读不懂。这些段留在磁盘上，但解析主配置时忽略；
    /// 读懂的段照常迁走。
    Broken {
        stripped: String,
        servers: std::collections::BTreeMap<String, McpServerConfig>,
        reasons: Vec<String>,
    },
}

/// 把独立目录里的 MCP 服务填进已解析的配置，并顺手把主配置里残留的
/// `[mcp_servers.*]` 迁出去。
///
/// 迁完才重写主配置；重写失败就把刚迁出的文件删掉，下次启动再迁。
/// 读不懂的段一直留在磁盘上，但不参与解析。返回没能采用的文件说明。
pub(super) fn attach(
    file: &mut ConfigFile,
    config_path: &Path,
    original: &str,
    legacy: LegacyMcp,
) -> Result<Vec<String>> {
    let directory = mcp_files::directory_for(Some(config_path))?;
    let mut skipped = Vec::new();
    let (rewrite, servers) = match legacy {
        LegacyMcp::Absent | LegacyMcp::Unreadable => (None, std::collections::BTreeMap::new()),
        LegacyMcp::Clean { stripped, servers } => (Some(stripped), servers),
        LegacyMcp::Broken {
            servers, reasons, ..
        } => {
            skipped.extend(reasons);
            // 坏段不能从主配置里删：删了就没法再改回来。好段仍然迁走。
            (None, servers)
        }
    };
    let mut migrated = Vec::new();
    if !servers.is_empty() {
        match mcp_files::extract_legacy(&directory, original, &servers) {
            Ok(written) => migrated = written,
            Err(error) => skipped.push(format!(
                "could not migrate [mcp_servers] out of {}: {error:#}",
                config_path.display()
            )),
        }
    }
    let covered = servers.keys().all(|name| {
        migrated.contains(name)
            || mcp_files::file_for(&directory, name).is_ok_and(|path| path.is_file())
    });
    if let Some(stripped) = rewrite
        && covered
        && !servers.is_empty()
    {
        // 每个服务都有独立副本了：要么这次新写成，要么本来就有同名文件。
        // 主配置里的旧段可以删。删不掉就回滚这次新写的文件，下次再迁。
        if let Err(error) =
            crate::model_routing::write_private_atomic(config_path, stripped.as_bytes())
        {
            for name in &migrated {
                if let Err(remove_error) = mcp_files::remove_server(&directory, name) {
                    skipped.push(format!(
                        "could not roll back migrated MCP server {name}: {remove_error:#}"
                    ));
                }
            }
            skipped.push(format!(
                "could not remove migrated [mcp_servers] from {}: {error:#}",
                config_path.display()
            ));
        }
    }
    let loaded = mcp_files::load_directory(&directory);
    // 独立文件是权威来源。主配置里同名的残留只在独立文件还没写成时兜底，
    // 免得一次迁移写到一半把服务弄丢。
    for (name, config) in servers {
        file.mcp_servers.entry(name).or_insert(config);
    }
    for (name, config) in loaded.servers {
        file.mcp_servers.insert(name, config);
    }
    skipped.extend(loaded.skipped);
    for line in &skipped {
        eprintln!("warning: {line}");
    }
    Ok(skipped)
}

/// [`LegacyMcp`] 里摘掉 MCP 段之后的正文。没有 MCP 段时就是原文。
pub(super) fn parseable<'a>(contents: &'a str, legacy: &'a LegacyMcp) -> &'a str {
    match legacy {
        LegacyMcp::Clean { stripped, .. } | LegacyMcp::Broken { stripped, .. } => stripped,
        LegacyMcp::Absent | LegacyMcp::Unreadable => contents,
    }
}

/// 从正文里拆出 `[mcp_servers.*]`。
///
/// 主配置有别的语法错误时不猜哪些行是 MCP。每一段都能读懂，才允许之后
/// 把这些段从主配置里删掉；有一段读不懂，读懂的段仍然返回，但主配置原文不动。
pub(super) fn take_legacy(contents: &str) -> LegacyMcp {
    let value: toml::Value = match toml::from_str(contents) {
        Ok(value) => value,
        Err(error) => {
            eprintln!(
                "warning: configuration has a TOML syntax error ({error}); MCP servers were not migrated"
            );
            return LegacyMcp::Unreadable;
        }
    };
    let Some(table) = value.as_table() else {
        return LegacyMcp::Absent;
    };
    let Some(servers) = table.get("mcp_servers").and_then(toml::Value::as_table) else {
        return LegacyMcp::Absent;
    };
    if servers.is_empty() {
        return LegacyMcp::Absent;
    }
    let mut parsed = std::collections::BTreeMap::new();
    let mut reasons = Vec::new();
    for (name, value) in servers {
        match value.clone().try_into::<McpServerConfig>() {
            Ok(config) => {
                if let Err(error) = config.validate(name) {
                    reasons.push(format!(
                        "[mcp_servers.{name}] left in config.toml: {error}"
                    ));
                    continue;
                }
                parsed.insert(name.clone(), config);
            }
            Err(error) => reasons.push(format!(
                "[mcp_servers.{name}] left in config.toml because it does not match an MCP server: {error}"
            )),
        }
    }
    let stripped = crate::model_routing::strip_mcp_sections(contents);
    if reasons.is_empty() {
        LegacyMcp::Clean {
            stripped,
            servers: parsed,
        }
    } else {
        LegacyMcp::Broken {
            stripped,
            servers: parsed,
            reasons,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{LoadedConfig, parse_config_text};

    fn scratch_config(body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("willdeep-mcp-load-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        (root, path)
    }

    #[test]
    fn legacy_mcp_sections_move_into_standalone_files() {
        let (root, path) = scratch_config(
            "version = 1\n\n# keep me\n[providers.main]\nprovider = \"openai-compatible\"\nmodel = \"kept\"\n\n[mcp_servers.filesystem]\ncommand = \"npx\"\nargs = [\"-y\", \"server\"]\nstartup_timeout_seconds = 12\nenabled = false\n\n[mcp_servers.filesystem.env]\nNODE_NO_WARNINGS = \"1\"\n",
        );
        let loaded = LoadedConfig::load(Some(&path)).expect("legacy MCP still loads");
        let server = loaded
            .file
            .mcp_servers
            .get("filesystem")
            .expect("filesystem migrated");
        assert_eq!(server.command.as_deref(), Some("npx"));
        assert_eq!(
            server.env.get("NODE_NO_WARNINGS").map(String::as_str),
            Some("1")
        );
        assert!(!server.enabled);
        assert_eq!(
            loaded
                .file
                .providers
                .get("main")
                .and_then(|profile| profile.model.as_deref()),
            Some("kept")
        );

        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert!(!rewritten.contains("mcp_servers"), "{rewritten}");
        assert!(rewritten.contains("# keep me"), "{rewritten}");
        assert!(rewritten.contains("model = \"kept\""), "{rewritten}");

        let standalone = std::fs::read_to_string(root.join(".mcp/filesystem.toml")).unwrap();
        assert!(standalone.contains("command = \"npx\""), "{standalone}");
        assert!(standalone.contains("[env]"), "{standalone}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_broken_legacy_section_does_not_fail_the_rest_of_the_config() {
        let (root, path) = scratch_config(
            "version = 1\n[providers.main]\nprovider = \"openai-compatible\"\nmodel = \"kept\"\n\n[mcp_servers.dingtalk]\nurl = \"https://minutes.example.com/mcp\"\ntransport = \"sse\"\nenabled = true\n\n[mcp_servers.filesystem]\ncommand = \"npx\"\nstartup_timeout_seconds = 10\nenabled = true\n",
        );
        let loaded = LoadedConfig::load(Some(&path)).expect("broken MCP must not fail load");
        assert_eq!(
            loaded
                .file
                .providers
                .get("main")
                .and_then(|profile| profile.model.as_deref()),
            Some("kept")
        );
        assert!(loaded.file.mcp_servers.contains_key("filesystem"));
        assert!(!loaded.file.mcp_servers.contains_key("dingtalk"));
        assert!(
            loaded
                .mcp_skipped
                .iter()
                .any(|line| line.contains("dingtalk")),
            "{:?}",
            loaded.mcp_skipped
        );

        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert!(
            rewritten.contains("transport"),
            "the unread section stays: {rewritten}"
        );
        assert!(
            rewritten.contains("[mcp_servers.filesystem]"),
            "{rewritten}"
        );
        assert!(root.join(".mcp/filesystem.toml").is_file());
        assert!(!root.join(".mcp/dingtalk.toml").exists());
        // 模型配置页读的是同一份正文：坏段还在磁盘上，解析必须照样成功。
        let parsed = parse_config_text(&rewritten)
            .expect("model settings still parse while the broken section remains");
        assert_eq!(parsed.providers["main"].model.as_deref(), Some("kept"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_existing_standalone_file_wins_over_the_legacy_copy() {
        let (root, path) = scratch_config(
            "version = 1\n[mcp_servers.filesystem]\ncommand = \"old\"\nstartup_timeout_seconds = 10\nenabled = true\n",
        );
        let directory = root.join(".mcp");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("filesystem.toml"),
            "command = \"new\"\nstartup_timeout_seconds = 10\nenabled = true\n",
        )
        .unwrap();
        let loaded = LoadedConfig::load(Some(&path)).expect("load");
        assert_eq!(
            loaded.file.mcp_servers["filesystem"].command.as_deref(),
            Some("new")
        );
        let standalone = std::fs::read_to_string(directory.join("filesystem.toml")).unwrap();
        assert!(standalone.contains("command = \"new\""), "{standalone}");
        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert!(
            !rewritten.contains("mcp_servers"),
            "the superseded legacy copy is removed: {rewritten}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn provider_settings_ignore_a_broken_mcp_section() {
        let (root, path) = scratch_config(
            "version = 1\ndefault_provider = \"main\"\n[providers.main]\nprovider = \"openai-compatible\"\napi_base = \"https://api.example.com/v1\"\napi_key = \"fixture\"\nmodel = \"kept\"\n\n[mcp_servers.dingtalk]\ntransport = \"sse\"\n",
        );
        let parsed = parse_config_text(&std::fs::read_to_string(&path).unwrap())
            .expect("a bad MCP section does not fail the model configuration");
        assert_eq!(parsed.providers["main"].model.as_deref(), Some("kept"));
        assert!(parsed.mcp_servers.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
