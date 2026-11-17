//! Always-allow commits publish memory at the atomic replacement boundary.
//!
//! The stable sidecar lock coordinates registries and processes using this protocol.
//! Writers in another client that ignore the lock are outside this guarantee.

use std::collections::HashSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::tools::{ToolError, ToolRegistry, rule_carries_credentials};

const LOCK_WAIT_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(20);

impl ToolRegistry {
    pub(in crate::tools) fn load_always_allowed(path: &Path) -> Result<HashSet<String>, ToolError> {
        Ok(LockedStore::acquire(path)?.update(None)?.into_rules())
    }

    pub(super) async fn persist_always_allowed(
        &self,
        signature: String,
    ) -> Result<PersistenceReceipt, ToolError> {
        if rule_carries_credentials(&signature) {
            return Err(ToolError::ApprovalDenied(
                "cannot remember a rule carrying credentials".to_owned(),
            ));
        }
        let Some(path) = &self.always_allow_path else {
            self.always_allowed
                .lock()
                .expect("always allow rules")
                .insert(signature);
            return Ok(PersistenceReceipt::MemoryOnly);
        };
        // Dropping this future while waiting cannot leave a detached writer.
        // Once acquired, there is no await through replacement and publication:
        // cancellation after that boundary cannot undo an already active rule.
        let store = LockedStore::acquire_async(path, LOCK_WAIT_TIMEOUT).await?;
        // Merge only this approved signature, never a stale in-memory snapshot.
        let receipt = store
            .update(Some(&signature))?
            .publish(&self.always_allowed);
        // Hold the disk lock through publication, even when the rename committed
        // but its directory sync failed. Such a rule is already active on disk.
        drop(store);
        Ok(receipt)
    }
}

struct LockedStore<'a> {
    path: &'a Path,
    lock: File,
}

impl<'a> LockedStore<'a> {
    fn open(path: &'a Path) -> io::Result<Self> {
        let parent = store_parent(path);
        std::fs::create_dir_all(parent)
            .map_err(|error| store_io(path, "create parent for", error))?;
        let mut lock_name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing store filename"))?
            .to_os_string();
        lock_name.push(".lock");
        let mut options = private_options();
        options.read(true).create(true).truncate(false);
        let lock = options
            .open(parent.join(lock_name))
            .map_err(|error| store_io(path, "open lock for", error))?;
        // Never unlink this file: every participant must lock the same inode,
        // even while the JSON store is replaced or a process exits unexpectedly.
        Ok(Self { path, lock })
    }

    fn try_acquire(&self) -> io::Result<bool> {
        match self.lock.try_lock() {
            Ok(()) => Ok(true),
            Err(TryLockError::WouldBlock) => Ok(false),
            Err(TryLockError::Error(error)) => Err(store_io(self.path, "lock", error)),
        }
    }

    fn lock_timeout(&self) -> io::Error {
        store_io(
            self.path,
            "lock (not committed)",
            io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for the store lock",
            ),
        )
    }

    fn acquire(path: &'a Path) -> io::Result<Self> {
        let store = Self::open(path)?;
        let deadline = Instant::now() + LOCK_WAIT_TIMEOUT;
        loop {
            if store.try_acquire()? {
                return Ok(store);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(store.lock_timeout());
            }
            std::thread::sleep(LOCK_RETRY_INTERVAL.min(remaining));
            if Instant::now() >= deadline {
                return Err(store.lock_timeout());
            }
        }
    }

    async fn acquire_async(path: &'a Path, wait: Duration) -> io::Result<Self> {
        let store = Self::open(path)?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if store.try_acquire()? {
                return Ok(store);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(store.lock_timeout());
            }
            tokio::time::sleep(LOCK_RETRY_INTERVAL.min(remaining)).await;
            if tokio::time::Instant::now() >= deadline {
                return Err(store.lock_timeout());
            }
        }
    }

    fn update(&self, addition: Option<&str>) -> Result<StoreUpdate, ToolError> {
        self.update_with_replace(addition, |path, bytes| {
            atomic_replace(path, |file| file.write_all(bytes))
        })
    }

    fn update_with_replace(
        &self,
        addition: Option<&str>,
        replace: impl FnOnce(&Path, &[u8]) -> io::Result<PersistenceReceipt>,
    ) -> Result<StoreUpdate, ToolError> {
        let stored = self.read()?;
        let mut rules: HashSet<String> = stored
            .iter()
            .filter(|rule| !rule_carries_credentials(rule))
            .cloned()
            .collect();
        let pruned = stored.iter().any(|rule| rule_carries_credentials(rule));
        if let Some(signature) = addition {
            rules.insert(signature.to_owned());
        }
        let commit = if pruned || addition.is_some() {
            let mut sorted: Vec<&String> = rules.iter().collect();
            sorted.sort();
            let bytes = serde_json::to_vec_pretty(&sorted).map_err(|error| {
                ToolError::Network(format!("encode always-allow store: {error}"))
            })?;
            replace(self.path, &bytes)
                .map_err(|error| store_io(self.path, "commit (not committed)", error))?
        } else {
            PersistenceReceipt::Durable
        };
        Ok(StoreUpdate { rules, commit })
    }

    fn read(&self) -> Result<Vec<String>, ToolError> {
        let file = match File::open(self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(store_io(self.path, "read", error).into()),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = file
                .metadata()
                .map_err(|error| store_io(self.path, "inspect permissions for", error))?;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "always-allow store must have 0600 permissions",
                )
                .into());
            }
        }
        serde_json::from_reader(file).map_err(|error| {
            // Serde data errors can quote the rejected value. Report location
            // and category without echoing a credential from an invalid store.
            ToolError::Network(format!(
                "invalid always-allow store {}: {:?} at line {}, column {}",
                self.path.display(),
                error.classify(),
                error.line(),
                error.column()
            ))
        })
    }
}

