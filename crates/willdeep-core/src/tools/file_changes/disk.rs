//! Bounded reads and staged per-file installation. Unix uses pinned directory
//! descriptors and *at syscalls, never following a swapped destination symlink.
//! Non-Unix uses canonical-path revalidation and the platform atomic rename/link
//! primitives. Neither backend supplies external-process content CAS.

use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata, Permissions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub(super) struct Snapshot {
    pub(super) bytes: Vec<u8>,
    pub(super) permissions: Permissions,
}

pub(super) struct DirectoryIdentity {
    path: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    created: Option<std::time::SystemTime>,
}

impl DirectoryIdentity {
    pub(super) fn capture(destination: &Path) -> io::Result<Self> {
        let parent = destination
            .parent()
            .ok_or_else(|| io::Error::other("destination needs a parent directory"))?;
        let canonical = parent.canonicalize()?;
        if canonical != parent {
            return Err(io::Error::other(format!(
                "destination parent is no longer canonical: {}",
                parent.display()
            )));
        }
        let metadata = std::fs::metadata(parent)?;
        if !metadata.is_dir() {
            return Err(io::Error::other(format!(
                "destination parent is not a directory: {}",
                parent.display()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                path: canonical,
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                path: canonical,
                created: metadata.created().ok(),
            })
        }
    }

    pub(super) fn check(&self, destination: &Path) -> io::Result<()> {
        let parent = destination
            .parent()
            .ok_or_else(|| io::Error::other("destination needs a parent"))?;
        if parent != self.path || parent.canonicalize()? != self.path {
            return Err(io::Error::other(format!(
                "destination parent real path changed: {}",
                destination.display()
            )));
        }
        self.check_metadata(&std::fs::metadata(parent)?)
    }

    fn check_metadata(&self, metadata: &Metadata) -> io::Result<()> {
        #[cfg(unix)]
        let same = {
            use std::os::unix::fs::MetadataExt;
            metadata.dev() == self.device && metadata.ino() == self.inode
        };
        #[cfg(not(unix))]
        let same = metadata.created().ok() == self.created;
        if !metadata.is_dir() || !same {
            return Err(io::Error::other(format!(
                "destination parent identity changed: {}",
                self.path.display()
            )));
        }
        Ok(())
    }
}

struct Directory {
    path: PathBuf,
    #[cfg(unix)]
    file: File,
}

impl Directory {
    fn open(destination: &Path, identity: &DirectoryIdentity) -> io::Result<Self> {
        identity.check(destination)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&identity.path)?;
            identity.check_metadata(&file.metadata()?)?;
            Ok(Self {
                path: identity.path.clone(),
                file,
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                path: identity.path.clone(),
            })
        }
    }

    #[cfg(unix)]
    fn open_file(&self, name: &OsStr, create: bool) -> io::Result<File> {
        use std::os::fd::{AsRawFd, FromRawFd};
        let name = c_name(name)?;
        let flags = if create {
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL
        } else {
            // NONBLOCK prevents a replaced FIFO from blocking before fstat.
            libc::O_RDONLY | libc::O_NONBLOCK
        } | libc::O_NOFOLLOW
            | libc::O_CLOEXEC;
        // SAFETY: self.file owns a live directory fd, name is NUL-terminated,
        // and the returned fd is transferred exactly once into File.
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                flags,
                // openat is variadic on macOS; mode_t is u16 there and must
                // undergo the C integer promotion before crossing the ABI.
                0o600 as libc::c_uint,
            )
        };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }

    #[cfg(not(unix))]
    fn open_file(&self, name: &OsStr, create: bool) -> io::Result<File> {
        let path = self.path.join(name);
        let mut options = std::fs::OpenOptions::new();
        if create {
            options.write(true).create_new(true);
        } else {
            if !std::fs::symlink_metadata(&path)?.is_file() {
                return Err(io::Error::other(
                    "destination must be a regular file, not a symlink",
                ));
            }
            options.read(true);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Open the reparse point itself rather than following a last-moment
            // swapped leaf; the caller's is_file check rejects that handle.
            options.custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            );
        }
        options.open(path)
    }

    #[cfg(unix)]
    fn rename(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let from = c_name(from)?;
        let to = c_name(to)?;
        // SAFETY: both names are valid C strings and the owned directory fd
        // stays alive through this call. renameat replaces the entry, not its target.
        let result = unsafe {
            libc::renameat(
                self.file.as_raw_fd(),
                from.as_ptr(),
                self.file.as_raw_fd(),
                to.as_ptr(),
            )
        };
        syscall_result(result)
    }

    #[cfg(not(unix))]
    fn rename(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        std::fs::rename(self.path.join(from), self.path.join(to))
    }

    #[cfg(unix)]
    fn hard_link(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let from = c_name(from)?;
        let to = c_name(to)?;
        // SAFETY: names and directory fd remain valid. No AT_SYMLINK_FOLLOW;
        // an existing destination makes linkat fail, so creation cannot clobber.
        let result = unsafe {
            libc::linkat(
                self.file.as_raw_fd(),
                from.as_ptr(),
                self.file.as_raw_fd(),
                to.as_ptr(),
                0,
            )
        };
        syscall_result(result)
    }

    #[cfg(not(unix))]
    fn hard_link(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        std::fs::hard_link(self.path.join(from), self.path.join(to))
    }

    #[cfg(unix)]
    fn unlink(&self, name: &OsStr) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let name = c_name(name)?;
        // SAFETY: name and fd are valid for the duration of unlinkat.
        syscall_result(unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) })
    }

    #[cfg(not(unix))]
    fn unlink(&self, name: &OsStr) -> io::Result<()> {
        std::fs::remove_file(self.path.join(name))
    }
}

