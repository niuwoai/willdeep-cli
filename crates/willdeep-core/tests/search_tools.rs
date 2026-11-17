//! Real HTTP on isolated loopback listeners; no external service or credentials.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use willdeep_core::tools::{SearchBackend, SearchSettings, ToolError};
use willdeep_core::{
    ApprovalDecision, ApprovalMode, Approver, ToolCall, ToolRegistry, WebToolConfig,
};

const DEADLINE: Duration = Duration::from_secs(3);
const QUIET_WINDOW: Duration = Duration::from_millis(30);
const LIMIT: usize = 1024 * 1024;
const FIXTURE_KEY: &str = "synthetic-search-fixture-key";

struct Fixture {
    listener: TcpListener,
    base: String,
}

struct Request {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Fixture {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        Self { listener, base }
    }

    fn registry(&self) -> ToolRegistry {
        registry(ApprovalMode::FullAccess).with_search_settings(SearchSettings {
            backend: SearchBackend::Searxng,
            api_base: Some(format!("{}/prefix/", self.base)),
            api_key: Some(FIXTURE_KEY.into()),
            allow_private_endpoint: true,
        })
    }

    async fn serve(&self, status: u16, headers: &str, body: &[u8]) -> Request {
        let (mut socket, _) = tokio::time::timeout(DEADLINE, self.listener.accept())
            .await
            .unwrap()
            .unwrap();
        let request = read_request(&mut socket).await;
        let header = format!(
            "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
            body.len()
        );
        socket.write_all(header.as_bytes()).await.unwrap();
        // Early close is expected for status errors and oversized responses.
        if let Err(error) = socket.write_all(body).await {
            assert!(matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
            ));
        }
        request
    }

    async fn assert_no_request(&self) {
        assert!(
            tokio::time::timeout(QUIET_WINDOW, self.listener.accept())
                .await
                .is_err()
        );
    }
}

fn registry(mode: ApprovalMode) -> ToolRegistry {
    ToolRegistry::new(env!("CARGO_MANIFEST_DIR"), mode).unwrap()
}

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: "search-http-fixture".into(),
        name: name.into(),
        arguments: args.to_string(),
    }
}

async fn search(registry: &ToolRegistry, args: Value) -> Result<Value, ToolError> {
    let output = registry.execute(&call("web_search", args)).await?;
    Ok(serde_json::from_str(&output).expect("structured search result"))
}

async fn read_request(socket: &mut TcpStream) -> Request {
    tokio::time::timeout(DEADLINE, async {
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0, "request closed before headers");
            bytes.extend_from_slice(&buffer[..count]);
            assert!(bytes.len() <= 64 * 1024);
            if let Some(position) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let header = std::str::from_utf8(&bytes[..header_end]).unwrap();
        let mut lines = header.split("\r\n");
        let request_line: Vec<_> = lines.next().unwrap().split_whitespace().collect();
        let method = request_line[0].to_owned();
        let target = request_line[1].to_owned();
        let headers: BTreeMap<_, _> = lines
            .filter_map(|line| {
                line.split_once(':')
                    .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
            })
            .collect();
        let length: usize = headers
            .get("content-length")
            .map(|value| value.parse().unwrap())
            .unwrap_or(0);
        while bytes.len() < header_end + length {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            bytes.extend_from_slice(&buffer[..count]);
            assert!(bytes.len() <= 64 * 1024);
        }
        Request {
            method,
            target,
            headers,
            body: bytes[header_end..header_end + length].to_vec(),
        }
    })
    .await
    .expect("read request deadline")
}

fn result_body() -> Vec<u8> {
    serde_json::to_vec(&json!({"results": [{
        "title": "Rust source", "url": "https://example.org/rust", "content": "A source snippet",
        "engine": "fixture-engine", "engines": ["fixture-engine", "second-engine"]
    }]}))
    .unwrap()
}

