//! Fixed server profiles. These reduce automatic execution; they do not turn
//! language-server processes into a read sandbox.
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) const MAX_DOCUMENT_BYTES: usize = 512 * 1024;
const MAX_CONFIG_FILES: usize = 256;
const MAX_SCANNED_ENTRIES: usize = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Language {
    Rust,
    TypeScript,
}

impl Language {
    pub(super) fn from_path(path: &Path) -> Result<Self, String> {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("rs") => Ok(Self::Rust),
            Some("ts" | "tsx") => Ok(Self::TypeScript),
            _ => Err("lsp path must name a Rust (.rs) or TypeScript (.ts/.tsx) file".into()),
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
        }
    }

    pub(super) fn command(self) -> &'static str {
        match self {
            Self::Rust => "rust-analyzer",
            Self::TypeScript => "typescript-language-server --stdio",
        }
    }

    pub(super) fn executable(self) -> &'static str {
        match self {
            Self::Rust => "rust-analyzer",
            Self::TypeScript => "typescript-language-server",
        }
    }

    pub(super) fn installation_hint(self) -> &'static str {
        match self {
            Self::Rust => {
                "Install rust-analyzer yourself (for rustup: rustup component add rust-analyzer), then put it on PATH. No dependency was downloaded."
            }
            Self::TypeScript => {
                "Install typescript-language-server and typescript yourself in a trusted tool installation and put the server on PATH. No dependency was downloaded."
            }
        }
    }

    pub(super) fn initialization_options(self) -> Value {
        match self {
            Self::Rust => json!({
                // Metadata must reload after the first VFS load to finish the
                // crate graph. This does not enable build scripts or checks.
                "cargo": {"buildScripts": {"enable": false}, "autoreload": true},
                "procMacro": {"enable": false},
                "checkOnSave": false,
                "check": {"enable": false},
                "files": {"watcher": "client"},
                "cachePriming": {"enable": false}
            }),
            Self::TypeScript => json!({
                "hostInfo": "willdeep",
                "plugins": [],
                "disableAutomaticTypingAcquisition": true,
                "preferences": {"includePackageJsonAutoImports": "off"},
                // The syntax server may answer definition/references while the
                // semantic project is loading, returning only local aliases.
                // One semantic server makes the first query wait for real symbols.
                "tsserver": {"logVerbosity": "off", "useSyntaxServer": "never"}
            }),
        }
    }

    pub(super) fn configuration(self) -> Value {
        match self {
            Self::Rust => self.initialization_options(),
            Self::TypeScript => json!({
                "typescript": {"disableAutomaticTypeAcquisition": true,
                    "preferences": {"includePackageJsonAutoImports": "off"}},
                "javascript": {"disableAutomaticTypeAcquisition": true,
                    "preferences": {"includePackageJsonAutoImports": "off"}},
                "completions": {"completeFunctionCalls": false}
            }),
        }
    }

    pub(super) fn document_language(self, path: &Path) -> &'static str {
        match (self, path.extension().and_then(|ext| ext.to_str())) {
            (Self::Rust, _) => "rust",
            (_, Some("tsx")) => "typescriptreact",
            _ => "typescript",
        }
    }
}

pub(super) struct Launch {
    pub(super) command: String,
    pub(super) options: Value,
    pub(super) path: String,
}

/// Resolve only the fixed server names, from absolute PATH directories outside
/// the workspace. A workspace-local TypeScript installation is executable code,
/// so do not silently select its tsserver.js or NODE_PATH plugins.
pub(super) fn resolve_launch(language: Language, root: &Path) -> Result<Launch, String> {
    let paths = std::env::var_os("PATH").ok_or_else(|| language.installation_hint().to_owned())?;
    resolve_launch_from_path(language, root, &paths)
}

