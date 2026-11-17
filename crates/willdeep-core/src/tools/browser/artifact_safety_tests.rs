use super::*;
use std::time::Duration;

const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQEBAOHN7x8AAAAASUVORK5CYII=";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(private_directory(&std::env::temp_dir(), "browser-artifact-safety").unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("remove artifact safety fixture: {error}");
        }
    }
}

async fn wait_empty(path: &Path) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while std::fs::read_dir(path).unwrap().next().is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("uncommitted screenshot removed");
}

#[tokio::test]
async fn uncommitted_artifact_is_removed_when_finalization_is_cancelled() {
    let fixture = Fixture::new();
    let artifacts = Artifacts::new(&fixture.0).unwrap();
    let pending = artifacts.save(PNG.into()).await.unwrap();
    let path = PathBuf::from(pending.metadata()["path"].as_str().unwrap());
    assert!(path.is_file());
    drop(pending);
    wait_empty(path.parent().unwrap()).await;
}

#[cfg(unix)]
#[tokio::test]
async fn parent_swap_during_write_never_creates_a_file_in_replacement_directory() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let root = fixture.0.join("root");
    let moved = fixture.0.join("moved");
    let outside = fixture.0.join("outside");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let artifacts = Artifacts::new(&root).unwrap();
    let private_name = artifacts.directory.path.file_name().unwrap().to_owned();
    let root_copy = root.clone();
    let moved_copy = moved.clone();
    let outside_copy = outside.clone();
    let result = artifacts
        .save_using(PNG.into(), move |file, bytes| {
            std::fs::rename(root_copy, moved_copy)?;
            symlink(outside_copy, root)?;
            file.write_all(bytes)?;
            file.sync_all()
        })
        .await;
    assert!(result.is_err());
    assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
    wait_empty(&moved.join(private_name)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_writer_does_not_block_cancellation_or_leave_a_named_file() {
    let fixture = Fixture::new();
    let artifacts = Arc::new(Artifacts::new(&fixture.0).unwrap());
    let path = artifacts.directory.path.clone();
    let (started, entered) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let running = artifacts.clone();
    let task = tokio::spawn(async move {
        running
            .save_using(PNG.into(), move |file, bytes| {
                let _ = started.send(());
                let _ = blocked.recv();
                file.write_all(bytes)
            })
            .await
    });
    entered.await.unwrap();
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    // Cleanup does not wait for write/sync to finish; the descriptor may remain
    // open, but no cancelled artifact is accessible by its name.
    wait_empty(&path).await;
    release.send(()).unwrap();
}
