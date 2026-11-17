//! Native process-tree cancellation coverage, without Node/Chrome or a shell.
use super::*;
use std::time::Duration;

const FIXTURE_NAME: &str = "tools::browser::transport::tests::owned_process_tree_fixture";
const ROOT_ENV: &str = "WILLDEEP_BROWSER_TREE_TEST_ROOT";
const ROLE_ENV: &str = "WILLDEEP_BROWSER_TREE_TEST_ROLE";

#[test]
fn initialization_transmits_configured_startup_budget_with_response_reserve() {
    let root = std::env::temp_dir().join("willdeep-browser-init-budget-test");
    for (seconds, startup_ms) in [(1, 750), (30, 29_000), (60, 59_000)] {
        let config = BrowserConfig {
            timeout_seconds: seconds,
            allowed_origins: vec!["https://example.test".into()],
            ..BrowserConfig::default()
        };
        let request = initialization(&config, &root);
        assert_eq!(request["action"], "init");
        assert_eq!(request["startup_timeout_ms"], startup_ms);
        assert_eq!(request["profile"], json!(root.join("profile")));
        assert_eq!(request["allowed_origins"], json!(config.allowed_origins));
        assert!(startup_ms < seconds * 1000);
    }
}

#[test]
#[ignore = "internal subprocess fixture; exits immediately without its private test environment"]
fn owned_process_tree_fixture() {
    let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    if std::env::var(ROLE_ENV).as_deref() == Ok("leader") {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", FIXTURE_NAME, "--nocapture"])
            .env_clear()
            .env(ROOT_ENV, &root)
            .env(ROLE_ENV, "descendant")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Publish readiness only after all PID bytes have been written. Creating
        // the destination before writing lets parallel tests observe an empty file.
        let pending = root.join("descendant.pid.pending");
        std::fs::write(&pending, child.id().to_string()).unwrap();
        std::fs::rename(pending, root.join("descendant.pid")).unwrap();
        // Both test processes remain in the process group established by parent.
        child.wait().unwrap();
        return;
    }
    loop {
        std::thread::sleep(Duration::from_millis(100));
    }
}

async fn fixture() -> (Bridge, u32, u32) {
    let root = private_directory(&std::env::temp_dir(), "willdeep-browser-tree-test").unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", FIXTURE_NAME, "--nocapture"])
        .env_clear()
        .env(ROOT_ENV, &root)
        .env(ROLE_ENV, "leader")
        .current_dir(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command.spawn().unwrap();
    let leader = child.id().unwrap();
    let bridge = Bridge {
        input: child.stdin.take().unwrap(),
        output: BufReader::new(child.stdout.take().unwrap()),
        child: Some(child),
        group: Some(leader),
        root: root.clone(),
        next_id: 1,
    };
    let descendant = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(pid) = tokio::fs::read_to_string(root.join("descendant.pid")).await {
                break pid.parse::<u32>().unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("native fixture descendant ready");
    (bridge, leader, descendant)
}

fn gone(pid: u32) -> bool {
    // SAFETY: Signal zero only checks existence; it never signals another process.
    if (unsafe { libc::kill(pid as i32, 0) }) == -1
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    {
        return true;
    }
    // Container PID 1 may leave a killed orphan as a zombie. It has terminated
    // and cannot execute, even though signal-zero still sees its PID.
    #[cfg(target_os = "linux")]
    if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        return status
            .rsplit_once(") ")
            .is_some_and(|(_, state)| state.starts_with('Z'));
    }
    false
}

async fn assert_tree_gone(leader: u32, descendant: u32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !gone(leader) || !gone(descendant) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned leader and descendant terminated after lease drop");
}

#[tokio::test]
async fn cancellation_drops_bridge_and_kills_owned_leader_and_descendant() {
    let (bridge, leader, descendant) = fixture().await;
    let pending = tokio::spawn(async move {
        let _lease = bridge;
        std::future::pending::<()>().await;
    });
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    assert_tree_gone(leader, descendant).await;
}

#[test]
fn dropping_bridge_after_runtime_shutdown_removes_profile_and_owned_tree() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (bridge, leader, descendant) = runtime.block_on(fixture());
    let root = bridge.root.clone();
    std::fs::create_dir(root.join("profile")).unwrap();
    drop(runtime);
    drop(bridge);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while (!gone(leader) || !gone(descendant) || root.exists())
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        gone(leader) && gone(descendant),
        "owned tree survived runtime shutdown"
    );
    assert!(!root.exists(), "private profile survived runtime shutdown");
}

#[tokio::test]
async fn unresponsive_online_status_does_not_report_cached_ready() {
    let (bridge, leader, descendant) = fixture().await;
    let root = bridge.root.clone();
    let executable = std::env::current_exe().unwrap();
    let registry = crate::ToolRegistry::new(&root, crate::ApprovalMode::FullAccess)
        .unwrap()
        .with_browser_config(BrowserConfig {
            enabled: true,
            node_path: Some(executable.clone()),
            chrome_path: Some(executable),
            artifact_dir: Some(root.clone()),
            allowed_origins: vec!["http://127.0.0.1:19851".into()],
            ..BrowserConfig::default()
        })
        .unwrap();
    *registry.browser.session.lock().await = Some(super::super::Session {
        bridge,
        artifacts: super::super::Artifacts::new(&root).unwrap(),
        refs: Default::default(),
        epoch: 1,
    });
    let result = registry.browser_status().await;
    assert_eq!(result["ready"], false);
    assert_eq!(result["runtime"], "failed");
    assert_tree_gone(leader, descendant).await;
}

#[tokio::test]
async fn deadline_drops_bridge_and_kills_owned_leader_and_descendant() {
    let (bridge, leader, descendant) = fixture().await;
    let result = tokio::time::timeout(Duration::from_millis(20), async move {
        let _lease = bridge;
        std::future::pending::<()>().await;
    })
    .await;
    assert!(result.is_err());
    assert_tree_gone(leader, descendant).await;
}