#[tokio::test]
async fn searxng_encodes_query_and_authentication_and_returns_sources() {
    let fixture = Fixture::new().await;
    let registry = fixture.registry();
    let body = result_body();
    let query = "中文 & q=other + / ? #";
    let (request, result) = tokio::join!(
        fixture.serve(200, "Content-Type: application/json\r\n", &body),
        search(&registry, json!({"query": query}))
    );
    assert_eq!(request.method, "GET");
    let target = reqwest::Url::parse(&format!("http://fixture.invalid{}", request.target)).unwrap();
    assert_eq!(target.path(), "/prefix/search");
    let parameters: BTreeMap<_, _> = target.query_pairs().into_owned().collect();
    assert_eq!(parameters.get("q").map(String::as_str), Some(query));
    assert_eq!(parameters.get("format").map(String::as_str), Some("json"));
    assert_eq!(parameters.len(), 2);
    assert_eq!(
        request.headers["authorization"],
        format!("Bearer {FIXTURE_KEY}")
    );
    assert_eq!(request.headers["accept"], "application/json");
    let result = result.unwrap();
    assert_eq!(result["backend"], "searxng");
    assert_eq!(result["source"], fixture.base);
    assert_eq!(result["results"][0]["title"], "Rust source");
    assert_eq!(result["results"][0]["url"], "https://example.org/rust");
    assert_eq!(result["results"][0]["content"], "A source snippet");
    assert_eq!(
        result["results"][0]["sources"],
        json!(["fixture-engine", "second-engine"])
    );
    assert_eq!(result["empty"], false);
    fixture.assert_no_request().await;
}

#[tokio::test]
async fn anonymous_searxng_does_not_borrow_someim_credential() {
    let fixture = Fixture::new().await;
    let registry = registry(ApprovalMode::FullAccess)
        .with_web_tools(Some(WebToolConfig {
            some_im_base_url: "https://uncontacted-provider.invalid".into(),
            api_key: "synthetic-chat-fixture-key".into(),
        }))
        .with_search_settings(SearchSettings {
            backend: SearchBackend::Searxng,
            api_base: Some(fixture.base.clone()),
            api_key: None,
            allow_private_endpoint: true,
        });
    let (request, result) = tokio::join!(
        fixture.serve(200, "", br#"{"results":[]}"#),
        search(&registry, json!({"query":"fixture"}))
    );
    assert!(!request.headers.contains_key("authorization"));
    let result = result.unwrap();
    assert_eq!(result["empty"], true);
    assert_eq!(result["returned_count"], 0);
    assert_eq!(result["truncated"], false);
}

#[tokio::test]
async fn count_defaults_to_eight_and_clamps_to_twenty() {
    for (requested, expected) in [(None, 8), (Some(100), 20), (Some(0), 1)] {
        let fixture = Fixture::new().await;
        let registry = fixture.registry();
        let body = serde_json::to_vec(&json!({"results": (0..25).map(|index| json!({
            "title": format!("result {index}"), "url": format!("https://example.org/{index}"), "content":"fixture"
        })).collect::<Vec<_>>()})).unwrap();
        let mut arguments = json!({"query":"fixture"});
        if let Some(count) = requested {
            arguments["count"] = json!(count);
        }
        let (_, result) = tokio::join!(fixture.serve(200, "", &body), search(&registry, arguments));
        let result = result.unwrap();
        assert_eq!(result["returned_count"], expected);
        assert_eq!(result["received_count"], 25);
        assert_eq!(result["truncated"], true);
    }
}

#[tokio::test]
async fn authentication_forbidden_rate_limit_and_http_errors_are_safe_and_not_retried() {
    for (status, code) in [
        (401, "authentication"),
        (403, "forbidden"),
        (429, "rate_limited"),
        (500, "http"),
    ] {
        let fixture = Fixture::new().await;
        let fallback = Fixture::new().await;
        let registry = fixture.registry().with_web_tools(Some(WebToolConfig {
            some_im_base_url: fallback.base.clone(),
            api_key: "synthetic-fallback-fixture-key".into(),
        }));
        let error_body = format!("server reflected {FIXTURE_KEY}; private error detail");
        let (_, result) = tokio::join!(
            fixture.serve(status, "", error_body.as_bytes()),
            search(&registry, json!({"query":"fixture"}))
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains(&format!("search[{code}]")), "{error}");
        assert!(!error.contains(FIXTURE_KEY));
        assert!(!error.contains("private error detail"));
        fixture.assert_no_request().await;
        fallback.assert_no_request().await;
    }
}

#[tokio::test]
async fn malformed_response_never_returns_unparsed_body() {
    for body in [
        b"invalid fixture raw body".as_slice(),
        br#"{"results":[{"title":7}]}"#.as_slice(),
    ] {
        let fixture = Fixture::new().await;
        let registry = fixture.registry();
        let (_, result) = tokio::join!(
            fixture.serve(200, "", body),
            search(&registry, json!({"query":"fixture"}))
        );
        let error = result.unwrap_err().to_string();
        assert!(error.contains("search[malformed_response]"));
        assert!(!error.contains("invalid fixture raw body"));
    }
}

#[tokio::test]
async fn declared_and_chunked_oversized_responses_are_rejected() {
    for chunked in [false, true] {
        let fixture = Fixture::new().await;
        let registry = fixture.registry();
        let server = async {
            let (mut socket, _) = tokio::time::timeout(DEADLINE, fixture.listener.accept())
                .await
                .unwrap()
                .unwrap();
            read_request(&mut socket).await;
            let header = if chunked {
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
                    .to_owned()
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    LIMIT + 1
                )
            };
            socket.write_all(header.as_bytes()).await.unwrap();
            if chunked {
                let body = vec![b' '; LIMIT + 1];
                socket
                    .write_all(format!("{:x}\r\n", body.len()).as_bytes())
                    .await
                    .unwrap();
                if let Err(error) = socket.write_all(&body).await {
                    assert!(matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ));
                }
                // The client can close immediately upon observing the excess byte.
                match socket.write_all(b"\r\n0\r\n\r\n").await {
                    Ok(()) => {}
                    Err(error) => assert!(matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    )),
                }
            }
        };
        let (_, result) = tokio::join!(server, search(&registry, json!({"query":"fixture"})));
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("search[response_limit]")
        );
    }
}

