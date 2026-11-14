//! 启动时把过大的 `events.ndjson` 归档到 `backups/`，并重新开始空日志。

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::EventLogSettings;

const ARCHIVE_PREFIX: &str = "runtime-events-";

/// 在建立索引之前调用：超限则整文件移走并创建新的空 `events.ndjson`。
pub(super) fn maybe_rotate_before_open(
    path: &Path,
    home: &Path,
    settings: &EventLogSettings,
) -> Result<()> {
    if settings.rotate_bytes == 0 {
        return Ok(());
    }
    if !path.exists() {
        return Ok(());
    }
    let size = std::fs::metadata(path)
        .with_context(|| format!("stat Runtime event log at {}", path.display()))?
        .len();
    if size < settings.rotate_bytes {
        return Ok(());
    }
    // Keep the sequence in the new log. Both live clients and persisted TUI
    // bookmarks use it as a cursor; restarting it at 1 strands old readers.
    let head = last_sequence(path)?;
    let archive = archive_path(home, now_unix())?;
    std::fs::create_dir_all(
        archive
            .parent()
            .context("resolve Runtime event archive dir")?,
    )
    .with_context(|| format!("create {}", archive.parent().unwrap().display()))?;
    std::fs::rename(path, &archive)
        .with_context(|| format!("archive Runtime event log to {}", archive.display()))?;
    create_empty_log(path)?;
    if head > 0 {
        append_boundary(path, head)?;
    }
    if settings.retain_archives > 0 {
        prune_archives(archive.parent().unwrap(), settings.retain_archives)?;
    }
    eprintln!(
        "WillDeep Runtime: archived event log ({} bytes) to {}; started a new event log",
        size,
        archive.display()
    );
    Ok(())
}

/// Repair logs rotated by older versions, which restarted numbering at 1.
/// Run before another rotation prunes the archive holding the old cursor.
pub(super) fn resume_archived_cursor(
    path: &Path,
    home: &Path,
    settings: &EventLogSettings,
) -> Result<bool> {
    let backups = home.join("backups");
    if !backups.is_dir() {
        return Ok(false);
    }
    let mut archived_head = 0;
    for entry in std::fs::read_dir(&backups)
        .with_context(|| format!("read Runtime backups at {}", backups.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(ARCHIVE_PREFIX) && name.ends_with(".ndjson") {
            archived_head = archived_head.max(last_sequence(&entry.path())?);
        }
    }
    if archived_head == 0 {
        return Ok(false);
    }
    let first = match first_sequence(path) {
        Ok(first) => first,
        Err(error) if can_archive_damaged_log(&error, path, settings) => {
            return archive_damaged_log(path, &backups, archived_head);
        }
        Err(error) => return Err(error),
    };
    if let Some(first) = first {
        if first < archived_head {
            // Preserve already-written replies: an appended high boundary alone
            // would hide all earlier low-numbered events from a persisted cursor.
            if let Err(error) = renumber_active_log(path, archived_head.max(last_sequence(path)?)) {
                if can_archive_damaged_log(&error, path, settings) {
                    return archive_damaged_log(path, &backups, archived_head);
                }
                return Err(error);
            }
            return Ok(true);
        }
    } else if last_sequence(path)? < archived_head {
        create_empty_log(path)?;
        append_boundary(path, archived_head)?;
    }
    Ok(false)
}

fn can_archive_damaged_log(
    error: &anyhow::Error,
    path: &Path,
    settings: &EventLogSettings,
) -> bool {
    error.downcast_ref::<serde_json::Error>().is_some()
        && settings.rotate_bytes > 0
        && std::fs::metadata(path).is_ok_and(|metadata| metadata.len() >= settings.rotate_bytes)
}

fn archive_damaged_log(path: &Path, backups: &Path, archived_head: u64) -> Result<bool> {
    let head = archived_head.max(last_sequence(path)?);
    let archive = backups.join(format!(
        "{ARCHIVE_PREFIX}{}-{}.ndjson",
        now_unix(),
        uuid::Uuid::new_v4()
    ));
    std::fs::rename(path, &archive)
        .with_context(|| format!("archive damaged Runtime event log to {}", archive.display()))?;
    create_empty_log(path)?;
    append_boundary(path, head)?;
    eprintln!(
        "warning: Runtime event log contained a malformed event; preserved the original at {}",
        archive.display()
    );
    Ok(true)
}

fn first_sequence(path: &Path) -> Result<Option<u64>> {
    if !path.exists() {
        return Ok(None);
    }
    for line in BufReader::new(File::open(path)?).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            let event: super::RuntimeEvent = serde_json::from_str(&line)
                .with_context(|| format!("read first Runtime event from {}", path.display()))?;
            return Ok(Some(event.sequence));
        }
    }
    Ok(None)
}

