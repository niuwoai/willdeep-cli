use std::path::{Path, PathBuf};

#[cfg(unix)]
use anyhow::bail;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum LocalTransportState {
    UnixSocket { path: PathBuf },
    WindowsNamedPipe { name: String },
}

#[cfg(unix)]
pub(super) type LocalListener = tokio::net::UnixListener;

#[cfg(windows)]
pub(super) type LocalListener = WindowsNamedPipeListener;

/// Kept alive for exactly the listener's Runtime lifetime, including startup
/// failures after bind. A fallback directory is private and created by us.
pub(super) struct EndpointCleanup {
    path: PathBuf,
    #[cfg(unix)]
    directory: Option<PathBuf>,
    #[cfg(unix)]
    identity: Option<(u64, u64)>,
}

impl EndpointCleanup {
    fn owns_path(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{FileTypeExt, MetadataExt};
            self.identity.is_some_and(|identity| {
                std::fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
                    metadata.file_type().is_socket() && (metadata.dev(), metadata.ino()) == identity
                })
            })
        }
        #[cfg(windows)]
        {
            false
        }
    }
}

impl Drop for EndpointCleanup {
    fn drop(&mut self) {
        if self.owns_path() {
            remove_if_owned(&self.path);
        }
        #[cfg(unix)]
        if let Some(directory) = &self.directory
            && let Err(error) = std::fs::remove_dir(directory)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("remove temporary Runtime socket directory: {error}");
        }
    }
}

#[cfg(unix)]
fn fits_unix_socket(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    // Reserve one byte for the terminating NUL, using this platform's ABI.
    const CAPACITY: usize = std::mem::size_of::<libc::sockaddr_un>()
        - std::mem::offset_of!(libc::sockaddr_un, sun_path);
    path.as_os_str().as_bytes().len() < CAPACITY
}

#[cfg(unix)]
fn endpoint_cleanup(preferred: &Path) -> Result<EndpointCleanup> {
    use std::os::unix::fs::DirBuilderExt;
    if fits_unix_socket(preferred) {
        return Ok(EndpointCleanup {
            path: preferred.to_path_buf(),
            directory: None,
            identity: None,
        });
    }
    // Discovery records the actual endpoint, so clients need no guessed path.
    // Never shorten into a shared predictable socket filename or relax modes.
    for base in [std::env::temp_dir(), PathBuf::from("/tmp")] {
        let directory = base.join(format!("wd-{}", uuid::Uuid::new_v4().simple()));
        let path = directory.join("s");
        if !fits_unix_socket(&path) {
            continue;
        }
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .context("create private short Runtime socket directory")?;
        return Ok(EndpointCleanup {
            path,
            directory: Some(directory),
            identity: None,
        });
    }
    bail!("no temporary directory supports the Runtime Unix Socket path length")
}

#[cfg(unix)]
pub(super) fn bind(
    path: &Path,
    _token: &str,
) -> Result<(LocalListener, LocalTransportState, EndpointCleanup)> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    let mut cleanup = endpoint_cleanup(path)?;
    let path = &cleanup.path;
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if !metadata.file_type().is_socket() {
            bail!(
                "refusing to replace non-socket Runtime endpoint: {}",
                path.display()
            );
        }
        std::fs::remove_file(path)?;
    }
    let listener = tokio::net::UnixListener::bind(path).context("bind Runtime Unix Socket")?;
    let metadata = std::fs::symlink_metadata(path)?;
    cleanup.identity = Some((metadata.dev(), metadata.ino()));
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok((
        listener,
        LocalTransportState::UnixSocket {
            path: path.to_path_buf(),
        },
        cleanup,
    ))
}

#[cfg(windows)]
pub(super) fn bind(
    path: &Path,
    token: &str,
) -> Result<(LocalListener, LocalTransportState, EndpointCleanup)> {
    let name = format!(r"\\.\pipe\willdeep-{token}");
    let listener =
        WindowsNamedPipeListener::bind(&name).context("bind Runtime Windows Named Pipe")?;
    Ok((
        listener,
        LocalTransportState::WindowsNamedPipe { name },
        EndpointCleanup {
            path: path.to_path_buf(),
        },
    ))
}

#[cfg(unix)]
pub(super) fn remove_if_owned(path: &Path) {
    use std::os::unix::fs::FileTypeExt;

    if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket()) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(windows)]
pub(super) fn remove_if_owned(_path: &Path) {}

/// 同时挂着等连接的管道实例数。
///
/// Named Pipe 的每个实例只服务一个客户端。客户端连上时如果没有空闲实例，拿到的是
/// `ERROR_PIPE_BUSY`，reqwest 的 Named Pipe 连接器不重试，直接报「error sending
/// request」。以前只挂一个实例、等它被连上后才建下一个，中间那段空窗里的并发连接
/// （事件流 + 状态查询 + 正在跑的 turn）就会随机失败。始终预留几个等待中的实例，
/// 一个被占用时其余的还能接住，同时补建一个。
#[cfg(windows)]
const PIPE_INSTANCE_BACKLOG: usize = 4;

