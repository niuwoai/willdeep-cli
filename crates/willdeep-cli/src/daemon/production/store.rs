use super::*;
use std::fs::{File, OpenOptions};

pub(super) struct Store {
    root: PathBuf,
    home: PathBuf,
}

impl Store {
    pub(super) fn new(home: &Path) -> Self {
        Self {
            root: home.join("runtime/productions"),
            home: home.to_path_buf(),
        }
    }

    pub(super) fn lock(&self, id: Uuid, execution: bool) -> Result<File> {
        std::fs::create_dir_all(&self.root)?;
        let suffix = if execution { "execution" } else { "state" };
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(self.root.join(format!("{id}.{suffix}.lock")))?;
        if execution {
            file.try_lock().context("production_busy")?;
        } else {
            file.lock().context("lock production state")?;
        }
        Ok(file)
    }

    pub(super) fn get(&self, id: Uuid) -> Result<ProductionWorkflow> {
        let workflow: ProductionWorkflow =
            serde_json::from_slice(&std::fs::read(self.root.join(format!("{id}.json")))?)
                .context("read production state")?;
        anyhow::ensure!(
            workflow.schema == SCHEMA_VERSION && workflow.id == id,
            "production_archive_identity_mismatch"
        );
        Ok(workflow)
    }

    fn put(&self, workflow: &ProductionWorkflow) -> Result<()> {
        super::super::write_json_atomic(&self.root.join(format!("{}.json", workflow.id)), workflow)
    }

    pub(super) fn create(
        &self,
        id: Uuid,
        mut input: ProductionStart,
    ) -> Result<ProductionWorkflow> {
        input.validate().map_err(anyhow::Error::msg)?;
        input.workspace = std::fs::canonicalize(input.workspace)?;
        anyhow::ensure!(
            input.workspace.is_dir(),
            "production_workspace_not_directory"
        );
        if let Some(session) = input.origin_session_id {
            let source = willdeep_core::SessionStore::new(&self.home).load(session)?;
            anyhow::ensure!(
                source.workspace.canonicalize()? == input.workspace && session != id,
                "production_origin_workspace_mismatch"
            );
        }
        let _lock = self.lock(id, false)?;
        match self.get(id) {
            Ok(existing) => {
                anyhow::ensure!(existing.input == input, "production_request_id_conflict");
                return Ok(existing);
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
            Err(error) => return Err(error),
        }
        let at = super::super::now();
        let workflow = ProductionWorkflow {
            schema: SCHEMA_VERSION,
            id,
            nodes: initial_nodes(&input.spec),
            input,
            status: ProductionStatus::Running,
            created_at: at,
            updated_at: at,
            cancel_requested: false,
            pause_requested: false,
            music_deferred: false,
            control_history: vec![],
            generations: vec![],
            budget_changes: vec![],
            domain: None,
            error: None,
        };
        self.put(&workflow)?;
        Ok(workflow)
    }

    pub(super) fn update<T>(
        &self,
        id: Uuid,
        edit: impl FnOnce(&mut ProductionWorkflow) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.lock(id, false)?;
        let mut workflow = self.get(id)?;
        let result = edit(&mut workflow)?;
        workflow.updated_at = super::super::now();
        self.put(&workflow)?;
        Ok(result)
    }

    pub(super) fn list(&self) -> Result<Vec<ProductionWorkflow>> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut values = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .context("production_archive_name")?
                .parse::<Uuid>()?;
            values.push(self.get(id)?);
        }
        Ok(values)
    }

    pub(super) fn save_receipt(
        &self,
        id: Uuid,
        request: Uuid,
        receipt: &serde_json::Value,
    ) -> Result<()> {
        let directory = self.root.join(id.to_string());
        std::fs::create_dir_all(&directory)?;
        super::super::write_json_atomic(&directory.join(format!("{request}.json")), receipt)
    }

    pub(super) fn receipt(&self, id: Uuid, request: Uuid) -> Result<Option<serde_json::Value>> {
        let path = self
            .root
            .join(id.to_string())
            .join(format!("{request}.json"));
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}