fn renumber_active_log(path: &Path, previous_head: u64) -> Result<()> {
    let temp = path.with_file_name(format!("events-recover-{}.ndjson", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut output = BufWriter::new(options.open(&temp)?);
        let mut sequence = previous_head;
        for line in BufReader::new(File::open(path)?).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let mut event: super::RuntimeEvent = serde_json::from_str(&line)
                .with_context(|| format!("renumber Runtime event from {}", path.display()))?;
            sequence = sequence
                .checked_add(1)
                .context("Runtime event sequence overflow")?;
            event.sequence = sequence;
            serde_json::to_writer(&mut output, &event)?;
            output.write_all(b"\n")?;
        }
        output.flush()?;
        output.get_ref().sync_data()?;
        std::fs::rename(&temp, path).with_context(|| {
            format!("replace recovered Runtime event log at {}", path.display())
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn append_boundary(path: &Path, sequence: u64) -> Result<()> {
    let boundary = super::RuntimeEvent {
        sequence,
        timestamp: now_unix(),
        kind: "events.rotated".to_owned(),
        message: String::new(),
    };
    let mut line = serde_json::to_vec(&boundary)?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("open Runtime event cursor in {}", path.display()))?;
    file.write_all(&line)
        .with_context(|| format!("record Runtime event cursor in {}", path.display()))?;
    file.sync_data()
        .with_context(|| format!("sync Runtime event cursor in {}", path.display()))?;
    Ok(())
}

/// Read just the final complete event, even when an archive is very large.
fn last_sequence(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(0);
    }
    let mut window = 4096u64;
    loop {
        let start = len.saturating_sub(window);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        for line in bytes.rsplit(|byte| *byte == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            // The first fragment may start halfway through an event.
            if start > 0 && !bytes[..bytes.len() - line.len()].contains(&b'\n') {
                break;
            }
            match serde_json::from_slice::<super::RuntimeEvent>(line) {
                Ok(event) => return Ok(event.sequence),
                Err(error) => eprintln!(
                    "warning: skip malformed trailing Runtime event in {}: {error}",
                    path.display()
                ),
            }
        }
        if start == 0 {
            return Ok(0);
        }
        window = window.saturating_mul(2);
    }
}

fn create_empty_log(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let parent = path
        .parent()
        .context("Runtime event log has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create Runtime directory {}", parent.display()))?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("create Runtime event log at {}", path.display()))?;
    Ok(())
}

fn archive_path(home: &Path, timestamp: u64) -> Result<PathBuf> {
    Ok(home
        .join("backups")
        .join(format!("{ARCHIVE_PREFIX}{timestamp}.ndjson")))
}

