use super::*;

#[path = "fixture_processes.rs"]
mod fixture_processes;

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);
const FORCE_REAP_WINDOW: Duration = Duration::from_secs(2);
const CLEANUP_POLL: Duration = Duration::from_millis(25);

pub(super) struct TestGuard {
    root: PathBuf,
    home: PathBuf,
    pub(super) skip_daemon_cleanup: bool,
}

pub(super) struct ChildGuard(pub(super) Option<std::process::Child>);

impl ChildGuard {
    pub(super) fn stop(&mut self) {
        if let Some(mut child) = self.0.take() {
            if child.try_wait().is_ok_and(|status| status.is_some()) {
                return;
            }
            if let Err(error) = child.kill() {
                eprintln!("kill test child {}: {error}", child.id());
            }
            if let Err(error) = child.wait() {
                eprintln!("reap test child {}: {error}", child.id());
            }
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) fn fixture_executable(home: &Path) -> PathBuf {
    fixture_processes::executable(home)
}

impl TestGuard {
    pub(super) fn new(root: PathBuf, home: PathBuf) -> Self {
        fixture_processes::prepare(&home).expect("prepare test's private executable");
        Self {
            root,
            home,
            skip_daemon_cleanup: false,
        }
    }

    pub(super) fn stop_daemon(&mut self) {
        self.cleanup_daemon()
            .unwrap_or_else(|error| panic!("{error}"));
    }

    fn cleanup_daemon(&self) -> Result<(), String> {
        if self.skip_daemon_cleanup {
            return Ok(());
        }
        stop_daemon_until(&self.home, std::time::Instant::now() + CLEANUP_TIMEOUT)
    }
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        let cleanup = self.cleanup_daemon();
        if thread::panicking() || cleanup.is_err() {
            eprintln!(
                "preserving failed Headless Runtime test at {}",
                self.root.display()
            );
            if let Err(error) = cleanup {
                if thread::panicking() {
                    eprintln!("{error}");
                } else {
                    panic!("{error}");
                }
            }
            return;
        }
        std::fs::remove_dir_all(&self.root).unwrap_or_else(|error| {
            if error.kind() != std::io::ErrorKind::NotFound {
                panic!("remove cleaned test root {}: {error}", self.root.display());
            }
        });
    }
}

fn stop_daemon_until(home: &Path, deadline: std::time::Instant) -> Result<(), String> {
    stop_daemon_observing_startup(home, deadline, || {})
}

fn stop_daemon_observing_startup(
    home: &Path,
    deadline: std::time::Instant,
    mut on_starting: impl FnMut(),
) -> Result<(), String> {
    let runtime = home.join("runtime");
    let state_path = runtime.join("daemon.json");
    let image = fixture_executable(home);
    let mut normal_deadline = deadline.checked_sub(FORCE_REAP_WINDOW).unwrap_or(deadline);
    let mut observed = Vec::new();
    let mut forced = false;
    let mut empty_snapshot = false;
    loop {
        let owned = fixture_processes::owned_until(&image, deadline)?;
        for &pid in &owned {
            if !observed.contains(&pid) {
                observed.push(pid);
            }
        }
        let state = match std::fs::read(&state_path) {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(state) => Some(state),
                Err(error) => {
                    eprintln!(
                        "invalid test Daemon state {}: {error}; using private image identity",
                        state_path.display()
                    );
                    None
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "read test Daemon state {}: {error}",
                    state_path.display()
                ));
            }
        };
        let starting = runtime.join("daemon.lock").exists();
        if state.is_none() && starting {
            on_starting();
        }
        let empty = owned.is_empty() && (!starting || forced);
        // Re-snapshot after a launcher exits during PID enumeration: its newly
        // forked daemon may not have been present in that first PID snapshot.
        if empty && empty_snapshot {
            if !observed.is_empty() {
                eprintln!(
                    "test process cleanup confirmed: home={} exited PIDs={observed:?} forced={forced}",
                    home.display()
                );
            }
            return Ok(());
        }
        empty_snapshot = empty;
        if std::time::Instant::now() >= normal_deadline {
            for pid in owned {
                match fixture_processes::terminate(&image, pid) {
                    Ok(()) => forced = true,
                    Err(error) => eprintln!("{error}"),
                }
            }
        } else if state
            .as_ref()
            .and_then(|state| state["pid"].as_u64())
            .and_then(|pid| u32::try_from(pid).ok())
            .is_some_and(|pid| owned.contains(&pid))
        {
            // Never contact a state file whose PID does not belong to our image.
            let child = willdeep(home)
                .args(["daemon", "stop"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            let mut stop = match child {
                Ok(child) => ChildGuard(Some(child)),
                Err(error) => {
                    eprintln!(
                        "spawn test Daemon stop for {}: {error}; using private image cleanup",
                        home.display()
                    );
                    normal_deadline = std::time::Instant::now();
                    continue;
                }
            };
            loop {
                match stop.0.as_mut().unwrap().try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {}
                    Err(error) => {
                        eprintln!("wait for test Daemon stop: {error}");
                        normal_deadline = std::time::Instant::now();
                        break;
                    }
                }
                if std::time::Instant::now() >= normal_deadline {
                    break;
                }
                thread::sleep(CLEANUP_POLL);
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "test process cleanup exceeded {CLEANUP_TIMEOUT:?} for {}; observed PIDs: {observed:?}",
                home.display()
            ));
        }
        thread::sleep(CLEANUP_POLL);
    }
}

#[path = "process_cleanup_tests.rs"]
mod tests;

#[cfg(unix)]
fn process_exists(pid: u32) -> Result<bool, String> {
    let pid = i32::try_from(pid).map_err(|error| format!("invalid test PID: {error}"))?;
    // SAFETY: signal zero only checks existence and never sends a signal.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(format!("check test PID {pid}: {error}"))
    }
}

#[cfg(windows)]
fn process_exists(pid: u32) -> Result<bool, String> {
    type Handle = *mut std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    const WAIT_OBJECT_0: u32 = 0;
    const WAIT_TIMEOUT: u32 = 258;
    // SAFETY: query-only access to this fixture's PID; the handle is closed below.
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER) {
            Ok(false)
        } else {
            Err(format!("open test PID {pid}: {error}"))
        };
    }
    // SAFETY: handle is valid; zero timeout never blocks.
    let status = unsafe { WaitForSingleObject(handle, 0) };
    let error = std::io::Error::last_os_error();
    // SAFETY: close the one owned handle exactly once.
    unsafe {
        CloseHandle(handle);
    }
    match status {
        WAIT_OBJECT_0 => Ok(false),
        WAIT_TIMEOUT => Ok(true),
        _ => Err(format!("check test PID {pid}: {error}")),
    }
}
