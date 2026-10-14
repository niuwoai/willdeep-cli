use super::*;
use crate::subagent::AgentRunStore;
use crate::usage_ledger::{Execution, append_record};
use std::io::Write;

struct Fixture {
    directory: PathBuf,
    parent: Uuid,
}

impl Fixture {
    fn new() -> Self {
        Self {
            directory: std::env::temp_dir().join(format!("team-ledger-{}", Uuid::new_v4())),
            parent: Uuid::new_v4(),
        }
    }

    fn record(&self, tokens: u64) -> UsageLedgerRecord {
        let mut record = UsageLedgerRecord::new(UsageKind::Main, Execution::Daemon);
        record.session_id = Some(self.parent);
        record.usage.total_tokens = Some(tokens);
        record
    }

    fn shard(&self) -> PathBuf {
        std::fs::read_dir(&self.directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
    }

    fn summary(&self, parent: Uuid) -> AgentTeamSummary {
        // Each API request constructs a fresh store, all share the ledger index.
        AgentRunStore::new(self.directory.join("workers"), parent)
            .team_summary(&self.directory)
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.directory.exists() {
            std::fs::remove_dir_all(&self.directory).unwrap();
        }
    }
}

#[test]
fn repeated_polls_and_other_teams_do_not_reread_unchanged_ledger() {
    let fixture = Fixture::new();
    let record = fixture.record(7);
    append_record(&fixture.directory, &record).unwrap();
    let mut cache = LedgerCache::default();
    cache.refresh(&fixture.directory).unwrap();
    let bytes = cache.bytes_read;
    for _ in 0..20 {
        cache.refresh(&fixture.directory).unwrap();
    }
    assert_eq!(
        cache.bytes_read, bytes,
        "polls must perform no ledger reads"
    );
    assert_eq!(fixture.summary(fixture.parent).usage.accounted_tokens, 7);
    let shared = shared(&fixture.directory).unwrap();
    let bytes = shared.lock().unwrap().bytes_read;
    assert_eq!(fixture.summary(Uuid::new_v4()).usage.model_calls, 0);
    assert_eq!(fixture.summary(fixture.parent).usage.model_calls, 1);
    assert_eq!(shared.lock().unwrap().bytes_read, bytes);
}

#[test]
fn append_reads_only_new_bytes_and_preserves_per_session_deduplication() {
    let fixture = Fixture::new();
    let record = fixture.record(7);
    append_record(&fixture.directory, &record).unwrap();
    let mut cache = LedgerCache::default();
    cache.refresh(&fixture.directory).unwrap();
    let before = cache.bytes_read;
    let first_len = std::fs::metadata(fixture.shard()).unwrap().len();
    append_record(&fixture.directory, &record).unwrap();
    let other = Uuid::new_v4();
    let mut foreign = record.clone();
    foreign.session_id = Some(other); // identical id in a different session is independent
    append_record(&fixture.directory, &foreign).unwrap();
    let next = fixture.record(13);
    append_record(&fixture.directory, &next).unwrap();
    cache.refresh(&fixture.directory).unwrap();
    let appended = std::fs::metadata(fixture.shard()).unwrap().len() - first_len;
    assert_eq!(cache.bytes_read - before, appended);
    assert_eq!(cache.sessions[&fixture.parent].total.accounted_tokens, 20);
    assert_eq!(cache.sessions[&fixture.parent].total.model_calls, 2);
    assert_eq!(cache.sessions[&other].total.accounted_tokens, 7);
}

#[test]
fn partial_line_is_retried_without_double_counting_completed_lines() {
    let fixture = Fixture::new();
    append_record(&fixture.directory, &fixture.record(7)).unwrap();
    let mut bytes = serde_json::to_vec(&fixture.record(13)).unwrap();
    bytes.push(b'\n');
    let split = bytes.len() / 2;
    let mut file = OpenOptions::new()
        .append(true)
        .open(fixture.shard())
        .unwrap();
    file.write_all(&bytes[..split]).unwrap();
    let partial = fixture.summary(fixture.parent);
    assert!(partial.ledger_incomplete);
    assert_eq!(partial.usage.accounted_tokens, 7);
    file.write_all(&bytes[split..]).unwrap();
    let complete = fixture.summary(fixture.parent);
    assert!(!complete.ledger_incomplete);
    assert_eq!(complete.usage.accounted_tokens, 20);
    assert_eq!(complete.usage.model_calls, 2);
}

#[test]
fn rotation_and_deletion_rebuild_totals_and_deduplication() {
    let fixture = Fixture::new();
    let record = fixture.record(7);
    append_record(&fixture.directory, &record).unwrap();
    let original = fixture.shard();
    let copy = fixture.directory.join("other.jsonl");
    std::fs::copy(&original, &copy).unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.accounted_tokens, 7);
    std::fs::remove_file(original).unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.accounted_tokens, 7);
    std::fs::remove_file(copy).unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.model_calls, 0);
    std::fs::remove_dir_all(&fixture.directory).unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.model_calls, 0);
    append_record(&fixture.directory, &record).unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.accounted_tokens, 7);
}