struct StoreUpdate {
    rules: HashSet<String>,
    commit: PersistenceReceipt,
}

impl StoreUpdate {
    fn into_rules(self) -> HashSet<String> {
        // Startup pruning has the same commit boundary as an explicit addition.
        // A post-commit warning must not hide the rules that are already on disk.
        self.commit.warn_if_unconfirmed();
        self.rules
    }

    fn publish(self, memory: &Mutex<HashSet<String>>) -> PersistenceReceipt {
        // A failed post-rename sync cannot roll back a visible replacement.
        // Publish the actual disk state and return its successful commit receipt.
        *memory.lock().expect("always allow rules") = self.rules;
        self.commit
    }
}

/// An Err from persistence always means the approved addition was not committed.
/// A visible replacement returns Ok, even when crash durability is unconfirmed.
#[must_use = "report any committed durability warning"]
#[derive(Debug)]
pub(super) enum PersistenceReceipt {
    MemoryOnly,
    Durable,
    DurabilityUnconfirmed(io::Error),
}

impl PersistenceReceipt {
    fn durability_warning(&self) -> Option<serde_json::Value> {
        let Self::DurabilityUnconfirmed(error) = self else {
            return None;
        };
        Some(serde_json::json!({
            "level": "warn",
            "event": "always_allow_durability_unconfirmed",
            "committed": true,
            "active": true,
            "durability": "unconfirmed",
            "error": error.to_string(),
        }))
    }

    pub(super) fn warn_if_unconfirmed(self) {
        if let Some(warning) = self.durability_warning() {
            // Do not include the rule: it may carry private command arguments.
            eprintln!("{warning}");
        }
    }
}

fn store_io(path: &Path, operation: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("{operation} always-allow store {}: {error}", path.display()),
    )
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn store_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn atomic_replace(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<PersistenceReceipt> {
    // Open the directory before creating a temporary or replacing the store.
    // Keep this handle for both the preflight and post-commit durability checks.
    #[cfg(unix)]
    let parent = File::open(store_parent(path))?;
    atomic_replace_with_sync(path, write, || {
        #[cfg(unix)]
        parent.sync_all()?;
        Ok(())
    })
}

fn atomic_replace_with_sync(
    path: &Path,
    write: impl FnOnce(&mut File) -> io::Result<()>,
    mut sync_parent: impl FnMut() -> io::Result<()>,
) -> io::Result<PersistenceReceipt> {
    // Catch an unsupported or denied directory sync before any authorization
    // changes. A second sync can still fail after a successful replacement.
    sync_parent()?;
    let temporary = store_parent(path).join(format!(".always-allow-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = private_options();
    options.write(true).create_new(true);
    // Open outside the cleanup scope: a create_new collision is not our file.
    let mut file = options.open(&temporary)?;
    let result: io::Result<()> = (|| {
        let written = write(&mut file).and_then(|()| file.sync_all());
        drop(file);
        written?;
        replace_file(&temporary, path)
    })();
    if let Err(error) = result {
        // Only clean our uncommitted temporary; deleting a rule or the old
        // destination would not roll back a failed store transaction.
        if let Err(cleanup) = std::fs::remove_file(&temporary)
            && cleanup.kind() != io::ErrorKind::NotFound
        {
            return Err(io::Error::new(
                error.kind(),
                format!("always-allow commit failed: {error}; temporary cleanup failed: {cleanup}"),
            ));
        }
        return Err(error);
    }
    // Rename is the commit boundary. Do not attempt cleanup/rollback after it.
    Ok(match sync_parent() {
        Ok(()) => PersistenceReceipt::Durable,
        Err(error) => PersistenceReceipt::DurabilityUnconfirmed(io::Error::new(
            error.kind(),
            format!(
                "always-allow store {} was committed and is active, but directory durability could not be confirmed: {error}",
                path.display()
            ),
        )),
    })
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };
    let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // Windows has no portable directory fsync. Request write-through replacement
    // instead; both UTF-16 buffers stay alive and terminated throughout the call.
    let replaced = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
#[path = "always_allow/tests.rs"]
mod tests;
