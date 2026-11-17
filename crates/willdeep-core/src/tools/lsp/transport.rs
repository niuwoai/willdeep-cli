//! Bounded, continuously drained Content-Length JSON-RPC transport.
//! A query takes exclusive ownership of its client. Dropping that query destroys
//! the transport/process, so cancelled requests cannot poison the next query.
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::{Mutex as AsyncMutex, Notify, oneshot};
use tokio::task::JoinHandle;

use super::config::Language;

pub(super) const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 8192;
const MAX_STDERR_BYTES: usize = 8192;
const MAX_PUBLICATIONS: usize = 32;
const MAX_DIAGNOSTICS: usize = 200;
const MAX_NOTIFICATION_BYTES: usize = 128 * 1024;
const MAX_SERVER_REQUEST_ITEMS: usize = 128;
const MAX_STATUS_MESSAGE_BYTES: usize = 1000;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Health {
    Ok,
    Warning,
    Error,
}

#[derive(Clone, Debug, Deserialize)]
struct ServerStatus {
    health: Health,
    quiescent: bool,
    message: Option<String>,
}

#[derive(Default)]
struct Readiness {
    status: Option<ServerStatus>,
    revision: u64,
}

type Writer = Arc<AsyncMutex<Box<dyn AsyncWrite + Unpin + Send>>>;
type Reply = Result<Value, String>;

#[derive(Clone, Debug)]
pub(super) struct Publication {
    pub(super) version: Option<i64>,
    pub(super) diagnostics: Vec<Value>,
    pub(super) truncated: bool,
}

#[derive(Default)]
struct Shared {
    pending: Mutex<BTreeMap<u64, oneshot::Sender<Reply>>>,
    publications: Mutex<BTreeMap<String, Option<Publication>>>,
    rust_status: Mutex<Readiness>,
    failure: Mutex<Option<String>>,
    changed: Notify,
    stderr: Mutex<VecDeque<u8>>,
    process: Mutex<ProcessTree>,
}

#[derive(Default)]
struct ProcessTree {
    #[cfg(unix)]
    group: Option<u32>,
    #[cfg(windows)]
    job: Option<crate::execution::windows_job::Job>,
}

impl ProcessTree {
    fn terminate(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.group.take() {
            // SAFETY: The owned child leader has not been reaped. Its PID cannot
            // be reused while this process-group identifier is retained.
            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        }
        #[cfg(windows)]
        if let Some(mut job) = self.job.take() {
            job.terminate();
        }
    }
}

impl Shared {
    fn fail(&self, reason: String) {
        self.process.lock().expect("lsp process").terminate();
        self.failure
            .lock()
            .expect("lsp failure")
            .get_or_insert_with(|| reason.clone());
        for (_, sender) in std::mem::take(&mut *self.pending.lock().expect("lsp pending")) {
            let _ = sender.send(Err(reason.clone()));
        }
        self.changed.notify_waiters();
    }

    fn error(&self) -> Option<String> {
        self.failure.lock().expect("lsp failure").clone()
    }
}

pub(super) struct Rpc {
    writer: Writer,
    shared: Arc<Shared>,
    reader: JoinHandle<()>,
    stderr: Option<JoinHandle<()>>,
    child: Option<Child>,
    next_id: u64,
}

impl Rpc {
    pub(super) fn from_io(
        read: impl AsyncRead + Unpin + Send + 'static,
        write: impl AsyncWrite + Unpin + Send + 'static,
        language: Language,
        root_uri: String,
    ) -> Self {
        let writer: Writer = Arc::new(AsyncMutex::new(Box::new(write)));
        let shared = Arc::new(Shared::default());
        let task_shared = shared.clone();
        let task_writer = writer.clone();
        let reader = tokio::spawn(async move {
            if let Err(error) =
                read_loop(read, task_writer, task_shared.clone(), language, root_uri).await
            {
                task_shared.fail(error);
            }
        });
        Self {
            writer,
            shared,
            reader,
            stderr: None,
            child: None,
            next_id: 1,
        }
    }

