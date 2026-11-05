use super::*;
use std::sync::mpsc;

const INJECTED_PANIC: &str = "injected test cleanup panic";

// The observer owns the real daemon Child independently of the guard under test.
// Its fallback only runs on failure; a successful assertion must precede Drop.
struct OwnedDaemon {
    release: mpsc::Sender<()>,
    abort: Arc<AtomicBool>,
    exited: Arc<AtomicBool>,
    pid: Arc<AtomicUsize>,
    worker: Option<thread::JoinHandle<Result<(), String>>>,
}

impl OwnedDaemon {
    fn new(home: &Path) -> Self {
        let runtime = home.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        write_private_text(&runtime.join("daemon.lock"), &serde_json::json!({
            "token": token,
            "created_at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
        }).to_string());
        std::fs::write(runtime.join("daemon.log"), b"").unwrap();
        let (release, gate) = mpsc::channel();
        let abort = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(AtomicBool::new(false));
        let pid = Arc::new(AtomicUsize::new(0));
        let worker_abort = abort.clone();
        let worker_exited = exited.clone();
        let worker_pid = pid.clone();
        let home = home.to_owned();
        let worker = thread::spawn(move || {
            loop {
                if worker_abort.load(Ordering::SeqCst) {
                    return Err("fixture aborted before startup".into());
                }
                if gate.recv_timeout(CLEANUP_POLL).is_ok() {
                    break;
                }
            }
            let mut daemon = ChildGuard(Some(
                willdeep(&home)
                    .args(["daemon", "run"])
                    .env("WILLDEEP_DAEMON_LOCK_TOKEN", token)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|error| format!("spawn owned test Daemon: {error}"))?,
            ));
            worker_pid.store(daemon.0.as_ref().unwrap().id() as usize, Ordering::SeqCst);
            loop {
                if worker_abort.load(Ordering::SeqCst) {
                    return Err("fixture fallback needed".into());
                }
                if let Some(status) = daemon
                    .0
                    .as_mut()
                    .unwrap()
                    .try_wait()
                    .map_err(|error| format!("reap owned test Daemon: {error}"))?
                {
                    worker_exited.store(true, Ordering::SeqCst);
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(format!("owned test Daemon exited {status}"))
                    };
                }
                thread::sleep(CLEANUP_POLL);
            }
        });
        Self {
            release,
            abort,
            exited,
            pid,
            worker: Some(worker),
        }
    }

    fn assert_clean(&mut self) {
        assert!(
            self.pid.load(Ordering::SeqCst) > 0,
            "real Daemon was spawned"
        );
        wait_until(Duration::from_secs(1), || {
            self.worker.as_ref().unwrap().is_finished()
        });
        assert!(
            self.exited.load(Ordering::SeqCst),
            "cleanup must finish before the observer's fallback"
        );
        self.worker.take().unwrap().join().unwrap().unwrap();
        assert!(!process_exists(self.pid.load(Ordering::SeqCst) as u32).unwrap());
    }
}

impl Drop for OwnedDaemon {
    fn drop(&mut self) {
        self.abort.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            // Never hide an assertion failure with a second panic.
            let _ = worker.join();
        }
    }
}

fn test_paths() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    fixture_processes::prepare(&home).unwrap();
    let config = root.join("config.toml");
    write_private_config(&config, "http://127.0.0.1:9/v1".to_owned());
    (root, home, workspace, config)
}

#[test]
fn cleanup_reaps_real_cli_web_and_daemon_after_panic() {
    assert_failed_scope_is_clean(false);
}

#[test]
fn cleanup_reaps_real_cli_web_and_daemon_after_wait_timeout() {
    assert_failed_scope_is_clean(true);
}

