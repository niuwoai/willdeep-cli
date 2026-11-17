use super::*;
use crate::tools::lsp::tests::{receive, send};
use tokio::io::AsyncReadExt;

fn duplex_rpc() -> (Rpc, tokio::io::DuplexStream) {
    let (client, server) = tokio::io::duplex(1024 * 1024);
    let (read, write) = tokio::io::split(client);
    (
        Rpc::from_io(read, write, Language::Rust, "file:///workspace".into()),
        server,
    )
}

#[tokio::test]
async fn frames_reject_oversize_duplicate_headers_and_invalid_envelopes() {
    for header in [
        format!("Content-Length: {}\r\n\r\n", MAX_FRAME_BYTES + 1),
        "Content-Length: 1\r\nContent-Length: 1\r\n\r\n".to_owned(),
        "Content-Type: application/json\r\n\r\n".to_owned(),
        "Content-Length: -1\r\n\r\n".to_owned(),
        "x".repeat(MAX_HEADER_BYTES),
    ] {
        let (mut writer, mut reader) = tokio::io::duplex(16 * 1024);
        writer.write_all(header.as_bytes()).await.unwrap();
        assert!(read_frame(&mut reader).await.is_err());
    }
    let (mut rpc, mut server) = duplex_rpc();
    let task = tokio::spawn(async move { rpc.request("fake", json!({})).await });
    receive(&mut server).await;
    send(&mut server, json!({"jsonrpc":"1.0","id":1,"result":[]})).await;
    assert!(task.await.unwrap().unwrap_err().contains("envelope"));
}

#[tokio::test]
async fn server_requests_are_answered_and_apply_edit_is_refused_during_query() {
    let (mut rpc, mut server) = duplex_rpc();
    let task = tokio::spawn(async move {
        let result = rpc.request("textDocument/definition", json!({})).await;
        (rpc, result)
    });
    let query = receive(&mut server).await;
    send(&mut server,json!({"jsonrpc":"2.0","id":"cfg","method":"workspace/configuration","params":{"items":[
        {"section":"rust-analyzer.cargo.buildScripts.enable"},{"section":"rust-analyzer.procMacro.enable"}
    ]}})).await;
    let configuration = receive(&mut server).await;
    assert_eq!(configuration["id"], "cfg");
    assert_eq!(configuration["result"], json!([false, false]));
    send(&mut server,json!({"jsonrpc":"2.0","id":"edit","method":"workspace/applyEdit","params":{"edit":{"changes":{}}}})).await;
    let edit = receive(&mut server).await;
    assert_eq!(edit["result"]["applied"], false);
    send(&mut server,json!({"jsonrpc":"2.0","id":"run","method":"workspace/executeCommand","params":{"command":"dangerous"}})).await;
    assert_eq!(receive(&mut server).await["error"]["code"], -32601);
    // Unknown response ids are never mistaken for the active request.
    send(
        &mut server,
        json!({"jsonrpc":"2.0","id":9999,"result":"stale"}),
    )
    .await;
    send(
        &mut server,
        json!({"jsonrpc":"2.0","id":query["id"],"result":"current"}),
    )
    .await;
    let (_rpc, result) = task.await.unwrap();
    assert_eq!(result.unwrap(), "current");
}

#[tokio::test]
async fn rust_status_before_waiter_is_retained_and_new_loading_invalidates_revision() {
    let (rpc, mut server) = duplex_rpc();
    for quiescent in [true, false] {
        send(
            &mut server,
            json!({"jsonrpc":"2.0","method":"experimental/serverStatus",
            "params":{"health":"ok","quiescent":quiescent}}),
        )
        .await;
        send(
            &mut server,
            json!({"jsonrpc":"2.0","id":"observed","method":"workspace/configuration",
            "params":{"items":[]}}),
        )
        .await;
        assert_eq!(receive(&mut server).await["id"], "observed");
        if quiescent {
            let revision = tokio::time::timeout(Duration::from_secs(1), rpc.rust_ready())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(revision, 1);
        }
    }
    // The second status was fully consumed before this waiter is created. It
    // must wait for new readiness rather than reusing the first ready state.
    let mut task = tokio::spawn(async move {
        let revision = rpc.rust_ready().await;
        (rpc, revision)
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut task)
            .await
            .is_err()
    );
    send(
        &mut server,
        json!({"jsonrpc":"2.0","method":"experimental/serverStatus",
        "params":{"health":"ok","quiescent":true}}),
    )
    .await;
    let (_rpc, revision) = task.await.unwrap();
    assert_eq!(revision.unwrap(), 3);
}

