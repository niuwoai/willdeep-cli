//! Shared-execute tests plus explicitly selected local Node/Chrome acceptance.
//! Normal tests do not require Node, Chrome, a network service or user config.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use willdeep_core::tools::{ApprovalDecision, Approver, BrowserConfig};
use willdeep_core::{ApprovalMode, ToolCall, ToolRegistry};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "willdeep-browser-integration-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn config(&self) -> BrowserConfig {
        BrowserConfig {
            enabled: true,
            node_path: Some(
                std::env::var_os("WILLDEEP_TEST_BROWSER_NODE")
                    .map(PathBuf::from)
                    .expect("explicit trusted Node 24 path"),
            ),
            chrome_path: Some(
                std::env::var_os("WILLDEEP_TEST_BROWSER_CHROME")
                    .map(PathBuf::from)
                    .expect("explicit trusted Chrome path"),
            ),
            artifact_dir: Some(self.0.clone()),
            allowed_origins: vec!["http://127.0.0.1:19851".into()],
            ..BrowserConfig::default()
        }
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("remove private browser integration workspace: {error}");
        }
    }
}

fn call(args: Value) -> ToolCall {
    ToolCall {
        id: uuid::Uuid::new_v4().to_string(),
        name: "browser".into(),
        arguments: args.to_string(),
    }
}
async fn query(registry: &ToolRegistry, args: Value) -> Value {
    serde_json::from_str(
        &registry
            .execute(&call(args))
            .await
            .expect("browser execute"),
    )
    .expect("structured browser result")
}
fn reference(snapshot: &Value, id: &str) -> String {
    snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == id)
        .unwrap_or_else(|| panic!("missing snapshot node {id}"))["ref"]
        .as_str()
        .unwrap()
        .into()
}

#[tokio::test]
async fn shared_execute_reports_disabled_browser_without_ready_claim() {
    let workspace = Workspace::new();
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::Smart).unwrap();
    let status = query(&registry, json!({"action":"status"})).await;
    assert_eq!(status["enabled"], false);
    assert_eq!(status["ready"], false);
    assert_eq!(status["runtime"], "not_started");
    assert!(
        registry
            .definitions()
            .iter()
            .any(|definition| definition.name == "browser")
    );
}

#[tokio::test]
async fn shared_execute_rejects_arbitrary_js_user_paths_and_profile_attachment() {
    let workspace = Workspace::new();
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::FullAccess).unwrap();
    for input in [
        json!({"action":"evaluate","source":"document.title"}),
        json!({"action":"screenshot","path":"/tmp/unapproved.png"}),
        json!({"action":"open","url":"http://127.0.0.1:19851/","profile":"/tmp/user-profile"}),
        json!({"action":"fill","ref":"forged","text":"test","files":["/tmp/private"]}),
    ] {
        assert!(registry.execute(&call(input)).await.is_err());
    }
}

#[tokio::test]
async fn shared_execute_read_only_and_worker_tool_restrictions_block_browser() {
    let workspace = Workspace::new();
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::ReadOnly).unwrap();
    assert!(
        registry
            .execute(&call(json!({"action":"status"})))
            .await
            .is_err()
    );
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::Smart)
        .unwrap()
        .with_allowed_tools(["read_file".into()]);
    assert!(
        registry
            .execute(&call(
                json!({"action":"open","url":"http://127.0.0.1:19851/"})
            ))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn read_only_close_is_strictly_limited_to_the_owning_parent() {
    let workspace = Workspace::new();
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::ReadOnly).unwrap();
    assert_eq!(
        query(&registry, json!({"action":"close"})).await["closed"],
        false
    );
    for args in [
        json!({"action":"open","url":"http://127.0.0.1:19851/"}),
        json!({"action":"close","text":"injected"}),
        json!({"action":"close","timeout_seconds":0}),
    ] {
        assert!(registry.execute(&call(args)).await.is_err());
    }
    let worker = ToolRegistry::new(&workspace.0, ApprovalMode::ReadOnly)
        .unwrap()
        .with_delegation_read_only();
    assert!(
        worker
            .execute(&call(json!({"action":"close"})))
            .await
            .is_err()
    );
    let limited = ToolRegistry::new(&workspace.0, ApprovalMode::ReadOnly)
        .unwrap()
        .with_allowed_tools(["read_file".into()]);
    assert!(
        limited
            .execute(&call(json!({"action":"close"})))
            .await
            .is_err()
    );
}

struct ExactApprover {
    count: AtomicUsize,
    pause: AtomicBool,
    entered: tokio::sync::Notify,
}
impl ExactApprover {
    fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            pause: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(),
        }
    }
}
#[async_trait]
impl Approver for ExactApprover {
    async fn approve(&self, description: &str, always: bool) -> ApprovalDecision {
        assert!(!always, "browser cannot authorize the tool permanently");
        assert!(
            description.contains("sha256:"),
            "approval must identify the exact action"
        );
        self.count.fetch_add(1, Ordering::SeqCst);
        if self.pause.load(Ordering::SeqCst) {
            self.entered.notify_one();
            return std::future::pending().await;
        }
        ApprovalDecision::AllowOnce
    }
}

