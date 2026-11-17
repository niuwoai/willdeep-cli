use super::*;
use axum::body::Body;
use axum::http::{Response, StatusCode, header};
use axum::routing::{get, post};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const ZERO_REQUEST_WINDOW: Duration = Duration::from_millis(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const IMAGE_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nfixture-image";

struct ImageFixture {
    root: PathBuf,
    config: PathBuf,
    target: tokio::net::TcpListener,
    requests: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

impl ImageFixture {
    async fn new(status: u16, body: &'static [u8]) -> Self {
        let root = std::env::temp_dir().join(format!("wd-image-download-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".willdeep")).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let location = format!(
            "http://{}/must-not-follow.png",
            target.local_addr().unwrap()
        );
        let config = root.join("config.toml");
        std::fs::write(&config, format!(
            "version=1\n[providers.some-im]\nprovider='some-im'\napi_base='{origin}/v1'\napi_key='fixture'\nmodel='gpt-image-2'\n",
        )).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        // Bind the literal loopback origin so the test bypasses process proxy
        // settings and never resolves or contacts an external service.
        std::fs::write(
            root.join(".willdeep/model-policy.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "allow": [{"provider": "some-im", "model": "gpt-image-2", "origin": origin}],
            }))
            .unwrap(),
        )
        .unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let generation_count = requests.clone();
        let download_count = requests.clone();
        let url = format!("{origin}/generated.png?signature=fixture");
        let app = axum::Router::new()
            .route(
                "/v1/images/generations",
                post(move || {
                    let url = url.clone();
                    let count = generation_count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"data": [{"url": url}]}))
                    }
                }),
            )
            .route(
                "/generated.png",
                get(move || {
                    let location = location.clone();
                    let count = download_count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        Response::builder()
                            .status(StatusCode::from_u16(status).unwrap())
                            .header(header::LOCATION, location)
                            .body(Body::from(body))
                            .unwrap()
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            root,
            config,
            target,
            requests,
            server,
        }
    }

    async fn generate(&self) -> Result<Value, PluginWebError> {
        let host = PluginAiHost {
            home: &self.root,
            config_path: &self.config,
            usage_context: None,
            production_prompt: false,
            workspaces: vec![self.root.clone()],
        };
        let request = ImageRequest {
            prompt: "fixture image".into(),
            provider: None,
            model: Some("gpt-image-2".into()),
            size: None,
            reference_image_paths: Vec::new(),
        };
        tokio::time::timeout(REQUEST_TIMEOUT, generate_image(&host, "fixture", request))
            .await
            .expect("local image request must finish")
    }
}

impl Drop for ImageFixture {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn redirects_and_http_errors_are_download_failures_without_files_or_redirect_requests() {
    for status in [302, 307, 308, 404, 500] {
        for body in [b"".as_slice(), b"<html>not an image</html>".as_slice()] {
            let fixture = ImageFixture::new(status, body).await;
            let error = fixture.generate().await.unwrap_err();
            assert!(matches!(error, PluginWebError::BadRequest(ref message)
                if message == &format!("downloadFailed: {status}")));
            assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
            assert!(!fixture.root.join("plugin-media").exists());
            assert!(
                tokio::time::timeout(ZERO_REQUEST_WINDOW, fixture.target.accept())
                    .await
                    .is_err()
            );
        }
    }
}

#[tokio::test]
async fn successful_image_download_still_writes_exact_response_bytes() {
    let fixture = ImageFixture::new(200, IMAGE_BYTES).await;
    let result = fixture
        .generate()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let path = PathBuf::from(result.get("filePath").and_then(Value::as_str).unwrap());
    let media_root = fixture.root.join("plugin-media/fixture");
    assert!(path.starts_with(&media_root));
    assert_eq!(std::fs::read(path).unwrap(), IMAGE_BYTES);
    assert_eq!(std::fs::read_dir(media_root).unwrap().count(), 1);
    assert_eq!(fixture.requests.load(Ordering::SeqCst), 2);
    assert!(
        tokio::time::timeout(ZERO_REQUEST_WINDOW, fixture.target.accept())
            .await
            .is_err()
    );
}