fn assert_failed_scope_is_clean(timeout: bool) {
    let _serial = process_test_guard();
    let (root, home, workspace, config) = test_paths();
    let mut daemon = OwnedDaemon::new(&home);
    daemon.release.send(()).unwrap();
    let mut pids = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = TestGuard::new(root.clone(), home.clone());
        let cli = ChildGuard(Some(
            willdeep(&home)
                .args(["daemon", "logs", "--follow"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        ));
        pids.push(cli.0.as_ref().unwrap().id());
        let web = ChildGuard(Some(
            willdeep(&home)
                .args([
                    "--web",
                    "--listen",
                    &free_loopback_address(),
                    "--config",
                    path_text(&config),
                    "--workspace",
                    path_text(&workspace),
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        ));
        pids.push(web.0.as_ref().unwrap().id());
        if timeout {
            wait_until(Duration::ZERO, || false);
        }
        panic!("{INJECTED_PANIC}");
    }));
    let payload = result.expect_err("injected failure must be reached");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied());
    assert_eq!(
        message,
        Some(if timeout {
            "condition was not met within 0ns"
        } else {
            INJECTED_PANIC
        })
    );
    for pid in &pids {
        assert!(!process_exists(*pid).unwrap(), "unreaped test client {pid}");
    }
    assert!(
        fixture_processes::owned(&fixture_executable(&home))
            .unwrap()
            .is_empty()
    );
    daemon.assert_clean();
    assert!(root.exists(), "failed test evidence must be preserved");
    eprintln!(
        "cleanup failed scope (timeout={timeout}): home={} clients={pids:?} daemon={} all exited",
        home.display(),
        daemon.pid.load(Ordering::SeqCst)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cleanup_waits_for_a_daemon_that_has_not_published_state() {
    let _serial = process_test_guard();
    let (root, home, _, _) = test_paths();
    let mut daemon = OwnedDaemon::new(&home);
    let mut released = false;
    stop_daemon_observing_startup(&home, std::time::Instant::now() + CLEANUP_TIMEOUT, || {
        if !released {
            assert!(!home.join("runtime/daemon.json").exists());
            assert_eq!(daemon.pid.load(Ordering::SeqCst), 0);
            daemon.release.send(()).unwrap();
            released = true;
        }
    })
    .unwrap();
    assert!(
        released,
        "teardown must observe the pre-state startup boundary"
    );
    daemon.assert_clean();
    assert!(!home.join("runtime/daemon.json").exists());
    assert!(!home.join("runtime/daemon.lock").exists());
    eprintln!(
        "cleanup delayed startup: home={} daemon={} exited",
        home.display(),
        daemon.pid.load(Ordering::SeqCst)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cleanup_reaps_a_completed_client_and_idle_daemon_on_normal_return() {
    let _serial = process_test_guard();
    let (root, home, _, _) = test_paths();
    let mut daemon = OwnedDaemon::new(&home);
    daemon.release.send(()).unwrap();
    let pid;
    {
        let mut guard = TestGuard::new(root.clone(), home.clone());
        let mut cli = ChildGuard(Some(
            willdeep(&home)
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        ));
        pid = cli.0.as_ref().unwrap().id();
        assert!(cli.0.as_mut().unwrap().wait().unwrap().success());
        cli.stop();
        cli.stop();
        guard.stop_daemon();
        daemon.assert_clean();
        // A prior explicit stop must not suppress Drop cleanup after a restart.
        daemon = OwnedDaemon::new(&home);
        daemon.release.send(()).unwrap();
    }
    assert!(!process_exists(pid).unwrap());
    daemon.assert_clean();
    assert!(
        !root.exists(),
        "normal cleanup removes its root only after exit"
    );
    eprintln!(
        "cleanup normal: home={} client={pid} daemon={} all exited",
        home.display(),
        daemon.pid.load(Ordering::SeqCst)
    );
}

#[test]
fn cleanup_deadline_reports_an_unfinished_startup_without_removing_evidence() {
    let _serial = process_test_guard();
    let (root, home, _, _) = test_paths();
    std::fs::create_dir_all(home.join("runtime")).unwrap();
    std::fs::write(home.join("runtime/daemon.lock"), b"unfinished test startup").unwrap();
    let error = stop_daemon_until(&home, std::time::Instant::now()).unwrap_err();
    assert!(error.contains("cleanup exceeded"), "{error}");
    assert!(home.join("runtime/daemon.lock").exists());
    assert!(root.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn cleanup_finds_pre_state_daemon_after_lease_disappears_and_spares_another_home() {
    use std::os::fd::AsRawFd;
    use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt, process::CommandExt};
    let _serial = process_test_guard();
    let (root, home, _, _) = test_paths();
    let runtime = home.join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    let lease = runtime.join("daemon.lock");
    let name = std::ffi::CString::new(lease.as_os_str().as_bytes()).unwrap();
    // SAFETY: a new FIFO at this test's private lease path, mode 0600.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&lease)
        .unwrap();
    const PIPE_CHUNK_BYTES: usize = 4096;
    loop {
        match pipe.write(&[b' '; PIPE_CHUNK_BYTES]) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("prime startup FIFO: {error}"),
        }
    }
    let mut daemon = ChildGuard(Some(
        willdeep(&home)
            .args(["daemon", "run"])
            .env("WILLDEEP_DAEMON_LOCK_TOKEN", "synthetic-startup-gate")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    ));
    let pid = daemon.0.as_ref().unwrap().id();
    // Only the real daemon reads the primed FIFO. Writable means it has entered
    // load_daemon_lock; keeping our writer open prevents read-to-end completing.
    wait_until(CLEANUP_TIMEOUT, || {
        let mut fd = libc::pollfd {
            fd: pipe.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: a single valid pollfd, queried without blocking.
        (unsafe { libc::poll(&mut fd, 1, 0) }) == 1 && fd.revents & libc::POLLOUT != 0
    });
    assert!(
        fixture_processes::owned(&fixture_executable(&home))
            .unwrap()
            .contains(&pid)
    );
    assert!(!runtime.join("daemon.json").exists());
    std::fs::remove_file(&lease).unwrap();

    let (other_root, other_home, _, _) = test_paths();
    let mut other = OwnedDaemon::new(&other_home);
    other.release.send(()).unwrap();
    wait_until(CLEANUP_TIMEOUT, || {
        other_home.join("runtime/daemon.json").exists()
    });
    // argv[0] can lie; a different image bearing our path must not be killed.
    let mut decoy = ChildGuard(Some(
        willdeep(&other_home)
            .args(["daemon", "logs", "--follow"])
            .arg0(fixture_executable(&home))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    ));
    let decoy_pid = decoy.0.as_ref().unwrap().id();
    stop_daemon_until(&home, std::time::Instant::now() + FORCE_REAP_WINDOW).unwrap();
    assert!(!daemon.0.as_mut().unwrap().wait().unwrap().success());
    assert!(!process_exists(pid).unwrap());
    assert!(
        process_exists(other.pid.load(Ordering::SeqCst) as u32).unwrap(),
        "another fixture must survive cleanup"
    );
    assert!(
        decoy.0.as_mut().unwrap().try_wait().unwrap().is_none(),
        "argv[0] decoy must survive cleanup"
    );
    decoy.stop();
    stop_daemon_until(&other_home, std::time::Instant::now() + CLEANUP_TIMEOUT).unwrap();
    other.assert_clean();
    assert!(!process_exists(decoy_pid).unwrap());
    eprintln!(
        "cleanup without lease/state: home={} daemon={pid} exited; other home {} survived",
        home.display(),
        other_home.display()
    );
    drop(pipe);
    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(other_root).unwrap();
}

#[test]
fn cleanup_reaps_upgrade_and_both_daemon_generations_after_panic() {
    let _serial = process_test_guard();
    let (root, home, _, _) = test_paths();
    let mut old = OwnedDaemon::new(&home);
    old.release.send(()).unwrap();
    let mut upgrade_pid = 0;
    let mut replacement_pid = 0;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = TestGuard::new(root.clone(), home.clone());
        wait_until(CLEANUP_TIMEOUT, || {
            willdeep(&home)
                .args(["daemon", "status"])
                .output()
                .is_ok_and(|output| {
                    output.status.success() && output.stdout.starts_with(b"running\t")
                })
        });
        let upgrade = ChildGuard(Some(
            willdeep(&home)
                .args(["daemon", "upgrade", "--force", "--timeout", "15"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        ));
        upgrade_pid = upgrade.0.as_ref().unwrap().id();
        wait_until(CLEANUP_TIMEOUT, || {
            replacement_pid = std::fs::read(home.join("runtime/daemon.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|state| state["pid"].as_u64())
                .unwrap_or(0);
            replacement_pid > 0 && replacement_pid != old.pid.load(Ordering::SeqCst) as u64
        });
        panic!("{INJECTED_PANIC}");
    }));
    let payload = result.expect_err("handoff panic must be reached");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied());
    assert_eq!(message, Some(INJECTED_PANIC));
    assert!(
        fixture_processes::owned(&fixture_executable(&home))
            .unwrap()
            .is_empty()
    );
    old.assert_clean();
    assert!(!process_exists(upgrade_pid).unwrap());
    assert!(!process_exists(replacement_pid as u32).unwrap());
    eprintln!(
        "cleanup handoff panic: home={} upgrade={upgrade_pid} daemons=[{}, {replacement_pid}] all exited",
        home.display(),
        old.pid.load(Ordering::SeqCst)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn cleanup_reaps_client_after_stdin_write_panics() {
    use std::io::Read;

    fn wait_owned_child(pid: libc::pid_t, options: libc::c_int) -> (libc::pid_t, Option<i32>) {
        loop {
            // SAFETY: only the positive PID of this test's directly spawned child.
            let waited = unsafe { libc::waitpid(pid, std::ptr::null_mut(), options) };
            let error = if waited == -1 {
                std::io::Error::last_os_error().raw_os_error()
            } else {
                None
            };
            if error != Some(libc::EINTR) {
                return (waited, error);
            }
        }
    }

    let _serial = process_test_guard();
    for guarded in [false, true] {
        let (root, home, _, _) = test_paths();
        let mut pid = None;
        let mut write_error = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut guard = ChildGuard(Some(
                Command::new(fixture_executable(&home))
                    .env_clear()
                    .env("WILLDEEP_HOME", &home)
                    .env("HOME", &home)
                    .env("XDG_CONFIG_HOME", home.join(".config"))
                    .current_dir(&home)
                    .arg("--version")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            ));
            pid = Some(guard.0.as_ref().unwrap().id() as libc::pid_t);
            // The negative control drops a standard Child during the same unwind.
            let mut unguarded = if guarded { None } else { guard.0.take() };
            let cli = guard.0.as_mut().or(unguarded.as_mut()).unwrap();
            let mut stdout = Vec::new();
            // EOF proves the real version client closed stdout; this does not reap it.
            cli.stdout.take().unwrap().read_to_end(&mut stdout).unwrap();
            assert!(stdout.starts_with(b"willdeep "));
            let write = cli
                .stdin
                .take()
                .unwrap()
                .write_all(br#"{"prompt":"/compress","attachments":[]}"#);
            write_error = write.as_ref().err().map(std::io::Error::kind);
            write.expect(INJECTED_PANIC);
        }));

        // Never wait/try_wait before the failed write. Unlike image enumeration,
        // this detects an exited but unreaped child and reaps the negative control.
        let observed = pid.map(|pid| {
            let deadline = std::time::Instant::now() + CLEANUP_TIMEOUT;
            loop {
                let waited = wait_owned_child(pid, libc::WNOHANG);
                if waited.0 != 0 || std::time::Instant::now() >= deadline {
                    break waited;
                }
                thread::sleep(CLEANUP_POLL);
            }
        });
        let fallback = if let (Some(pid), Some((0, _))) = (pid, observed) {
            // SAFETY: waitpid just confirmed this direct child is still ours.
            if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
                eprintln!(
                    "kill stdin regression child {pid}: {}",
                    std::io::Error::last_os_error()
                );
            }
            Some(wait_owned_child(pid, 0))
        } else {
            None
        };
        // Clean before asserting, including when a broken/no-op guard is detected.
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(write_error, Some(std::io::ErrorKind::BrokenPipe));
        let payload = result.expect_err("stdin write must panic before normal wait");
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied());
        assert!(
            message.is_some_and(|message| message.starts_with(INJECTED_PANIC)),
            "unexpected panic: {message:?}"
        );
        if guarded {
            assert_eq!(
                observed,
                Some((-1, Some(libc::ECHILD))),
                "ChildGuard must reap during stdin panic; fallback={fallback:?}"
            );
        } else {
            assert_eq!(
                observed,
                pid.map(|pid| (pid, None)),
                "unguarded control must remain waitable; fallback={fallback:?}"
            );
        }
    }
}
