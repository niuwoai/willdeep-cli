use super::super::{WorkflowError, WorkflowResult};
use serde::{Serialize, de::DeserializeOwned};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

fn failure(path: &Path, action: &str, error: impl std::fmt::Display) -> WorkflowError {
    WorkflowError::Storage(format!("{action} {}: {error}", path.display()))
}

fn directory(path: &Path) -> WorkflowResult<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(failure(path, "create directory", error)),
    }
    let metadata = fs::symlink_metadata(path).map_err(|e| failure(path, "inspect directory", e))?;
    if !metadata.is_dir() {
        return Err(failure(
            path,
            "inspect directory",
            "expected a real directory, not a symlink",
        ));
    }
    Ok(())
}

pub(super) fn lock(root: &Path) -> WorkflowResult<File> {
    let workflows = root
        .parent()
        .ok_or_else(|| failure(root, "lock", "missing parent"))?;
    // home is supplied by the host; generated path components must not redirect
    // state into another parent's directory.
    let home = workflows
        .parent()
        .ok_or_else(|| failure(root, "lock", "missing home"))?;
    fs::create_dir_all(home).map_err(|e| failure(home, "create home", e))?;
    directory(workflows)?;
    directory(root)?;
    let path = root.join(".lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(&path)
        .map_err(|e| failure(&path, "open lock", e))?;
    if !file
        .metadata()
        .map_err(|e| failure(&path, "inspect lock", e))?
        .is_file()
    {
        return Err(failure(&path, "lock", "not a regular file"));
    }
    file.lock().map_err(|e| failure(&path, "lock", e))?;
    Ok(file)
}

pub(super) fn read<T: DeserializeOwned>(path: &Path, max: usize) -> WorkflowResult<Option<T>> {
    if let Some(parent) = path.parent() {
        match fs::symlink_metadata(parent) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(failure(parent, "read", "expected a real directory")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(failure(parent, "inspect", error)),
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failure(path, "open", error)),
    };
    let metadata = file.metadata().map_err(|e| failure(path, "inspect", e))?;
    if !metadata.is_file() {
        return Err(failure(path, "read", "not a regular file"));
    }
    if metadata.len() > max as u64 {
        return Err(WorkflowError::Limit(format!(
            "{} exceeds {max} bytes",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| failure(path, "read", e))?;
    if bytes.len() > max {
        return Err(WorkflowError::Limit(format!(
            "{} exceeds {max} bytes",
            path.display()
        )));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| failure(path, "decode", e))
}

pub(super) fn write(path: &Path, value: &impl Serialize, max: usize) -> WorkflowResult<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| failure(path, "encode", e))?;
    if bytes.len() > max {
        return Err(WorkflowError::Limit(format!(
            "{} exceeds {max} bytes",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| failure(path, "write", "missing parent"))?;
    directory(parent)?;
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|e| failure(&temporary, "create temporary", e))?;
        file.write_all(&bytes)
            .map_err(|e| failure(&temporary, "write", e))?;
        file.sync_all()
            .map_err(|e| failure(&temporary, "sync", e))?;
        fs::rename(&temporary, path).map_err(|e| failure(path, "commit", e))?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|file| file.sync_all())
            .map_err(|e| failure(parent, "sync directory", e))?;
        Ok(())
    })();
    if result.is_err()
        && let Err(error) = fs::remove_file(&temporary)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(failure(
            &temporary,
            "clean temporary after failed commit",
            error,
        ));
    }
    result
}

pub(super) fn json_files(path: &Path) -> WorkflowResult<Vec<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(failure(path, "list", "expected a real directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(failure(path, "inspect", error)),
    }
    fs::read_dir(path)
        .map_err(|e| failure(path, "list", e))?
        .map(|entry| {
            entry
                .map(|e| e.path())
                .map_err(|e| failure(path, "list entry", e))
        })
        .filter(|entry| match entry {
            Ok(p) => p.extension().is_some_and(|s| s == "json"),
            Err(_) => true,
        })
        .collect()
}
