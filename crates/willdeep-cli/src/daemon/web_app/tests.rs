use super::*;

fn request(root: &Path) -> WebAppStartParams {
    WebAppStartParams {
        listen: "127.0.0.1:0".parse().unwrap(),
        workspace: root.into(),
        config: Some(root.join("config.toml")),
        profile: None,
        language: "zh-CN".into(),
    }
}

#[tokio::test]
async fn disabling_an_offline_runtime_persists_intent_without_starting_it() {
    let root = std::env::temp_dir().join(format!("web-app-intent-{}", uuid::Uuid::new_v4()));
    save(
        &root,
        &Settings {
            enabled: true,
            request: request(&root),
        },
    )
    .unwrap();
    assert!(remote_status(&root).await.unwrap().enabled);
    assert!(!remote_status(&root).await.unwrap().running);
    remote_stop(&root).await.unwrap();
    assert!(!load(&root).unwrap().unwrap().enabled);
    assert!(!DaemonPaths::new(&root).state.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn runtime_service_rejects_non_loopback_before_any_service_is_started() {
    let root = std::env::temp_dir().join(format!("web-app-reject-{}", uuid::Uuid::new_v4()));
    let service = WebAppService::new(&root);
    let mut params = request(&root);
    params.listen = "0.0.0.0:9847".parse().unwrap();
    assert!(service.start(params).await.is_err());
    assert!(!root.exists());
    assert!(!service.status().running);
}

#[tokio::test]
async fn managed_listener_survives_client_drop_and_restores_on_runtime_restart() {
    use std::future::Future;
    use std::task::Poll;
    let root = std::env::temp_dir().join(format!("web-app-owned-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let first_runtime = Arc::new(WebAppService::new(&root));
    {
        let mut starting = Box::pin(first_runtime.start(request(&root)));
        // Poll exactly once on this current-thread test runtime. Initialization
        // is now owned by Runtime, but the requesting future has not completed.
        let result =
            std::future::poll_fn(|context| Poll::Ready(starting.as_mut().poll(context))).await;
        assert!(result.is_pending());
        assert!(first_runtime.status().enabled);
        assert!(!first_runtime.status().running);
    } // The requesting client disappears before readiness.
    tokio::time::timeout(Duration::from_secs(35), async {
        while !first_runtime.status().running {
            assert!(
                first_runtime.status().error.is_none(),
                "{:?}",
                first_runtime.status()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let address = first_runtime.status().listen.unwrap();
    let client = reqwest::Client::new();
    assert!(
        client
            .get(format!("http://{address}/health"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    drop(client);
    assert!(
        reqwest::get(format!("http://{address}/health"))
            .await
            .unwrap()
            .status()
            .is_success()
    );
    // A Runtime process exit drops its owned listener but preserves intent.
    drop(first_runtime);
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(address).await.is_err() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(load(&root).unwrap().unwrap().enabled);
    let second_runtime = Arc::new(WebAppService::new(&root));
    let saved = load(&root).unwrap().unwrap().request;
    let restored = second_runtime.start(saved).await.unwrap();
    assert_eq!(restored.listen, Some(address));
    second_runtime.stop().await.unwrap();
    assert!(!load(&root).unwrap().unwrap().enabled);
    let stop_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::net::TcpStream::connect(address).await.is_ok() {
        assert!(
            tokio::time::Instant::now() < stop_deadline,
            "Web App listener remained reachable after stop"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(second_runtime);
    std::fs::remove_dir_all(root).unwrap();
}
