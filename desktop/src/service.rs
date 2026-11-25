use std::io::{self, BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tauri::Url;

const READY_TIMEOUT: Duration = Duration::from_secs(45);
const SERVICE: &str = if cfg!(windows) {
    "willdeep.exe"
} else {
    "willdeep"
};

pub(super) struct Service {
    child: Child,
    pub(super) url: Url,
}

impl Service {
    pub(super) fn start() -> io::Result<Self> {
        let binary = std::env::current_exe()?.with_file_name(SERVICE);
        let mut command = Command::new(binary);
        command
            .arg("desktop-serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        let result = wait_for_url(&mut child);
        match result {
            Ok(url) => Ok(Self { child, url }),
            Err(error) => {
                stop(&mut child);
                Err(error)
            }
        }
    }
}

fn wait_for_url(child: &mut Child) -> io::Result<Url> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing Web service pipe"))?;
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut sender = Some(sender);
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(url) = ready_url(&line)
                && let Some(sender) = sender.take()
            {
                let _ = sender.send(url);
            }
        }
    });
    receiver
        .recv_timeout(READY_TIMEOUT)
        .map_err(|error| io::Error::other(format!("Web service could not start: {error}")))
}

fn ready_url(line: &str) -> Option<Url> {
    let frame: serde_json::Value = serde_json::from_str(line).ok()?;
    let url = Url::parse(frame.get("willdeep_desktop_url")?.as_str()?).ok()?;
    (url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some()
        && url.path().starts_with("/__willdeep_desktop/"))
    .then_some(url)
}

fn stop(child: &mut Child) {
    // Stop only our Web adapter. The daemon owns tasks and can be shared with CLI users.
    drop(child.stdin.take());
    let _ = child.kill();
    let _ = child.wait();
}

impl Drop for Service {
    fn drop(&mut self) {
        stop(&mut self.child);
    }
}
