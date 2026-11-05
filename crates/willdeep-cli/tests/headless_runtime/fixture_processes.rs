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

fn check_deadline(deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        Err("test process cleanup exceeded deadline during image enumeration".into())
    } else {
        Ok(())
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
pub(super) fn terminate(executable: &Path, pid: u32) -> Result<(), String> {
    let expected = executable
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if unix_image(pid).is_none_or(|path| path != expected) {
        return Ok(());
    }
    let pid = i32::try_from(pid).map_err(|error| error.to_string())?;
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
pub(super) fn terminate(executable: &Path, pid: u32) -> Result<(), String> {
    if let Some(handle) = windows::matching_handle(executable, pid, true)? {
        handle.terminate()?;
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
        fn exited(&self) -> bool {
            const WAIT_OBJECT_0: u32 = 0;
            // SAFETY: the valid process handle has SYNCHRONIZE; zero never blocks.
            unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
        }
        pub(super) fn terminate(&self) -> Result<(), String> {
            // SAFETY: TERMINATE access and the image were checked on this same handle.
            if unsafe { TerminateProcess(self.0, 1) } == 0 {
                let error = std::io::Error::last_os_error();
                if !self.exited() {
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
                && matching_handle(executable, entry.pid, false)?.is_some()
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
    ) -> Result<Option<Handle>, String> {
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
            if !sync.is_null() && Handle(sync).exited() {
                return Ok(None);
            }
            return Err(format!("open fixture PID {pid}: {error}"));
        }
        let handle = Handle(raw);
        let mut buffer = vec![0u16; IMAGE_PATH_CAPACITY];
        let mut length = buffer.len() as u32;
        // SAFETY: a valid process handle and writable buffer of the reported size.
        if unsafe { QueryFullProcessImageNameW(raw, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
            let error = std::io::Error::last_os_error();
            return if handle.exited() {
                Ok(None)
            } else {
                Err(format!("query fixture PID {pid} image: {error}"))
            };
        }
        if handle.exited() {
            return Ok(None);
        }
        let image = PathBuf::from(String::from_utf16_lossy(&buffer[..length as usize]))
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let expected = executable
            .canonicalize()
            .map_err(|error| error.to_string())?;
        Ok(image
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
            .then_some(handle))
    }
}
