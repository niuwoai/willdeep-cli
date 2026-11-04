use super::*;

#[test]
fn background_supervisor_completes_work_and_kills_it_when_parent_disconnects() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).expect("create test home");
    std::fs::create_dir_all(&workspace).expect("create test Workspace");

    let rejected = willdeep(&home)
        .args(["daemon", "background-supervisor"])
        .env_remove("WILLDEEP_INTERNAL_BACKGROUND_SUPERVISOR")
        .output()
        .expect("invoke untrusted background supervisor");
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr)
            .contains("background supervisor is an internal command")
    );

    let mut completed = spawn_background_supervisor(&home);
    let completed_liveness = send_supervisor_request(
        &mut completed,
        serde_json::json!({
            "command": supervisor_print_command(),
            "sandbox": { "policy": "Off", "writable_roots": [] },
            "workspace": workspace,
            "timeout_seconds": 300
        }),
    );
    let completed = completed
        .wait_with_output()
        .expect("wait for completed background supervisor");
    drop(completed_liveness);
    assert_success(&completed, "completed background supervisor");
    let completed: serde_json::Value =
        serde_json::from_slice(&completed.stdout).expect("parse completed supervisor result");
    assert_eq!(completed["status"], "completed");
    assert!(
        completed["output"]
            .as_str()
            .is_some_and(|value| value.contains("supervisor-ok"))
    );

    let mut disconnected = spawn_background_supervisor(&home);
    let child_pid_path = workspace.join("supervisor-child.pid");
    let disconnected_liveness = send_supervisor_request(
        &mut disconnected,
        serde_json::json!({
            "command": supervisor_wait_command(),
            "sandbox": { "policy": "Off", "writable_roots": [] },
            "workspace": workspace,
            "timeout_seconds": 60
        }),
    );
    // 文件存在时仍可能被 Windows 写入方锁住，或只写入了 PID 的一部分。
    // 等完整 PID 就绪；失败仍先断连收拢子进程，再带上最后的读取错误和输出。
    let pid_timeout = Duration::from_secs(5);
    #[cfg_attr(not(unix), allow(unused_variables))]
    let child_pid = match wait_for_supervised_child_pid(&child_pid_path, pid_timeout) {
        Ok(pid) => pid,
        Err(error) => {
            drop(disconnected_liveness);
            let output = disconnected
                .wait_with_output()
                .unwrap_or_else(|wait_error| {
                    panic!(
                        "read supervised child PID {} within {pid_timeout:?}: {error:?}; collect background supervisor output: {wait_error}",
                        child_pid_path.display()
                    )
                });
            let cleanup = std::fs::remove_dir_all(&root);
            panic!(
                "supervised child PID {} was not ready within {pid_timeout:?}: {error:?}; supervisor exited {:?}\nstdout:\n{}\nstderr:\n{}\ncleanup: {cleanup:?}",
                child_pid_path.display(),
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    };
    let started = std::time::Instant::now();
    drop(disconnected_liveness);
    let disconnected = disconnected
        .wait_with_output()
        .expect("wait for disconnected background supervisor");
    assert_success(&disconnected, "disconnected background supervisor");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "parent disconnect must not wait for the 30 second Shell command"
    );
    let disconnected: serde_json::Value =
        serde_json::from_slice(&disconnected.stdout).expect("parse disconnected result");
    assert_eq!(disconnected["status"], "killed");
    #[cfg(unix)]
    wait_until(Duration::from_secs(5), || !process_exists(child_pid));
    std::fs::remove_dir_all(root).expect("remove supervisor test root");
}

#[cfg(unix)]
#[test]
fn background_supervisor_applies_read_only_sandbox() {
    let _serial = process_test_guard();
    let root = temporary_root();
    let home = root.join("home");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let mut child = spawn_background_supervisor(&home);
    let liveness = send_supervisor_request(
        &mut child,
        serde_json::json!({
            "command": "touch sandbox-must-not-write",
            "workspace": workspace,
            "timeout_seconds": 10,
            "sandbox": { "policy": "ReadOnly", "writable_roots": [] }
        }),
    );
    let output = child.wait_with_output().unwrap();
    drop(liveness);
    assert_success(&output, "read-only supervisor result");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_ne!(result["status"], "completed");
    assert!(!workspace.join("sandbox-must-not-write").exists());
    std::fs::remove_dir_all(root).unwrap();
}

fn wait_for_supervised_child_pid(path: &Path, timeout: Duration) -> std::io::Result<u32> {
    read_supervised_child_pid_until_ready(timeout, || std::fs::read_to_string(path))
}

fn read_supervised_child_pid_until_ready(
    timeout: Duration,
    mut read: impl FnMut() -> std::io::Result<String>,
) -> std::io::Result<u32> {
    const POLL_INTERVAL: Duration = Duration::from_millis(25);
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let result = read().and_then(|contents| {
            // 换行是夹具的完成标记，不能把尚未写完的数字前缀当成 PID。
            let line = contents.strip_suffix('\n').ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("PID file has no complete line: {contents:?}"),
                )
            })?;
            let digits = line.strip_suffix('\r').unwrap_or(line);
            if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid child PID {contents:?}: expected ASCII digits"),
                ));
            }
            digits
                .parse::<std::num::NonZeroU32>()
                .map(std::num::NonZeroU32::get)
                .map_err(|error| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("invalid child PID {contents:?}: {error}"),
                    )
                })
        });
        let now = std::time::Instant::now();
        match result {
            Ok(pid) => return Ok(pid),
            Err(error) if now >= deadline => return Err(error),
            Err(_) => std::thread::sleep(POLL_INTERVAL.min(deadline - now)),
        }
    }
}

