//! The browser never chooses an output path. The host validates and writes PNGs.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[path = "artifact_directory.rs"]
mod directory;
use directory::Directory;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;

pub(super) fn private_directory(parent: &Path, prefix: &str) -> Result<PathBuf, String> {
    let path = parent.join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
    let builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = builder;
        builder.mode(0o700);
        builder
    };
    builder
        .create(&path)
        .map_err(|e| format!("create private browser directory: {e}"))?;
    Ok(path)
}

pub(super) struct Artifacts {
    directory: Arc<Directory>,
}

// The future and blocking writer share only this short creation lock. It is
// never held during write/sync, so cancellation can unlink a blocked writer's
// output without waiting for that IO or for any Tokio runtime to remain alive.
struct PendingState {
    directory: Arc<Directory>,
    name: String,
    cancelled: AtomicBool,
    creation: Mutex<()>,
}

pub(super) struct PendingArtifact {
    state: Arc<PendingState>,
    metadata: Value,
    committed: bool,
}

impl PendingArtifact {
    pub(super) fn metadata(&self) -> &Value {
        &self.metadata
    }

    pub(super) fn commit(mut self) -> Result<(), String> {
        self.state.directory.validate_path()?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PendingArtifact {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        self.state.cancelled.store(true, Ordering::SeqCst);
        let state = self.state.clone();
        // Dedicated OS thread, not a runtime task or blocking-pool task. Runtime
        // shutdown and an indefinitely blocked write cannot strand a named file.
        super::cleanup::dispatch(move || {
            let _creation = state.creation.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(error) = state.directory.unlink_file(&state.name) {
                eprintln!("{error}");
            }
        });
    }
}

impl Artifacts {
    pub(super) fn new(root: &Path) -> Result<Self, String> {
        Ok(Self {
            directory: Arc::new(Directory::new(root)?),
        })
    }

    pub(super) async fn save(&self, encoded: String) -> Result<PendingArtifact, String> {
        self.save_using(encoded, |file, bytes| {
            file.write_all(bytes).and_then(|()| file.sync_all())
        })
        .await
    }

    async fn save_using(
        &self,
        encoded: String,
        write: impl FnOnce(&mut std::fs::File, &[u8]) -> std::io::Result<()> + Send + 'static,
    ) -> Result<PendingArtifact, String> {
        let mut pending = PendingArtifact {
            state: Arc::new(PendingState {
                directory: self.directory.clone(),
                name: format!("screenshot-{}.png", uuid::Uuid::new_v4()),
                cancelled: AtomicBool::new(false),
                creation: Mutex::new(()),
            }),
            metadata: Value::Null,
            committed: false,
        };
        let state = pending.state.clone();
        pending.metadata = tokio::task::spawn_blocking(move || {
            let bytes = decode_base64(&encoded)?;
            let (width, height) = png_dimensions(&bytes)?;
            let mut file = {
                let _creation = state.creation.lock().map_err(|_| "browser artifact creation lock poisoned")?;
                if state.cancelled.load(Ordering::SeqCst) { return Err("browser screenshot cancelled before creation".into()); }
                state.directory.validate_path()?;
                state.directory.create_file(&state.name)?
            };
            write(&mut file, &bytes).map_err(|e| format!("write browser screenshot: {e}"))?;
            if state.cancelled.load(Ordering::SeqCst) { return Err("browser screenshot cancelled during IO".into()); }
            state.directory.validate_path()?;
            Ok::<_, String>(json!({
                "path":state.directory.path.join(&state.name), "sha256":format!("sha256:{:x}", Sha256::digest(&bytes)),
                "mime":"image/png", "width":width, "height":height, "bytes":bytes.len()
            }))
        }).await.map_err(|e| format!("browser screenshot writer failed: {e}"))??;
        Ok(pending)
    }
}

fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), String> {
    if bytes.len() < 33 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[8..16] != b"\0\0\0\rIHDR" {
        return Err("browser screenshot is not a PNG with an IHDR header".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().map_err(|_| "invalid PNG width")?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().map_err(|_| "invalid PNG height")?);
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err("browser screenshot dimensions are outside 1..4096".into());
    }
    Ok((width, height))
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    if input.is_empty()
        || input.len() > MAX_PNG_BYTES.div_ceil(3) * 4
        || !input.len().is_multiple_of(4)
    {
        return Err("browser PNG data exceeds its limit or has invalid base64 length".into());
    }
    let mut result = Vec::with_capacity(input.len() / 4 * 3);
    let chunks = input.as_bytes().chunks_exact(4);
    let count = chunks.len();
    for (index, chunk) in chunks.enumerate() {
        let mut sextets = [0_u8; 4];
        let padding = chunk.iter().rev().take_while(|&&b| b == b'=').count();
        if padding > 2 || (padding != 0 && index + 1 != count) {
            return Err("browser PNG base64 padding is invalid".into());
        }
        for i in 0..4 - padding {
            sextets[i] = match chunk[i] {
                b'A'..=b'Z' => chunk[i] - b'A',
                b'a'..=b'z' => chunk[i] - b'a' + 26,
                b'0'..=b'9' => chunk[i] - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => return Err("browser PNG base64 alphabet is invalid".into()),
            };
        }
        if (padding == 2 && sextets[1] & 15 != 0) || (padding == 1 && sextets[2] & 3 != 0) {
            return Err("browser PNG base64 has noncanonical trailing bits".into());
        }
        result.push((sextets[0] << 2) | (sextets[1] >> 4));
        if padding < 2 {
            result.push((sextets[1] << 4) | (sextets[2] >> 2));
        }
        if padding == 0 {
            result.push((sextets[2] << 6) | sextets[3]);
        }
    }
    if result.len() > MAX_PNG_BYTES {
        return Err("browser PNG exceeds 8 MiB".into());
    }
    Ok(result)
}

#[cfg(all(test, unix))]
#[path = "artifact_safety_tests.rs"]
mod safety_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_malformed_png_base64_and_oversized_dimensions() {
        for invalid in ["", "A===", "AA=A", "AB==", "AAAA====", "!!!!"] {
            assert!(decode_base64(invalid).is_err(), "{invalid}");
        }
        assert_eq!(decode_base64("TWE=").unwrap(), b"Ma");
        assert!(png_dimensions(b"not png").is_err());
        let mut header = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        header.extend(5000_u32.to_be_bytes());
        header.extend(720_u32.to_be_bytes());
        header.resize(33, 0);
        assert!(png_dimensions(&header).is_err());
    }
}