#[test]
fn replacement_and_truncation_discard_old_totals() {
    let fixture = Fixture::new();
    append_record(&fixture.directory, &fixture.record(111)).unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.accounted_tokens, 111);
    let path = fixture.shard();
    std::fs::write(&path, b"").unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.model_calls, 0);
    append_record(&fixture.directory, &fixture.record(3)).unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.accounted_tokens, 3);
    let previous = Stamp::for_path(&path).unwrap();
    // Atomic replacement can have the same size or grow; inode change must rebuild.
    let mut bytes = serde_json::to_vec(&fixture.record(999)).unwrap();
    bytes.push(b'\n');
    let replacement = fixture.directory.join("replacement.tmp");
    std::fs::write(&replacement, bytes).unwrap();
    // File replacement must be detected even when the modification time is
    // preserved; Windows creation-time tunneling can also preserve creation.
    OpenOptions::new()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(previous.modified))
        .unwrap();
    std::fs::rename(replacement, path).unwrap();
    #[cfg(any(unix, windows))]
    assert_ne!(
        previous.identity,
        Stamp::for_path(&fixture.shard()).unwrap().identity
    );
    assert_eq!(fixture.summary(fixture.parent).usage.accounted_tokens, 999);
}

#[test]
fn malformed_and_oversized_rows_keep_budget_fail_closed() {
    let fixture = Fixture::new();
    append_record(&fixture.directory, &fixture.record(7)).unwrap();
    let mut file = OpenOptions::new()
        .append(true)
        .open(fixture.shard())
        .unwrap();
    file.write_all(b"invalid json\n").unwrap();
    assert!(fixture.summary(fixture.parent).ledger_incomplete);
    append_record(&fixture.directory, &fixture.record(13)).unwrap();
    let summary = fixture.summary(fixture.parent);
    assert!(summary.ledger_incomplete);
    assert_eq!(summary.usage.accounted_tokens, 20);
    file.write_all(&vec![b'x'; MAX_LINE_BYTES + 1]).unwrap();
    assert!(fixture.summary(fixture.parent).ledger_incomplete);
    append_record(&fixture.directory, &fixture.record(17)).unwrap();
    let summary = fixture.summary(fixture.parent);
    assert!(summary.ledger_incomplete);
    assert_eq!(summary.usage.accounted_tokens, 20, "stop at oversized row");
}

#[test]
fn same_length_edit_is_detected_by_metadata() {
    let fixture = Fixture::new();
    let mut record = fixture.record(7);
    append_record(&fixture.directory, &record).unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.accounted_tokens, 7);
    let path = fixture.shard();
    let before = std::fs::metadata(&path).unwrap();
    record.usage.total_tokens = Some(3);
    let mut bytes = serde_json::to_vec(&record).unwrap();
    bytes.push(b'\n');
    assert_eq!(bytes.len() as u64, before.len());
    std::fs::write(&path, bytes).unwrap();
    let file = OpenOptions::new().write(true).open(&path).unwrap();
    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(before.modified().unwrap() + std::time::Duration::from_secs(1)),
    )
    .unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.accounted_tokens, 3);
}

// Retain the legacy full-scan semantics as a regression oracle.
fn legacy_usage(directory: &Path, parent: Uuid) -> (SessionUsage, bool) {
    let mut usage = SessionUsage::default();
    let mut incomplete = false;
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    paths.sort();
    for path in paths {
        let mut reader = BufReader::new(std::fs::File::open(path).unwrap());
        loop {
            let mut bytes = Vec::new();
            let size = (&mut reader)
                .take(MAX_LINE_BYTES as u64 + 1)
                .read_until(b'\n', &mut bytes)
                .unwrap();
            if size == 0 {
                break;
            }
            if size > MAX_LINE_BYTES || bytes.last() != Some(&b'\n') {
                incomplete = true;
                break;
            }
            match serde_json::from_slice::<UsageLedgerRecord>(&bytes) {
                Ok(record) if record.schema == SCHEMA && record.session_id == Some(parent) => {
                    usage.include(record)
                }
                Ok(_) => {}
                Err(_) => incomplete = true,
            }
        }
    }
    (usage, incomplete)
}

#[test]
fn cached_projection_matches_full_scan_for_all_call_kinds_and_unknown_usage() {
    let fixture = Fixture::new();
    for kind in [
        UsageKind::Main,
        UsageKind::Subagent,
        UsageKind::Compression,
        UsageKind::Auxiliary,
    ] {
        let mut record = fixture.record(7);
        record.kind = kind;
        record.run_id = Some(Uuid::new_v4());
        append_record(&fixture.directory, &record).unwrap();
        append_record(&fixture.directory, &record).unwrap();
        record.id = Uuid::new_v4();
        record.usage = Default::default();
        append_record(&fixture.directory, &record).unwrap();
        let (legacy, incomplete) = legacy_usage(&fixture.directory, fixture.parent);
        let summary = fixture.summary(fixture.parent);
        assert_eq!(summary.usage, legacy.total);
        assert_eq!(summary.parent_usage, legacy.main);
        assert_eq!(summary.child_usage, legacy.child);
        assert_eq!(summary.compression_usage, legacy.compression);
        assert_eq!(summary.auxiliary_usage, legacy.auxiliary);
        assert_eq!(summary.ledger_incomplete, incomplete);
        let cache = shared(&fixture.directory).unwrap();
        assert_eq!(
            cache.lock().unwrap().sessions[&fixture.parent].recorded,
            legacy.recorded
        );
    }
}