fn resolve_launch_from_path(
    language: Language,
    root: &Path,
    paths: &std::ffi::OsStr,
) -> Result<Launch, String> {
    let mut directories = Vec::new();
    for directory in std::env::split_paths(paths).filter(|p| p.is_absolute()) {
        if let Ok(real) = directory.canonicalize()
            && real.is_dir()
            && !directory.starts_with(root)
            && !real.starts_with(root)
            && !directories.contains(&real)
        {
            directories.push(real);
        }
    }
    let trusted_path = std::env::join_paths(&directories)
        .map_err(|e| format!("construct trusted LSP PATH: {e}"))?
        .into_string()
        .map_err(|_| "trusted LSP PATH is not UTF-8")?;
    let mut candidates = Vec::new();
    for directory in &directories {
        candidates.push(directory.join(language.executable()));
        #[cfg(windows)]
        for suffix in [".cmd", ".exe", ".bat"] {
            candidates.push(directory.join(format!("{}{suffix}", language.executable())));
        }
    }
    let executable = candidates
        .into_iter()
        .find(|candidate| {
            candidate.is_file()
                && candidate
                    .canonicalize()
                    .is_ok_and(|real| !real.starts_with(root))
                && is_executable(candidate)
        })
        .ok_or_else(|| {
            format!(
                "{} is missing from trusted PATH directories outside the workspace. {}",
                language.executable(),
                language.installation_hint()
            )
        })?;
    let mut options = language.initialization_options();
    if language == Language::TypeScript {
        let real = executable
            .canonicalize()
            .map_err(|e| format!("resolve TypeScript server installation: {e}"))?;
        let mut tsserver = None;
        for directory in real.ancestors().skip(1).take(6) {
            for candidate in [
                directory.join("typescript/lib/tsserver.js"),
                directory.join("node_modules/typescript/lib/tsserver.js"),
            ] {
                if let Ok(real) = candidate.canonicalize()
                    && real.is_file()
                    && !real.starts_with(root)
                {
                    tsserver = Some(real);
                    break;
                }
            }
            if tsserver.is_some() {
                break;
            }
        }
        let tsserver = tsserver.ok_or("TypeScript runtime was not found beside the trusted language-server installation. Install typescript alongside typescript-language-server outside the workspace; workspace tsserver code is not executed.")?;
        options["tsserver"]["path"] = json!(tsserver.to_string_lossy());
        options["tsserver"]["fallbackPath"] = json!(tsserver.to_string_lossy());
    }
    #[cfg(windows)]
    let command = format!(
        "& '{}'{}",
        executable.to_string_lossy().replace('\'', "''"),
        if language == Language::TypeScript {
            " --stdio"
        } else {
            ""
        }
    );
    #[cfg(not(windows))]
    let command = format!(
        "'{}'{}",
        executable.to_string_lossy().replace('\'', "'\\''"),
        if language == Language::TypeScript {
            " --stdio"
        } else {
            ""
        }
    );
    Ok(Launch {
        command,
        options,
        path: trusted_path,
    })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

pub(super) fn uri(path: &Path) -> Result<String, String> {
    reqwest::Url::from_file_path(path)
        .map(String::from)
        .map_err(|_| "cannot encode workspace file URI".into())
}

pub(super) fn workspace_path(root: &Path, raw_uri: &str) -> Option<PathBuf> {
    let url = reqwest::Url::parse(raw_uri).ok()?;
    if url.scheme() != "file" || url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let path = url.to_file_path().ok()?.canonicalize().ok()?;
    if !path.starts_with(root) || !path.is_file() || Language::from_path(&path).is_err() {
        return None;
    }
    Some(path)
}

pub(super) fn snapshot(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(super) async fn read_document(path: &Path) -> Result<String, String> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("open source file: {e}"))?;
    if !file
        .metadata()
        .await
        .map_err(|e| format!("source metadata: {e}"))?
        .is_file()
    {
        return Err("lsp requires a regular source file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| format!("read source file: {e}"))?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("lsp source file exceeds 512 KiB limit".into());
    }
    String::from_utf8(bytes).map_err(|_| "lsp source file is not UTF-8".into())
}

pub(super) async fn configuration_id(language: Language, root: &Path) -> Result<String, String> {
    match language {
        Language::Rust => Ok(String::new()),
        Language::TypeScript => validate_typescript_workspace(root).await,
    }
}

/// Inspect all nested directories, including build/dist/node_modules: tsserver
/// does not treat the caller's ignore list as a configuration boundary. Parent
/// discovery may climb above root, even with a root workspaceFolder, so inspect
/// every ancestor's discoverable config too. References/inheritance and source
/// symlinks are refused rather than following an unbounded external graph.
pub(super) async fn validate_typescript_workspace(root: &Path) -> Result<String, String> {
    let mut directories = vec![root.to_path_buf()];
    let mut configs = Vec::new();
    let mut entries = 0;
    while let Some(directory) = directories.pop() {
        let mut reader = tokio::fs::read_dir(&directory)
            .await
            .map_err(|e| format!("inspect TypeScript workspace configuration: {e}"))?;
        while let Some(entry) = reader
            .next_entry()
            .await
            .map_err(|e| format!("inspect config entry: {e}"))?
        {
            entries += 1;
            if entries > MAX_SCANNED_ENTRIES {
                return Err(
                    "TypeScript config inspection exceeded 30000 entries; use a smaller workspace"
                        .into(),
                );
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // Git internals cannot be queried as source files (checked below).
            if name == ".git" {
                continue;
            }
            let kind = entry
                .file_type()
                .await
                .map_err(|e| format!("inspect config entry type: {e}"))?;
            let is_config = config_name(&name);
            if kind.is_symlink() {
                return Err(format!(
                    "TypeScript config inspection refuses symlinks at {}; linked sources/directories can discover uninspected parent configs",
                    entry
                        .path()
                        .strip_prefix(root)
                        .unwrap_or(&entry.path())
                        .display()
                ));
            }
            if kind.is_dir() {
                directories.push(entry.path());
            } else if is_config {
                if !kind.is_file() {
                    return Err("TypeScript configuration is not a regular file".into());
                }
                configs.push(entry.path());
                if configs.len() > MAX_CONFIG_FILES {
                    return Err("too many TypeScript configuration files".into());
                }
            }
        }
    }
    for parent in root.ancestors().skip(1) {
        for name in ["tsconfig.json", "jsconfig.json"] {
            let path = parent.join(name);
            match tokio::fs::symlink_metadata(&path).await {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    configs.push(path)
                }
                Ok(_) => {
                    return Err(
                        "parent TypeScript configuration must be a regular non-symlink file".into(),
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("inspect parent TypeScript configuration: {error}"));
                }
            }
        }
    }
    if configs.len() > MAX_CONFIG_FILES {
        return Err("too many TypeScript configuration files including parents".into());
    }
    configs.sort();
    let mut digest = Sha256::new();
    for path in configs {
        let text = read_document(&path).await?;
        let config: Value = serde_json::from_str(&strip_jsonc(&text)?).map_err(|e| {
            format!(
                "cannot validate TypeScript config {}: {e}",
                path.strip_prefix(root).unwrap_or(&path).display()
            )
        })?;
        validate_ts_config(&config).map_err(|e| {
            format!(
                "TypeScript config {}: {e}",
                path.strip_prefix(root).unwrap_or(&path).display()
            )
        })?;
        digest.update(path.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(text.as_bytes());
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn config_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    (lower.starts_with("tsconfig") || lower.starts_with("jsconfig")) && lower.ends_with(".json")
}

fn validate_ts_config(config: &Value) -> Result<(), String> {
    if !config.is_object() {
        return Err("TypeScript configuration must be an object".into());
    }
    if config.get("extends").is_some_and(|v| !v.is_null()) {
        return Err("TypeScript config inheritance (extends) is disabled in the query-only profile; inherited configs can load plugins. Use a standalone workspace config.".into());
    }
    if config
        .get("references")
        .is_some_and(|p| p.as_array().is_none_or(|v| !v.is_empty()))
    {
        return Err("TypeScript project references are disabled in the query-only profile; referenced configurations can load plugins outside the inspected workspace".into());
    }
    if config
        .pointer("/compilerOptions/plugins")
        .is_some_and(|p| p.as_array().is_none_or(|v| !v.is_empty()))
    {
        return Err(
            "TypeScript compilerOptions.plugins is disabled in the query-only LSP profile".into(),
        );
    }
    Ok(())
}

/// JSONC comments and trailing commas, preserving strings and line structure.
fn strip_jsonc(text: &str) -> Result<String, String> {
    let mut bytes = text.as_bytes().to_vec();
    let mut index = 0;
    let mut quoted = false;
    while index < bytes.len() {
        if quoted {
            if bytes[index] == b'\\' {
                index += 2;
                continue;
            }
            if bytes[index] == b'"' {
                quoted = false;
            }
            index += 1;
        } else if bytes[index] == b'"' {
            quoted = true;
            index += 1;
        } else if bytes.get(index..index + 2) == Some(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                bytes[index] = b' ';
                index += 1;
            }
        } else if bytes.get(index..index + 2) == Some(b"/*") {
            bytes[index..index + 2].fill(b' ');
            index += 2;
            loop {
                if index + 1 >= bytes.len() {
                    return Err("unterminated TypeScript config comment".into());
                }
                if bytes[index..index + 2] == *b"*/" {
                    bytes[index..index + 2].fill(b' ');
                    index += 2;
                    break;
                }
                if bytes[index] != b'\n' {
                    bytes[index] = b' ';
                }
                index += 1;
            }
        } else {
            index += 1;
        }
    }
    quoted = false;
    index = 0;
    while index < bytes.len() {
        if quoted {
            if bytes[index] == b'\\' {
                index += 2;
                continue;
            }
            if bytes[index] == b'"' {
                quoted = false;
            }
        } else if bytes[index] == b'"' {
            quoted = true;
        } else if bytes[index] == b',' {
            let mut next = index + 1;
            while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
                next += 1;
            }
            if matches!(bytes.get(next), Some(b'}' | b']')) {
                bytes[index] = b' ';
            }
        }
        index += 1;
    }
    String::from_utf8(bytes).map_err(|_| "invalid TypeScript config encoding".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jsonc_keeps_strings_and_rejects_hidden_plugin_settings() {
        let parsed: Value = serde_json::from_str(
            &strip_jsonc(
                r#"{
            // test
            "compilerOptions": {"plugins": [{"name":"http://plugin"}],},
        }"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(validate_ts_config(&parsed).is_err());
        let safe: Value = serde_json::from_str(
            &strip_jsonc(r#"{"compilerOptions": {"strict":true,}, /* ok */}"#).unwrap(),
        )
        .unwrap();
        assert!(validate_ts_config(&safe).is_ok());
        assert!(validate_ts_config(&json!({"extends":"../outside.json"})).is_err());
    }
    #[tokio::test]
    async fn parent_and_nested_configs_cannot_hide_plugins_or_references() {
        use crate::tools::lsp::tests::Fixture;
        let fixture = Fixture::new();
        fixture.write("workspace/a.ts", "export const a = 1;\n");
        let root = fixture.root.join("workspace");
        fixture.write(
            "tsconfig.json",
            r#"{"compilerOptions":{"plugins":[{"name":"outside-plugin"}]}}"#,
        );
        assert!(
            validate_typescript_workspace(&root)
                .await
                .unwrap_err()
                .contains("plugins")
        );
        fixture.write("tsconfig.json", "{}");
        let before = validate_typescript_workspace(&root).await.unwrap();
        fixture.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
        assert_ne!(before, validate_typescript_workspace(&root).await.unwrap());
        for directory in [
            "build",
            "dist",
            "target",
            ".willdeep",
            "node_modules/package",
        ] {
            let name = format!("workspace/{directory}/tsconfig.json");
            fixture.write(&name, r#"{"references":[{"path":"/uninspected"}]}"#);
            assert!(
                validate_typescript_workspace(&root)
                    .await
                    .unwrap_err()
                    .contains("references")
            );
            fixture.write(&name, "{}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn linked_directories_cannot_skip_config_inspection() {
        use crate::tools::lsp::tests::Fixture;
        let fixture = Fixture::new();
        fixture.write("workspace/a.ts", "export {};\n");
        fixture.write("external/tsconfig.json", r#"{"extends":"other"}"#);
        std::os::unix::fs::symlink(
            fixture.root.join("external"),
            fixture.root.join("workspace/linked"),
        )
        .unwrap();
        assert!(
            validate_typescript_workspace(&fixture.root.join("workspace"))
                .await
                .unwrap_err()
                .contains("symlinks")
        );
    }

    #[cfg(unix)]
    #[test]
    fn launch_rejects_workspace_servers_and_requires_external_typescript_runtime() {
        use crate::tools::lsp::tests::Fixture;
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let root = fixture.root.join("workspace");
        let local = fixture.write(
            "workspace/bin/typescript-language-server",
            "#!/usr/bin/env node\n",
        );
        let trusted = fixture.write(
            "installed/bin/typescript-language-server",
            "#!/usr/bin/env node\n",
        );
        for executable in [&local, &trusted] {
            std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        fixture.write(
            "workspace/node_modules/typescript/lib/tsserver.js",
            "// workspace code\n",
        );
        let local_path = std::env::join_paths([root.join("bin")]).unwrap();
        assert!(resolve_launch_from_path(Language::TypeScript, &root, &local_path).is_err());
        let paths =
            std::env::join_paths([root.join("bin"), fixture.root.join("installed/bin")]).unwrap();
        assert!(resolve_launch_from_path(Language::TypeScript, &root, &paths).is_err());
        let runtime = fixture.write(
            "installed/node_modules/typescript/lib/tsserver.js",
            "// trusted fixture\n",
        );
        let launch = resolve_launch_from_path(Language::TypeScript, &root, &paths).unwrap();
        assert!(
            launch
                .command
                .contains("installed/bin/typescript-language-server")
        );
        assert_eq!(
            launch.options["tsserver"]["path"],
            runtime.to_string_lossy().as_ref()
        );
        assert!(!launch.path.contains("workspace"));
        assert_eq!(launch.options["disableAutomaticTypingAcquisition"], true);
        assert_eq!(launch.options["plugins"], json!([]));
        assert_eq!(launch.options["tsserver"]["useSyntaxServer"], "never");
    }

    #[test]
    fn rust_profile_disables_automatic_execution() {
        let options = Language::Rust.initialization_options();
        // Initial VFS discovery needs a metadata reload before quiescence.
        assert_eq!(options.pointer("/cargo/autoreload"), Some(&json!(true)));
        for pointer in [
            "/cargo/buildScripts/enable",
            "/procMacro/enable",
            "/check/enable",
            "/checkOnSave",
        ] {
            assert_eq!(options.pointer(pointer), Some(&json!(false)));
        }
    }
}
