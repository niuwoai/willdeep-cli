use std::fs::Metadata;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{ToolError, error};

const MAX_ENTRIES: usize = 20_000;
const MAX_DEPTH: usize = 64;
const MAX_FILES: usize = 128;
const MAX_FILE_BYTES: usize = 512 * 1024;
const MAX_TOTAL_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub(super) enum Language {
    Rust,
    #[serde(rename = "typescript")]
    TypeScript,
    Tsx,
}

impl Language {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
        }
    }
    fn accepts(self, path: &Path) -> bool {
        path.extension().and_then(|e| e.to_str())
            == Some(match self {
                Self::Rust => "rs",
                Self::TypeScript => "ts",
                Self::Tsx => "tsx",
            })
    }
}

pub(super) fn limits() -> Value {
    json!({"entries":MAX_ENTRIES,"depth":MAX_DEPTH,"files":MAX_FILES,"file_bytes":MAX_FILE_BYTES,"total_source_bytes":MAX_TOTAL_BYTES})
}

#[derive(Debug)]
pub(super) struct Snapshot {
    pub(super) path: PathBuf,
    pub(super) relative: String,
    pub(super) text: String,
    pub(super) hash: String,
    metadata: Metadata,
}

impl Snapshot {
    pub(super) fn read(root: &Path, path: &Path) -> Result<Self, ToolError> {
        safe_path(root, path)?;
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // A raced FIFO must not block, and a raced final symlink must not
            // dereference an external file before the post-open checks.
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
            return Err(error("source must be an ordinary file of at most 512 KiB"));
        }
        if !same_metadata(&metadata, &std::fs::symlink_metadata(path)?) {
            return Err(error(
                "snapshot_changed: source identity changed while opening",
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(error(
                "source byte limit exceeded; no partial snapshot accepted",
            ));
        }
        safe_path(root, path)?;
        if !same_metadata(&metadata, &std::fs::symlink_metadata(path)?) {
            return Err(error("snapshot_changed: source changed while reading"));
        }
        let text = String::from_utf8(bytes).map_err(|_| error("source is not UTF-8"))?;
        let relative = path
            .strip_prefix(root)
            .map_err(error)?
            .to_str()
            .ok_or_else(|| error("source path is not UTF-8"))?;
        #[cfg(unix)]
        if relative.contains('\\') {
            return Err(error("source path contains an ambiguous backslash"));
        }
        let relative = relative.replace('\\', "/");
        let hash = format!("sha256:{:x}", Sha256::digest(text.as_bytes()));
        Ok(Self {
            path: path.to_path_buf(),
            relative,
            text,
            hash,
            metadata,
        })
    }

    pub(super) fn same(&self, other: &Self) -> bool {
        self.path == other.path
            && self.hash == other.hash
            && same_metadata(&self.metadata, &other.metadata)
    }

    pub(super) fn verify(&self, root: &Path) -> Result<(), ToolError> {
        if !self.same(&Self::read(root, &self.path)?) {
            return Err(error(format!(
                "snapshot_changed: {} changed; query result discarded",
                self.relative
            )));
        }
        Ok(())
    }
}

fn same_metadata(left: &Metadata, right: &Metadata) -> bool {
    if !left.is_file()
        || !right.is_file()
        || left.len() != right.len()
        || left.modified().ok() != right.modified().ok()
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if left.dev() != right.dev()
            || left.ino() != right.ino()
            || left.ctime() != right.ctime()
            || left.ctime_nsec() != right.ctime_nsec()
        {
            return false;
        }
    }
    true
}

fn excluded(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return true;
    };
    name.starts_with('.')
        || matches!(
            name,
            "node_modules" | "target" | "dist" | "build" | "vendor"
        )
}

fn safe_path(root: &Path, path: &Path) -> Result<(), ToolError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| error("source path escapes workspace"))?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        let Component::Normal(name) = part else {
            return Err(error("source path is not normalized"));
        };
        if excluded(name) {
            return Err(error("hidden, build or dependency paths cannot be queried"));
        }
        current.push(name);
        if std::fs::symlink_metadata(&current)?
            .file_type()
            .is_symlink()
        {
            return Err(error("symlink paths cannot be queried"));
        }
    }
    if !path.canonicalize()?.starts_with(root) {
        return Err(error("source path escapes workspace"));
    }
    Ok(())
}

pub(super) fn collect(
    root: &Path,
    requested: &str,
    language: Language,
) -> Result<Vec<Snapshot>, ToolError> {
    if requested.is_empty() || requested.contains('\0') || requested.contains('\\') {
        return Err(error(
            "path must be a nonempty workspace-relative path using '/' separators",
        ));
    }
    let requested = Path::new(requested);
    let mut target = root.to_path_buf();
    for part in requested.components() {
        match part {
            Component::Normal(name) if !excluded(name) => target.push(name),
            Component::CurDir => {}
            _ => {
                return Err(error(
                    "absolute, parent, hidden, build or dependency paths are not accepted",
                ));
            }
        }
    }
    safe_path(root, &target)?;
    let metadata = std::fs::symlink_metadata(&target)?;
    if metadata.is_file() && !language.accepts(&target) {
        return Err(error(
            "source extension does not match the selected language",
        ));
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(error("query path must name a regular file or directory"));
    }
    let mut pending = vec![(target, 0usize)];
    let mut entries = 0usize;
    let mut total_bytes = 0usize;
    let mut snapshots = Vec::new();
    while let Some((path, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            return Err(error("source traversal depth budget exceeded"));
        }
        safe_path(root, &path)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            // Bound while enumerating, before collecting/sorting directory entries.
            let mut children = Vec::new();
            for child in std::fs::read_dir(&path)? {
                let child = child?;
                entries += 1;
                if entries > MAX_ENTRIES {
                    return Err(error(
                        "source traversal entry budget exceeded; no partial search accepted",
                    ));
                }
                if excluded(&child.file_name()) {
                    continue;
                }
                let kind = child.file_type()?;
                if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
                    continue;
                }
                children.push(child.path());
            }
            children.sort();
            pending.extend(children.into_iter().rev().map(|p| (p, depth + 1)));
        } else if metadata.is_file() && language.accepts(&path) {
            if snapshots.len() >= MAX_FILES {
                return Err(error("source file budget exceeded; narrow the query path"));
            }
            let snapshot = Snapshot::read(root, &path)?;
            total_bytes += snapshot.text.len();
            if total_bytes > MAX_TOTAL_BYTES {
                return Err(error("source byte budget exceeded; narrow the query path"));
            }
            snapshots.push(snapshot);
        } else if !metadata.is_file() {
            return Err(error(
                "source changed into a non-regular entry during traversal",
            ));
        }
    }
    snapshots.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(snapshots)
}
