use super::*;
use std::error::Error;

#[tokio::test]
async fn missing_process_reports_server_and_stage_without_configuration_secrets() {
    // A bare name searches the runner's PATH, whose inaccessible directories
    // can produce PermissionDenied instead of NotFound on Unix.
    let directory = std::env::temp_dir().join(format!(
        "willdeep-missing-mcp-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir(&directory).expect("private missing-executable fixture");
    let missing_program = directory.join("missing-mcp");
    let config = McpServerConfig {
        command: Some(missing_program.to_string_lossy().into_owned()),
        args: vec!["argument-canary".to_owned()],
        env: BTreeMap::from([("TEST_SECRET".to_owned(), "environment-canary".to_owned())]),
        ..McpServerConfig::default()
    };
    let configs = BTreeMap::from([("broken-server".to_owned(), config)]);
    let result = McpRegistry::connect(&configs).await;
    std::fs::remove_dir(&directory).expect("remove private missing-executable fixture");
    let error = match result {
        Ok(_) => panic!("missing executable unexpectedly started"),
        Err(error) => error,
    };
    let text = error.to_string();
    assert!(text.contains("\"broken-server\" (stdio)"), "{text}");
    assert!(text.contains("process spawn"), "{text}");
    assert!(!text.contains("argument-canary"), "{text}");
    assert!(!text.contains("environment-canary"), "{text}");
    let source = error.source().expect("original MCP error");
    assert!(matches!(
        source.downcast_ref::<Box<McpError>>().map(Box::as_ref),
        Some(McpError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound
    ));
}

#[test]
fn startup_context_preserves_os_error_and_distinguishes_stages() {
    for stage in [
        "process spawn",
        "initialize handshake",
        "notifications/initialized",
    ] {
        let error = McpError::from(std::io::Error::from_raw_os_error(35)).during_startup(
            "resource-server",
            McpTransportKind::Stdio,
            stage,
        );
        let text = error.to_string();
        assert!(text.contains("\"resource-server\" (stdio)"), "{text}");
        assert!(text.contains(stage), "{text}");
        assert!(text.contains("os error 35"), "{text}");
        assert!(matches!(
            error.source().and_then(|source| source.downcast_ref::<Box<McpError>>().map(Box::as_ref)),
            Some(McpError::Io(error)) if error.raw_os_error() == Some(35)
        ));
    }
}

#[test]
fn startup_context_escapes_control_characters_in_server_names() {
    let error = McpError::MissingPipe.during_startup(
        "bad\nserver\u{1b}[31m",
        McpTransportKind::Stdio,
        "stdio pipe setup",
    );
    let text = error.to_string();
    assert!(!text.contains('\n'), "{text:?}");
    assert!(!text.contains('\u{1b}'), "{text:?}");
    assert!(text.contains("bad\\nserver"), "{text}");
}

#[test]
fn wrapped_http_authorization_failure_retains_status() {
    let configs = BTreeMap::from([(
        "remote".to_owned(),
        McpServerConfig {
            url: Some("https://example.invalid/mcp".to_owned()),
            ..McpServerConfig::default()
        },
    )]);
    let registry = McpRegistry::configuration_only(&configs);
    let error = McpError::Unauthorized {
        server: "remote".to_owned(),
        hint: None,
    }
    .during_startup(
        "remote",
        McpTransportKind::StreamableHttp,
        "initialize handshake",
    );
    registry.status["remote"]
        .observation
        .record::<()>(&Err(error));
    let status = registry.configuration_status();
    assert_eq!(
        status["servers"][0]["last_request"],
        "authorization_required"
    );
    assert_eq!(status["servers"][0]["authentication"]["state"], "required");
}

#[tokio::test]
async fn recoverable_http_failure_is_delivered_to_startup_warning_sink() {
    let variable = format!("WILLDEEP_TEST_MCP_{}", uuid::Uuid::new_v4().simple());
    let configs = BTreeMap::from([(
        "remote".to_owned(),
        McpServerConfig {
            url: Some("http://127.0.0.1:1/mcp".to_owned()),
            bearer_token_env: Some(variable.clone()),
            ..McpServerConfig::default()
        },
    )]);
    assert!(std::env::var_os(&variable).is_none());
    let mut warnings = Vec::new();
    let registry = McpRegistry::connect_in_with_warnings(None, &configs, |kind, name, error| {
        warnings.push((kind, name.to_owned(), error.to_string()));
    })
    .await
    .expect("a remote configuration failure is recoverable");
    assert_eq!(warnings.len(), 1);
    let (kind, name, detail) = &warnings[0];
    assert_eq!(*kind, McpStartupWarningKind::Unavailable);
    assert_eq!(name, "remote");
    assert!(detail.contains(&format!("environment variable {variable} is not set")));
    assert!(registry.servers.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn remote_handshake_error_reports_named_server_and_protocol_stage() {
    let script = r#"read init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"handshake rejected"}}'
"#;
    let configs = BTreeMap::from([(
        "rejecting-server".to_owned(),
        McpServerConfig {
            command: Some("/bin/sh".to_owned()),
            args: vec!["-c".to_owned(), script.to_owned()],
            startup_timeout_seconds: 5,
            ..McpServerConfig::default()
        },
    )]);
    let error = match McpRegistry::connect(&configs).await {
        Ok(_) => panic!("rejected initialization unexpectedly succeeded"),
        Err(error) => error,
    };
    let text = error.to_string();
    assert!(text.contains("\"rejecting-server\" (stdio)"), "{text}");
    assert!(text.contains("initialize handshake"), "{text}");
    assert!(text.contains("handshake rejected"), "{text}");
}