#[tokio::test]
async fn redirect_is_not_followed_and_does_not_leak_authentication() {
    let source = Fixture::new().await;
    let destination = Fixture::new().await;
    let registry = source.registry();
    let headers = format!("Location: {}/collect\r\n", destination.base);
    let (request, result) = tokio::join!(
        source.serve(302, &headers, b""),
        search(&registry, json!({"query":"fixture"}))
    );
    assert_eq!(
        request.headers["authorization"],
        format!("Bearer {FIXTURE_KEY}")
    );
    assert!(result.unwrap_err().to_string().contains("search[redirect]"));
    destination.assert_no_request().await;
    source.assert_no_request().await;
}

#[tokio::test]
async fn status_disabled_unconfigured_and_denial_make_zero_requests() {
    let fixture = Fixture::new().await;
    let configured = fixture.registry();
    let status: Value = serde_json::from_str(
        &configured
            .execute(&call("web_search_status", json!({})))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status["state"], "configured");
    assert_eq!(status["network_checked"], false);
    assert!(!status.to_string().contains(FIXTURE_KEY));
    assert!(
        configured
            .execute(&call(
                "web_search_status",
                json!({"operation":"search","query":"fixture"})
            ))
            .await
            .is_err()
    );
    let unconfigured = registry(ApprovalMode::FullAccess);
    let error = search(&unconfigured, json!({"query":"fixture"}))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("search[not_configured]"));
    let disabled = fixture.registry().with_search_settings(SearchSettings {
        backend: SearchBackend::Disabled,
        ..Default::default()
    });
    assert!(
        search(&disabled, json!({"query":"fixture"}))
            .await
            .unwrap_err()
            .to_string()
            .contains("search[disabled]")
    );
    let denied = registry(ApprovalMode::Strict).with_search_settings(SearchSettings {
        backend: SearchBackend::Searxng,
        api_base: Some(fixture.base.clone()),
        api_key: None,
        allow_private_endpoint: true,
    });
    let error = search(&denied, json!({"query":"fixture"}))
        .await
        .unwrap_err();
    assert!(matches!(error, ToolError::ApprovalDenied(_)));
    // An unresolvable host also returns Denied, proving approval precedes DNS.
    let denied_dns = registry(ApprovalMode::Strict).with_search_settings(SearchSettings {
        backend: SearchBackend::Searxng,
        api_base: Some("https://search-do-not-resolve.invalid".into()),
        ..Default::default()
    });
    assert!(matches!(
        search(&denied_dns, json!({"query":"fixture"}))
            .await
            .unwrap_err(),
        ToolError::ApprovalDenied(_)
    ));
    fixture.assert_no_request().await;
}

