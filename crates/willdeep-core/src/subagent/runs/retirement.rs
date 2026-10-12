use super::*;

impl AgentRunStore {
    pub fn from_dispatch(home: &Path, agent: Uuid) -> Result<Option<Self>, String> {
        Ok(super::super::dispatch_store::load(home, agent)
            .map_err(storage_error)?
            .and_then(|dispatch| {
                dispatch
                    .parent_session
                    .map(|parent| Self::new(home, parent))
            }))
    }

    pub(crate) fn ensure_agent_available(&self, agent: Uuid) -> Result<(), String> {
        match std::fs::symlink_metadata(self.directory.join(format!(".retired-{agent}"))) {
            Ok(_) => Err(
                "The Agent worktree was merged or archived; create a new Agent for further work."
                    .into(),
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(storage_error(error)),
        }
    }

    /// Shares admission's lock: a reviewed merge cannot race a queued continuation.
    pub fn retire_agent(&self, agent: Uuid) -> Result<bool, String> {
        self.locked(|| {
            if self
                .entries()?
                .iter()
                .any(|entry| entry.record.agent_id == agent && entry.record.is_pending())
            {
                return Err(
                    "Agent has an unfinished Run; stop or await it before merging its worktree"
                        .into(),
                );
            }
            let path = self.directory.join(format!(".retired-{agent}"));
            if std::fs::symlink_metadata(&path).is_ok() {
                return Ok(false);
            }
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(&path).map_err(storage_error)?;
            file.sync_all().map_err(storage_error)?;
            #[cfg(unix)]
            std::fs::File::open(&self.directory)
                .and_then(|file| file.sync_all())
                .map_err(storage_error)?;
            Ok(true)
        })
    }

    pub fn restore_retired_agent(&self, agent: Uuid) -> Result<(), String> {
        self.locked(|| {
            std::fs::remove_file(self.directory.join(format!(".retired-{agent}")))
                .map_err(storage_error)?;
            #[cfg(unix)]
            std::fs::File::open(&self.directory)
                .and_then(|file| file.sync_all())
                .map_err(storage_error)?;
            Ok(())
        })
    }
}
