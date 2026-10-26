//! Verified production exports are streamed from an opened, hashed file handle.
//! No arbitrary paths, whole-video JSON buffers or cached acceptance bypasses.
use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

const PLUGIN: &str = "willdeep-video-studio";
const CHUNK_BYTES: usize = 64 * 1024;

#[derive(Deserialize, Default)]
pub(super) struct Options {
    #[serde(default)]
    download: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileProof {
    filename: String,
    file_path: PathBuf,
    sha256: String,
    bytes: u64,
    mime_type: String,
}

fn invalid() -> PluginWebError {
    PluginWebError::BadRequest("production_delivery_invalid".into())
}

pub(super) async fn serve(
    State(state): State<Arc<PluginWebState>>,
    Path((workflow, selector)): Path<(Uuid, String)>,
    Query(options): Query<Options>,
    headers: HeaderMap,
) -> Result<Response, PluginWebError> {
    if !["manifest", "records"].contains(&selector.as_str())
        && !selector.strip_prefix("episode-").is_some_and(|order| {
            order
                .parse::<u16>()
                .is_ok_and(|number| (1..=80).contains(&number) && number.to_string() == order)
        })
    {
        return Err(invalid());
    }
    if state.host.approval_gap(PLUGIN).await?.is_some() {
        return Err(invalid());
    }
    let workspaces = state.workspaces.read().map_err(|_| invalid())?.clone();
    let runtime = crate::plugin_production::control(
        &state.home,
        &workspaces,
        None,
        json!({"operation":"inspect","arguments":{"id":workflow}}),
    )
    .await
    .map_err(|_| invalid())?;
    let current = &runtime["workflow"];
    if current["status"] != "completed" || current["usage"]["unknown_requests"] != json!([]) {
        return Err(invalid());
    }
    // mcp() enforces enabled state. The plugin's file tool obtains a
    // fresh workspace-scoped Runtime snapshot and revalidates domain acceptance.
    let mcp = state.host.mcp(PLUGIN).await?;
    let raw = mcp
        .call_tool_on(
            "video-studio",
            "production.file",
            json!({"id":workflow,"selector":selector}),
        )
        .await
        .map_err(|_| invalid())?;
    let payload: Value = raw["content"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|entry| {
            entry["text"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
        })
        .ok_or_else(invalid)?;
    if payload["ok"] != true {
        return Err(invalid());
    }
    let proof: FileProof = serde_json::from_value(payload).map_err(|_| invalid())?;
    let delivery = &current["domain"]["delivery"];
    let (sha, bytes, filename, mime) = if selector == "manifest" {
        (
            &delivery["manifestSHA256"],
            &delivery["manifestBytes"],
            &delivery["manifestFilename"],
            "application/json",
        )
    } else if selector == "records" {
        (
            &delivery["records"]["sha256"],
            &delivery["records"]["bytes"],
            &delivery["records"]["filename"],
            "application/json",
        )
    } else {
        let order: u16 = selector[8..].parse().map_err(|_| invalid())?;
        let entry = delivery["episodes"]
            .as_array()
            .and_then(|entries| entries.iter().find(|entry| entry["episodeOrder"] == order))
            .ok_or_else(invalid)?;
        (
            &entry["sha256"],
            &entry["bytes"],
            &entry["filename"],
            "video/mp4",
        )
    };
    if sha.as_str() != Some(&proof.sha256)
        || bytes.as_u64() != Some(proof.bytes)
        || filename.as_str() != Some(&proof.filename)
        || proof.mime_type != mime
    {
        return Err(invalid());
    }
    let directory = state
        .home
        .join("plugin-data")
        .join(PLUGIN)
        .join("production-exports")
        .join(workflow.to_string());
    let (file, proof) = tokio::task::spawn_blocking(move || verified_file(&directory, proof))
        .await
        .map_err(|_| invalid())??;
    stream_file(file, proof, options.download, &headers)
}

fn stream_file(
    mut file: std::fs::File,
    proof: FileProof,
    download: bool,
    headers: &HeaderMap,
) -> Result<Response, PluginWebError> {
    let range = match byte_range(
        headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok()),
        proof.bytes,
    ) {
        Some(range) => range,
        None => {
            return Ok((
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{}", proof.bytes))],
            )
                .into_response());
        }
    };
    file.seek(SeekFrom::Start(range.0)).map_err(|_| invalid())?;
    let length = range.1 - range.0 + 1;
    let stream = futures_util::stream::try_unfold(
        (tokio::fs::File::from_std(file), length),
        |(mut file, remaining)| async move {
            if remaining == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            let mut chunk = vec![0; remaining.min(CHUNK_BYTES as u64) as usize];
            let read = file.read(&mut chunk).await?;
            if read == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
            }
            chunk.truncate(read);
            Ok(Some((chunk, (file, remaining - read as u64))))
        },
    );
    let disposition = if download { "attachment" } else { "inline" };
    let mut response = Response::builder()
        .status(if headers.contains_key(header::RANGE) {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(header::CONTENT_TYPE, proof.mime_type)
        .header(header::CONTENT_LENGTH, length)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(
            header::CONTENT_DISPOSITION,
            format!("{disposition}; filename=\"{}\"", proof.filename),
        )
        .header(header::ETAG, format!("\"{}\"", proof.sha256))
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if headers.contains_key(header::RANGE) {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", range.0, range.1, proof.bytes),
        );
    }
    let mut response = response
        .body(Body::from_stream(stream))
        .map_err(|_| invalid())?;
    apply_sandbox_cors(&mut response, headers);
    Ok(response)
}

fn verified_file(
    directory: &FsPath,
    proof: FileProof,
) -> Result<(std::fs::File, FileProof), PluginWebError> {
    let canonical = proof.file_path.canonicalize().map_err(|_| invalid())?;
    let root = directory.canonicalize().map_err(|_| invalid())?;
    if canonical.parent() != Some(root.as_path())
        || directory.ancestors().take(4).any(|path| {
            std::fs::symlink_metadata(path).map_or(true, |meta| meta.file_type().is_symlink())
        })
        || canonical.file_name().and_then(|name| name.to_str()) != Some(&proof.filename)
        || proof.bytes == 0
        || proof.sha256.len() != 64
        || !proof.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        || !["application/json", "video/mp4"].contains(&proof.mime_type.as_str())
        || proof.filename.contains(['/', '\\', '"', '\r', '\n'])
    {
        return Err(invalid());
    }
    let mut file = std::fs::File::open(canonical).map_err(|_| invalid())?;
    if file.metadata().map_err(|_| invalid())?.len() != proof.bytes {
        return Err(invalid());
    }
    let mut digest = Sha256::new();
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(|_| invalid())?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if format!("{:x}", digest.finalize()) != proof.sha256 {
        return Err(invalid());
    }
    Ok((file, proof))
}

fn byte_range(value: Option<&str>, bytes: u64) -> Option<(u64, u64)> {
    if bytes == 0 {
        return None;
    }
    let Some(value) = value else {
        return Some((0, bytes - 1));
    };
    let (first, last) = value.strip_prefix("bytes=")?.split_once('-')?;
    if first.is_empty() {
        let suffix = last.parse::<u64>().ok()?;
        return (suffix > 0).then(|| (bytes.saturating_sub(suffix), bytes - 1));
    }
    let first = first.parse::<u64>().ok()?;
    let last = if last.is_empty() {
        bytes - 1
    } else {
        last.parse::<u64>().ok()?.min(bytes - 1)
    };
    (first <= last && first < bytes).then_some((first, last))
}

#[cfg(test)]
#[path = "production_delivery_browser_fixture.rs"]
mod browser_fixture;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_support_playback_and_reject_ambiguous_or_invalid_requests() {
        assert_eq!(byte_range(None, 10), Some((0, 9)));
        assert_eq!(byte_range(Some("bytes=2-5"), 10), Some((2, 5)));
        assert_eq!(byte_range(Some("bytes=8-"), 10), Some((8, 9)));
        assert_eq!(byte_range(Some("bytes=-3"), 10), Some((7, 9)));
        for value in [
            "bytes=10-",
            "bytes=5-2",
            "bytes=0-1,3-4",
            "bytes=-0",
            "other=0-2",
        ] {
            assert_eq!(byte_range(Some(value), 10), None);
        }
    }
    #[test]
    fn file_proofs_require_the_exact_export_directory_and_current_bytes() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("production-download-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("episode.mp4");
        std::fs::write(&path, b"fixture-only").unwrap();
        let proof = || FileProof {
            filename: "episode.mp4".into(),
            file_path: path.clone(),
            sha256: format!("{:x}", Sha256::digest(b"fixture-only")),
            bytes: 12,
            mime_type: "video/mp4".into(),
        };
        assert!(verified_file(&root, proof()).is_ok());
        assert!(verified_file(root.parent().unwrap(), proof()).is_err());
        std::fs::write(&path, b"changed-data").unwrap();
        assert!(verified_file(&root, proof()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn a_real_http_download_and_playback_range_keep_the_accepted_bytes() {
        use futures_util::StreamExt;
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("production-stream-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("fixture.mp4");
        let expected: Vec<_> = (0..CHUNK_BYTES * 3 + 17)
            .map(|index| (index % 251) as u8)
            .collect();
        std::fs::write(&path, &expected).unwrap();
        let proof = FileProof {
            filename: "fixture.mp4".into(),
            file_path: path,
            sha256: format!("{:x}", Sha256::digest(&expected)),
            bytes: expected.len() as u64,
            mime_type: "video/mp4".into(),
        };
        let test_root = root.clone();
        let test_proof = proof.clone();
        let app = Router::new().route(
            "/fixture",
            get(move |headers: HeaderMap| {
                let root = test_root.clone();
                let proof = test_proof.clone();
                async move {
                    let (file, proof) = verified_file(&root, proof)?;
                    stream_file(file, proof, true, &headers)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client
            .get(format!("http://{address}/fixture"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_DISPOSITION],
            "attachment; filename=\"fixture.mp4\""
        );
        let destination = root.join("downloaded.mp4");
        let mut target = tokio::fs::File::create(&destination).await.unwrap();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            tokio::io::AsyncWriteExt::write_all(&mut target, &chunk.unwrap())
                .await
                .unwrap();
        }
        drop(target);
        assert_eq!(std::fs::read(&destination).unwrap(), expected);
        let response = client
            .get(format!("http://{address}/fixture"))
            .header(header::RANGE, "bytes=65530-65545")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.bytes().await.unwrap().as_ref(),
            &expected[65530..=65545]
        );
        std::fs::write(&proof.file_path, b"changed").unwrap();
        assert_eq!(
            client
                .get(format!("http://{address}/fixture"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        server.abort();
        std::fs::remove_dir_all(root).unwrap();
    }
}
