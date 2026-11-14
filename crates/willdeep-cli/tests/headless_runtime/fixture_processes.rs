use super::*;
use std::time::Instant;

pub(super) fn executable(home: &Path) -> PathBuf {
    let root = home.parent().expect("fixture home has a root");
    root.join(format!(
        "{}{}",
        root.file_name().unwrap().to_string_lossy(),
        std::env::consts::EXE_SUFFIX
    ))
}

pub(super) fn prepare(home: &Path) -> std::io::Result<()> {
    let path = executable(home);
    std::fs::create_dir_all(path.parent().unwrap())?;
    if !path.exists() {
        // current_exe() keeps every daemon generation bound to this private image.
        std::fs::copy(env!("CARGO_BIN_EXE_willdeep"), &path)?;
    }
    Ok(())
}

/// 看护器与测试进程之间的管道写端；测试进程被 SIGKILL 时内核关闭它，
/// 看护器的 stdin 读到 EOF，开始清理夹具 daemon。
pub(super) struct Reaper {
    pub(super) child: std::process::Child,
    pub(super) stdin: std::process::ChildStdin,
}

/// 测试进程被强杀时 `TestGuard::drop` 不会执行，而 daemon 又用
/// `process_group(0)` 脱离了调用方的进程组。留一个同样脱离的看护进程：
/// 它阻塞读 stdin，测试进程退出后读到 EOF，只停止这个夹具 HOME 的 daemon，
/// 并按镜像路径校验后再精确 kill 残留 PID；不使用 `pkill -f` 的正则匹配。
#[cfg(unix)]
pub(super) fn reap_on_abort(home: &Path) -> std::io::Result<Option<Reaper>> {
    use std::os::unix::process::CommandExt;

    let image = executable(home);
    // 正常清理发 done；调用方被强杀时管道直接 EOF，才执行兜底扫描。
    let script = r#"if read -r status && [ "$status" = done ]; then exit 0; fi
kill_image() {
  ps -Ao pid=,command= -ww | while read -r pid command; do
    case "$command" in
      "$1"|"$1 "*) kill "$pid" 2>/dev/null || true ;;
    esac
  done
}
kill_image_hard() {
  ps -Ao pid=,command= -ww | while read -r pid command; do
    case "$command" in
      "$1"|"$1 "*) kill -9 "$pid" 2>/dev/null || true ;;
    esac
  done
}
i=0
while [ "$i" -lt 10 ]; do
  kill_image "$1"
  sleep 1
  i=$((i+1))
done
kill_image_hard "$1"
WILLDEEP_HOME="$2" "$1" daemon stop >/dev/null 2>&1 || true"#;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .arg("willdeep-test-reaper")
        .arg(&image)
        .arg(home)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()?;
    let stdin = child.stdin.take().expect("the reaper has piped stdin");
    Ok(Some(Reaper { child, stdin }))
}

#[cfg(not(unix))]
pub(super) fn reap_on_abort(_home: &Path) -> std::io::Result<Option<Reaper>> {
    Ok(None)
}

fn check_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("test process cleanup exceeded deadline during image enumeration".into())
    } else {
        Ok(())
    }
}

#[cfg(any(windows, test))]
const IMAGE_QUERY_RETRY: Duration = Duration::from_millis(250);

// Keep querying and waiting on the *same* process handle: reopening a PID could
// observe an unrelated replacement. A failed query alone never proves exit.
#[cfg(any(windows, test))]
fn query_image_until<T>(
    deadline: Instant,
    mut now: impl FnMut() -> Instant,
    mut query: impl FnMut() -> Result<T, String>,
    mut wait_exited: impl FnMut(Duration) -> Result<bool, String>,
) -> Result<Option<T>, String> {
    let retry_deadline = deadline.min(now() + IMAGE_QUERY_RETRY);
    let mut failure = "fixture image query exceeded cleanup deadline".to_owned();
    loop {
        if now() >= retry_deadline {
            return Err(failure);
        }
        match query() {
            Ok(image) => {
                return wait_exited(Duration::ZERO).map(|exited| (!exited).then_some(image));
            }
            Err(error) => {
                failure = format!("{error}; process did not exit within image query retry budget");
                let remaining = retry_deadline.saturating_duration_since(now());
                if wait_exited(remaining.min(CLEANUP_POLL))? {
                    return Ok(None);
                }
            }
        }
    }
}

