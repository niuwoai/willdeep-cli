use super::*;
use std::fs::{File, OpenOptions};

pub(super) struct Store {
    root: PathBuf,
}

impl Store {
    pub(super) fn new(home: &Path) -> Self {
        Self {
            root: home.join("runtime/screenplays"),
        }
    }

    pub(super) fn lock(&self, id: Uuid, execution: bool) -> Result<File> {
        std::fs::create_dir_all(&self.root)?;
        let suffix = if execution { "execution" } else { "state" };
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(format!("{id}.{suffix}.lock")))?;
        if execution {
            lock.try_lock()
                .map_err(|_| anyhow::anyhow!("screenplay is busy"))?;
        } else {
            lock.lock().context("lock screenplay state")?;
        }
        Ok(lock)
    }

    pub(super) fn get(&self, id: Uuid) -> Result<ScreenplayWorkflow> {
        let path = self.root.join(format!("{id}.json"));
        serde_json::from_slice(&std::fs::read(path)?).context("read screenplay state")
    }

    pub(super) fn put(&self, workflow: &ScreenplayWorkflow) -> Result<()> {
        super::super::write_json_atomic(&self.root.join(format!("{}.json", workflow.id)), workflow)
    }

    pub(super) fn save_response(&self, workflow: Uuid, call: Uuid, response: &Value) -> Result<()> {
        let directory = self.root.join(workflow.to_string());
        std::fs::create_dir_all(&directory)?;
        super::super::write_json_atomic(&directory.join(format!("{call}.json")), response)
    }

    pub(super) fn response(&self, workflow: Uuid, call: Uuid) -> Result<Value> {
        let path = self
            .root
            .join(workflow.to_string())
            .join(format!("{call}.json"));
        serde_json::from_slice(&std::fs::read(path)?).context("read saved model response")
    }

    pub(super) fn reconcile(&self, id: Uuid) -> Result<()> {
        self.update(id, |workflow| {
            for call in workflow
                .calls
                .iter_mut()
                .filter(|call| call.state != "completed")
            {
                match self.response(id, call.id) {
                    Ok(response) => host::settle(call, &response)?,
                    Err(error)
                        if error
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        })
    }

    pub(super) fn update<T>(
        &self,
        id: Uuid,
        edit: impl FnOnce(&mut ScreenplayWorkflow) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.lock(id, false)?;
        let mut workflow = self.get(id)?;
        let result = edit(&mut workflow)?;
        workflow.updated_at = super::super::now();
        self.put(&workflow)?;
        Ok(result)
    }

    pub(super) fn list(&self) -> Result<Vec<ScreenplayWorkflow>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut values = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            // An unreadable archive is never silently replaced with an empty workflow.
            values.push(serde_json::from_slice(&std::fs::read(path)?)?);
        }
        Ok(values)
    }
}
