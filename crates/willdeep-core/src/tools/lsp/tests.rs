use super::*;
use client::{Client, QueryContext};
use config::{Language, uri};
use tokio::io::{AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf, duplex, split};

#[path = "readiness_tests.rs"]
mod readiness;

pub(super) async fn receive(stream: &mut DuplexStream) -> Value {
    tokio::time::timeout(Duration::from_secs(2), transport::read_frame(stream))
        .await
        .expect("fake server request deadline")
        .expect("read fake server request")
}

pub(super) async fn send(stream: &mut DuplexStream, message: Value) {
    let bytes = serde_json::to_vec(&message).expect("encode fake server message");
    stream
        .write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes())
        .await
        .expect("write fake server header");
    stream
        .write_all(&bytes)
        .await
        .expect("write fake server body");
    stream.flush().await.expect("flush fake server message");
}

pub(super) struct Fixture {
    pub(super) root: PathBuf,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let root = std::env::temp_dir().join(format!("willdeep-lsp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self {
            root: root.canonicalize().unwrap(),
        }
    }

    pub(super) fn write(&self, path: &str, text: &str) -> PathBuf {
        let path = self.root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    pub(super) fn context(&self, path: &str, text: &str) -> QueryContext {
        let path = self.write(path, text);
        let document = client::Document {
            uri: uri(&path).unwrap(),
            path,
            version: 7,
            snapshot: snapshot(text.as_bytes()),
        };
        QueryContext {
            document: document.clone(),
            text: text.into(),
            checked_documents: vec![document],
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(super) struct FakeServer {
    read: ReadHalf<DuplexStream>,
    write: WriteHalf<DuplexStream>,
}

impl FakeServer {
    pub(super) async fn closed(&mut self) -> bool {
        matches!(
            tokio::time::timeout(
                Duration::from_secs(2),
                transport::read_frame(&mut self.read)
            )
            .await,
            Ok(Err(_))
        )
    }

    pub(super) async fn next(&mut self) -> Value {
        tokio::time::timeout(
            Duration::from_secs(2),
            transport::read_frame(&mut self.read),
        )
        .await
        .unwrap()
        .unwrap()
    }

    pub(super) async fn send(&mut self, message: Value) {
        self.send_raw(&serde_json::to_vec(&message).unwrap()).await;
    }

    pub(super) async fn send_raw(&mut self, bytes: &[u8]) {
        self.write
            .write_all(format!("Content-Length: {}\r\n\r\n", bytes.len()).as_bytes())
            .await
            .unwrap();
        self.write.write_all(bytes).await.unwrap();
        self.write.flush().await.unwrap();
    }
}

fn capabilities(pull: bool) -> Value {
    json!({"textDocumentSync":{"openClose":true,"change":2},"positionEncoding":"utf-16",
        "definitionProvider":true,"referencesProvider":true,"documentSymbolProvider":true,
        "workspaceSymbolProvider":true,"diagnosticProvider":pull})
}

pub(super) async fn fake_client(root: &Path, capabilities: Value) -> (Client, FakeServer) {
    fake_language_client(root, Language::Rust, capabilities).await
}

pub(super) async fn fake_language_client(
    root: &Path,
    language: Language,
    capabilities: Value,
) -> (Client, FakeServer) {
    let (client, server) = duplex(64 * 1024);
    let (read, write) = split(client);
    let (read_server, write_server) = split(server);
    let rpc = transport::Rpc::from_io(read, write, language, uri(root).unwrap());
    let fake = tokio::spawn(async move {
        let mut server = FakeServer {
            read: read_server,
            write: write_server,
        };
        let initialize = server.next().await;
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(
            initialize.pointer("/params/capabilities/general/positionEncodings"),
            Some(&json!(["utf-16"]))
        );
        assert_eq!(
            initialize.pointer("/params/capabilities/workspace/workspaceEdit/documentChanges"),
            Some(&json!(true))
        );
        assert_eq!(
            initialize.pointer("/params/capabilities/workspace/workspaceEdit/resourceOperations"),
            Some(&json!([]))
        );
        assert_eq!(
            initialize.pointer("/params/capabilities/textDocument/rename/prepareSupport"),
            Some(&json!(false))
        );
        if language == Language::Rust {
            assert_eq!(
                initialize.pointer("/params/initializationOptions/procMacro/enable"),
                Some(&json!(false))
            );
            assert_eq!(
                initialize.pointer("/params/capabilities/experimental/serverStatusNotification"),
                Some(&json!(true))
            );
        } else {
            assert!(
                initialize
                    .pointer("/params/capabilities/experimental")
                    .is_none()
            );
        }
        server.send(json!({"jsonrpc":"2.0","id":initialize["id"],"result":{
            "capabilities":capabilities,"serverInfo":{"name":if language == Language::Rust {"rust-analyzer"} else {"typescript-language-server"}}
        }})).await;
        assert_eq!(server.next().await["method"], "initialized");
        let configuration = server.next().await;
        if language == Language::Rust {
            assert_eq!(
                configuration.pointer("/params/settings/rust-analyzer/cargo/buildScripts/enable"),
                Some(&json!(false))
            );
            server
                .send(json!({"jsonrpc":"2.0","method":"experimental/serverStatus",
                "params":{"health":"ok","quiescent":true}}))
                .await;
        }
        server
    });
    let client = tokio::time::timeout(
        Duration::from_secs(2),
        Client::initialize(
            rpc,
            root,
            language,
            "test".into(),
            language.initialization_options(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    (client, fake.await.unwrap())
}

fn arguments(operation: Operation) -> Arguments {
    Arguments {
        operation,
        path: Some("a.rs".into()),
        line: Some(0),
        character: Some(0),
        query: None,
        new_name: None,
        limit: None,
        timeout_seconds: None,
    }
}

#[test]
fn input_positions_are_zero_based_utf16_and_do_not_split_surrogates() {
    let text = "a😀b\r\n";
    for character in [0, 1, 3, 4] {
        assert!(validate_position(text, Some(0), Some(character)).is_ok());
    }
    assert!(
        validate_position(text, Some(0), Some(2))
            .unwrap_err()
            .contains("surrogate")
    );
    assert!(validate_position(text, Some(0), Some(5)).is_err());
    assert!(validate_position(text, Some(1), Some(0)).is_ok());
    assert!(validate_position(text, Some(2), Some(0)).is_err());
}

#[tokio::test]
async fn querying_a_refreshes_previously_opened_b_and_rejects_mid_query_changes() {
    let fixture = Fixture::new();
    let a = fixture.write("a.rs", "fn a() {}\n");
    let b = fixture.write("b.rs", "fn old() {}\n");
    let (mut client, mut server) = fake_client(&fixture.root, capabilities(false)).await;
    client.refresh_documents(&fixture.root, &b).await.unwrap();
    assert_eq!(server.next().await["method"], "textDocument/didOpen");
    fixture.write("b.rs", "fn new() {}\n");
    let context = client.refresh_documents(&fixture.root, &a).await.unwrap();
    let changed = server.next().await;
    assert_eq!(changed["method"], "textDocument/didChange");
    assert_eq!(
        changed.pointer("/params/contentChanges/0/text"),
        Some(&json!("fn new() {}\n"))
    );
    assert_eq!(
        changed.pointer("/params/textDocument/uri"),
        Some(&json!(uri(&b).unwrap()))
    );
    assert_eq!(server.next().await["method"], "textDocument/didOpen");
    assert!(
        context
            .checked_documents
            .iter()
            .any(|d| d.path == b && d.snapshot == snapshot(b"fn new() {}\n"))
    );
    let fake = tokio::spawn(async move {
        let request = server.next().await;
        assert_eq!(request["method"], "textDocument/references");
        server
            .send(json!({"jsonrpc":"2.0","id":request["id"],"result":[]}))
            .await;
        server
    });
    client
        .query(&arguments(Operation::References), &context.document)
        .await
        .unwrap();
    let _server = fake.await.unwrap();
    fixture.write("b.rs", "fn changed_while_querying() {}\n");
    assert!(
        client::verify_snapshots(&fixture.root, &context.checked_documents)
            .await
            .unwrap_err()
            .contains("stale result discarded")
    );
}

#[tokio::test]
async fn deleted_open_document_is_closed_before_next_query() {
    let fixture = Fixture::new();
    let a = fixture.write("a.rs", "fn a() {}\n");
    let b = fixture.write("b.rs", "fn b() {}\n");
    let (mut client, mut server) = fake_client(&fixture.root, capabilities(false)).await;
    client.refresh_documents(&fixture.root, &b).await.unwrap();
    server.next().await;
    std::fs::remove_file(&b).unwrap();
    client.refresh_documents(&fixture.root, &a).await.unwrap();
    let close = server.next().await;
    assert_eq!(close["method"], "textDocument/didClose");
    assert_eq!(
        close.pointer("/params/textDocument/uri"),
        Some(&json!(uri(&b).unwrap()))
    );
    assert!(!client.documents.contains_key(&uri(&b).unwrap()));
}

#[tokio::test]
async fn push_diagnostics_wait_for_real_publication_and_advance_versions() {
    let fixture = Fixture::new();
    let a = fixture.write("a.rs", "fn a() {}\n");
    let (mut client, mut server) = fake_client(&fixture.root, capabilities(false)).await;
    let first = client.refresh_documents(&fixture.root, &a).await.unwrap();
    server.next().await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            client.query(&arguments(Operation::Diagnostics), &first.document)
        )
        .await
        .is_err()
    );
    // publication() alone has no pending JSON-RPC request. Re-synchronization
    // still resets the slot and advances the version for a new cross-file state.
    let next = client.refresh_documents(&fixture.root, &a).await.unwrap();
    assert!(next.document.version > first.document.version);
    server.next().await;
    server
        .send(
            json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics",
        "params":{"uri":next.document.uri,"version":first.document.version,"diagnostics":[]}}),
        )
        .await;
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            client.query(&arguments(Operation::Diagnostics), &next.document)
        )
        .await
        .is_err()
    );
    server
        .send(
            json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics",
        "params":{"uri":next.document.uri,"diagnostics":[]}}),
        )
        .await;
    let payload = tokio::time::timeout(
        Duration::from_secs(1),
        client.query(&arguments(Operation::Diagnostics), &next.document),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!payload.version_verified);
    let rendered = output::render(&fixture.root, &next, Operation::Diagnostics, 10, payload)
        .await
        .unwrap();
    let value: Value = serde_json::from_str(&rendered.text).unwrap();
    assert_eq!(value.pointer("/diagnostics/complete"), Some(&json!(false)));
    assert!(value["diagnostics"]["clean"].is_null());
}