pub(super) fn owned(executable: &Path) -> Result<Vec<u32>, String> {
    owned_until(executable, Instant::now() + CLEANUP_TIMEOUT)
}

#[cfg(target_os = "linux")]
pub(super) fn owned_until(executable: &Path, deadline: Instant) -> Result<Vec<u32>, String> {
    let expected = executable
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let mut result = Vec::new();
    for entry in std::fs::read_dir("/proc").map_err(|error| error.to_string())? {
        check_deadline(deadline)?;
        let entry = entry.map_err(|error| error.to_string())?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        // Zombies have no active /proc/PID/exe and cannot execute or spawn work.
        if unix_image(pid).is_some_and(|path| path == expected) {
            result.push(pid);
        }
    }
    check_deadline(deadline)?;
    Ok(result)
}

#[cfg(target_os = "linux")]
fn unix_image(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(target_os = "macos")]
pub(super) fn owned_until(executable: &Path, deadline: Instant) -> Result<Vec<u32>, String> {
    let expected = executable
        .canonicalize()
        .map_err(|error| error.to_string())?;
    const PID_SLACK: usize = 32;
    // SAFETY: null buffer asks libproc for the current PID count.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count < 0 {
        return Err(format!(
            "count fixture image processes: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut pids = vec![0i32; count as usize + PID_SLACK];
    let count = loop {
        check_deadline(deadline)?;
        let bytes = i32::try_from(pids.len() * std::mem::size_of::<i32>())
            .map_err(|error| error.to_string())?;
        // SAFETY: the PID buffer has exactly the declared writable byte capacity.
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if count < 0 {
            return Err(format!(
                "list fixture image processes: {}",
                std::io::Error::last_os_error()
            ));
        }
        if (count as usize) < pids.len() {
            break count as usize;
        }
        pids.resize(pids.len() * 2, 0);
    };
    let mut result = Vec::new();
    for &pid in &pids[..count] {
        check_deadline(deadline)?;
        if pid > 0 && unix_image(pid as u32).is_some_and(|path| path == expected) {
            result.push(pid as u32);
        }
    }
    check_deadline(deadline)?;
    Ok(result)
}

#[cfg(target_os = "macos")]
fn unix_image(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: proc_pidpath writes a NUL-terminated kernel image path into the buffer.
    let length = unsafe {
        libc::proc_pidpath(
            i32::try_from(pid).ok()?,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
        )
    };
    if length <= 0 {
        return None;
    }
    // SAFETY: the initialized buffer contains the NUL-terminated path above.
    let path = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr().cast()) };
    Path::new(std::ffi::OsStr::from_bytes(path.to_bytes()))
        .canonicalize()
        .ok()
}

#[cfg(unix)]
pub(super) fn terminate(executable: &Path, pid: u32, deadline: Instant) -> Result<(), String> {
    check_deadline(deadline)?;
    let expected = executable
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if unix_image(pid).is_none_or(|path| path != expected) {
        return Ok(());
    }
    let pid = i32::try_from(pid).map_err(|error| error.to_string())?;
    check_deadline(deadline)?;
    // SAFETY: a kernel image query just bound this PID to this test's private copy.
    if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(format!("terminate fixture PID {pid}: {error}"));
        }
    }
    Ok(())
}

#[cfg(windows)]
pub(super) use windows::owned_until;
#[cfg(windows)]
pub(super) fn terminate(executable: &Path, pid: u32, deadline: Instant) -> Result<(), String> {
    if let Some(handle) = windows::matching_handle(executable, pid, true, deadline)? {
        handle
            .terminate(deadline)
            .map_err(|error| format!("terminate fixture PID {pid}: {error}"))?;
    }
    Ok(())
}