    pub(super) fn attach_process(
        &mut self,
        child: Child,
        stderr: impl AsyncRead + Unpin + Send + 'static,
        #[cfg(windows)] job: crate::execution::windows_job::Job,
    ) {
        {
            let mut process = self.shared.process.lock().expect("lsp process");
            #[cfg(unix)]
            {
                process.group = child.id();
            }
            #[cfg(windows)]
            {
                process.job = Some(job);
            }
        }
        self.child = Some(child);
        // EOF may have raced attachment; do not leave an already-failed server
        // alive merely because it failed before its process guard was attached.
        if self.shared.error().is_some() {
            self.shared.process.lock().expect("lsp process").terminate();
        }
        let shared = self.shared.clone();
        self.stderr = Some(tokio::spawn(async move {
            let mut stream = stderr;
            let mut buffer = [0_u8; 4096];
            loop {
                match stream.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(count) => {
                        let mut tail = shared.stderr.lock().expect("lsp stderr");
                        let discard = tail
                            .len()
                            .saturating_add(count)
                            .saturating_sub(MAX_STDERR_BYTES);
                        tail.drain(..discard);
                        tail.extend(&buffer[..count]);
                    }
                    Err(error) => {
                        shared.fail(format!("language server stderr read failed: {error}"));
                        break;
                    }
                }
            }
        }));
    }

    pub(super) fn failure(&self) -> Option<String> {
        // Do not reap the leader while its process-group id is retained: an
        // exited, unreaped leader prevents PID reuse until Drop kills the group.
        // Closing stdout is observed by the reader and fails pending requests.
        self.shared.error()
    }

    pub(super) fn stderr_bytes(&self) -> usize {
        self.shared.stderr.lock().expect("lsp stderr").len()
    }

    pub(super) fn forget_document(&self, uri: &str) {
        self.shared
            .publications
            .lock()
            .expect("lsp publications")
            .remove(uri);
    }

    pub(super) async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        if let Some(error) = self.shared.error() {
            return Err(error);
        }
        write_frame(
            &self.writer,
            &json!({"jsonrpc":"2.0", "method":method, "params":params}),
        )
        .await
    }

    pub(super) async fn request(&mut self, method: &str, params: Value) -> Reply {
        if let Some(error) = self.failure() {
            return Err(error);
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("language server request id exhausted")?;
        let (sender, receiver) = oneshot::channel();
        // One request per exclusively leased client; never an unbounded queue.
        self.shared
            .pending
            .lock()
            .expect("lsp pending")
            .insert(id, sender);
        if let Err(error) = write_frame(
            &self.writer,
            &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
        )
        .await
        {
            self.shared.pending.lock().expect("lsp pending").remove(&id);
            return Err(error);
        }
        receiver
            .await
            .map_err(|_| "language server response channel closed".to_owned())?
    }

    pub(super) fn track_document(&self, uri: &str, reset: bool) -> Result<(), String> {
        let mut publications = self.shared.publications.lock().expect("lsp publications");
        if !publications.contains_key(uri) && publications.len() >= MAX_PUBLICATIONS {
            return Err(
                "language server open-document limit reached; retry after restarting the Harness"
                    .into(),
            );
        }
        if reset {
            publications.insert(uri.to_owned(), None);
        } else {
            publications.entry(uri.to_owned()).or_insert(None);
        }
        Ok(())
    }

    /// rust-analyzer's serverStatusNotification client capability enables
    /// experimental/serverStatus (not a server capability). In the fixed
    /// 2026-11-16 implementation, quiescent means project/VFS loading and cache
    /// priming have finished; initialize itself does not provide this barrier.
    /// The owning tool deadline bounds this notification wait and drops Rpc on
    /// timeout/cancellation. Retain status even when it precedes the waiter.
    pub(super) async fn rust_ready(&self) -> Result<u64, String> {
        loop {
            let changed = self.shared.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(error) = self.shared.error() {
                return Err(error);
            }
            let (status, revision) = {
                let readiness = self.shared.rust_status.lock().expect("lsp rust status");
                (readiness.status.clone(), readiness.revision)
            };
            if let Some(status) = status {
                if status.health == Health::Error
                    || (status.quiescent && status.health != Health::Ok)
                {
                    return Err(format!(
                        "rust-analyzer project readiness failed ({:?}, quiescent={}): {}",
                        status.health,
                        status.quiescent,
                        status.message.as_deref().unwrap_or("no status explanation")
                    ));
                }
                if status.quiescent && status.health == Health::Ok {
                    return Ok(revision);
                }
            }
            changed.await;
        }
    }

    pub(super) async fn publication(&self, uri: &str, version: i64) -> Result<Publication, String> {
        loop {
            // Register before inspecting state to avoid losing a publication
            // between inspection and await.
            let changed = self.shared.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(error) = self.shared.error() {
                return Err(error);
            }
            let publication = self
                .shared
                .publications
                .lock()
                .expect("lsp publications")
                .get(uri)
                .cloned()
                .flatten();
            if let Some(publication) = publication
                && publication
                    .version
                    .is_none_or(|published| published == version)
            {
                return Ok(publication);
            }
            changed.await;
        }
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        self.reader.abort();
        if let Some(task) = &self.stderr {
            task.abort();
        }
        self.shared.fail("language server client disposed".into());
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
    }
}

