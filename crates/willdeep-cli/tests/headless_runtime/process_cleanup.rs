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
    reaper: Option<fixture_processes::Reaper>,
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
            reaper: None,
        }
    }

    /// 只给会启动隔离 daemon 的测试用：进程被强杀时由看护进程回收。
    pub(super) fn reap_on_abort(&mut self) {
        if self.reaper.is_some() {
            return;
        }
        match fixture_processes::reap_on_abort(&self.home) {
            Ok(reaper) => self.reaper = reaper,
            Err(error) => eprintln!("cannot start the fixture reaper: {error}"),
        }
    }

    fn stop_reaper(&mut self) {
        let Some(mut reaper) = self.reaper.take() else {
            return;
        };
        // daemon 已确认退出：发完成标记，让看护器直接退出；异常 EOF 才兜底清理。
        use std::io::Write;
        if let Err(error) = reaper.stdin.write_all(b"done\n") {
            eprintln!("notify test fixture reaper of successful cleanup: {error}");
        }
        drop(reaper.stdin);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match reaper.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if std::time::Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(None) | Err(_) => break,
            }
        }
        let _ = reaper.child.kill();
        let _ = reaper.child.wait();
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
        // daemon 已确认收掉时，看护器没有兜底任务，必须回收；否则每个节点
        // 都会留一个 sh 轮询到整个测试二进制退出。cleanup 失败时保留看护器，
        // 让它在测试进程退出后继续清理。
        if cleanup.is_ok() && !self.skip_daemon_cleanup {
            self.stop_reaper();
        } else if self.reaper.is_some() {
            // 进程内收不掉时让看护器接手：关闭写端触发清理，不等待也不杀它。
            self.reaper.take();
        }
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

struct CleanupProgress {
    started: std::time::Instant,
    stage_started: std::time::Instant,
    stage: &'static str,
    observed: Vec<u32>,
    starting: Option<bool>,
    forced: bool,
    enumerations: usize,
    enumeration_elapsed: Duration,
    stop_status: Option<String>,
}

impl CleanupProgress {
    fn new() -> Self {
        let started = std::time::Instant::now();
        Self {
            started,
            stage_started: started,
            stage: "initializing",
            observed: Vec::new(),
            starting: None,
            forced: false,
            enumerations: 0,
            enumeration_elapsed: Duration::ZERO,
            stop_status: None,
        }
    }

    fn enter(&mut self, stage: &'static str) {
        self.stage = stage;
        self.stage_started = std::time::Instant::now();
    }

    fn describe(&self, home: &Path) -> String {
        format!(
            "home={} elapsed={:?} stage={} stage_elapsed={:?} enumerations={} enumeration_elapsed={:?} observed PIDs={:?} starting={:?} forced={} daemon_stop={:?}",
            home.display(),
            self.started.elapsed(),
            self.stage,
            self.stage_started.elapsed(),
            self.enumerations,
            self.enumeration_elapsed,
            self.observed,
            self.starting,
            self.forced,
            self.stop_status,
        )
    }
}

fn stop_daemon_observing_startup(
    home: &Path,
    deadline: std::time::Instant,
    mut on_starting: impl FnMut(),
) -> Result<(), String> {
    let mut progress = CleanupProgress::new();
    let result = cleanup_daemon_loop(home, deadline, &mut on_starting, &mut progress);
    let context = progress.describe(home);
    match result {
        Ok(()) => {
            eprintln!("test process cleanup confirmed: {context}");
            Ok(())
        }
        Err(error) => Err(format!("{error}; {context}")),
    }
}

fn cleanup_daemon_loop(
    home: &Path,
    deadline: std::time::Instant,
    on_starting: &mut impl FnMut(),
    progress: &mut CleanupProgress,
) -> Result<(), String> {
    let runtime = home.join("runtime");
    let state_path = runtime.join("daemon.json");
    let image = fixture_executable(home);
    let mut normal_deadline = deadline.checked_sub(FORCE_REAP_WINDOW).unwrap_or(deadline);
    let mut empty_snapshot = false;
    loop {
        progress.enter("image enumeration");
        let owned = fixture_processes::owned_until(&image, deadline);
        progress.enumerations += 1;
        progress.enumeration_elapsed += progress.stage_started.elapsed();
        let owned = owned?;
        for &pid in &owned {
            if !progress.observed.contains(&pid) {
                progress.observed.push(pid);
            }
        }
        progress.enter("read daemon state");
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
        progress.starting = Some(starting);
        if state.is_none() && starting {
            progress.enter("startup observer");
            on_starting();
        }
        // An empty image snapshot cannot distinguish a stale lease from a
        // launcher that has not started yet. Only an actual owned-process
        // termination permits ignoring the startup marker.
        let empty = owned.is_empty() && (!starting || progress.forced);
        // Re-snapshot after a launcher exits during PID enumeration: its newly
        // forked daemon may not have been present in that first PID snapshot.
        if empty && empty_snapshot {
            if std::time::Instant::now() >= deadline {
                return Err("test process cleanup exceeded total deadline".into());
            }
            progress.enter("confirm empty snapshots");
            return Ok(());
        }
        empty_snapshot = empty;
        if std::time::Instant::now() >= normal_deadline {
            progress.enter("force terminate");
            for pid in owned {
                fixture_processes::terminate(&image, pid, deadline)?;
                progress.forced = true;
            }
        } else if state
            .as_ref()
            .and_then(|state| state["pid"].as_u64())
            .and_then(|pid| u32::try_from(pid).ok())
            .is_some_and(|pid| owned.contains(&pid))
        {
            progress.enter("spawn daemon stop");
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
                    progress.stop_status = Some(format!("spawn failed: {error}"));
                    eprintln!(
                        "test Daemon stop failed; using private image cleanup: {}",
                        progress.describe(home)
                    );
                    normal_deadline = std::time::Instant::now();
                    continue;
                }
            };
            progress.enter("wait daemon stop");
            let waited = loop {
                match stop.0.as_mut().unwrap().try_wait() {
                    Ok(Some(status)) => break Ok(Some(status)),
                    Ok(None) => {}
                    Err(error) => break Err(format!("wait for test Daemon stop: {error}")),
                }
                let remaining =
                    normal_deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    break Ok(None);
                }
                thread::sleep(CLEANUP_POLL.min(remaining));
            };
            progress.stop_status = Some(match &waited {
                Ok(Some(status)) => status.to_string(),
                Ok(None) => "graceful stop deadline exceeded".into(),
                Err(error) => error.clone(),
            });
            progress.enter("reap daemon stop");
            // Preserve the existing direct-child ownership and reap contract:
            // a deadline must not discard a Child whose exit is still pending.
            stop.stop();
            let status = waited?;
            if status.is_none_or(|status| !status.success()) {
                normal_deadline = std::time::Instant::now();
            }
            eprintln!("test Daemon stop completed: {}", progress.describe(home));
        }
        progress.enter("cleanup poll");
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err("test process cleanup exceeded total deadline".into());
        }
        thread::sleep(CLEANUP_POLL.min(remaining));
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
