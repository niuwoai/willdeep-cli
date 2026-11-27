use super::*;
use crate::mcp::test_http::{Request, Response, spawn_http_server};
use std::sync::atomic::{AtomicBool, AtomicUsize};

#[test]
fn configuration_status_preserves_disabled_invalid_and_unknown_authentication() {
    let private = "private-do-not-echo";
    let configs = BTreeMap::from([
        (
            "a-disabled".into(),
            McpServerConfig {
                command: Some(format!("/private/{private}/never-start")),
                args: vec![private.into()],
                env: BTreeMap::from([("PRIVATE".into(), private.into())]),
                enabled: false,
                ..McpServerConfig::default()
            },
        ),
        (
            "b-invalid".into(),
            McpServerConfig {
                command: Some(private.into()),
                url: Some(format!("https://example.invalid/{private}")),
                ..McpServerConfig::default()
            },
        ),
        (
            "c-oauth".into(),
            McpServerConfig {
                url: Some(format!("https://example.invalid/{private}")),
                oauth: Some(McpOAuthConfig {
                    client_id: Some(private.into()),
                    client_secret_env: Some(private.into()),
                    scopes: vec![private.into()],
                }),
                ..McpServerConfig::default()
            },
        ),
        (
            "d-bearer".into(),
            McpServerConfig {
                url: Some(format!("https://example.invalid/{private}")),
                bearer_token_env: Some(private.into()),
                ..McpServerConfig::default()
            },
        ),
        (
            format!("e-{private}"),
            McpServerConfig {
                command: Some(format!("/private/{private}/never-start")),
                ..McpServerConfig::default()
            },
        ),
    ]);
    let registry = McpRegistry::configuration_only(&configs);
    for _ in 0..2 {
        let value = registry.configuration_status();
        assert_eq!(value["configured"], 5);
        assert_eq!(value["enabled"], 4);
        assert_eq!(value["servers"][0]["connection"], "disabled");
        assert_eq!(value["servers"][1]["connection"], "invalid_configuration");
        assert_eq!(value["servers"][1]["configuration_check"]["valid"], false);
        assert_eq!(value["servers"][2]["authentication"]["mode"], "oauth");
        assert_eq!(
            value["servers"][3]["authentication"]["mode"],
            "bearer_environment"
        );
        assert_eq!(value["servers"][4]["connection"], "not_attempted");
        for server in value["servers"].as_array().unwrap() {
            assert_eq!(server["authentication"]["state"], "unknown");
            assert_eq!(server["authentication"]["credentials_checked"], false);
            assert!(server["recovery"].as_str().is_some_and(|s| !s.is_empty()));
        }
        let text = value.to_string();
        assert!(!text.contains(private));
        assert!(!text.contains("example.invalid"));
        assert!(!text.contains("/private/"));
    }

    let inline = BTreeMap::from([(
        "panel".into(),
        McpServerConfig {
            url: Some("https://panel.example.com/mcp".into()),
            token: Some(private.into()),
            ..McpServerConfig::default()
        },
    )]);
    let inline_status = McpRegistry::configuration_only(&inline).configuration_status();
    assert_eq!(
        inline_status["servers"][0]["authentication"]["mode"],
        "bearer_inline"
    );
    assert!(!inline_status.to_string().contains(private));
    assert!(registry.server_names().is_empty());
}

fn json_response(status: u16, body: Value) -> Response {
    Response {
        status,
        headers: vec![("content-type".into(), "application/json".into())],
        body: body.to_string().into_bytes(),
    }
}

#[tokio::test]
async fn actual_http_lifecycle_is_observed_without_status_requests() {
    let requests = Arc::new(AtomicUsize::new(0));
    let reject = Arc::new(AtomicBool::new(false));
    let log = requests.clone();
    let reject_requests = reject.clone();
    let (address, task) = spawn_http_server(move |request: Request| {
        log.fetch_add(1, Ordering::SeqCst);
        if reject_requests.load(Ordering::SeqCst) {
            return Response {
                status: 401,
                headers: vec![("www-authenticate".into(), "Bearer resource_metadata=\"https://private.invalid/do-not-copy\"".into())],
                body: b"private remote error".to_vec(),
            };
        }
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let result = match body["method"].as_str() {
            Some("initialize") => json!({"protocolVersion":PROTOCOL_VERSION,"capabilities":{},"serverInfo":{"name":"local","version":"1"}}),
            Some("notifications/initialized") => return Response { status: 202, headers: vec![], body: vec![] },
            Some("tools/list") => json!({"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}),
            Some("tools/call") => json!({"content":[{"type":"text","text":"ok"}]}),
            method => panic!("unexpected method: {method:?}"),
        };
        json_response(200, json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
    }).await;
    let configs = BTreeMap::from([(
        "local".into(),
        McpServerConfig {
            url: Some(format!("http://{address}/mcp")),
            ..McpServerConfig::default()
        },
    )]);
    let registry = McpRegistry::connect(&configs).await.unwrap();
    let initialized_requests = requests.load(Ordering::SeqCst);
    assert_eq!(initialized_requests, 3);
    let before = registry.configuration_status();
    assert_eq!(before["servers"][0]["connection"], "initialized_unverified");
    assert_eq!(before["servers"][0]["last_request"], "succeeded");
    assert_eq!(before["servers"][0]["authentication"]["state"], "unknown");
    assert_eq!(requests.load(Ordering::SeqCst), initialized_requests);

    reject.store(true, Ordering::SeqCst);
    assert!(matches!(
        registry.call("mcp__local__echo", json!({})).await,
        Err(McpError::Unauthorized { .. })
    ));
    let after_request = requests.load(Ordering::SeqCst);
    let rejected = registry.configuration_status();
    assert_eq!(
        rejected["servers"][0]["authentication"]["state"],
        "required"
    );
    assert_eq!(
        rejected["servers"][0]["last_request"],
        "authorization_required"
    );
    assert!(!rejected.to_string().contains("private.invalid"));
    assert!(!rejected.to_string().contains("private remote error"));
    assert_eq!(requests.load(Ordering::SeqCst), after_request);

    reject.store(false, Ordering::SeqCst);
    registry.call("mcp__local__echo", json!({})).await.unwrap();
    assert_eq!(
        registry.configuration_status()["servers"][0]["authentication"]["state"],
        "unknown"
    );
    // Drop the real HTTP connection before terminating its local fixture.
    drop(registry);
    task.abort();
}

#[tokio::test]
async fn failed_http_startup_retains_only_safe_failure_and_authentication_state() {
    let (address, task) = spawn_http_server(|_: Request| Response {
        status: 401,
        headers: vec![(
            "www-authenticate".into(),
            "Bearer resource_metadata=\"https://private.invalid/secret\"".into(),
        )],
        body: b"arbitrary-private-response".to_vec(),
    })
    .await;
    let configs = BTreeMap::from([(
        "local".into(),
        McpServerConfig {
            url: Some(format!("http://{address}/mcp")),
            ..McpServerConfig::default()
        },
    )]);
    let registry = McpRegistry::connect(&configs).await.unwrap();
    assert!(registry.server_names().is_empty());
    let value = registry.configuration_status();
    assert_eq!(value["configured"], 1);
    assert_eq!(value["servers"][0]["connection"], "failed");
    assert_eq!(value["servers"][0]["authentication"]["state"], "required");
    assert!(!value.to_string().contains("private.invalid"));
    assert!(!value.to_string().contains("arbitrary-private-response"));
    task.abort();
}
