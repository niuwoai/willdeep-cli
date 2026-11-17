use super::*;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

use crate::tools::{ApprovalDecision, Approver, SharedApprovalMode};
use async_trait::async_trait;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(artifact::private_directory(&std::env::temp_dir(), "willdeep-browser-unit").unwrap())
    }
    fn config(&self) -> BrowserConfig {
        // Never executed: rejection tests must stop before launch.
        let executable = std::env::current_exe().unwrap();
        BrowserConfig {
            enabled: true,
            node_path: Some(executable.clone()),
            chrome_path: Some(executable),
            allowed_origins: vec!["http://127.0.0.1:19851".into()],
            artifact_dir: Some(self.0.clone()),
            ..BrowserConfig::default()
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("remove private browser test directory: {error}");
        }
    }
}

#[test]
fn explicit_config_rejects_profile_injection_relative_paths_and_wildcard_origins() {
    assert!(!BrowserConfig::default().enabled);
    assert!(BrowserConfig::default().validate().is_ok());
    for patch in [
        json!({"node_path":"node"}),
        json!({"chrome_path":"/tmp/../chrome"}),
        json!({"allowed_origins":["https://example.test/"]}),
        json!({"allowed_origins":["https://*.example.test"]}),
        json!({"allowed_origins":["https://example.test/path"]}),
        json!({"allowed_origins":["https://user@example.test"]}),
        json!({"allowed_origins":["file:///tmp"]}),
        json!({"timeout_seconds":0}),
    ] {
        let parsed: BrowserConfig = serde_json::from_value(patch).unwrap();
        assert!(parsed.validate().is_err(), "{parsed:?}");
    }
    for forbidden in [
        "profile",
        "user_data_dir",
        "cdp_url",
        "node_options",
        "chrome_args",
        "env",
    ] {
        assert!(serde_json::from_value::<BrowserConfig>(json!({(forbidden):"injected"})).is_err());
    }
}

#[test]
fn navigation_allows_exact_origin_only_and_rejects_schemes_credentials_and_controls() {
    let config = BrowserConfig {
        allowed_origins: vec!["http://127.0.0.1:19851".into()],
        ..BrowserConfig::default()
    };
    assert!(
        config
            .checked_navigation("http://127.0.0.1:19851/form?mode=test#name")
            .is_ok()
    );
    for url in [
        "http://127.0.0.1:19852/",
        "http://localhost:19851/",
        "https://127.0.0.1:19851/",
        "http://user@127.0.0.1:19851/",
        "file:///tmp/private",
        "javascript:alert(1)",
        "data:text/html,<h1>test</h1>",
        "http://127.0.0.1:19851/\npath",
        "http://127.0.0.1:19851\\outside",
    ] {
        assert!(config.checked_navigation(url).is_err(), "{url}");
    }
}