#[tokio::test]
async fn pull_diagnostic_request_requires_full_report() {
    let fixture = Fixture::new();
    let a = fixture.write("a.rs", "fn a() {}\n");
    let (mut client, mut server) = fake_client(&fixture.root, capabilities(true)).await;
    let context = client.refresh_documents(&fixture.root, &a).await.unwrap();
    server.next().await;
    let fake = tokio::spawn(async move {
        let request = server.next().await;
        assert_eq!(request["method"], "textDocument/diagnostic");
        server.send(json!({"jsonrpc":"2.0","id":request["id"],"result":{"kind":"unchanged","resultId":"unrequested"}})).await;
        server
    });
    assert!(
        client
            .query(&arguments(Operation::Diagnostics), &context.document)
            .await
            .err()
            .unwrap()
            .contains("full report")
    );
    let _server = fake.await.unwrap();
}

#[tokio::test]
async fn status_does_not_create_clients_and_read_only_queries_are_denied() {
    let fixture = Fixture::new();
    fixture.write("a.rs", "fn a() {}\n");
    let registry = ToolRegistry::new(&fixture.root, ApprovalMode::ReadOnly).unwrap();
    let value: Value =
        serde_json::from_str(&registry.lsp_status(Some("a.rs")).await.unwrap()).unwrap();
    assert_eq!(value["servers"][0]["runtime"]["state"], "not_started");
    assert!(registry.lsp.clients.lock().unwrap().is_empty());
    let call = ToolCall {
        id: "test".into(),
        name: "lsp".into(),
        arguments: json!({"operation":"definition","path":"a.rs","line":0,"character":0})
            .to_string(),
    };
    assert!(matches!(
        registry.lsp(&call).await,
        Err(ToolError::ReadOnlyPolicy(_))
    ));
}

#[tokio::test]
async fn fixed_server_gate_cannot_borrow_narrow_worker_command_permissions() {
    let fixture = Fixture::new();
    let launch = config::Launch {
        command: "'/trusted/bin/rust-analyzer'".into(),
        options: json!({}),
        path: "/trusted/bin".into(),
    };
    let mut registry = ToolRegistry::new(&fixture.root, ApprovalMode::FullAccess).unwrap();
    registry.command_allowlist = Some(["cargo test".to_owned()].into_iter().collect());
    assert!(matches!(
        registry.gate_lsp_launch(Language::Rust, &launch).await,
        Err(ToolError::ApprovalDenied(_))
    ));
    registry.command_allowlist = None;
    registry.read_only_git_shell = true;
    assert!(matches!(
        registry.gate_lsp_launch(Language::Rust, &launch).await,
        Err(ToolError::ApprovalDenied(_))
    ));
    registry.read_only_git_shell = false;
    registry.delegation_read_only = true;
    assert!(matches!(
        registry.gate_lsp_launch(Language::Rust, &launch).await,
        Err(ToolError::ReadOnlyPolicy(_))
    ));
}