#[test]
#[ignore = "read-only benchmark of an explicitly supplied local ledger directory"]
fn local_ledger_read_only_benchmark() {
    let directory =
        PathBuf::from(std::env::var_os("WD_TEAM_LEDGER_BENCH_DIR").expect("ledger directory"));
    let parent = std::fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .find_map(|entry| {
            BufReader::new(std::fs::File::open(entry.path()).unwrap())
                .lines()
                .map_while(Result::ok)
                .find_map(|line| {
                    serde_json::from_str::<UsageLedgerRecord>(&line)
                        .ok()
                        .and_then(|record| record.session_id)
                })
        })
        .unwrap_or_else(Uuid::new_v4);
    let store = AgentRunStore::new(
        std::env::temp_dir().join(Uuid::new_v4().to_string()),
        parent,
    );
    let started = std::time::Instant::now();
    let (legacy, incomplete) = legacy_usage(&directory, parent);
    let old_elapsed = started.elapsed();
    let started = std::time::Instant::now();
    let summary = store.team_summary(&directory).unwrap();
    let cold_elapsed = started.elapsed();
    assert_eq!(summary.usage, legacy.total);
    assert_eq!(summary.ledger_incomplete, incomplete);
    let cache = shared(&directory).unwrap();
    let bytes_before = cache.lock().unwrap().bytes_read;
    let started = std::time::Instant::now();
    for _ in 0..100 {
        assert_eq!(store.team_summary(&directory).unwrap().usage, legacy.total);
    }
    let warm_elapsed = started.elapsed();
    let bytes_after = cache.lock().unwrap().bytes_read;
    assert_eq!(
        bytes_before, bytes_after,
        "unchanged local ledger must never be reread"
    );
    eprintln!(
        "{}",
        serde_json::json!({
            "legacy_full_scan_ms":old_elapsed.as_secs_f64()*1000.0,
            "incremental_cold_ms":cold_elapsed.as_secs_f64()*1000.0,
            "warm_100_polls_ms":warm_elapsed.as_secs_f64()*1000.0,
            "history_bytes_reread":bytes_after-bytes_before,
            "usage_matches":true
        })
    );
}

#[cfg(unix)]
#[test]
fn shard_symlinks_are_rejected_even_after_a_cache_hit() {
    let fixture = Fixture::new();
    append_record(&fixture.directory, &fixture.record(7)).unwrap();
    fixture.summary(fixture.parent);
    let path = fixture.shard();
    let saved = fixture.directory.join("saved.tmp");
    std::fs::rename(&path, &saved).unwrap();
    std::os::unix::fs::symlink(&saved, &path).unwrap();
    let store = AgentRunStore::new(fixture.directory.join("workers"), fixture.parent);
    assert!(store.team_summary(&fixture.directory).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(saved, path).unwrap();
    assert_eq!(fixture.summary(fixture.parent).usage.accounted_tokens, 7);
}

#[test]
fn concurrent_team_queries_share_one_initial_scan() {
    let fixture = Fixture::new();
    append_record(&fixture.directory, &fixture.record(7)).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let barrier = barrier.clone();
            let directory = fixture.directory.clone();
            let parent = fixture.parent;
            std::thread::spawn(move || {
                barrier.wait();
                AgentRunStore::new(directory.join("workers"), parent)
                    .team_summary(&directory)
                    .unwrap()
                    .usage
                    .accounted_tokens
            })
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), 7);
    }
    let cache = shared(&fixture.directory).unwrap();
    assert_eq!(
        cache.lock().unwrap().bytes_read,
        std::fs::metadata(fixture.shard()).unwrap().len()
    );
}

#[test]
fn many_teams_poll_large_ledger_without_reparsing_history() {
    let fixture = Fixture::new();
    std::fs::create_dir_all(&fixture.directory).unwrap();
    let mut bytes = Vec::new();
    for _ in 0..10_000 {
        serde_json::to_writer(&mut bytes, &fixture.record(7)).unwrap();
        bytes.push(b'\n');
    }
    std::fs::write(fixture.directory.join("fixture.jsonl"), &bytes).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(fixture.summary(fixture.parent).usage.model_calls, 10_000);
    let cold = started.elapsed();
    let started = std::time::Instant::now();
    for _ in 0..100 {
        assert_eq!(
            fixture.summary(fixture.parent).usage.accounted_tokens,
            70_000
        );
    }
    let warm = started.elapsed();
    let cache = shared(&fixture.directory).unwrap();
    assert_eq!(cache.lock().unwrap().bytes_read, bytes.len() as u64);
    eprintln!(
        "team ledger benchmark: {} bytes, cold={cold:?}, 100 unchanged polls={warm:?}, history bytes reread=0",
        bytes.len()
    );
}
