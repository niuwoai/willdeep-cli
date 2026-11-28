//! Which sessions a live TUI is showing.
//!
//! The desktop shell posts a system notification when a task finishes or parks
//! on a human gate. A TUI that is open on that same session has already rung
//! its bell for the same moment, so on macOS and Linux the desktop stays quiet.
//! Windows has no reliable terminal bell, so it notifies either way.
//!
//! The daemon cannot see the TUI: the TUI is a separate process that only pulls
//! the event stream. So each TUI keeps its own record, one file per process,
//! holding a shared lock for as long as the process lives. The desktop adapter
//! reads them. A crash never runs the cleanup, but the kernel drops the lock
//! when the process dies, and a record whose lock can be taken exclusively is
//! a leftover and does not count.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use uuid::Uuid;

use super::DaemonPaths;

const DIR_NAME: &str = "tui-presence";

fn directory(home: &Path) -> PathBuf {
    DaemonPaths::new(home).directory.join(DIR_NAME)
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// One open TUI. Dropping it releases the lock and removes the record; a killed
/// process skips the removal, and the reader treats the unlocked file as stale.
pub(crate) struct TuiPresence {
    file: File,
    path: PathBuf,
    session: Uuid,
}

impl TuiPresence {
    pub(crate) fn acquire(home: &Path, session: Uuid) -> Result<Self> {
        let directory = directory(home);
        std::fs::create_dir_all(&directory).context("create the TUI presence directory")?;
        let path = directory.join(std::process::id().to_string());
        let file = options()
            .truncate(true)
            .open(&path)
            .context("open the TUI presence record")?;
        // Shared, so the reader can probe with an exclusive try-lock without
        // ever blocking the TUI. Rust opens with O_CLOEXEC, so a child process
        // does not keep the lock alive after this one exits.
        FileExt::lock_shared(&file).context("lock the TUI presence record")?;
        let mut presence = Self {
            file,
            path,
            session,
        };
        presence.write(session)?;
        Ok(presence)
    }

    /// Rewrites the record when the open session changes. A no-op when it has
    /// not, so the event loop can call this on every pass.
    pub(crate) fn set_session(&mut self, session: Uuid) -> Result<()> {
        if self.session == session {
            return Ok(());
        }
        self.write(session)?;
        self.session = session;
        Ok(())
    }

    fn write(&mut self, session: Uuid) -> Result<()> {
        use std::io::Seek;
        self.file
            .set_len(0)
            .context("clear the TUI presence record")?;
        self.file
            .rewind()
            .context("rewind the TUI presence record")?;
        self.file
            .write_all(format!("{session}\n").as_bytes())
            .context("write the TUI presence record")?;
        self.file
            .sync_all()
            .context("store the TUI presence record")?;
        Ok(())
    }
}

impl Drop for TuiPresence {
    fn drop(&mut self) {
        // Best effort. A failure here only leaves a file the reader ignores,
        // because the lock goes with the file handle.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A session counts as watched when some live TUI holds a record naming it.
/// A file that can be locked exclusively belongs to a dead process and is
/// removed; anything unreadable is treated as nobody watching, so the desktop
/// notifies instead of staying silent.
pub(crate) fn watching(home: &Path, session: Uuid) -> bool {
    let Ok(entries) = std::fs::read_dir(directory(home)) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| record_matches(&entry.path(), session))
}

fn record_matches(path: &Path, session: Uuid) -> bool {
    let Ok(file) = options().create(false).open(path) else {
        return false;
    };
    match FileExt::try_lock_exclusive(&file) {
        Ok(()) => {
            let _ = std::fs::remove_file(path);
            false
        }
        Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|contents| contents.trim().parse::<Uuid>().ok())
                == Some(session)
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let home = std::env::temp_dir().join(format!("tui-presence-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    #[test]
    fn a_held_record_marks_only_its_own_session() {
        let home = scratch();
        let session = Uuid::new_v4();
        let mut presence = TuiPresence::acquire(&home, session).unwrap();
        assert!(watching(&home, session));
        assert!(!watching(&home, Uuid::new_v4()));

        let other = Uuid::new_v4();
        presence.set_session(other).unwrap();
        assert!(!watching(&home, session));
        assert!(watching(&home, other));
        // Rewriting the same session must not touch the file again.
        presence.set_session(other).unwrap();
        assert!(watching(&home, other));

        drop(presence);
        assert!(!watching(&home, other));
    }

    #[test]
    fn two_live_tuis_keep_their_own_sessions() {
        let home = scratch();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut presence = TuiPresence::acquire(&home, first).unwrap();
        // A second record from the same process uses a distinct name here; the
        // real second process has its own pid and so its own file.
        let other_path = directory(&home).join("other");
        let mut other = options().open(&other_path).unwrap();
        FileExt::lock_shared(&other).unwrap();
        other.write_all(format!("{second}\n").as_bytes()).unwrap();

        assert!(watching(&home, first));
        assert!(watching(&home, second));

        presence.set_session(second).unwrap();
        assert!(watching(&home, second));
        drop(other);
        let _ = std::fs::remove_file(&other_path);
        assert!(watching(&home, second));
    }

    #[test]
    fn an_unlocked_or_broken_record_is_not_watching() {
        let home = scratch();
        let session = Uuid::new_v4();
        let directory = directory(&home);
        std::fs::create_dir_all(&directory).unwrap();
        let stale = directory.join("424242");
        std::fs::write(&stale, format!("{session}\n")).unwrap();
        assert!(!watching(&home, session));
        assert!(!stale.exists(), "a stale record is removed when read");

        let broken = directory.join("broken");
        std::fs::write(&broken, "not a session\n").unwrap();
        let held = options().open(&broken).unwrap();
        FileExt::lock_shared(&held).unwrap();
        assert!(!watching(&home, session));
    }
}