#[tokio::test]
#[ignore = "requires explicit Node/Chrome paths and the existing local fixture at 127.0.0.1:19851; no downloads"]
async fn real_local_browser_shared_execute_form_refs_artifact_route_and_origin_denial() {
    let workspace = Workspace::new();
    let approver = Arc::new(ExactApprover::new());
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::Strict)
        .unwrap()
        .with_browser_config(workspace.config())
        .unwrap()
        .with_approver(approver.clone());
    query(
        &registry,
        json!({"action":"open","url":"http://127.0.0.1:19851/"}),
    )
    .await;
    let snapshot = query(&registry, json!({"action":"snapshot"})).await;
    let old_input = reference(&snapshot, "name");
    query(
        &registry,
        json!({"action":"fill","ref":old_input,"text":"Native browser synthetic item"}),
    )
    .await;
    assert!(
        registry
            .execute(&call(
                json!({"action":"fill","ref":old_input,"text":"stale"})
            ))
            .await
            .is_err()
    );
    // Fresh process after a failed lease, with a fresh profile and no saved refs.
    query(
        &registry,
        json!({"action":"open","url":"http://127.0.0.1:19851/"}),
    )
    .await;
    let snapshot = query(&registry, json!({"action":"snapshot"})).await;
    query(&registry, json!({"action":"fill","ref":reference(&snapshot,"name"),"text":"Native browser synthetic item"})).await;
    let snapshot = query(&registry, json!({"action":"snapshot"})).await;
    query(
        &registry,
        json!({"action":"click","ref":reference(&snapshot,"save")}),
    )
    .await;
    let mut saved = false;
    for _ in 0..50 {
        let snapshot = query(&registry, json!({"action":"snapshot"})).await;
        let assertion = query(&registry, json!({"action":"assert","assertion":"text_equals","ref":reference(&snapshot,"status"),"expected":"Saved"})).await;
        if assertion["passed"] == true {
            saved = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(saved, "local form did not report Saved");
    let screenshot = query(&registry, json!({"action":"screenshot"})).await;
    let artifact = &screenshot["artifact"];
    let path = PathBuf::from(artifact["path"].as_str().unwrap());
    assert!(path.starts_with(workspace.0.canonicalize().unwrap()));
    assert_eq!(artifact["mime"], "image/png");
    assert_eq!(artifact["width"], 1280);
    assert!(artifact["sha256"].as_str().unwrap().starts_with("sha256:"));
    let snapshot = query(&registry, json!({"action":"snapshot"})).await;
    query(
        &registry,
        json!({"action":"click","ref":reference(&snapshot,"next")}),
    )
    .await;
    assert_eq!(query(&registry, json!({"action":"assert","assertion":"url_equals","expected":"http://127.0.0.1:19851/next"})).await["passed"], true);
    assert!(
        registry
            .execute(&call(
                json!({"action":"open","url":"http://127.0.0.1:19852/denied"})
            ))
            .await
            .is_err()
    );
    assert!(approver.count.load(Ordering::SeqCst) >= 6);
    query(&registry, json!({"action":"close"})).await;
}

#[tokio::test]
#[ignore = "requires explicit Node/Chrome paths and the local synthetic fixture; no downloads"]
async fn real_local_browser_rejects_page_world_spoofing_and_can_close_after_read_only_switch() {
    let workspace = Workspace::new();
    let registry = ToolRegistry::new(&workspace.0, ApprovalMode::FullAccess)
        .unwrap()
        .with_browser_config(workspace.config())
        .unwrap();
    query(
        &registry,
        json!({"action":"open","url":"http://127.0.0.1:19851/hostile-dom"}),
    )
    .await;
    let snapshot = query(&registry, json!({"action":"snapshot"})).await;
    let error = registry
        .execute(&call(
            json!({"action":"click","ref":reference(&snapshot,"approved")}),
        ))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ELEMENT_DENIED"), "{error}");
    query(
        &registry,
        json!({"action":"open","url":"http://127.0.0.1:19851/"}),
    )
    .await;
    let mode = registry.approval_mode_handle();
    mode.set(ApprovalMode::ReadOnly);
    assert!(
        registry
            .execute(&call(json!({"action":"snapshot"})))
            .await
            .is_err()
    );
    assert_eq!(
        query(&registry, json!({"action":"close"})).await["closed"],
        true
    );
    mode.set(ApprovalMode::FullAccess);
    assert_eq!(
        query(&registry, json!({"action":"status"})).await["runtime"],
        "not_started"
    );
}

#[tokio::test]
#[ignore = "requires explicit Node/Chrome paths and the existing local fixture; kills only the owned isolated browser"]
async fn real_local_browser_cancellation_during_approval_retires_owned_session() {
    let workspace = Workspace::new();
    let approver = Arc::new(ExactApprover::new());
    let registry = Arc::new(
        ToolRegistry::new(&workspace.0, ApprovalMode::Strict)
            .unwrap()
            .with_browser_config(workspace.config())
            .unwrap()
            .with_approver(approver.clone()),
    );
    query(
        &registry,
        json!({"action":"open","url":"http://127.0.0.1:19851/"}),
    )
    .await;
    let snapshot = query(&registry, json!({"action":"snapshot"})).await;
    approver.pause.store(true, Ordering::SeqCst);
    let running = registry.clone();
    let pending = tokio::spawn(async move {
        running.execute(&call(json!({"action":"fill","ref":reference(&snapshot,"name"),"text":"cancelled before write"}))).await
    });
    tokio::time::timeout(Duration::from_secs(3), approver.entered.notified())
        .await
        .unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    let status = query(&registry, json!({"action":"status"})).await;
    assert_eq!(status["runtime"], "not_started");
    assert_eq!(status["ready"], false);
}
