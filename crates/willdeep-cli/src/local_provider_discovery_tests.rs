use super::*;
use std::io::{Read, Write};

fn fixture(name: &'static str, valid: bool) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let count = if name == "magpie" && valid { 2 } else { 1 };
        for _ in 0..count {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = [0; 8192];
            let len = socket.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..len]);
            assert!(!request.to_lowercase().contains("authorization:"));
            let body = if !valid {
                r#"{"name":"unrelated"}"#
            } else if request.starts_with("GET /api/hello ") {
                r#"{"name":"magpie","version":"fixture"}"#
            } else if name == "ollama" {
                assert!(request.starts_with("GET /api/tags "));
                r#"{"models":[{"name":"qwen:local"},{"name":"qwen:local"}]}"#
            } else {
                assert!(request.starts_with("GET /v1/models "));
                r#"{"data":[{"id":"group/coder"}]}"#
            };
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    (base, thread)
}

#[tokio::test]
async fn probes_both_services_without_cloud_credentials() {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(PROBE_TIMEOUT)
        .build()
        .unwrap();
    let (ollama, first) = fixture("ollama", true);
    let (magpie, second) = fixture("magpie", true);
    let (a, b) = tokio::join!(
        probe(&client, "ollama", Some(ollama)),
        probe(&client, "magpie", Some(magpie))
    );
    assert_eq!(a.unwrap().models, ["qwen:local"]);
    assert_eq!(b.unwrap().models, ["group/coder"]);
    first.join().unwrap();
    second.join().unwrap();
}

#[tokio::test]
async fn unrelated_http_service_is_not_magpie_or_ollama() {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(PROBE_TIMEOUT)
        .build()
        .unwrap();
    for name in ["ollama", "magpie"] {
        let (base, worker) = fixture(name, false);
        assert!(probe(&client, name, Some(base)).await.is_none());
        worker.join().unwrap();
    }
}

#[test]
fn discovery_only_accepts_loopback_and_normalizes_bind_addresses() {
    assert_eq!(
        loopback_base("0.0.0.0:11434").as_deref(),
        Some("http://127.0.0.1:11434")
    );
    assert!(loopback_base("[::1]:3425").is_some());
    for value in [
        "https://remote.example",
        "http://192.168.1.2:3425",
        "http://key@localhost:3425",
        "file:///tmp/test",
    ] {
        assert!(loopback_base(value).is_none());
    }
}

fn provider(name: &'static str, port: u16) -> LocalProvider {
    LocalProvider {
        name,
        base: format!("http://127.0.0.1:{port}/v1"),
        models: vec!["local-model".into()],
    }
}

#[test]
fn persistence_preserves_comments_defaults_and_deduplicates_on_second_run() {
    let root = std::env::temp_dir().join(format!("wd-discovery-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("config.toml");
    let original = "# preserve this comment\nversion=1\ndefault_provider='cloud'\n[providers.cloud]\napi_base='https://cloud.example/v1'\napi_key='fixture'\nmodel='cloud-model'\n";
    std::fs::write(&path, original).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let providers = [provider("ollama", 11434), provider("magpie", 3425)];
    assert_eq!(persist(&path, &providers).unwrap(), 2);
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.starts_with(original));
    assert_eq!(persist(&path, &providers).unwrap(), 0);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), saved);
    let loaded = crate::config::LoadedConfig::load(Some(&path)).unwrap();
    assert_eq!(loaded.file.default_provider.as_deref(), Some("cloud"));
    assert_eq!(loaded.file.providers.len(), 3);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn existing_endpoint_and_conflicting_name_are_preserved() {
    let root = std::env::temp_dir().join(format!("wd-discovery-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("config.toml");
    std::fs::write(&path, "[providers.mine]\napi_base='http://localhost:11434/v1/'\n[providers.magpie]\napi_base='https://remote.example/v1'\n").unwrap();
    assert_eq!(
        persist(
            &path,
            &[provider("ollama", 11434), provider("magpie", 3425)]
        )
        .unwrap(),
        1
    );
    let loaded = crate::config::LoadedConfig::load(Some(&path)).unwrap();
    assert!(!loaded.file.providers.contains_key("ollama"));
    assert_eq!(
        loaded.file.providers["magpie"].api_base.as_deref(),
        Some("https://remote.example/v1")
    );
    assert!(loaded.file.providers.contains_key("magpie-local-1"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn first_discovery_creates_valid_configuration_and_can_be_retried() {
    let root = std::env::temp_dir().join(format!("wd-discovery-{}", uuid::Uuid::new_v4()));
    let path = root.join("config.toml");
    assert_eq!(persist(&path, &[provider("magpie", 3425)]).unwrap(), 1);
    let loaded = crate::config::LoadedConfig::load(Some(&path)).unwrap();
    assert_eq!(loaded.file.default_provider.as_deref(), Some("magpie"));
    assert_eq!(persist(&path, &[provider("magpie", 3425)]).unwrap(), 0);
    std::fs::remove_dir_all(root).unwrap();
}
