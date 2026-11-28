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
    pub(super) notices: Option<mpsc::Receiver<Notice>>,
}

/// One line the Web adapter printed after the launch URL. The body is the
/// sentence; the title is the session name and may be empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Notice {
    pub(super) title: String,
    pub(super) body: String,
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
        let result = connect(&mut child);
        match result {
            Ok((url, notices)) => Ok(Self {
                child,
                url,
                notices: Some(notices),
            }),
            Err(error) => {
                stop(&mut child);
                Err(error)
            }
        }
    }
}

fn connect(child: &mut Child) -> io::Result<(Url, mpsc::Receiver<Notice>)> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing Web service pipe"))?;
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    // A burst of finished tasks must not stall the reader, or the adapter's
    // stdout pipe fills and its event loop blocks. Drop the oldest instead.
    let (notice_tx, notice_rx) = mpsc::sync_channel(32);
    thread::spawn(move || {
        let mut ready_tx = Some(ready_tx);
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(url) = ready_url(&line)
                && let Some(sender) = ready_tx.take()
            {
                let _ = sender.send(url);
            } else if let Some(notice) = notice_line(&line) {
                // Full means the window is behind. Drop this one rather than
                // block: a blocked reader fills the pipe and stalls the adapter.
                let _ = notice_tx.try_send(notice);
            }
        }
    });
    let url = ready_rx
        .recv_timeout(READY_TIMEOUT)
        .map_err(|error| io::Error::other(format!("Web service could not start: {error}")))?;
    Ok((url, notice_rx))
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

/// A notification frame. Anything that is not one — a log line, a repeat of the
/// launch URL — is ignored, so ordinary output can never become a notification.
fn notice_line(line: &str) -> Option<Notice> {
    let frame: serde_json::Value = serde_json::from_str(line).ok()?;
    let notice = frame.get("willdeep_desktop_notice")?;
    let body = notice.get("body")?.as_str()?.trim();
    if body.is_empty() {
        return None;
    }
    Some(Notice {
        title: notice
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_owned(),
        body: body.to_owned(),
    })
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

#[cfg(test)]
mod tests {
    use super::{Notice, notice_line, ready_url};

    const LAUNCH: &str =
        "{\"willdeep_desktop_url\":\"http://127.0.0.1:43123/__willdeep_desktop/abc\"}";

    #[test]
    fn the_launch_line_is_a_local_url_and_not_a_notice() {
        let url = ready_url(LAUNCH).unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert!(notice_line(LAUNCH).is_none());
    }

    #[test]
    fn a_notice_line_never_reads_as_the_launch_url() {
        let line = "{\"willdeep_desktop_notice\":{\"kind\":\"completed\",\"title\":\"修登录\",\"body\":\"这一轮做完了\"}}";
        assert!(ready_url(line).is_none());
        assert_eq!(
            notice_line(line),
            Some(Notice {
                title: "修登录".to_owned(),
                body: "这一轮做完了".to_owned(),
            })
        );
    }

    #[test]
    fn ordinary_output_and_an_empty_body_are_dropped() {
        for line in [
            "listening on 127.0.0.1:43123",
            "{\"willdeep_desktop_notice\":{\"title\":\"x\",\"body\":\"  \"}}",
            "{\"willdeep_desktop_url\":\"https://example.com/\"}",
            "",
        ] {
            assert!(ready_url(line).is_none(), "{line}");
            assert!(notice_line(line).is_none(), "{line}");
        }
    }
}