fn prune_archives(backups: &Path, retain: u32) -> Result<()> {
    if retain == 0 {
        return Ok(());
    }
    let mut archives = Vec::new();
    if backups.is_dir() {
        for entry in std::fs::read_dir(backups)
            .with_context(|| format!("read Runtime backups at {}", backups.display()))?
        {
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.starts_with(ARCHIVE_PREFIX) && name.ends_with(".ndjson") {
                archives.push(entry.path());
            }
        }
    }
    archives.sort();
    while archives.len() > retain as usize {
        let oldest = archives.remove(0);
        if let Err(error) = std::fs::remove_file(&oldest) {
            eprintln!(
                "warning: could not remove old Runtime event archive {}: {error:#}",
                oldest.display()
            );
        }
    }
    Ok(())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_when_over_limit_and_starts_fresh_log() {
        let root = std::env::temp_dir().join(format!("wd-event-rotate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        let path = root.join("runtime/events.ndjson");
        let log = super::super::EventLog::open_for_tests(path.clone()).unwrap();
        log.append("task.completed", "done").unwrap();
        let old_head = log.latest_sequence();
        drop(log);
        let settings = EventLogSettings {
            rotate_bytes: 1,
            retain_archives: 3,
        };
        maybe_rotate_before_open(&path, &root, &settings).unwrap();
        let reopened = super::super::EventLog::open_for_tests(path.clone()).unwrap();
        assert_eq!(reopened.latest_sequence(), old_head);
        assert!(reopened.read_after(old_head, 10).unwrap().is_empty());
        assert_eq!(
            reopened.append("task.completed", "next").unwrap().sequence,
            old_head + 1
        );
        assert_eq!(
            reopened.read_after(old_head, 10).unwrap()[0].message,
            "next"
        );
        drop(reopened);
        let archive = std::fs::read_dir(root.join("backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(
            archive
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with(ARCHIVE_PREFIX)
        );
        assert!(std::fs::metadata(&archive).unwrap().len() > 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn restores_bookmark_from_a_log_already_rotated_by_an_older_version() {
        let root = std::env::temp_dir().join(format!("wd-event-legacy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        std::fs::create_dir_all(root.join("backups")).unwrap();
        let path = root.join("runtime/events.ndjson");
        let archive = root.join("backups/runtime-events-1.ndjson");
        let old = super::super::RuntimeEvent {
            sequence: 500,
            timestamp: 1,
            kind: "task.completed".into(),
            message: "previous task".repeat(500),
        };
        let current = super::super::RuntimeEvent {
            sequence: 1,
            timestamp: 2,
            kind: "task.completed".into(),
            message: "already saved".into(),
        };
        std::fs::write(
            &archive,
            format!("{}\n", serde_json::to_string(&old).unwrap()),
        )
        .unwrap();
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&current).unwrap()),
        )
        .unwrap();
        let settings = EventLogSettings {
            rotate_bytes: 0,
            retain_archives: 1,
        };
        let log = super::super::EventLog::open(path.clone(), &root, &settings).unwrap();
        assert_eq!(log.latest_sequence(), 501);
        assert_eq!(log.read_after(500, 10).unwrap()[0].message, "already saved");
        assert_eq!(
            log.append("task.output", "new answer").unwrap().sequence,
            502
        );
        assert_eq!(log.read_after(500, 10).unwrap().len(), 2);
        drop(log);
        let reopened = super::super::EventLog::open(path, &root, &settings).unwrap();
        assert_eq!(reopened.latest_sequence(), 502);
        assert_eq!(reopened.read_after(500, 10).unwrap().len(), 2);
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovers_saved_events_before_pruning_the_only_high_archive() {
        let root = std::env::temp_dir().join(format!("wd-event-prune-{}", uuid::Uuid::new_v4()));
        let path = root.join("runtime/events.ndjson");
        let backups = root.join("backups");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&backups).unwrap();
        let old = super::super::RuntimeEvent {
            sequence: 500,
            timestamp: 1,
            kind: "task.output".into(),
            message: "old".into(),
        };
        let saved = super::super::RuntimeEvent {
            sequence: 1,
            timestamp: 2,
            kind: "task.output".into(),
            message: "saved during old rotation".into(),
        };
        std::fs::write(
            backups.join("runtime-events-1.ndjson"),
            format!("{}\n\n", serde_json::to_string(&old).unwrap()),
        )
        .unwrap();
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&saved).unwrap()),
        )
        .unwrap();
        let settings = EventLogSettings {
            rotate_bytes: 1,
            retain_archives: 1,
        };
        let log = super::super::EventLog::open(path.clone(), &root, &settings).unwrap();
        assert_eq!(log.latest_sequence(), 501);
        assert_eq!(
            log.append("task.output", "new answer").unwrap().sequence,
            502
        );
        let recovered = log.read_after(500, 10).unwrap();
        assert_eq!(recovered[0].message, "saved during old rotation");
        assert_eq!(recovered[1].message, "new answer");
        drop(log);
        let reopened = super::super::EventLog::open(path, &root, &settings).unwrap();
        assert_eq!(
            reopened
                .append("task.output", "after restart")
                .unwrap()
                .sequence,
            503
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn damaged_legacy_log_keeps_its_archive_and_the_old_cursor() {
        let root =
            std::env::temp_dir().join(format!("wd-event-bad-legacy-{}", uuid::Uuid::new_v4()));
        let path = root.join("runtime/events.ndjson");
        let backups = root.join("backups");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&backups).unwrap();
        let old = super::super::RuntimeEvent {
            sequence: 500,
            timestamp: 1,
            kind: "task.output".into(),
            message: "old".into(),
        };
        let current = super::super::RuntimeEvent {
            sequence: 1,
            timestamp: 2,
            kind: "task.output".into(),
            message: "current".into(),
        };
        std::fs::write(
            backups.join("runtime-events-1.ndjson"),
            format!("{}\n", serde_json::to_string(&old).unwrap()),
        )
        .unwrap();
        let original = format!("{}\nmalformed\n", serde_json::to_string(&current).unwrap());
        std::fs::write(&path, &original).unwrap();
        let settings = EventLogSettings {
            rotate_bytes: 1,
            retain_archives: 1,
        };
        let log = super::super::EventLog::open(path.clone(), &root, &settings).unwrap();
        assert_eq!(
            log.append("task.output", "after recovery")
                .unwrap()
                .sequence,
            501
        );
        drop(log);
        assert!(
            std::fs::read_dir(&backups)
                .unwrap()
                .any(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap() == original)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_log_with_malformed_tail_still_rotates_and_keeps_the_original() {
        let root = std::env::temp_dir().join(format!("wd-event-damaged-{}", uuid::Uuid::new_v4()));
        let path = root.join("runtime/events.ndjson");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let old = super::super::RuntimeEvent {
            sequence: 500,
            timestamp: 1,
            kind: "task.output".into(),
            message: "saved".into(),
        };
        let original = format!("{}\nmalformed\n", serde_json::to_string(&old).unwrap());
        std::fs::write(&path, &original).unwrap();
        let settings = EventLogSettings {
            rotate_bytes: 1,
            retain_archives: 1,
        };
        let log = super::super::EventLog::open(path.clone(), &root, &settings).unwrap();
        assert_eq!(
            log.append("task.output", "after recovery")
                .unwrap()
                .sequence,
            501
        );
        drop(log);
        let archive = std::fs::read_dir(root.join("backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read_to_string(archive).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_when_under_limit() {
        let root =
            std::env::temp_dir().join(format!("wd-event-rotate-skip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        let path = root.join("runtime/events.ndjson");
        std::fs::write(&path, b"small").unwrap();
        let settings = EventLogSettings {
            rotate_bytes: 10_000,
            retain_archives: 1,
        };
        maybe_rotate_before_open(&path, &root, &settings).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 5);
        assert!(!root.join("backups").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