async fn read_loop(
    read: impl AsyncRead + Unpin,
    writer: Writer,
    shared: Arc<Shared>,
    language: Language,
    root_uri: String,
) -> Result<(), String> {
    let mut reader = BufReader::new(read);
    loop {
        let message = read_frame(&mut reader).await?;
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || !message.is_object() {
            return Err("language server sent an invalid JSON-RPC envelope".into());
        }
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            if let Some(id) = message.get("id") {
                if !id.is_string() && !id.is_number() {
                    return Err("language server request has invalid id".into());
                }
                let reply =
                    server_reply(id.clone(), method, &message["params"], language, &root_uri);
                // A blocked server request must not deadlock a query forever.
                tokio::time::timeout(Duration::from_secs(5), write_frame(&writer, &reply))
                    .await
                    .map_err(|_| "language server request reply timed out".to_owned())??;
            } else if language == Language::Rust && method == "experimental/serverStatus" {
                let mut status: ServerStatus = serde_json::from_value(message["params"].clone())
                    .map_err(|e| format!("invalid rust-analyzer readiness notification: {e}"))?;
                status.message = status
                    .message
                    .map(|message| super::short_text(&message, MAX_STATUS_MESSAGE_BYTES));
                {
                    let mut readiness = shared.rust_status.lock().expect("lsp rust status");
                    readiness.revision = readiness
                        .revision
                        .checked_add(1)
                        .ok_or("rust-analyzer readiness revision exhausted")?;
                    readiness.status = Some(status);
                }
                shared.changed.notify_waiters();
            } else if method == "textDocument/publishDiagnostics" {
                let params = &message["params"];
                if let (Some(uri), Some(diagnostics)) =
                    (params["uri"].as_str(), params["diagnostics"].as_array())
                {
                    let mut publications = shared.publications.lock().expect("lsp publications");
                    if let Some(slot) = publications.get_mut(uri) {
                        let mut kept = Vec::new();
                        let mut bytes = 0;
                        for diagnostic in diagnostics.iter().take(MAX_DIAGNOSTICS) {
                            let size = diagnostic.to_string().len();
                            if bytes + size > MAX_NOTIFICATION_BYTES {
                                break;
                            }
                            bytes += size;
                            kept.push(diagnostic.clone());
                        }
                        *slot = Some(Publication {
                            version: params.get("version").and_then(Value::as_i64),
                            truncated: kept.len() < diagnostics.len(),
                            diagnostics: kept,
                        });
                        shared.changed.notify_waiters();
                    }
                }
            }
        } else if let Some(id) = message.get("id").and_then(Value::as_u64) {
            let sender = shared.pending.lock().expect("lsp pending").remove(&id);
            if let Some(sender) = sender {
                let result = if let Some(error) = message.get("error") {
                    let code = error["code"].as_i64().unwrap_or(-32603);
                    let description = super::short_text(
                        error["message"].as_str().unwrap_or("unspecified error"),
                        1000,
                    );
                    Err(format!("language server error {code}: {description}"))
                } else if let Some(result) = message.get("result") {
                    Ok(result.clone())
                } else {
                    Err("language server response has neither result nor error".into())
                };
                let _ = sender.send(result);
            }
            // Responses with unknown ids are discarded, never consumed as the
            // next response. Cancellation retires the entire transport.
        } else {
            return Err("language server sent an invalid response id".into());
        }
    }
}