#[test]
fn fixed_actions_reject_js_upload_paths_and_unrelated_parameters() {
    for input in [
        json!({"action":"evaluate","source":"document.title"}),
        json!({"action":"screenshot","path":"/tmp/override.png"}),
        json!({"action":"fill","ref":"a","text":"b","files":["/tmp/file"]}),
        json!({"action":"open","url":"http://127.0.0.1:19851/","profile":"/tmp/profile"}),
    ] {
        assert!(serde_json::from_value::<Arguments>(input).is_err());
    }
    for input in [
        json!({"action":"fill","ref":"a"}),
        json!({"action":"status","text":"x"}),
        json!({"action":"assert","assertion":"url_equals","ref":"a","expected":"x"}),
        json!({"action":"assert","assertion":"text_contains","expected":"x"}),
        json!({"action":"click","ref":"a","url":"http://127.0.0.1:19851/"}),
    ] {
        assert!(
            serde_json::from_value::<Arguments>(input)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}

#[test]
fn host_refs_are_opaque_epoch_bound_and_replaced_by_each_snapshot() {
    let mut refs = BTreeMap::new();
    let mut epoch = 0;
    let payload = json!({"epoch":7,"nodes":[{"backend_node_id":42,"name":"Save"}]});
    let first = bind_snapshot(&mut refs, &mut epoch, payload.clone()).unwrap();
    let key = first["nodes"][0]["ref"].as_str().unwrap().to_owned();
    assert!(first["nodes"][0].get("backend_node_id").is_none());
    assert_ne!(key, "42");
    assert_eq!(
        lookup_reference(&refs, 7, &key).unwrap().backend_node_id,
        42
    );
    assert!(lookup_reference(&refs, 8, &key).is_err());
    let second = bind_snapshot(&mut refs, &mut epoch, payload).unwrap();
    assert_ne!(first["snapshot_id"], second["snapshot_id"]);
    assert!(lookup_reference(&refs, 7, &key).is_err());
    assert!(lookup_reference(&refs, 7, "42").is_err());
}

#[test]
fn snapshot_requires_bounded_nodes_and_backend_identities() {
    let mut refs = BTreeMap::new();
    let mut epoch = 0;
    for payload in [
        json!({"epoch":1,"nodes":[{"name":"missing identity"}]}),
        json!({"nodes":[]}),
        json!({"epoch":1,"nodes":vec![json!({"backend_node_id":1}); MAX_SNAPSHOT_NODES+1]}),
    ] {
        assert!(bind_snapshot(&mut refs, &mut epoch, payload).is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn screenshot_host_generates_unique_private_paths_hash_mime_and_dimensions() {
    let fixture = Fixture::new();
    let artifacts = Artifacts::new(&fixture.0).unwrap();
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQEBAOHN7x8AAAAASUVORK5CYII=";
    let first_pending = artifacts.save(png.into()).await.unwrap();
    let first = first_pending.metadata().clone();
    first_pending.commit().unwrap();
    let second_pending = artifacts.save(png.into()).await.unwrap();
    let second = second_pending.metadata().clone();
    second_pending.commit().unwrap();
    assert_ne!(first["path"], second["path"]);
    assert_eq!(first["mime"], "image/png");
    assert_eq!(first["width"], 1);
    assert_eq!(first["height"], 1);
    let path = PathBuf::from(first["path"].as_str().unwrap());
    assert!(path.starts_with(fixture.0.canonicalize().unwrap()));
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        first["sha256"],
        format!("sha256:{:x}", Sha256::digest(&bytes))
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

struct RecordingApprover {
    calls: StdMutex<Vec<(String, bool)>>,
    switch: Option<SharedApprovalMode>,
    decision: ApprovalDecision,
}
#[async_trait]
impl Approver for RecordingApprover {
    async fn approve(&self, description: &str, always: bool) -> ApprovalDecision {
        self.calls
            .lock()
            .unwrap()
            .push((description.into(), always));
        if let Some(mode) = &self.switch {
            mode.set(ApprovalMode::ReadOnly);
        }
        self.decision
    }
}

#[tokio::test]
async fn exact_action_approval_is_not_remembered_and_denial_never_starts_chrome() {
    let fixture = Fixture::new();
    let approver = Arc::new(RecordingApprover {
        calls: StdMutex::new(Vec::new()),
        switch: None,
        decision: ApprovalDecision::Deny,
    });
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
        .unwrap()
        .with_browser_config(fixture.config())
        .unwrap()
        .with_approver(approver.clone());
    for path in ["one", "two"] {
        let result = registry
            .browser(json!({"action":"open","url":format!("http://127.0.0.1:19851/{path}")}))
            .await;
        assert!(matches!(result, Err(ToolError::ApprovalDenied(_))));
    }
    {
        let calls = approver.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|(_, always)| !always));
        assert_ne!(calls[0].0, calls[1].0);
    }
    assert!(registry.browser.session.lock().await.is_none());
}

#[tokio::test]
async fn permission_is_rechecked_after_one_time_approval_before_launch() {
    let fixture = Fixture::new();
    let mode = SharedApprovalMode::new(ApprovalMode::Strict);
    let approver = Arc::new(RecordingApprover {
        calls: StdMutex::new(Vec::new()),
        switch: Some(mode.clone()),
        decision: ApprovalDecision::AllowOnce,
    });
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
        .unwrap()
        .with_shared_approval_mode(mode)
        .with_browser_config(fixture.config())
        .unwrap()
        .with_approver(approver);
    let result = registry
        .browser(json!({"action":"open","url":"http://127.0.0.1:19851/"}))
        .await;
    assert!(result.is_err());
    assert!(registry.browser.session.lock().await.is_none());
}

#[tokio::test]
async fn disabled_missing_dependency_and_unstarted_status_never_claim_ready() {
    let fixture = Fixture::new();
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::FullAccess).unwrap();
    let result = registry.browser_status().await;
    assert_eq!(result["ready"], false);
    assert_eq!(result["enabled"], false);
    let registry = registry.with_browser_config(fixture.config()).unwrap();
    assert_eq!(registry.browser_status().await["runtime"], "not_started");
    assert_eq!(registry.browser_status().await["ready"], false);
    let mut config = fixture.config();
    config.chrome_path = Some(fixture.0.join("missing-chrome"));
    let registry = registry.with_browser_config(config).unwrap();
    assert_eq!(
        registry.browser_status().await["dependencies"]["paths_available"],
        false
    );
    assert_eq!(registry.browser_status().await["ready"], false);
}

#[tokio::test]
async fn offline_configuration_status_never_waits_on_or_removes_a_busy_session() {
    let fixture = Fixture::new();
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Smart)
        .unwrap()
        .with_browser_config(fixture.config())
        .unwrap();
    let slot = registry.browser.session.lock().await;
    let status = registry.browser_configuration_status();
    assert_eq!(status["mode"], "configuration_only");
    assert_eq!(status["cache"], "busy");
    assert_eq!(status["ready"], false);
    assert_eq!(status["dependencies"]["versions_verified"], false);
    drop(slot);
    assert_eq!(registry.browser_configuration_status()["cache"], "absent");
}

#[tokio::test]
async fn worker_allowlist_and_enforcing_sandbox_cannot_be_bypassed_by_browser() {
    let fixture = Fixture::new();
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Strict)
        .unwrap()
        .with_allowed_tools(["read_file".into()]);
    assert!(browser_fence(&registry).is_err());
    let registry = ToolRegistry::new(&fixture.0, ApprovalMode::Smart)
        .unwrap()
        .with_workspace_sandbox(Some(crate::sandbox::SandboxSpec::new(
            crate::sandbox::SandboxPolicy::ReadOnly,
            [],
        )));
    // Workspace sandbox only applies in WorkspaceAccess.
    registry
        .approval_mode_handle()
        .set(ApprovalMode::WorkspaceAccess);
    assert!(browser_fence(&registry).is_err());
}
