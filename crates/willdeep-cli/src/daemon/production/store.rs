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
        let workflow = decode_workflow(&std::fs::read(self.root.join(format!("{id}.json")))?)
            .context("read production state")?;
        anyhow::ensure!(
            workflow.schema == SCHEMA_VERSION && workflow.id == id,
            "production_archive_identity_mismatch"
        );
        Ok(workflow)
    }

    fn put(&self, workflow: &ProductionWorkflow) -> Result<()> {
        let bytes = encode_workflow(workflow)?;
        super::super::write_serialized_atomic(
            &self.root.join(format!("{}.json", workflow.id)),
            &bytes,
        )
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

#[cfg(test)]
mod codec_tests {
    use super::*;

    struct Directory(PathBuf);

    impl Drop for Directory {
        fn drop(&mut self) {
            if let Err(error) = std::fs::remove_dir_all(&self.0) {
                eprintln!("remove isolated production codec test directory: {error}");
            }
        }
    }

    fn fixture() -> (Directory, Store, ProductionWorkflow) {
        let root =
            Directory(std::env::temp_dir().join(format!("production-codec-{}", Uuid::new_v4())));
        std::fs::create_dir_all(&root.0).unwrap();
        let store = Store::new(&root.0);
        let limits = [
            (ProductionUnit::ModelCalls, Some(8)),
            (ProductionUnit::Tokens, Some(100)),
            (ProductionUnit::Images, None),
            (ProductionUnit::TtsCharacters, None),
            (ProductionUnit::MusicCalls, None),
            (ProductionUnit::MusicSeconds, None),
            (ProductionUnit::VideoCalls, None),
            (ProductionUnit::VideoSeconds, None),
        ]
        .into_iter()
        .collect();
        let workflow = store
            .create(
                Uuid::new_v4(),
                ProductionStart {
                    brief: Some("fixture".into()),
                    source_screenplay_id: None,
                    workspace: root.0.clone(),
                    spec: SeriesSpec::default(),
                    limits,
                    max_model_calls_per_episode: None,
                    origin_session_id: None,
                },
            )
            .unwrap();
        (root, store, workflow)
    }

    #[test]
    fn codec_keeps_the_existing_archive_bytes_and_private_file_permissions() {
        let (_root, store, workflow) = fixture();
        let path = store.root.join(format!("{}.json", workflow.id));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            serde_json::to_vec_pretty(&workflow).unwrap()
        );
        assert_eq!(store.get(workflow.id).unwrap(), workflow);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn codec_still_rejects_mismatched_archive_identity() {
        let (_root, store, mut workflow) = fixture();
        let original = workflow.id;
        workflow.id = Uuid::new_v4();
        std::fs::write(
            store.root.join(format!("{original}.json")),
            serde_json::to_vec_pretty(&workflow).unwrap(),
        )
        .unwrap();
        assert!(
            store
                .get(original)
                .unwrap_err()
                .to_string()
                .contains("production_archive_identity_mismatch")
        );
    }
}