#[tokio::test]
async fn private_endpoint_requires_explicit_opt_in_and_public_http_is_refused() {
    let fixture = Fixture::new().await;
    let registry = fixture.registry().with_search_settings(SearchSettings {
        backend: SearchBackend::Searxng,
        api_base: Some(fixture.base.clone()),
        ..Default::default()
    });
    assert!(
        search(&registry, json!({"query":"fixture"}))
            .await
            .unwrap_err()
            .to_string()
            .contains("search[private_endpoint]")
    );
    fixture.assert_no_request().await;
    let public = fixture.registry().with_search_settings(SearchSettings {
        backend: SearchBackend::Searxng,
        api_base: Some("http://8.8.8.8".into()),
        allow_private_endpoint: true,
        ..Default::default()
    });
    // The literal IP is rejected before a socket is opened; no public request occurs.
    assert!(
        search(&public, json!({"query":"fixture"}))
            .await
            .unwrap_err()
            .to_string()
            .contains("search[https_required]")
    );
}

#[tokio::test]
async fn dropping_search_during_response_closes_the_socket() {
    let fixture = Fixture::new().await;
    let registry = fixture.registry();
    let call = call("web_search", json!({"query":"fixture"}));
    let mut search = Box::pin(registry.execute(&call));
    let mut socket = tokio::select! {
        result = search.as_mut() => panic!("search completed before stalled response: {result:?}"),
        socket = async {
            let (mut socket, _) = tokio::time::timeout(DEADLINE, fixture.listener.accept()).await.unwrap().unwrap();
            read_request(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{").await.unwrap();
            socket
        } => socket,
    };
    drop(search);
    let mut byte = [0; 1];
    let closed = tokio::time::timeout(DEADLINE, socket.read(&mut byte))
        .await
        .expect("cancellation closes transport");
    match closed {
        Ok(count) => assert_eq!(count, 0),
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset),
    }
    fixture.assert_no_request().await;
}

struct PendingApprover(Arc<AtomicUsize>);

#[async_trait]
impl Approver for PendingApprover {
    async fn approve(&self, _: &str, _: bool) -> ApprovalDecision {
        self.0.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }
}

#[tokio::test]
async fn dropping_search_while_approval_is_pending_makes_zero_requests() {
    let fixture = Fixture::new().await;
    let approvals = Arc::new(AtomicUsize::new(0));
    let registry = registry(ApprovalMode::Strict)
        .with_search_settings(SearchSettings {
            backend: SearchBackend::Searxng,
            api_base: Some(fixture.base.clone()),
            allow_private_endpoint: true,
            ..Default::default()
        })
        .with_approver(Arc::new(PendingApprover(approvals.clone())));
    let call = call("web_search", json!({"query":"fixture"}));
    let request = registry.execute(&call);
    assert!(tokio::time::timeout(QUIET_WINDOW, request).await.is_err());
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
    fixture.assert_no_request().await;
}

#[tokio::test]
async fn someim_auto_preserves_post_protocol_and_legacy_response_aliases() {
    for (shape, field, requested_count, expected_count) in [
        ("results", "snippet", None, 8),
        ("items", "description", Some(99), 20),
    ] {
        let fixture = Fixture::new().await;
        let registry = registry(ApprovalMode::FullAccess)
            .with_web_tools(Some(WebToolConfig {
                some_im_base_url: format!("{}/v1", fixture.base),
                api_key: FIXTURE_KEY.into(),
            }))
            .with_search_settings(SearchSettings {
                allow_private_endpoint: true,
                ..Default::default()
            });
        let body = serde_json::to_vec(&json!({"data": {(shape): [{"title":"legacy", "link":"https://example.org/legacy", (field):"legacy snippet"}]}})).unwrap();
        let mut arguments = json!({"query":"回归 & fixture"});
        if let Some(count) = requested_count {
            arguments["count"] = json!(count);
        }
        let (request, result) =
            tokio::join!(fixture.serve(200, "", &body), search(&registry, arguments));
        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/api/v1/customer/web-search");
        assert_eq!(
            request.headers["authorization"],
            format!("Bearer {FIXTURE_KEY}")
        );
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        assert_eq!(
            body,
            json!({"query":"回归 & fixture", "count":expected_count, "provider":"auto"})
        );
        let result = result.unwrap();
        assert_eq!(result["backend"], "some_im");
        assert_eq!(result["results"][0]["content"], "legacy snippet");
    }
}
