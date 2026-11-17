//! Exclusive bridge lease: cancellation/drop kills the entire owned process tree.
use std::path::PathBuf;
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use super::artifact::private_directory;
use super::config::BrowserConfig;

#[cfg(all(test, unix))]
#[path = "transport_tests.rs"]
mod tests;

const MAX_FRAME_BYTES: u64 = 12 * 1024 * 1024;
const STARTUP_RESPONSE_RESERVE_MS: u64 = 1000;
pub(super) const BRIDGE: &str = include_str!("bridge.mjs");

fn initialization(config: &BrowserConfig, root: &std::path::Path) -> Value {
    let total_ms = config.timeout_seconds * 1000;
    // Leave time to deliver a startup diagnostic before the outer operation deadline.
    // Short configurations still give Chrome at least three quarters of the budget.
    let startup_timeout_ms = total_ms - (total_ms / 4).min(STARTUP_RESPONSE_RESERVE_MS);
    json!({
        "action":"init", "chrome_path":config.chrome_path,
        "profile":root.join("profile"), "allowed_origins":config.allowed_origins,
        "startup_timeout_ms":startup_timeout_ms,
    })
}

pub(super) struct Bridge {
    child: Option<Child>,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    root: PathBuf,
    next_id: u64,
    #[cfg(unix)]
    group: Option<u32>,
    #[cfg(windows)]
    job: crate::execution::windows_job::Job,
}

impl Bridge {
    pub(super) async fn start(config: &BrowserConfig) -> Result<Self, String> {
        config.dependencies()?;
        let root = private_directory(&std::env::temp_dir(), "willdeep-browser")?;
        let mut command = Command::new(
            config
                .node_path
                .as_ref()
                .ok_or("browser.node_path missing")?,
        );
        command
            .args(["--input-type=module", "--eval", BRIDGE])
            .current_dir(&root)
            .env_clear()
            .env("HOME", &root)
            .env("TMPDIR", &root)
            .env("TMP", &root)
            .env("TEMP", &root)
            .env("LANG", "C.UTF-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        for name in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        // Node and Chrome inherit this group; neither is launched detached.
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        let job = crate::execution::windows_job::Job::new(&mut command)
            .map_err(|e| format!("create browser process job: {e}"))?;
        let spawned = command.spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                if let Err(cleanup) = std::fs::remove_dir_all(&root) {
                    eprintln!("remove unused private browser directory: {cleanup}");
                }
                return Err(format!("start configured Node browser bridge: {error}"));
            }
        };
        #[cfg(windows)]
        if let Err(error) = job.attach_and_resume(&child) {
            let _ = child.start_kill();
            return Err(format!("attach browser process job: {error}"));
        }
        let input = child
            .stdin
            .take()
            .ok_or("browser bridge stdin unavailable")?;
        let output = BufReader::new(
            child
                .stdout
                .take()
                .ok_or("browser bridge stdout unavailable")?,
        );
        let mut bridge = Self {
            #[cfg(unix)]
            group: child.id(),
            #[cfg(windows)]
            job,
            child: Some(child),
            input,
            output,
            root,
            next_id: 1,
        };
        bridge.request(initialization(config, &bridge.root)).await?;
        Ok(bridge)
    }

    pub(super) async fn request(&mut self, mut payload: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        payload["id"] = json!(id);
        let mut bytes =
            serde_json::to_vec(&payload).map_err(|e| format!("encode browser request: {e}"))?;
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .await
            .map_err(|e| format!("UNKNOWN: write browser request: {e}; not retried"))?;
        self.input
            .flush()
            .await
            .map_err(|e| format!("UNKNOWN: flush browser request: {e}; not retried"))?;
        let mut frame = Vec::new();
        (&mut self.output)
            .take(MAX_FRAME_BYTES + 1)
            .read_until(b'\n', &mut frame)
            .await
            .map_err(|e| format!("UNKNOWN: read browser response: {e}; not retried"))?;
        if frame.is_empty() || frame.len() as u64 > MAX_FRAME_BYTES || frame.last() != Some(&b'\n')
        {
            return Err(
                "UNKNOWN: browser bridge closed or exceeded its response budget; not retried"
                    .into(),
            );
        }
        let mut response: Value = serde_json::from_slice(&frame)
            .map_err(|_| "UNKNOWN: malformed browser bridge response; not retried")?;
        if response.get("id").and_then(Value::as_u64) != Some(id) {
            return Err("UNKNOWN: browser bridge response ID mismatch; not retried".into());
        }
        if response["ok"] != true {
            let code = response["error"]["code"].as_str().unwrap_or("UNKNOWN");
            let message = response["error"]["message"]
                .as_str()
                .unwrap_or("browser action failed");
            return Err(format!("{code}: {message}; no automatic retry"));
        }
        Ok(response["result"].take())
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self.group.take() {
            // SAFETY: We have not reaped the owned group leader while using its ID.
            unsafe {
                libc::kill(-(group as i32), libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        self.job.terminate();
        let mut child = self.child.take();
        if let Some(child) = &mut child {
            let _ = child.start_kill();
        }
        // A dedicated OS thread survives both cancellation and runtime shutdown.
        // try_wait is synchronous and does not need a Tokio reactor or timer.
        let root = self.root.clone();
        super::cleanup::dispatch(move || {
            if let Some(mut child) = child {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
                loop {
                    match child.try_wait() {
                        Ok(Some(_)) => break,
                        Ok(None) if std::time::Instant::now() < deadline => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Ok(None) => {
                            eprintln!("reap terminated browser bridge timed out");
                            break;
                        }
                        Err(error) => {
                            eprintln!("reap terminated browser bridge: {error}");
                            break;
                        }
                    }
                }
            }
            // Chrome descendants may still be finishing profile writes after kill.
            for attempt in 0..10 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                match std::fs::remove_dir_all(&root) {
                    Ok(()) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Err(error) if attempt == 9 => {
                        eprintln!("remove private browser profile after termination: {error}");
                    }
                    Err(_) => {}
                }
            }
        });
    }
}