fn server_reply(
    id: Value,
    method: &str,
    params: &Value,
    language: Language,
    root_uri: &str,
) -> Value {
    let result = match method {
        "workspace/applyEdit" => {
            json!({"applied": false, "failureReason": "willdeep LSP is query-only; edits must use the reviewed file-change tools"})
        }
        "workspace/configuration" => {
            let Some(items) = params["items"].as_array() else {
                return json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32602,"message":"configuration items must be an array"}});
            };
            if items.len() > MAX_SERVER_REQUEST_ITEMS {
                return json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32602,"message":"too many configuration items"}});
            }
            let configuration = language.configuration();
            Value::Array(
                items
                    .iter()
                    .map(|item| {
                        let section = item["section"].as_str().unwrap_or("");
                        let section = section.strip_prefix("rust-analyzer.").unwrap_or(section);
                        if section.is_empty() || section == "rust-analyzer" {
                            return configuration.clone();
                        }
                        section
                            .split('.')
                            .try_fold(&configuration, |value, part| value.get(part))
                            .cloned()
                            .unwrap_or(Value::Null)
                    })
                    .collect(),
            )
        }
        "workspace/workspaceFolders" => json!([{"uri":root_uri, "name":"workspace"}]),
        "window/workDoneProgress/create"
        | "workspace/diagnostic/refresh"
        | "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/codeLens/refresh" => Value::Null,
        "window/showMessageRequest" => Value::Null,
        _ => {
            return json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"unsupported server request in query-only LSP client"}});
        }
    };
    json!({"jsonrpc":"2.0", "id":id, "result":result})
}

pub(super) async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<Value, String> {
    let mut header = Vec::new();
    loop {
        if header.len() >= MAX_HEADER_BYTES {
            return Err("language server frame header exceeds 8 KiB".into());
        }
        let byte = reader
            .read_u8()
            .await
            .map_err(|e| format!("language server stdout closed or frame read failed: {e}"))?;
        header.push(byte);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    if !header.is_ascii() {
        return Err("language server header is not ASCII".into());
    }
    let header = std::str::from_utf8(&header)
        .map_err(|_| "language server header is not ASCII".to_owned())?;
    let mut length = None;
    for line in header.split("\r\n").filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or("invalid language server header")?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err("duplicate language server Content-Length".into());
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| "invalid language server Content-Length")?,
            );
        }
    }
    let length = length.ok_or("language server frame has no Content-Length")?;
    if length > MAX_FRAME_BYTES {
        return Err("language server frame exceeds 2 MiB".into());
    }
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .await
        .map_err(|e| format!("language server frame body read failed: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid language server JSON: {e}"))
}

async fn write_frame(writer: &Writer, message: &Value) -> Result<(), String> {
    let bytes =
        serde_json::to_vec(message).map_err(|e| format!("encode language server message: {e}"))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err("outgoing language server frame exceeds 2 MiB".into());
    }
    let header = format!("Content-Length: {}\r\n\r\n", bytes.len());
    let mut writer = writer.lock().await;
    writer
        .write_all(header.as_bytes())
        .await
        .map_err(|e| format!("write language server header: {e}"))?;
    writer
        .write_all(&bytes)
        .await
        .map_err(|e| format!("write language server body: {e}"))?;
    writer
        .flush()
        .await
        .map_err(|e| format!("flush language server message: {e}"))
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
