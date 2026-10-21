//! Public release metadata only: no credentials, workspace or model requests.
use super::*;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const RELEASES: &str = "https://api.github.com/repos/niuwoai/willdeep-cli/releases?per_page=20";
const MIN_INTERVAL: u64 = 6 * 60 * 60;
const MAX_BODY: usize = 512 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct ReleaseNotice {
    pub tag_name: String,
    #[serde(default)]
    draft: bool,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct Cache {
    next_check: u64,
    candidate: Option<ReleaseNotice>,
    notified: BTreeSet<String>,
}

pub(super) struct Worker(Option<tokio::task::JoinHandle<()>>);
impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(task) = &self.0 {
            task.abort();
        }
    }
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn jitter(max: u64) -> u64 {
    (uuid::Uuid::new_v4().as_u128() % u128::from(max)) as u64
}

fn lock(home: &Path) -> Option<File> {
    std::fs::create_dir_all(home).ok()?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(home.join("update-check.lock"))
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

fn load(home: &Path) -> Cache {
    std::fs::read(home.join("update-check.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(home: &Path, cache: &Cache) -> Option<()> {
    std::fs::write(
        home.join("update-check.json"),
        serde_json::to_vec(cache).ok()?,
    )
    .ok()
}

/// The repository uses rcN, whose numeric suffix must compare numerically (rc10 > rc9).
fn version(tag: &str) -> Option<(u64, u64, u64, Option<u64>)> {
    let text = tag.strip_prefix('v').unwrap_or(tag);
    let (base, rc) = match text.split_once("-rc") {
        Some((base, rc)) => (base, Some(rc.parse().ok()?)),
        None => (text, None),
    };
    let parts: Vec<_> = base.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
        rc,
    ))
}

fn rank(tag: &str) -> Option<(u64, u64, u64, bool, u64)> {
    let (major, minor, patch, rc) = version(tag)?;
    Some((major, minor, patch, rc.is_none(), rc.unwrap_or(0)))
}

fn newest(releases: Vec<ReleaseNotice>, current: &str) -> Option<ReleaseNotice> {
    let installed = rank(current)?;
    releases
        .into_iter()
        .filter(|release| {
            !release.draft
                && rank(&release.tag_name)
                    .is_some_and(|candidate| candidate > installed && (!installed.3 || candidate.3))
        })
        .max_by_key(|release| rank(&release.tag_name))
}

async fn fetch(client: &reqwest::Client, url: &str, current: &str) -> Option<ReleaseNotice> {
    let mut response = client.get(url).send().await.ok()?.error_for_status().ok()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BODY as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    newest(serde_json::from_slice(&bytes).ok()?, current)
}

async fn check(
    home: &Path,
    client: &reqwest::Client,
    url: &str,
    current: &str,
    now: u64,
) -> Option<ReleaseNotice> {
    let _lock = lock(home)?;
    let mut cache = load(home);
    if now >= cache.next_check {
        // Persist before network I/O: timeout/offline/rate limiting never becomes a retry storm.
        cache.next_check = now.saturating_add(MIN_INTERVAL + jitter(MIN_INTERVAL));
        save(home, &cache)?;
        if let Some(candidate) = fetch(client, url, current).await {
            cache.candidate = Some(candidate);
            save(home, &cache)?;
        }
    }
    newest(cache.candidate.into_iter().collect(), current)
        .filter(|release| !cache.notified.contains(&release.tag_name))
}

pub(super) fn spawn(
    home: PathBuf,
    enabled: bool,
) -> (Worker, mpsc::UnboundedReceiver<ReleaseNotice>) {
    let (tx, rx) = mpsc::unbounded_channel();
    if !enabled || std::env::var_os("WILLDEEP_NO_UPDATE_CHECK").is_some() {
        return (Worker(None), rx);
    }
    let task = tokio::spawn(async move {
        let Ok(client) = reqwest::Client::builder()
            .user_agent(concat!("WillDeep/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
        else {
            return;
        };
        tokio::time::sleep(Duration::from_secs(30 + jitter(60))).await;
        loop {
            if let Some(release) = check(
                &home,
                &client,
                RELEASES,
                willdeep_core::VERSION,
                timestamp(),
            )
            .await
                && tx.send(release).is_err()
            {
                return;
            }
            tokio::time::sleep(Duration::from_secs(MIN_INTERVAL + jitter(MIN_INTERVAL))).await;
        }
    });
    (Worker(Some(task)), rx)
}

pub(super) fn show_when_idle(app: &mut App, home: &Path) {
    if app.update_available.is_none()
        || !app.composer_ready()
        || !app.input.text().is_empty()
        || app.has_open_overlay()
        || app.selection_mode
        || app.native_selection_mode
        || app.scroll_from_bottom > 0
        || app.notice.is_some()
    {
        return;
    }
    let Some(_lock) = lock(home) else {
        return;
    };
    let mut cache = load(home);
    let release = app.update_available.as_ref().unwrap();
    if cache.notified.contains(&release.tag_name) {
        app.update_available = None;
        return;
    }
    cache.notified.insert(release.tag_name.clone());
    if save(home, &cache).is_none() {
        return;
    }
    let release = app.update_available.take().unwrap();
    let url = format!(
        "https://github.com/niuwoai/willdeep-cli/releases/tag/{}",
        release.tag_name
    );
    app.append_transcript(format!("System: {} {} · {url}\n{}",
        app.language.text("WillDeep CLI 有新版可用", "A new WillDeep CLI version is available", "WillDeep CLI の新バージョンがあります"),
        release.tag_name,
        app.language.text("手动更新客户端：brew update && brew upgrade niuwoai/tap/willdeep-cli；/daemon upgrade 仅升级 Runtime。可用 [ui] check_updates = false 关闭检查。", "Update the client manually: brew update && brew upgrade niuwoai/tap/willdeep-cli; /daemon upgrade only updates Runtime. Disable checks with [ui] check_updates = false.", "クライアントは brew update && brew upgrade niuwoai/tap/willdeep-cli で更新できます。/daemon upgrade は Runtime のみ更新します。[ui] check_updates = false で確認を無効化できます。")
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn public_release_fixture_is_cached_without_a_second_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let received = stream.read(&mut request).await.unwrap();
            assert!(received > 0);
            let body = r#"[{"tag_name":"v0.112.0-rc1","draft":false}]"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let home = std::env::temp_dir().join(format!("update-public-{}", uuid::Uuid::new_v4()));
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let release = check(
            &home,
            &client,
            &format!("http://{address}"),
            "0.111.0-rc1",
            100,
        )
        .await
        .unwrap();
        assert_eq!(release.tag_name, "v0.112.0-rc1");
        server.await.unwrap();
        let next_check = load(&home).next_check;
        let cached = check(&home, &client, "http://127.0.0.1:1", "0.111.0-rc1", 101)
            .await
            .unwrap();
        assert_eq!(cached.tag_name, release.tag_name);
        assert_eq!(load(&home).next_check, next_check);
        std::fs::remove_dir_all(home).unwrap();
    }
    fn release(tag: &str) -> ReleaseNotice {
        ReleaseNotice {
            tag_name: tag.into(),
            draft: false,
        }
    }

    #[test]
    fn release_order_and_channel_are_numeric_and_drafts_are_ignored() {
        assert!(rank("v0.111.0-rc10") > rank("0.111.0-rc9"));
        assert!(rank("0.111.0") > rank("0.111.0-rc99"));
        assert!(version("https://untrusted.invalid").is_none());
        assert!(newest(vec![release("v0.112.0-rc1")], "0.111.0").is_none());
        assert_eq!(
            newest(
                vec![release("v0.111.0-rc9"), release("v0.111.0-rc10")],
                "0.111.0-rc1"
            )
            .unwrap()
            .tag_name,
            "v0.111.0-rc10"
        );
        let mut draft = release("v99.0.0");
        draft.draft = true;
        assert!(newest(vec![draft, release("garbage")], "0.111.0").is_none());
    }

    #[test]
    fn notifications_wait_for_idle_preserve_drafts_and_deduplicate_across_clients() {
        let home = std::env::temp_dir().join(format!("update-notice-{}", uuid::Uuid::new_v4()));
        let mut app = App::new(vec![], Language::En);
        app.update_available = Some(release("v9.0.0"));
        app.input.insert("unsent");
        show_when_idle(&mut app, &home);
        assert!(app.transcript.is_empty());
        assert_eq!(app.input.text(), "unsent");
        app.input = PromptEditor::default();
        app.running = true;
        show_when_idle(&mut app, &home);
        assert!(app.transcript.is_empty());
        app.running = false;
        show_when_idle(&mut app, &home);
        assert_eq!(app.transcript.len(), 1);
        assert!(!app.bell_pending);
        app.update_available = Some(release("v9.0.0"));
        show_when_idle(&mut app, &home);
        assert_eq!(app.transcript.len(), 1);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn failed_checks_are_throttled_and_file_lock_prevents_parallel_checks() {
        let home = std::env::temp_dir().join(format!("update-check-{}", uuid::Uuid::new_v4()));
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap();
        assert!(
            check(&home, &client, "http://127.0.0.1:1", "0.111.0", 100)
                .await
                .is_none()
        );
        let cache = load(&home);
        assert!(cache.next_check >= 100 + MIN_INTERVAL);
        let _lock = lock(&home).unwrap();
        assert!(lock(&home).is_none());
        drop(_lock);
        std::fs::remove_dir_all(home).unwrap();
    }
}