#[test]
fn supervised_child_pid_requires_a_complete_nonzero_pid() {
    let root = temporary_root();
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("child.pid");
    assert_eq!(
        wait_for_supervised_child_pid(&path, Duration::ZERO)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    for contents in [
        "",
        "123",
        "123\r",
        "\n",
        "invalid\n",
        "0\n",
        "4294967296\n",
        "+123\n",
        " 123\n",
        "123\n\n",
    ] {
        std::fs::write(&path, contents).unwrap();
        let error = wait_for_supervised_child_pid(&path, Duration::ZERO).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains(&format!("{contents:?}")));
    }
    for contents in ["12345\n", "12345\r\n"] {
        std::fs::write(&path, contents).unwrap();
        assert_eq!(
            wait_for_supervised_child_pid(&path, Duration::ZERO).unwrap(),
            12345
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn supervised_child_pid_waits_for_the_rest_of_the_digits() {
    let root = temporary_root();
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("child.pid");
    // 每次读取前推进写入状态，确定性覆盖不存在、空文件和可解析的数字前缀。
    let mut attempts = 0;
    let pid = read_supervised_child_pid_until_ready(Duration::from_secs(5), || {
        attempts += 1;
        match attempts {
            1 => {}
            2 => std::fs::write(&path, "").unwrap(),
            3 => std::fs::write(&path, "123").unwrap(),
            _ => std::fs::write(&path, "12345\n").unwrap(),
        }
        std::fs::read_to_string(&path)
    })
    .unwrap();
    assert_eq!(pid, 12345);
    assert_eq!(attempts, 4);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn supervised_child_pid_deadline_preserves_the_last_read_error() {
    let timeout = Duration::from_millis(50);
    let started = std::time::Instant::now();
    let error = read_supervised_child_pid_until_ready(timeout, || {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "PID writer still owns the file",
        ))
    })
    .unwrap_err();
    assert!(started.elapsed() >= timeout);
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(error.to_string(), "PID writer still owns the file");
}

#[cfg(windows)]
#[test]
fn supervised_child_pid_retries_windows_sharing_violation() {
    use std::os::windows::fs::OpenOptionsExt;

    let root = temporary_root();
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("child.pid");
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    writer.write_all(b"12345\r\n").unwrap();
    let error = wait_for_supervised_child_pid(&path, Duration::ZERO).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(32));
    std::thread::scope(|scope| {
        let (observed, locked) = std::sync::mpsc::channel();
        let (released, unlocked) = std::sync::mpsc::channel();
        scope.spawn(move || {
            let locked = locked.recv_timeout(Duration::from_secs(5));
            drop(writer);
            released.send(()).unwrap();
            locked.expect("reader must observe the sharing violation before unlocking");
        });
        let mut observed_lock = false;
        let pid = read_supervised_child_pid_until_ready(Duration::from_secs(5), || {
            let result = std::fs::read_to_string(&path);
            if result
                .as_ref()
                .is_err_and(|error| error.raw_os_error() == Some(32))
                && !observed_lock
            {
                observed_lock = true;
                observed.send(()).unwrap();
                unlocked.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            result
        })
        .unwrap();
        assert!(observed_lock);
        assert_eq!(pid, 12345);
    });
    std::fs::remove_dir_all(root).unwrap();
}

fn spawn_background_supervisor(home: &Path) -> std::process::Child {
    willdeep(home)
        .args(["daemon", "background-supervisor"])
        .env("WILLDEEP_INTERNAL_BACKGROUND_SUPERVISOR", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn background supervisor")
}

fn send_supervisor_request(
    child: &mut std::process::Child,
    request: serde_json::Value,
) -> std::process::ChildStdin {
    let payload = serde_json::to_vec(&request).expect("serialize supervisor request");
    let mut input = child.stdin.take().expect("background supervisor stdin");
    input
        .write_all(&u32::try_from(payload.len()).unwrap().to_be_bytes())
        .expect("write supervisor request length");
    input.write_all(&payload).expect("write supervisor request");
    input.flush().expect("flush supervisor request");
    input
}

#[cfg(unix)]
fn supervisor_print_command() -> &'static str {
    "printf supervisor-ok"
}

#[cfg(windows)]
fn supervisor_print_command() -> &'static str {
    "Write-Output 'supervisor-ok'"
}

#[cfg(unix)]
fn supervisor_wait_command() -> &'static str {
    "sleep 30 & child=$!; printf '%s\\n' \"$child\" > supervisor-child.pid; wait \"$child\""
}

#[cfg(windows)]
fn supervisor_wait_command() -> &'static str {
    "$child = Start-Process powershell.exe -ArgumentList '-NoProfile','-Command','Start-Sleep -Seconds 30' -PassThru; Set-Content -Path supervisor-child.pid -Value $child.Id -Encoding Ascii; Wait-Process -Id $child.Id"
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    std::process::Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
