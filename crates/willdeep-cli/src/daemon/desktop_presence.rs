use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use super::DaemonPaths;

fn presence_path(home: &Path) -> PathBuf {
    DaemonPaths::new(home)
        .directory
        .join("desktop-presence.lock")
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// The Web adapter holds this handle while its desktop windows are reachable.
/// The OS drops the shared lock on exit, including when the process is killed.
pub(crate) fn acquire(home: &Path) -> io::Result<File> {
    std::fs::create_dir_all(DaemonPaths::new(home).directory)?;
    let file = options()
        .create(true)
        .truncate(false)
        .open(presence_path(home))?;
    FileExt::lock_shared(&file)?;
    Ok(file)
}

pub(super) fn active(home: &Path) -> io::Result<bool> {
    let file = match options().open(presence_path(home)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(false),
        Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
            Ok(true)
        }
        Err(error) => Err(error),
    }
}
