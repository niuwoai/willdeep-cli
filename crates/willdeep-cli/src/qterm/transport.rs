use super::State;
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_FILE_SIZE: u64 = 65_536;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    protocol_version: u32,
    #[serde(rename = "tabID")]
    tab_id: Uuid,
    root: PathBuf,
    token: String,
}

pub(super) struct Endpoint {
    descriptor: Descriptor,
    grant: Option<Value>,
    sequence: u64,
    session: Option<Uuid>,
}

impl Endpoint {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let descriptor: Descriptor = serde_json::from_slice(&read_private(path)?)?;
        if descriptor.protocol_version != 1
            || descriptor.token.len() != 72
            || !descriptor.root.is_absolute()
        {
            return Err(io::Error::other("unsupported QTerm status context"));
        }
        Ok(Self {
            descriptor,
            grant: None,
            sequence: 0,
            session: None,
        })
    }

    pub(super) fn report(&mut self, state: &State) -> io::Result<()> {
        if self.session != Some(state.context.session_id) {
            self.grant = None;
        }
        if self.grant.is_none() {
            let response = self
                .request(json!({"action":"bind_native_agent", "agentKind":"willdeep",
                "nativeAgentToken": self.descriptor.token}))?;
            let grant = response
                .get("agentGrant")
                .filter(|grant| {
                    grant.get("token").and_then(Value::as_str).is_some()
                        && grant.get("runID").and_then(Value::as_str).is_some()
                        && grant.get("instanceID").and_then(Value::as_str).is_some()
                })
                .ok_or_else(|| io::Error::other("invalid QTerm status grant"))?;
            self.grant = Some(grant.clone());
            self.session = Some(state.context.session_id);
            self.sequence = 0;
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("status sequence exhausted"))?;
        let mut report = serde_json::to_value(state)?;
        for field in ["token", "runID", "instanceID"] {
            report[field] = self.grant.as_ref().unwrap()[field].clone();
        }
        report["sequence"] = self.sequence.into();
        // A retired grant is never rebound automatically: a newer WillDeep owns this terminal now.
        self.request(json!({"action":"report_agent_state", "agentReport": report}))
            .map(|_| ())
    }

    fn request(&self, mut request: Value) -> io::Result<Value> {
        let id = Uuid::new_v4().to_string();
        request["id"] = id.clone().into();
        request["tab"] = self.descriptor.tab_id.to_string().into();
        let pending = self
            .descriptor
            .root
            .join("requests")
            .join(format!("{id}.pending"));
        let published = pending.with_extension("json");
        let response = self
            .descriptor
            .root
            .join("responses")
            .join(format!("{id}.json"));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options.open(&pending)?;
            file.write_all(&serde_json::to_vec(&request)?)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&pending, &published)?;
            let deadline = Instant::now() + REQUEST_TIMEOUT;
            loop {
                match read_private(&response) {
                    Ok(data) => {
                        let response: Value = serde_json::from_slice(&data)?;
                        if response["id"] != id || response["ok"] != true {
                            return Err(io::Error::other("QTerm status request rejected"));
                        }
                        return Ok(response);
                    }
                    Err(error)
                        if error.kind() == io::ErrorKind::NotFound && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(20))
                    }
                    Err(error) => return Err(error),
                }
            }
        })();
        for path in [&pending, &published, &response] {
            if let Err(error) = fs::remove_file(path)
                && error.kind() != io::ErrorKind::NotFound
            {
                return Err(io::Error::other("QTerm status request cleanup failed"));
            }
        }
        result
    }
}

fn read_private(path: &Path) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file: File = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_SIZE {
        return Err(io::Error::other("invalid QTerm status file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::getuid() } || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::other("unsafe QTerm status file permissions"));
        }
    }
    let mut data = Vec::new();
    file.take(MAX_FILE_SIZE + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_FILE_SIZE {
        return Err(io::Error::other("oversized QTerm status file"));
    }
    Ok(data)
}