#[cfg(windows)]
type PendingPipe = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = (
                    tokio::net::windows::named_pipe::NamedPipeServer,
                    std::io::Result<()>,
                ),
            > + Send,
    >,
>;

#[cfg(windows)]
pub(super) struct WindowsNamedPipeListener {
    name: std::ffi::OsString,
    pending: futures_util::stream::FuturesUnordered<PendingPipe>,
}

#[cfg(windows)]
impl WindowsNamedPipeListener {
    fn bind(name: impl Into<std::ffi::OsString>) -> std::io::Result<Self> {
        use tokio::net::windows::named_pipe::ServerOptions;

        let name = name.into();
        // 第一个实例带 first_pipe_instance：同名管道已被别人占着时直接失败，不和它共用。
        let first = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&name)?;
        let mut listener = Self {
            name,
            pending: futures_util::stream::FuturesUnordered::new(),
        };
        listener.pending.push(Self::wait_for_client(first));
        listener.refill()?;
        Ok(listener)
    }

    fn wait_for_client(server: tokio::net::windows::named_pipe::NamedPipeServer) -> PendingPipe {
        Box::pin(async move {
            let connected = server.connect().await;
            (server, connected)
        })
    }

    /// 把等待中的实例补回 [`PIPE_INSTANCE_BACKLOG`] 个。
    fn refill(&mut self) -> std::io::Result<()> {
        use tokio::net::windows::named_pipe::ServerOptions;

        while self.pending.len() < PIPE_INSTANCE_BACKLOG {
            let server = ServerOptions::new()
                .reject_remote_clients(true)
                .create(&self.name)?;
            self.pending.push(Self::wait_for_client(server));
        }
        Ok(())
    }
}

#[cfg(windows)]
impl axum::serve::Listener for WindowsNamedPipeListener {
    type Io = tokio::net::windows::named_pipe::NamedPipeServer;
    type Addr = String;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        use futures_util::StreamExt;

        loop {
            if let Err(error) = self.refill() {
                eprintln!("create Runtime Named Pipe instance: {error}");
                if self.pending.is_empty() {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            }
            let Some((server, connected)) = self.pending.next().await else {
                continue;
            };
            match connected {
                Ok(()) => {
                    // 先补一个新实例，再把这个交出去：交出之后到下一次 accept 之间，
                    // 等待中的实例数不低于 backlog - 1。
                    if let Err(error) = self.refill() {
                        eprintln!("create Runtime Named Pipe instance: {error}");
                    }
                    return (server, self.name.to_string_lossy().into_owned());
                }
                Err(error) => {
                    eprintln!("accept Runtime Named Pipe connection: {error}");
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.name.to_string_lossy().into_owned())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn a_long_runtime_home_gets_a_private_short_endpoint_and_cleans_it_up() {
        let preferred = std::env::temp_dir()
            .join("long-runtime-home".repeat(20))
            .join("control.sock");
        let (listener, state, cleanup) = bind(&preferred, "fixture").unwrap();
        let LocalTransportState::UnixSocket { path } = state else {
            panic!("Unix endpoint required")
        };
        assert_ne!(path, preferred);
        assert!(fits_unix_socket(&path));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let directory = cleanup.directory.clone().unwrap();
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        drop((client, server, listener, cleanup));
        assert!(!path.exists());
        assert!(!directory.exists());
    }

    #[tokio::test]
    async fn normal_endpoints_keep_their_path_and_never_replace_non_socket_files() {
        let directory =
            std::env::temp_dir().join(format!("wd-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("s");
        assert!(fits_unix_socket(&path));
        std::fs::write(&path, "preserve this file").unwrap();
        assert!(bind(&path, "fixture").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "preserve this file"
        );
        std::fs::remove_file(&path).unwrap();
        let (listener, state, cleanup) = bind(&path, "fixture").unwrap();
        assert_eq!(
            state,
            LocalTransportState::UnixSocket { path: path.clone() }
        );
        assert!(cleanup.directory.is_none());
        drop((listener, cleanup));
        assert!(directory.exists());
        std::fs::remove_dir(directory).unwrap();
    }

    #[tokio::test]
    async fn an_old_cleanup_cannot_remove_a_replacement_endpoint() {
        let directory =
            std::env::temp_dir().join(format!("wd-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("s");
        let (old_listener, _, old_cleanup) = bind(&path, "old").unwrap();
        std::fs::remove_file(&path).unwrap();
        let (new_listener, _, new_cleanup) = bind(&path, "new").unwrap();
        drop((old_listener, old_cleanup));
        assert!(path.exists());
        let client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let (connection, _) = new_listener.accept().await.unwrap();
        drop((client, connection, new_listener, new_cleanup));
        assert!(!path.exists());
        std::fs::remove_dir(directory).unwrap();
    }
}