#[cfg(windows)]
mod windows {
    use super::*;
    type RawHandle = *mut std::ffi::c_void;
    const QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const TERMINATE: u32 = 0x0001;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const SNAP_PROCESS: u32 = 0x0000_0002;
    const ERROR_INVALID_PARAMETER: i32 = 87;
    const ERROR_NO_MORE_FILES: i32 = 18;
    const IMAGE_PATH_CAPACITY: usize = 32768;
    const MAX_PATH: usize = 260;
    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        pid: u32,
        heap_id: usize,
        module_id: u32,
        threads: u32,
        parent_pid: u32,
        priority: i32,
        flags: u32,
        executable: [u16; MAX_PATH],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> RawHandle;
        fn QueryFullProcessImageNameW(
            process: RawHandle,
            flags: u32,
            path: *mut u16,
            length: *mut u32,
        ) -> i32;
        fn TerminateProcess(process: RawHandle, code: u32) -> i32;
        fn WaitForSingleObject(handle: RawHandle, milliseconds: u32) -> u32;
        fn CloseHandle(handle: RawHandle) -> i32;
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> RawHandle;
        fn Process32FirstW(snapshot: RawHandle, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: RawHandle, entry: *mut ProcessEntry) -> i32;
    }
    pub(super) struct Handle(RawHandle);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: one owned process/snapshot handle, closed exactly once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    impl Handle {
        fn wait_exited(&self, timeout: Duration) -> Result<bool, String> {
            const WAIT_OBJECT_0: u32 = 0;
            const WAIT_TIMEOUT: u32 = 258;
            // Round down so the wait never exceeds the remaining cleanup budget.
            let milliseconds = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
            // SAFETY: the valid process handle has SYNCHRONIZE; the wait is bounded.
            match unsafe { WaitForSingleObject(self.0, milliseconds) } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                status => Err(format!(
                    "wait for fixture process exit (status={status}): {}",
                    std::io::Error::last_os_error()
                )),
            }
        }
        pub(super) fn terminate(&self, deadline: Instant) -> Result<(), String> {
            check_deadline(deadline)?;
            // SAFETY: TERMINATE access and the image were checked on this same handle.
            if unsafe { TerminateProcess(self.0, 1) } == 0 {
                let error = std::io::Error::last_os_error();
                let timeout =
                    IMAGE_QUERY_RETRY.min(deadline.saturating_duration_since(Instant::now()));
                if !self.wait_exited(timeout)? {
                    return Err(format!("terminate fixture process: {error}"));
                }
            }
            Ok(())
        }
    }
    pub(in super::super) fn owned_until(
        executable: &Path,
        deadline: Instant,
    ) -> Result<Vec<u32>, String> {
        check_deadline(deadline)?;
        // SAFETY: snapshot only enumerates process IDs and image names, no environment.
        let raw = unsafe { CreateToolhelp32Snapshot(SNAP_PROCESS, 0) };
        if raw as isize == -1 {
            return Err(format!(
                "snapshot fixture processes: {}",
                std::io::Error::last_os_error()
            ));
        }
        let snapshot = Handle(raw);
        // SAFETY: ProcessEntry contains only integer fields and a fixed UTF-16 array.
        let mut entry: ProcessEntry = unsafe { std::mem::zeroed() };
        entry.size = std::mem::size_of::<ProcessEntry>() as u32;
        // SAFETY: initialized, correctly sized entry and a valid process snapshot.
        let mut available = unsafe { Process32FirstW(snapshot.0, &mut entry) };
        let mut result = Vec::new();
        while available != 0 {
            check_deadline(deadline)?;
            let length = entry
                .executable
                .iter()
                .position(|&ch| ch == 0)
                .unwrap_or(MAX_PATH);
            let name = String::from_utf16_lossy(&entry.executable[..length]);
            if name.eq_ignore_ascii_case(&executable.file_name().unwrap().to_string_lossy())
                && matching_handle(executable, entry.pid, false, deadline)?.is_some()
            {
                result.push(entry.pid);
            }
            // SAFETY: same valid snapshot and entry buffer.
            available = unsafe { Process32NextW(snapshot.0, &mut entry) };
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_NO_MORE_FILES) {
            return Err(format!("enumerate fixture processes: {error}"));
        }
        check_deadline(deadline)?;
        Ok(result)
    }
    pub(super) fn matching_handle(
        executable: &Path,
        pid: u32,
        terminate: bool,
        deadline: Instant,
    ) -> Result<Option<Handle>, String> {
        check_deadline(deadline)?;
        let access =
            QUERY_LIMITED_INFORMATION | SYNCHRONIZE | if terminate { TERMINATE } else { 0 };
        // SAFETY: OpenProcess returns an owned handle or null.
        let raw = unsafe { OpenProcess(access, 0, pid) };
        if raw.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER) {
                return Ok(None);
            }
            // A process can finish between the snapshot and OpenProcess.
            // SAFETY: a separate query-only synchronization handle, if still available.
            let sync = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
            if !sync.is_null() {
                let sync = Handle(sync);
                let timeout =
                    IMAGE_QUERY_RETRY.min(deadline.saturating_duration_since(Instant::now()));
                if sync
                    .wait_exited(timeout)
                    .map_err(|error| format!("wait fixture PID {pid}: {error}"))?
                {
                    return Ok(None);
                }
            }
            return Err(format!("open fixture PID {pid}: {error}"));
        }
        let handle = Handle(raw);
        let mut buffer = vec![0u16; IMAGE_PATH_CAPACITY];
        let Some(length) = query_image_until(
            deadline,
            Instant::now,
            || {
                let mut length = buffer.len() as u32;
                // SAFETY: the same owned process handle and a writable UTF-16 buffer.
                if unsafe { QueryFullProcessImageNameW(raw, 0, buffer.as_mut_ptr(), &mut length) }
                    == 0
                {
                    Err(format!(
                        "query fixture PID {pid} image: {}",
                        std::io::Error::last_os_error()
                    ))
                } else {
                    Ok(length as usize)
                }
            },
            |timeout| {
                handle
                    .wait_exited(timeout)
                    .map_err(|error| format!("wait fixture PID {pid}: {error}"))
            },
        )?
        else {
            return Ok(None);
        };
        let image = PathBuf::from(String::from_utf16_lossy(&buffer[..length]))
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let expected = executable
            .canonicalize()
            .map_err(|error| error.to_string())?;
        check_deadline(deadline)?;
        Ok(image
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
            .then_some(handle))
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn query_failure_waits_for_exit_instead_of_treating_denial_as_exit() {
        let started = Instant::now();
        let now = Cell::new(started);
        let queries = Cell::new(0);
        let waits = Cell::new(0);
        let result = query_image_until::<()>(
            started + CLEANUP_TIMEOUT,
            || now.get(),
            || {
                queries.set(queries.get() + 1);
                Err("injected access denied".into())
            },
            |timeout| {
                assert_eq!(timeout, CLEANUP_POLL);
                now.set(now.get() + timeout);
                waits.set(waits.get() + 1);
                Ok(waits.get() == 2)
            },
        );
        assert_eq!(result, Ok(None));
        assert_eq!(queries.get(), 2);
    }

    #[test]
    fn transient_query_failure_returns_the_image_for_identity_validation() {
        let started = Instant::now();
        let now = Cell::new(started);
        let queries = Cell::new(0);
        let expected = PathBuf::from("another-fixture/image.exe");
        let result = query_image_until(
            started + CLEANUP_TIMEOUT,
            || now.get(),
            || {
                queries.set(queries.get() + 1);
                if queries.get() == 1 {
                    Err("injected access denied".into())
                } else {
                    Ok(expected.clone())
                }
            },
            |timeout| {
                now.set(now.get() + timeout);
                Ok(false)
            },
        );
        assert_eq!(result, Ok(Some(expected)));
        assert_eq!(queries.get(), 2);
    }

    #[test]
    fn live_unqueryable_process_fails_within_both_budgets() {
        for budget in [Duration::from_millis(40), CLEANUP_TIMEOUT] {
            let started = Instant::now();
            let now = Cell::new(started);
            let result = query_image_until::<()>(
                started + budget,
                || now.get(),
                || Err("injected access denied".into()),
                |timeout| {
                    assert!(timeout <= CLEANUP_POLL);
                    now.set(now.get() + timeout);
                    Ok(false)
                },
            );
            let error = result.unwrap_err();
            assert!(error.contains("injected access denied"), "{error}");
            assert_eq!(now.get() - started, budget.min(IMAGE_QUERY_RETRY));
        }
    }

    #[test]
    fn wait_failure_is_not_reported_as_a_live_or_exited_process() {
        let error = query_image_until::<()>(
            Instant::now() + CLEANUP_TIMEOUT,
            Instant::now,
            || Err("injected access denied".into()),
            |_| Err("injected wait failure".into()),
        )
        .unwrap_err();
        assert_eq!(error, "injected wait failure");
    }

    #[test]
    fn expired_deadline_never_queries_or_waits() {
        let deadline = Instant::now();
        assert!(
            query_image_until::<()>(
                deadline,
                || deadline,
                || panic!("query after deadline"),
                |_| panic!("wait after deadline"),
            )
            .is_err()
        );
    }
}
