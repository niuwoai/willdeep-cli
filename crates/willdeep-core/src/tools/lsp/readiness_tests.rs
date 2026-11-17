//! rust-analyzer readiness protocol regressions; no external server or sleeps.
use super::*;

fn initializing(root: &Path) -> (tokio::task::JoinHandle<Result<Client, String>>, FakeServer) {
    initializing_with_deadline(root, None)
}

fn initializing_with_deadline(
    root: &Path,
    deadline: Option<Duration>,
) -> (tokio::task::JoinHandle<Result<Client, String>>, FakeServer) {
    let (client, server) = duplex(64 * 1024);
    let (read, write) = split(client);
    let (read_server, write_server) = split(server);
    let rpc = transport::Rpc::from_io(read, write, Language::Rust, uri(root).unwrap());
    let root = root.to_owned();
    let task = tokio::spawn(async move {
        let initialize = Client::initialize(
            rpc,
            &root,
            Language::Rust,
            "readiness-test".into(),
            Language::Rust.initialization_options(),
        );
        if let Some(deadline) = deadline {
            tokio::time::timeout(deadline, initialize)
                .await
                .map_err(|_| "readiness deadline exceeded".to_owned())?
        } else {
            initialize.await
        }
    });
    (
        task,
        FakeServer {
            read: read_server,
            write: write_server,
        },
    )
}

async fn initialize_reply(server: &mut FakeServer, server_info: Value) {
    let request = server.next().await;
    assert_eq!(request["method"], "initialize");
    assert_eq!(
        request.pointer("/params/capabilities/experimental/serverStatusNotification"),
        Some(&json!(true))
    );
    server
        .send(json!({"jsonrpc":"2.0","id":request["id"],"result":{
            "capabilities":capabilities(true),"serverInfo":server_info
        }}))
        .await;
}

async fn initialized(server: &mut FakeServer) {
    assert_eq!(server.next().await["method"], "initialized");
    assert_eq!(
        server.next().await["method"],
        "workspace/didChangeConfiguration"
    );
}

async fn status(server: &mut FakeServer, health: &str, quiescent: bool) {
    server
        .send(json!({"jsonrpc":"2.0","method":"experimental/serverStatus",
            "params":{"health":health,"quiescent":quiescent,"message":"fixture status"}}))
        .await;
    // A server request after the notification gives a deterministic reader
    // acknowledgement, without sleeps or scheduling assumptions.
    server
        .send(
            json!({"jsonrpc":"2.0","id":"status-observed","method":"workspace/configuration",
            "params":{"items":[]}}),
        )
        .await;
    let acknowledgement = server.next().await;
    assert_eq!(acknowledgement["id"], "status-observed");
    assert_eq!(acknowledgement["result"], json!([]));
}

