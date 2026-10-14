//! Preserve independently named old keys only inside the same trusted origin.
use super::*;
use std::{
    fs::{self, OpenOptions},
    io::Read,
};
const MAX_TRUST_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct TrustCache {
    origin: String,
    keys: BTreeMap<String, String>,
}

pub(super) fn cache(
    home: &Path,
    origin: &reqwest::Url,
    incoming: &BTreeMap<String, String>,
) -> Result<()> {
    ensure!(
        incoming.iter().all(|(id, key)| !id.is_empty()
            && id.len() <= 128
            && !id.chars().any(char::is_control)
            && valid_hash(key)),
        "invalid agent signing key metadata"
    );
    let library = AgentLibrary::new(home);
    fs::create_dir_all(library.root())?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(library.root().join(".trusted-keys.lock"))?;
    lock.lock()?;
    let path = library.root().join("trusted-keys.json");
    let previous = match std::fs::File::open(&path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(MAX_TRUST_BYTES + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_TRUST_BYTES,
                "agent trust cache exceeds size limit"
            );
            Some(serde_json::from_slice::<TrustCache>(&bytes)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut keys = match previous {
        Some(previous) if Service::new(&previous.origin)?.origin == *origin => previous.keys,
        _ => BTreeMap::new(),
    };
    for (id, key) in incoming {
        ensure!(
            keys.get(id).is_none_or(|old| old == key),
            "agent signing key ID changed public key; trust cache preserved"
        );
        keys.insert(id.clone(), key.clone());
    }
    let bytes = serde_json::to_vec(&TrustCache {
        origin: origin.as_str().into(),
        keys,
    })?;
    ensure!(
        bytes.len() as u64 <= MAX_TRUST_BYTES,
        "agent trust cache exceeds size limit"
    );
    private_write(&path, &bytes)
}
