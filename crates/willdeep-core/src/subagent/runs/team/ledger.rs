//! Shared append-only projection: one scan per directory, not per team/poll.
//! Every query checks shard metadata; budgets never rely on a time-based TTL.
use super::{AgentTeamSummary, AgentUsageSummary, counted, merge, storage_error};
use crate::usage_ledger::{MAX_LINE_BYTES, SCHEMA, UsageKind, UsageLedgerRecord};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{Metadata, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use uuid::Uuid;

const MAX_DIRECTORIES: usize = 16;
type SharedCache = Arc<Mutex<LedgerCache>>;
type CacheRegistry = VecDeque<(PathBuf, SharedCache)>;

fn shared(directory: &Path) -> Result<SharedCache, String> {
    static CACHES: OnceLock<Mutex<CacheRegistry>> = OnceLock::new();
    let mut caches = CACHES
        .get_or_init(Default::default)
        .lock()
        .map_err(storage_error)?;
    if let Some(index) = caches.iter().position(|(path, _)| path == directory) {
        let entry = caches.remove(index).expect("cache index exists");
        let cache = entry.1.clone();
        caches.push_back(entry);
        return Ok(cache);
    }
    if caches.len() >= MAX_DIRECTORIES {
        caches.pop_front();
    }
    let cache = Arc::new(Mutex::new(LedgerCache::default()));
    caches.push_back((directory.to_owned(), cache.clone()));
    Ok(cache)
}

pub(super) fn read(
    directory: &Path,
    summary: &mut AgentTeamSummary,
) -> Result<BTreeMap<Uuid, u64>, String> {
    let cache = shared(directory)?;
    let mut cache = cache.lock().map_err(storage_error)?;
    if let Err(error) = cache.refresh(directory) {
        // Never keep a partially applied append after an IO failure.
        *cache = LedgerCache::default();
        return Err(error);
    }
    summary.ledger_incomplete |= cache.shards.values().any(|shard| shard.incomplete());
    let Some(usage) = cache.sessions.get(&summary.parent_session_id) else {
        return Ok(BTreeMap::new());
    };
    summary.usage = usage.total.clone();
    summary.parent_usage = usage.main.clone();
    summary.child_usage = usage.child.clone();
    summary.auxiliary_usage = usage.auxiliary.clone();
    summary.compression_usage = usage.compression.clone();
    Ok(usage.recorded.clone())
}

#[derive(Clone, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64),
}

impl Stamp {
    fn new(metadata: &Metadata) -> Result<Self, String> {
        if !metadata.is_file() {
            return Err("usage ledger shard is not a regular file".into());
        }
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified().map_err(storage_error)?,
            created: metadata.created().ok(),
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (metadata.dev(), metadata.ino())
            },
        })
    }

    fn can_append_to(&self, previous: &Self) -> bool {
        #[cfg(unix)]
        if self.identity != previous.identity {
            return false;
        }
        self.created == previous.created && (self == previous || self.len > previous.len)
    }
}

#[derive(Default)]
struct LedgerCache {
    shards: BTreeMap<PathBuf, Shard>,
    sessions: HashMap<Uuid, SessionUsage>,
    #[cfg(test)]
    bytes_read: u64,
}

impl LedgerCache {
    fn refresh(&mut self, directory: &Path) -> Result<(), String> {
        let files = match std::fs::read_dir(directory) {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                *self = Self::default();
                return Ok(());
            }
            Err(error) => return Err(storage_error(error)),
        };
        let mut stamps = BTreeMap::new();
        for entry in files {
            let path = entry.map_err(storage_error)?.path();
            if path.extension().is_some_and(|ext| ext == "jsonl") {
                let metadata = std::fs::symlink_metadata(&path).map_err(storage_error)?;
                stamps.insert(path, Stamp::new(&metadata)?);
            }
        }
        // Replacement, truncation, edits in place or deletion invalidate totals
        // and deduplication together, including duplicates across month shards.
        if self.shards.iter().any(|(path, shard)| {
            stamps
                .get(path)
                .is_none_or(|stamp| !stamp.can_append_to(&shard.stamp))
        }) {
            *self = Self::default();
        }
        for (path, stamp) in stamps {
            if self
                .shards
                .get(&path)
                .is_some_and(|shard| shard.stamp == stamp)
            {
                continue;
            }
            self.append_shard(path, stamp)?;
        }
        Ok(())
    }

    fn append_shard(&mut self, path: PathBuf, expected: Stamp) -> Result<(), String> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(&path).map_err(storage_error)?;
        let stamp = Stamp::new(&file.metadata().map_err(storage_error)?)?;
        if !stamp.can_append_to(&expected) {
            return Err("usage ledger shard changed while opening; retry the query".into());
        }
        let offset = self.shards.get(&path).map_or(0, |shard| shard.offset);
        file.seek(SeekFrom::Start(offset)).map_err(storage_error)?;
        // Bound to observed length so concurrent appends are consumed next poll.
        let mut reader = BufReader::new(file.take(stamp.len - offset));
        let shard = self.shards.entry(path).or_insert_with(|| Shard {
            stamp: stamp.clone(),
            offset: 0,
            damaged: false,
            partial: false,
        });
        shard.partial = false;
        let mut bytes = Vec::with_capacity(MAX_LINE_BYTES);
        loop {
            bytes.clear();
            let size = (&mut reader)
                .take(MAX_LINE_BYTES as u64 + 1)
                .read_until(b'\n', &mut bytes)
                .map_err(storage_error)?;
            #[cfg(test)]
            {
                self.bytes_read += size as u64;
            }
            if size == 0 {
                break;
            }
            if size > MAX_LINE_BYTES || bytes.last() != Some(&b'\n') {
                // Leave the cursor before an unfinished line, retry on append.
                shard.partial = true;
                break;
            }
            shard.offset += size as u64;
            let record: UsageLedgerRecord = match serde_json::from_slice(&bytes) {
                Ok(record) => record,
                Err(_) => {
                    shard.damaged = true;
                    continue;
                }
            };
            if record.schema == SCHEMA
                && let Some(parent) = record.session_id
            {
                self.sessions.entry(parent).or_default().include(record);
            }
        }
        shard.stamp = stamp;
        Ok(())
    }
}

struct Shard {
    stamp: Stamp,
    offset: u64,
    damaged: bool,
    partial: bool,
}

impl Shard {
    fn incomplete(&self) -> bool {
        self.damaged || self.partial
    }
}

#[derive(Default)]
struct SessionUsage {
    seen: HashSet<Uuid>,
    recorded: BTreeMap<Uuid, u64>,
    total: AgentUsageSummary,
    main: AgentUsageSummary,
    child: AgentUsageSummary,
    auxiliary: AgentUsageSummary,
    compression: AgentUsageSummary,
}

impl SessionUsage {
    fn include(&mut self, record: UsageLedgerRecord) {
        if !self.seen.insert(record.id) {
            return;
        }
        let usage = counted(&record.usage);
        if let Some(id) = record.run_id {
            let count = self.recorded.entry(id).or_insert(0);
            *count = count.saturating_add(1);
        }
        merge(&mut self.total, &usage);
        match record.kind {
            UsageKind::Main => merge(&mut self.main, &usage),
            UsageKind::Subagent => merge(&mut self.child, &usage),
            UsageKind::Auxiliary => merge(&mut self.auxiliary, &usage),
            UsageKind::Compression => merge(&mut self.compression, &usage),
        }
    }
}

#[cfg(test)]
mod tests;
