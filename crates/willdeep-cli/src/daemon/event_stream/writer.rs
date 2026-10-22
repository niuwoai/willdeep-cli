//! Group commit preview events without blocking the Provider's token reader.
//! Subscribers only see events after the batch has reached durable storage.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Instant;

const GROUP_COMMIT_WINDOW: Duration = Duration::from_millis(16);
const GROUP_COMMIT_LIMIT: usize = 64;
const WRITE_QUEUE_CAPACITY: usize = 1024;

type Ack<T> = mpsc::Sender<std::result::Result<T, String>>;

enum Command {
    Append(RuntimeEvent, Option<Ack<()>>),
    Read(u64, usize, Ack<Vec<RuntimeEvent>>),
    Flush(Ack<()>),
}

pub(super) struct Writer {
    tx: SyncSender<Command>,
    committed: Arc<AtomicU64>,
    failure: Arc<Mutex<Option<String>>>,
    #[cfg(test)]
    commits: Arc<AtomicU64>,
    #[cfg(test)]
    reads: Arc<AtomicU64>,
}

impl Writer {
    pub(super) fn start(
        path: PathBuf,
        live: tokio::sync::broadcast::Sender<RuntimeEvent>,
        head: u64,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(WRITE_QUEUE_CAPACITY);
        let committed = Arc::new(AtomicU64::new(head));
        let failure = Arc::new(Mutex::new(None));
        let worker_head = committed.clone();
        let worker_failure = failure.clone();
        let commits = Arc::new(AtomicU64::new(0));
        let worker_commits = commits.clone();
        let reads = Arc::new(AtomicU64::new(0));
        let worker_reads = reads.clone();
        std::thread::Builder::new()
            .name("runtime-event-writer".into())
            .spawn(move || {
                run(
                    path,
                    live,
                    rx,
                    worker_head,
                    worker_failure,
                    (worker_commits, worker_reads),
                );
            })
            .context("start Runtime event writer")?;
        Ok(Self {
            tx,
            committed,
            failure,
            #[cfg(test)]
            commits,
            #[cfg(test)]
            reads,
        })
    }

    pub(super) fn append(&self, event: RuntimeEvent, preview: bool) -> Result<()> {
        self.check()?;
        if preview {
            self.tx
                .send(Command::Append(event, None))
                .context("queue Runtime preview event")?;
            return Ok(());
        }
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Command::Append(event, Some(tx)))
            .context("queue Runtime event")?;
        receive(rx)
    }

    pub(super) fn read(&self, after: u64, limit: usize) -> Result<Vec<RuntimeEvent>> {
        self.check()?;
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(Command::Read(after, limit, tx))
            .context("queue Runtime event read")?;
        receive(rx)
    }

    pub(super) fn head(&self) -> u64 {
        self.committed.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(super) fn commits(&self) -> u64 {
        self.commits.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(super) fn reads(&self) -> u64 {
        self.reads.load(Ordering::Acquire)
    }

    fn check(&self) -> Result<()> {
        if let Some(error) = self
            .failure
            .lock()
            .map_err(|_| anyhow::anyhow!("event writer lock poisoned"))?
            .as_ref()
        {
            anyhow::bail!("Runtime event persistence failed: {error}");
        }
        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Command::Flush(tx)).is_ok() {
            let _ = receive(rx);
        }
    }
}

fn receive<T>(rx: Receiver<std::result::Result<T, String>>) -> Result<T> {
    rx.recv()
        .context("Runtime event writer stopped")?
        .map_err(anyhow::Error::msg)
}

fn run(
    path: PathBuf,
    live: tokio::sync::broadcast::Sender<RuntimeEvent>,
    rx: Receiver<Command>,
    head: Arc<AtomicU64>,
    failure: Arc<Mutex<Option<String>>>,
    statistics: (Arc<AtomicU64>, Arc<AtomicU64>),
) {
    let (commits, reads) = statistics;
    let mut deferred = None;
    while let Some(command) = deferred.take().or_else(|| rx.recv().ok()) {
        match command {
            Command::Append(event, ack) => {
                let mut urgent = ack.is_some();
                let mut batch = vec![(event, ack)];
                let deadline = Instant::now() + GROUP_COMMIT_WINDOW;
                while !urgent && batch.len() < GROUP_COMMIT_LIMIT {
                    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(Command::Append(event, ack)) => {
                            urgent = ack.is_some();
                            batch.push((event, ack));
                        }
                        Ok(other) => {
                            deferred = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                let existing_failure = failure.lock().ok().and_then(|value| value.clone());
                let result = existing_failure.map_or_else(|| commit(&path, &batch), Err);
                if let Err(error) = &result {
                    if let Ok(mut slot) = failure.lock() {
                        *slot = Some(error.clone());
                    }
                    eprintln!("Runtime event persistence failed: {error}");
                } else {
                    commits.fetch_add(1, Ordering::Release);
                    head.store(batch.last().unwrap().0.sequence, Ordering::Release);
                    for (event, _) in &batch {
                        let _ = live.send(event.clone());
                    }
                }
                for (_, ack) in batch {
                    if let Some(ack) = ack {
                        let _ = ack.send(result.clone());
                    }
                }
            }
            Command::Read(after, limit, ack) => {
                reads.fetch_add(1, Ordering::Release);
                let error = failure.lock().ok().and_then(|value| value.clone());
                let result = error.map_or_else(
                    || read_events(&path, after, limit).map_err(|error| error.to_string()),
                    Err,
                );
                let _ = ack.send(result);
            }
            Command::Flush(ack) => {
                let error = failure.lock().ok().and_then(|value| value.clone());
                let _ = ack.send(error.map_or(Ok(()), Err));
            }
        }
    }
}

fn commit(
    path: &Path,
    batch: &[(RuntimeEvent, Option<Ack<()>>)],
) -> std::result::Result<(), String> {
    (|| -> Result<()> {
        let mut bytes = Vec::new();
        for (event, _) in batch {
            serde_json::to_writer(&mut bytes, event)?;
            bytes.push(b'\n');
        }
        let mut file = append_log(path)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.sync_data()?;
        Ok(())
    })()
    .map_err(|error| error.to_string())
}