#[tokio::test]
async fn publication_waits_for_matching_version_and_bounds_retained_items() {
    let (rpc, mut server) = duplex_rpc();
    rpc.track_document("file:///workspace/a.rs", true).unwrap();
    let mut task = tokio::spawn(async move {
        let publication = rpc.publication("file:///workspace/a.rs", 2).await;
        (rpc, publication)
    });
    send(
        &mut server,
        json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{
            "uri":"file:///workspace/a.rs","version":1,"diagnostics":[]
        }}),
    )
    .await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut task)
            .await
            .is_err()
    );
    send(&mut server,json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{
        "uri":"file:///workspace/a.rs","version":2,"diagnostics":vec![json!({"message":"bounded"});MAX_DIAGNOSTICS + 10]
    }})).await;
    let (rpc, publication) = task.await.unwrap();
    let publication = publication.unwrap();
    assert_eq!(publication.version, Some(2));
    assert_eq!(publication.diagnostics.len(), MAX_DIAGNOSTICS);
    assert!(publication.truncated);
    rpc.forget_document("file:///workspace/a.rs");
    assert!(rpc.shared.publications.lock().unwrap().is_empty());
}

#[tokio::test]
async fn timeout_and_cancellation_drop_streams_and_background_tasks() {
    for abort_query in [false, true] {
        let (mut rpc, mut server) = duplex_rpc();
        let reader = rpc.reader.abort_handle();
        let task = tokio::spawn(async move {
            let _ = rpc.request("never-answered", json!({})).await;
        });
        receive(&mut server).await;
        if abort_query {
            task.abort();
            let _ = task.await;
        } else {
            let mut task = task;
            assert!(
                tokio::time::timeout(Duration::from_millis(20), &mut task)
                    .await
                    .is_err()
            );
            // This matches the outer tool deadline, which drops its owned client.
            task.abort();
            let _ = task.await;
        }
        let eof = tokio::time::timeout(Duration::from_secs(2), server.read_u8())
            .await
            .unwrap();
        assert!(eof.is_err(), "cancelled transport must close its stream");
        tokio::task::yield_now().await;
        assert!(reader.is_finished());
    }
}

#[tokio::test]
async fn an_owned_rpc_deadline_destroys_the_transport() {
    let (mut rpc, mut server) = duplex_rpc();
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_millis(40), async move {
            rpc.request("never-answered", json!({})).await
        })
        .await
    });
    receive(&mut server).await;
    assert!(task.await.unwrap().is_err());
    assert!(
        tokio::time::timeout(Duration::from_secs(2), server.read_u8())
            .await
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn outgoing_frames_and_server_configuration_requests_are_bounded() {
    let (mut rpc, _server) = duplex_rpc();
    let oversized = "x".repeat(MAX_FRAME_BYTES);
    assert!(
        rpc.request("fake", json!({"text":oversized}))
            .await
            .unwrap_err()
            .contains("exceeds")
    );
    assert!(rpc.shared.pending.lock().unwrap().is_empty());
    let reply = server_reply(
        json!(1),
        "workspace/configuration",
        &json!({"items":vec![json!({});MAX_SERVER_REQUEST_ITEMS + 1]}),
        Language::Rust,
        "file:///workspace",
    );
    assert_eq!(reply["error"]["code"], -32602);
}

#[cfg(unix)]
#[tokio::test]
async fn process_drop_kills_descendants_and_aborts_stderr_drain() {
    use crate::tools::lsp::tests::Fixture;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use tokio::io::AsyncBufReadExt;

    let _fixture = Fixture::new();
    let mut command = tokio::process::Command::new("/bin/sh");
    // A local fake process, with a real descendant and an unbounded stderr
    // producer. This does not require any language-server installation.
    command.args(["-c","sleep 30 & printf '%s\\n' \"$!\"; while :; do printf 'stderr noise\\n' >&2; sleep 0.01; done"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    command.as_std_mut().process_group(0);
    let mut child = command.spawn().unwrap();
    let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap());
    let mut pid = String::new();
    tokio::time::timeout(Duration::from_secs(2), stdout.read_line(&mut pid))
        .await
        .unwrap()
        .unwrap();
    let descendant: u32 = pid.trim().parse().unwrap();
    let mut rpc = Rpc::from_io(
        stdout,
        child.stdin.take().unwrap(),
        Language::Rust,
        "file:///workspace".into(),
    );
    let stderr = child.stderr.take().unwrap();
    rpc.attach_process(child, stderr);
    let reader = rpc.reader.abort_handle();
    let stderr = rpc.stderr.as_ref().unwrap().abort_handle();
    let retained = rpc.shared.clone();
    drop(rpc);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let state = tokio::process::Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", &descendant.to_string()])
                .output()
                .await
                .unwrap();
            let state = String::from_utf8_lossy(&state.stdout);
            // An orphan can briefly remain a zombie on CI; it is terminated and
            // owns no stdio handles even before init reaps it.
            if state.trim().is_empty() || state.trim_start().starts_with('Z') {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::task::yield_now().await;
    assert!(reader.is_finished());
    assert!(stderr.is_finished());
    assert!(retained.stderr.lock().unwrap().len() <= MAX_STDERR_BYTES);
}
