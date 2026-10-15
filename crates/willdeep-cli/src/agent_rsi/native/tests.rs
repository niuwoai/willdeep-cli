use super::*;
use axum::{Json, Router, extract::State, http::HeaderMap, response::IntoResponse, routing::post};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
struct Broker {
    calls: AtomicUsize,
    requests: Mutex<Vec<Value>>,
    mode: &'static str,
}
async fn completion(
    State(broker): State<Arc<Broker>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    assert_eq!(headers["authorization"], "Bearer offline-native-capability");
    assert_eq!(headers["x-rsi-proposal-sha256"], PROPOSAL);
    assert_eq!(body["model"], "deepseek-v4-flash");
    assert_eq!(body["stream"], false);
    assert_eq!(body["temperature"], 0);
    assert_eq!(body["max_tokens"], 4000);
    let call = broker.calls.fetch_add(1, Ordering::SeqCst);
    broker.requests.lock().unwrap().push(body);
    let message = if call == 0 {
        json!({"content":null,"tool_calls":[{"id":"native-read","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"fixture.txt\"}"}}]})
    } else {
        json!({"content":"CONCLUSION\n{\"has_defect\":true,\"line\":4,\"reason\":\"The loop reaches one past the array end.\"}\nEVIDENCE\nfixture.txt:4\nOPEN QUESTIONS\nnone"})
    };
    let usage = if broker.mode == "unknown_usage" {
        Value::Null
    } else {
        json!({"prompt_tokens":60,"completion_tokens":20,"total_tokens":80})
    };
    let approved_model = if broker.mode == "wrong_route" {
        "unapproved-model"
    } else {
        "deepseek-v4-flash"
    };
    let model = "opaque-upstream-model-id";
    (
        [
            ("X-Request-Id", format!("request-{call}")),
            ("X-RSI-Receipt-Id", format!("receipt-{call}")),
            ("X-Routed-Model", model.into()),
            ("X-RSI-Approved-Model", approved_model.into()),
        ],
        Json(
            json!({"id":format!("response-{call}"),"model":model,"choices":[{"message":message,"finish_reason":if call==0 {"tool_calls"} else {"stop"}}],"usage":usage}),
        ),
    )
}
fn control(origin: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("native-eval-test-{}", uuid::Uuid::new_v4()));
    let golden: Value = serde_json::from_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/examples/agent-rsi-golden.json"
    )))
    .unwrap();
    let package = golden["values"]["candidate"]["baseline_package"].clone();
    let package_path = dir.join("package.json");
    private_write(&package_path, &serde_json::to_vec(&package).unwrap()).unwrap();
    let suite = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../bench/agent-rsi/reviewer-original-24-v1/suite.json");
    let value = json!({"schema":"willdeep.agent-rsi.native-trial-control.v1","proposal_sha256":PROPOSAL,
        "experiment_id":"native-free-test","trial_id":"trial-free-test","arm":"baseline","repetition":1,"attempt":1,"resume_of":null,
        "agent_package":package_path,"agent_revision":BASELINE,"suite":suite,"suite_sha256":SUITE_SHA,
        "task_id":"loop-bound-bug","broker_origin":origin,"capability":"offline-native-capability","output_directory":dir.join("result")});
    let path = dir.join("control.json");
    private_write(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
    (dir, path)
}
#[tokio::test]
async fn preview_captures_real_worker_request_without_network_or_oracle_leak() {
    let (dir, path) = control("http://127.0.0.1:1");
    let observed = execute(&path, true).await.unwrap();
    assert_eq!(observed["worker_status"], "preview_stopped_before_network");
    assert_eq!(observed["request_receipts"], json!([]));
    assert_eq!(observed["usage_complete"], false);
    assert!(observed["input_tokens"].is_null());
    let request = std::fs::read_to_string(dir.join("result/first_request.json")).unwrap();
    assert!(!request.contains("loop-bound-bug"));
    assert!(!request.contains("expected_output"));
    assert!(!request.contains("Offline oracle confirms"));
    assert!(!request.contains("offline-native-capability"));
    assert_eq!(observed["context"]["definition_revision"], BASELINE);
    assert_eq!(observed["context"]["max_turns"], 4);
    std::fs::remove_dir_all(dir).unwrap();
}
#[tokio::test]
async fn native_worker_uses_only_broker_and_real_read_file_with_receipts_and_raw_output() {
    for mode in ["normal", "wrong_route", "unknown_usage"] {
        let broker = Arc::new(Broker {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            mode,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/v1/chat/completions", post(completion))
            .with_state(broker.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (dir, path) = control(&origin);
        let observed = execute(&path, false).await.unwrap();
        assert!(
            !serde_json::to_string(&observed)
                .unwrap()
                .contains("offline-native-capability")
        );
        if mode == "normal" {
            assert_eq!(broker.calls.load(Ordering::SeqCst), 2);
            assert_eq!(observed["mechanism_verifier"]["passed"], true);
            assert_eq!(observed["input_tokens"], 120);
            assert_eq!(observed["loaded_revision"], BASELINE);
            assert_eq!(observed["grading"], "host_only_pending");
            assert_eq!(observed["status"], "unverified");
            assert_eq!(
                observed["request_receipts"][0]["routed_model"],
                "opaque-upstream-model-id"
            );
            assert_eq!(observed["total_tokens"], 160);
            assert!(
                observed["raw_model_content"]
                    .as_str()
                    .unwrap()
                    .contains("\"line\":4")
            );
            assert_eq!(observed["request_receipts"][1]["receipt_id"], "receipt-1");
            let requests = broker.requests.lock().unwrap();
            assert!(
                requests[1]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["role"] == "tool"
                        && message["content"]
                            .as_str()
                            .unwrap_or("")
                            .contains("lowercaseAll"))
            );
            assert!(
                requests
                    .iter()
                    .all(|request| !request.to_string().contains("loop-bound-bug"))
            );
        } else {
            assert_eq!(broker.calls.load(Ordering::SeqCst), 1);
            assert_eq!(observed["mechanism_verifier"]["passed"], false);
            if mode == "unknown_usage" {
                assert!(observed["input_tokens"].is_null());
            }
        }
        server.abort();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
#[test]
fn rejects_remote_broker_and_unapproved_control_before_execution() {
    assert!(BrokerProvider::new("https://some.im", "fake".into(), "trial".into(), false).is_err());
    let (dir, path) = control("http://127.0.0.1:1");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["agent_revision"] = "0".repeat(64).into();
    private_write(&path, &serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(read_control(&path).is_err());
    assert!(!dir.join("result").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn auxiliary_requests_are_rejected_before_any_network_request() {
    let provider = BrokerProvider::new(
        "http://127.0.0.1:1",
        "offline-only".into(),
        "trial".into(),
        false,
    )
    .unwrap();
    let error = willdeep_core::provider::Provider::complete(
        &provider,
        &[willdeep_core::Message::user("summarize this history")],
        &[],
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("auxiliary or context-summary"));
    assert!(provider.captured.lock().unwrap().receipts.is_empty());
}

#[test]
fn read_proof_requires_exact_fixture_result_in_a_delivered_request() {
    let dir = std::env::temp_dir().join(format!("native-read-proof-{}", uuid::Uuid::new_v4()));
    let fixture = dir.join("fixture.txt");
    let content = "one\ntwo\n";
    private_write(&fixture, content.as_bytes()).unwrap();
    private_write(&dir.join("other.txt"), content.as_bytes()).unwrap();
    let row = json!({"sequence":2,"transport_valid":true,"request_id":"r2","receipt_id":"b2",
        "read_results_in_request":[{"tool_call_id":"call1","arguments":{"path":"fixture.txt"},"tool_result_sha256":digest(b"     1  one\n     2  two\n")}]});
    assert_eq!(
        validated_fixture_reads(std::slice::from_ref(&row), &fixture, content).len(),
        1
    );
    let mut wrong_path = row.clone();
    wrong_path["read_results_in_request"][0]["arguments"]["path"] = "other.txt".into();
    assert!(validated_fixture_reads(&[wrong_path], &fixture, content).is_empty());
    let mut wrong_result = row.clone();
    wrong_result["read_results_in_request"][0]["tool_result_sha256"] = digest(b"spoof").into();
    assert!(validated_fixture_reads(&[wrong_result], &fixture, content).is_empty());
    let mut undelivered = row;
    undelivered["transport_valid"] = false.into();
    assert!(validated_fixture_reads(&[undelivered], &fixture, content).is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
