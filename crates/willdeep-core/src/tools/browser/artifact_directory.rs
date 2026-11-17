//! Pin the output directory; pathname checks alone cannot prevent parent swaps.
use std::fs::File;
use std::path::{Path, PathBuf};

pub(super) struct Directory {
    pub(super) path: PathBuf,
    #[cfg(unix)]
    handle: File,
}

impl Directory {
    pub(super) fn new(root: &Path) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|e| format!("resolve browser artifact root: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let parent = open_directory(&root)
                .map_err(|e| format!("open browser artifact root without symlinks: {e}"))?;
            let name = format!("browser-{}", uuid::Uuid::new_v4());
            let name_c = component(&name)?;
            // SAFETY: The pinned parent and NUL-terminated component remain live.
            if unsafe { libc::mkdirat(parent.as_raw_fd(), name_c.as_ptr(), 0o700) } != 0 {
                return Err(format!(
                    "create private browser artifact directory: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let handle = open_child_directory(&parent, &name_c)
                .map_err(|e| format!("pin browser artifact directory: {e}"))?;
            Ok(Self {
                path: root.join(name),
                handle,
            })
        }
        #[cfg(not(unix))]
        {
            // No pathname-only write fallback: directory-relative creation must
            // be implemented before screenshot persistence is enabled here.
            Ok(Self {
                path: super::private_directory(&root, "browser")?,
            })
        }
    }

    pub(super) fn validate_path(&self) -> Result<(), String> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let current = open_directory(&self.path)
                .and_then(|file| file.metadata())
                .map_err(|e| format!("browser artifact directory changed: {e}"))?;
            let pinned = self
                .handle
                .metadata()
                .map_err(|e| format!("inspect pinned browser artifact directory: {e}"))?;
            if current.dev() != pinned.dev() || current.ino() != pinned.ino() {
                return Err(
                    "browser artifact directory identity changed; screenshot discarded".into(),
                );
            }
            Ok(())
        }
        #[cfg(not(unix))]
        Err("safe directory-relative screenshot persistence is unavailable on this platform".into())
    }

    pub(super) fn create_file(&self, name: &str) -> Result<File, String> {
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            let name = component(name)?;
            // SAFETY: A single component is resolved relative to the held fd;
            // no pathname ancestor is traversed, including after a directory swap.
            let fd = unsafe {
                libc::openat(
                    self.handle.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd < 0 {
                return Err(format!(
                    "create pinned browser screenshot: {}",
                    std::io::Error::last_os_error()
                ));
            }
            // SAFETY: openat returned a new uniquely owned descriptor.
            Ok(unsafe { File::from_raw_fd(fd) })
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            Err(
                "safe directory-relative screenshot persistence is unavailable on this platform"
                    .into(),
            )
        }
    }

    pub(super) fn unlink_file(&self, name: &str) -> Result<(), String> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let name = component(name)?;
            // SAFETY: The component cannot escape the pinned directory.
            if unsafe { libc::unlinkat(self.handle.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(format!("remove uncommitted browser screenshot: {error}"));
                }
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            Ok(()) // create_file refuses writes on this platform.
        }
    }
}

#[cfg(unix)]
fn component(name: &str) -> Result<std::ffi::CString, String> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err("invalid browser artifact filename component".into());
    }
    std::ffi::CString::new(name).map_err(|_| "browser artifact component contains NUL".into())
}

#[cfg(unix)]
fn open_child_directory(parent: &File, name: &std::ffi::CStr) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    // SAFETY: Both arguments remain live; O_NOFOLLOW applies to each component.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openat returned a new uniquely owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn open_directory(path: &Path) -> std::io::Result<File> {
    use std::os::unix::ffi::OsStrExt;
    if !path.is_absolute() {
        return Err(std::io::Error::other("artifact directory is not absolute"));
    }
    let mut directory = File::open("/")?;
    for part in path.components() {
        match part {
            std::path::Component::RootDir | std::path::Component::CurDir => {}
            std::path::Component::Normal(name) => {
                let name =
                    std::ffi::CString::new(name.as_bytes()).map_err(std::io::Error::other)?;
                directory = open_child_directory(&directory, &name)?;
            }
            _ => {
                return Err(std::io::Error::other(
                    "invalid artifact directory component",
                ));
            }
        }
    }
    Ok(directory)
}
