//! 启动时把过大的 `events.ndjson` 归档到 `backups/`，并重新开始空日志。

use std::fs::OpenOptions;
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
    let archive = archive_path(home, now_unix())?;
    std::fs::create_dir_all(archive.parent().context("resolve Runtime event archive dir")?)
        .with_context(|| format!("create {}", archive.parent().unwrap().display()))?;
    std::fs::rename(path, &archive)
        .with_context(|| format!("archive Runtime event log to {}", archive.display()))?;
    create_empty_log(path)?;
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
        for entry in std::fs::read_dir(backups).with_context(|| {
            format!("read Runtime backups at {}", backups.display())
        })? {
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
        let root =
            std::env::temp_dir().join(format!("wd-event-rotate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("runtime")).unwrap();
        let path = root.join("runtime/events.ndjson");
        let payload = "x".repeat(4096);
        std::fs::write(&path, payload).unwrap();
        let settings = EventLogSettings {
            rotate_bytes: 1024,
            retain_archives: 3,
        };
        maybe_rotate_before_open(&path, &root, &settings).unwrap();
        assert!(path.exists());
        assert!(std::fs::metadata(&path).unwrap().len() <= 1);
        let archive = std::fs::read_dir(root.join("backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(archive.file_name().unwrap().to_str().unwrap().starts_with(ARCHIVE_PREFIX));
        assert!(std::fs::metadata(&archive).unwrap().len() > 1024);
        std::fs::remove_dir_all(&root).unwrap();
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