#[cfg(unix)]
fn c_name(name: &OsStr) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(name.as_bytes()).map_err(|_| io::Error::other("filename contains NUL"))
}

#[cfg(unix)]
fn syscall_result(result: libc::c_int) -> io::Result<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(super) fn read(path: &Path, identity: &DirectoryIdentity, cap: usize) -> io::Result<Snapshot> {
    let directory = Directory::open(path, identity)?;
    let file = directory.open_file(filename(path)?, false)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other(format!(
            "destination is not a regular file: {}",
            path.display()
        )));
    }
    if metadata.len() > cap as u64 {
        return Err(io::Error::other(format!(
            "file exceeds remaining {cap} byte snapshot budget: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > cap {
        return Err(io::Error::other(format!(
            "file grew beyond snapshot budget while reading: {}",
            path.display()
        )));
    }
    identity.check(path)?;
    Ok(Snapshot {
        bytes,
        permissions: metadata.permissions(),
    })
}

pub(super) fn ensure_absent(path: &Path) -> Result<(), super::ToolError> {
    // symlink_metadata sees dangling symlinks as existing entries too.
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(super::ToolError::FileAlreadyExists(
            path.display().to_string(),
        )),
    }
}

#[derive(Debug)]
pub(super) struct CleanupFailure {
    pub(super) path: PathBuf,
    pub(super) error: io::Error,
}

/// A failed staging attempt can leave a temporary file even though it never
/// installed a destination. Keep those cleanup failures separate from target
/// recovery, so the receipt never invents a target write or hides an orphan.
#[derive(Debug)]
pub(super) struct StageError {
    pub(super) error: io::Error,
    pub(super) cleanup: Option<CleanupFailure>,
}

impl From<io::Error> for StageError {
    fn from(error: io::Error) -> Self {
        Self {
            error,
            cleanup: None,
        }
    }
}

pub(super) struct StagedFile {
    directory: Directory,
    destination: OsString,
    temporary: OsString,
    live: bool,
}

impl StagedFile {
    pub(super) fn new(
        path: &Path,
        identity: &DirectoryIdentity,
        bytes: &[u8],
        permissions: Option<&Permissions>,
    ) -> Result<Self, StageError> {
        let destination = filename(path)?.to_os_string();
        let directory = Directory::open(path, identity)?;
        let temporary = OsString::from(format!(".willdeep-change-{}", uuid::Uuid::new_v4()));
        let mut file = directory.open_file(&temporary, true)?;
        let mut staged = Self {
            directory,
            destination,
            temporary,
            live: true,
        };
        let result = (|| {
            file.write_all(bytes)?;
            file.flush()?;
            if let Some(permissions) = permissions {
                file.set_permissions(permissions.clone())?;
            }
            file.sync_all()?;
            identity.check(path)
        })();
        drop(file);
        if let Err(error) = result {
            let cleanup = staged.cleanup().err().map(|error| CleanupFailure {
                path: staged.temporary_path(),
                error,
            });
            return Err(StageError { error, cleanup });
        }
        Ok(staged)
    }

    pub(super) fn install(&mut self, create: bool) -> io::Result<()> {
        if create {
            self.directory
                .hard_link(&self.temporary, &self.destination)?;
            // The installed link is already complete. Cleanup is reported
            // separately so an unlink error never hides a successful install.
        } else {
            self.directory.rename(&self.temporary, &self.destination)?;
            self.live = false;
        }
        Ok(())
    }

    pub(super) fn temporary_path(&self) -> PathBuf {
        self.directory.path.join(&self.temporary)
    }

    pub(super) fn cleanup(&mut self) -> io::Result<()> {
        if self.live {
            match self.directory.unlink(&self.temporary) {
                Ok(()) => self.live = false,
                Err(error) if error.kind() == io::ErrorKind::NotFound => self.live = false,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        // Explicit cleanup reports failures; Drop is the last-resort unwind
        // guard. Normal commits have no cancellation point while this is live.
        let _ = self.cleanup();
    }
}

pub(super) fn remove(path: &Path, identity: &DirectoryIdentity) -> io::Result<()> {
    Directory::open(path, identity)?.unlink(filename(path)?)
}

fn filename(path: &Path) -> io::Result<&OsStr> {
    path.file_name()
        .ok_or_else(|| io::Error::other("destination needs a filename"))
}