async fn assert_pending<T>(task: &mut tokio::task::JoinHandle<T>) {
    assert!(
        tokio::time::timeout(Duration::from_millis(20), task)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn initialize_waits_for_healthy_quiescence_and_retains_early_notification() {
    for early in [false, true] {
        let fixture = Fixture::new();
        let (mut task, mut server) = initializing(&fixture.root);
        initialize_reply(&mut server, json!({"name":"rust-analyzer"})).await;
        if early {
            // The reader observes status before initialize reaches its wait.
            assert_eq!(server.next().await["method"], "initialized");
            server
                .send(json!({"jsonrpc":"2.0","method":"experimental/serverStatus",
                    "params":{"health":"ok","quiescent":true}}))
                .await;
            assert_eq!(
                server.next().await["method"],
                "workspace/didChangeConfiguration"
            );
        } else {
            initialized(&mut server).await;
            assert_pending(&mut task).await;
            status(&mut server, "ok", false).await;
            assert_pending(&mut task).await;
            status(&mut server, "warning", false).await;
            assert_pending(&mut task).await;
            status(&mut server, "ok", true).await;
        }
        let client = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(client.documents.is_empty());
        // A notification already consumed by the reader must remain available
        // to subsequent waiters instead of requiring another status change.
        tokio::time::timeout(Duration::from_secs(1), client.wait_until_ready())
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn unknown_or_unidentified_rust_servers_are_rejected_without_waiting() {
    for info in [Value::Null, json!({"name":"other-rust-server"}), json!({})] {
        let fixture = Fixture::new();
        let (task, mut server) = initializing(&fixture.root);
        initialize_reply(&mut server, info).await;
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .err()
            .expect("unsupported server must fail");
        assert!(error.contains("unsupported"), "{error}");
        assert!(server.closed().await);
    }
}

#[tokio::test]
async fn missing_or_busy_readiness_deadlines_and_cancellation_close_the_transport() {
    for busy in [false, true] {
        for cancel in [false, true] {
            let fixture = Fixture::new();
            let (mut task, mut server) = initializing_with_deadline(
                &fixture.root,
                (!cancel).then_some(Duration::from_millis(200)),
            );
            initialize_reply(&mut server, json!({"name":"rust-analyzer"})).await;
            initialized(&mut server).await;
            if busy {
                status(&mut server, "ok", false).await;
            }
            assert_pending(&mut task).await;
            if cancel {
                task.abort();
                assert!(
                    task.await
                        .err()
                        .expect("task must be cancelled")
                        .is_cancelled()
                );
            } else {
                // The deadline owns the same initialize future and Rpc, just
                // as the public tool does; timeout must destroy that transport.
                let error = task.await.unwrap().err().expect("readiness must time out");
                assert!(error.contains("deadline"));
            }
            assert!(server.closed().await);
        }
    }
}

#[tokio::test]
async fn unhealthy_and_malformed_readiness_never_finish_initialization() {
    for params in [
        json!({"health":"warning","quiescent":true,"message":"missing dependencies"}),
        json!({"health":"error","quiescent":true,"message":"failed to load project"}),
        json!({"health":"error","quiescent":false}),
        json!({"health":"ok"}),
        json!({"health":"unknown","quiescent":true}),
        json!({"health":"ok","quiescent":"true"}),
    ] {
        let fixture = Fixture::new();
        let (task, mut server) = initializing(&fixture.root);
        initialize_reply(&mut server, json!({"name":"rust-analyzer"})).await;
        initialized(&mut server).await;
        server
            .send(json!({"jsonrpc":"2.0","method":"experimental/serverStatus","params":params}))
            .await;
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .err()
            .expect("invalid readiness must fail");
        assert!(error.contains("readiness"), "{error}");
        assert!(server.closed().await);
    }
}

#[tokio::test]
async fn every_semantic_query_waits_for_current_project_readiness() {
    for operation in [
        Operation::Definition,
        Operation::References,
        Operation::Symbols,
        Operation::Diagnostics,
    ] {
        let fixture = Fixture::new();
        let path = fixture.write("a.rs", "fn a() {}\n");
        let (mut client, mut server) = fake_client(&fixture.root, capabilities(true)).await;
        let context = client
            .refresh_documents(&fixture.root, &path)
            .await
            .unwrap();
        assert_eq!(server.next().await["method"], "textDocument/didOpen");
        status(&mut server, "ok", false).await;
        let mut task = tokio::spawn(async move {
            let result = client.query(&arguments(operation), &context.document).await;
            (client, result)
        });
        assert_pending(&mut task).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), server.next())
                .await
                .is_err()
        );
        server
            .send(json!({"jsonrpc":"2.0","method":"experimental/serverStatus",
            "params":{"health":"ok","quiescent":true}}))
            .await;
        let request = server.next().await;
        let expected = match operation {
            Operation::Definition => "textDocument/definition",
            Operation::References => "textDocument/references",
            Operation::Symbols => "textDocument/documentSymbol",
            Operation::Diagnostics => "textDocument/diagnostic",
            _ => unreachable!(),
        };
        assert_eq!(request["method"], expected);
        let result = if operation == Operation::Diagnostics {
            json!({"kind":"full","items":[]})
        } else {
            json!([])
        };
        server
            .send(json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
            .await;
        let (_client, result) = task.await.unwrap();
        assert!(result.is_ok());
    }
}

#[tokio::test]
async fn empty_diagnostics_are_rejected_if_readiness_fails_or_reloads_in_flight() {
    for reload in [false, true] {
        let fixture = Fixture::new();
        let path = fixture.write("a.rs", "fn broken( {\n");
        let (mut client, mut server) = fake_client(&fixture.root, capabilities(true)).await;
        let context = client
            .refresh_documents(&fixture.root, &path)
            .await
            .unwrap();
        server.next().await;
        let task = tokio::spawn(async move {
            let result = client
                .query(&arguments(Operation::Diagnostics), &context.document)
                .await;
            (client, result)
        });
        let request = server.next().await;
        assert_eq!(request["method"], "textDocument/diagnostic");
        if reload {
            status(&mut server, "ok", false).await;
            status(&mut server, "ok", true).await;
        } else {
            status(&mut server, "error", true).await;
        }
        server
            .send(json!({"jsonrpc":"2.0","id":request["id"],"result":{"kind":"full","items":[]}}))
            .await;
        let (_client, result) = task.await.unwrap();
        let error = result
            .err()
            .expect("empty diagnostics must not be rendered as clean");
        assert!(error.contains("readiness"), "{error}");
    }
}

#[tokio::test]
async fn rust_rename_discards_final_versioned_edit_when_readiness_fails() {
    let fixture = Fixture::new();
    let path = fixture.write("a.rs", "fn a() {}\n");
    let mut caps = capabilities(false);
    caps["renameProvider"] = json!(true);
    let (mut client, mut server) = fake_client(&fixture.root, caps).await;
    let context = client
        .refresh_documents(&fixture.root, &path)
        .await
        .unwrap();
    server.next().await;
    let mut args = arguments(Operation::RenamePreview);
    args.character = Some(3);
    args.new_name = Some("b".into());
    let root = fixture.root.clone();
    let task = tokio::spawn(async move {
        let result = rename::replacements(&mut client, &root, &args, &context).await;
        (client, result)
    });
    let discovery = server.next().await;
    assert_eq!(discovery["method"], "textDocument/rename");
    let edit = json!({"range":{"start":{"line":0,"character":3},"end":{"line":0,"character":4}},"newText":"b"});
    server
        .send(
            json!({"jsonrpc":"2.0","id":discovery["id"],"result":{"documentChanges":[{
                "textDocument":{"uri":uri(&path).unwrap(),"version":1},"edits":[edit]
            }]}}),
        )
        .await;
    let changed = server.next().await;
    assert_eq!(changed["method"], "textDocument/didChange");
    let final_request = server.next().await;
    assert_eq!(final_request["method"], "textDocument/rename");
    status(&mut server, "error", true).await;
    server.send(json!({"jsonrpc":"2.0","id":final_request["id"],"result":{"documentChanges":[{
        "textDocument":{"uri":uri(&path).unwrap(),"version":changed["params"]["textDocument"]["version"]},"edits":[edit]
    }]}})).await;
    let (_client, result) = task.await.unwrap();
    assert!(
        result
            .err()
            .expect("failed readiness cannot produce a rename preview")
            .contains("readiness")
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "fn a() {}\n");
}
