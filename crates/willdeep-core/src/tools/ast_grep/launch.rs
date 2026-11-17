use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use super::snapshot::Language;
use super::{ToolError, error};

const EMPTY_CONFIG: &[u8] = b"{}\n";
const CONFIG_NAME: &str = "sgconfig.yml";

pub(super) fn executable(root: &Path) -> Result<PathBuf, String> {
    let path = std::env::var_os("PATH")
        .ok_or("dependency_missing: PATH is unset; no ast-grep process started")?;
    executable_from_path(root, &path)
}

pub(super) fn executable_from_path(root: &Path, path: &OsStr) -> Result<PathBuf, String> {
    for directory in std::env::split_paths(path).filter(|p| p.is_absolute()) {
        let Ok(real_directory) = directory.canonicalize() else {
            continue;
        };
        if directory.starts_with(root) || real_directory.starts_with(root) {
            continue;
        }
        #[cfg(windows)]
        let candidate = real_directory.join("ast-grep.exe");
        #[cfg(not(windows))]
        let candidate = real_directory.join("ast-grep");
        let Ok(real) = candidate.canonicalize() else {
            continue;
        };
        if real.starts_with(root) {
            continue;
        }
        let Ok(metadata) = real.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
        }
        // Refuse shell/Node wrappers as well as sg aliases chosen by name.
        // Native executables may still be symlinked by package managers.
        let mut magic = [0u8; 4];
        if std::fs::File::open(&real)
            .and_then(|mut file| file.read_exact(&mut magic))
            .is_err()
        {
            continue;
        }
        let native = matches!(
            magic,
            [0x7f, b'E', b'L', b'F']
                | [0xfe, 0xed, 0xfa, 0xce | 0xcf]
                | [0xce | 0xcf, 0xfa, 0xed, 0xfe]
                | [0xca, 0xfe, 0xba, 0xbe | 0xbf]
                | [0xbe | 0xbf, 0xba, 0xfe, 0xca]
        ) || cfg!(windows) && magic.starts_with(b"MZ");
        if !native {
            continue;
        }
        return Ok(real);
    }
    Err("dependency_missing: native ast-grep not found on absolute PATH entries outside workspace; sg, scripts and workspace binaries are not selected".into())
}

pub(super) struct IsolatedConfig {
    directory: PathBuf,
    path: PathBuf,
}

impl IsolatedConfig {
    pub(super) fn new() -> Result<Self, ToolError> {
        let parent = std::env::temp_dir().canonicalize()?;
        let directory = parent.join(format!("willdeep-ast-grep-{}", uuid::Uuid::new_v4()));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(&directory)?;
        let config = Self {
            path: directory.join(CONFIG_NAME),
            directory,
        };
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&config.path)?;
        file.write_all(EMPTY_CONFIG)?;
        config.verify()?;
        Ok(config)
    }

    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(super) fn verify(&self) -> Result<(), ToolError> {
        let directory = std::fs::symlink_metadata(&self.directory)?;
        let file = std::fs::symlink_metadata(&self.path)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let opened = options.open(&self.path)?;
        if !opened.metadata()?.is_file() {
            return Err(error(
                "configuration_isolation_failed: config is not a regular file",
            ));
        }
        let mut bytes = Vec::new();
        opened
            .take(EMPTY_CONFIG.len() as u64 + 1)
            .read_to_end(&mut bytes)?;
        if !directory.is_dir()
            || directory.file_type().is_symlink()
            || !file.is_file()
            || file.file_type().is_symlink()
            || file.len() != EMPTY_CONFIG.len() as u64
            || bytes != EMPTY_CONFIG
        {
            return Err(error(
                "configuration_isolation_failed: private empty configuration changed; no discovered-config fallback permitted",
            ));
        }
        Ok(())
    }
}

impl Drop for IsolatedConfig {
    fn drop(&mut self) {
        // Only remove the two objects this lease created. Never recursively
        // delete unexpected contents, even if an external program left them.
        for path in [&self.path, &self.directory] {
            let result = if path == &self.directory {
                std::fs::remove_dir(path)
            } else {
                std::fs::remove_file(path)
            };
            if let Err(error) = result
                && error.kind() != std::io::ErrorKind::NotFound
            {
                report_cleanup_failure(error.kind());
            }
        }
    }
}

fn report_cleanup_failure(kind: std::io::ErrorKind) {
    // No paths or child output in diagnostics; configuration contains no secrets.
    eprintln!(
        "{}",
        serde_json::json!({"event":"ast_grep_config_cleanup_failed","error_kind":kind.to_string()})
    );
}

pub(super) fn query_arguments(
    config: &IsolatedConfig,
    language: Language,
    pattern: &str,
) -> Result<Vec<String>, ToolError> {
    let path = config
        .path
        .to_str()
        .ok_or_else(|| error("private config path is not UTF-8"))?;
    // --config is documented to select this single file instead of discovering
    // ancestors. {} has no rules, customLanguages or languageInjections.
    // Unknown/older CLI flags fail; never retry without the explicit config.
    Ok(vec![
        "run".into(),
        "--stdin".into(),
        "--json=compact".into(),
        format!("--config={path}"),
        format!("--lang={}", language.name()),
        format!("--pattern={pattern}"),
    ])
}

pub(super) fn display_command(
    executable: &Path,
    arguments: &[String],
) -> Result<String, ToolError> {
    let program = executable
        .to_str()
        .ok_or_else(|| error("ast-grep executable path must be UTF-8"))?;
    Ok(std::iter::once(program)
        .chain(arguments.iter().map(String::as_str))
        .map(|arg| format!("'{}'", arg.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" "))
}
