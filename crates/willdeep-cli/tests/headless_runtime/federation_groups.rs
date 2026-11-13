//! 三台隔离 daemon、签名联邦 HTTP 和真实 Runtime 的群聊往返。
use super::*;

struct GroupNode {
    home: PathBuf,
    listen: String,
    _provider: MockProvider,
    guard: TestGuard,
}

fn node(root: &Path, name: &str, response: &'static str) -> GroupNode {
    node_with_mode(root, name, MockMode::FederationReply(response))
}

fn node_with_mode(root: &Path, name: &str, mode: MockMode) -> GroupNode {
    let node_root = root.join(name);
    let home = node_root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let provider = MockProvider::start_with_mode(mode);
    let listen = free_loopback_address();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let config = format!(
        r#"version = 1
default_provider = "mock"
[agent]
max_turns = 4
approval = "smart"
auto_title = false
[providers.mock]
provider = "openai-compatible"
api = "chat-completions"
api_base = "{}"
api_key = "integration-test-only"
model = "mock-model"
[intake]
enabled = true
listen = "{listen}"
public_url = "http://{listen}"
[federation]
enabled = true
workspace = "{}"
reply = "always"
max_auto_turns = 20
"#,
        provider.api_base(),
        workspace.display()
    );
    write_private_text(&home.join("config.toml"), &config);
    let mut guard = TestGuard::new(node_root, home.clone());
    guard.reap_on_abort();
    let named = willdeep(&home)
        .args(["identity", "rename", name])
        .output()
        .unwrap();
    assert_success(&named, "name isolated federation instance");
    let started = willdeep(&home).args(["daemon", "start"]).output().unwrap();
    if !started.status.success() {
        // 繁忙机器上的首次冷启动可能超过 CLI 的 5 秒健康探测。
        thread::sleep(Duration::from_secs(2));
        let retried = willdeep(&home).args(["daemon", "start"]).output().unwrap();
        assert_success(
            &retried,
            "start isolated federation Runtime after cold start",
        );
    }
    wait_until(Duration::from_secs(15), || intake_responds(&listen));
    GroupNode {
        home,
        listen,
        _provider: provider,
        guard,
    }
}

fn intake_responds(address: &str) -> bool {
    use std::io::Read as _;
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(
        &address.parse().expect("valid intake address"),
        Duration::from_secs(2),
    ) else {
        return false;
    };
    if stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .is_err()
        || stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .is_err()
    {
        return false;
    }
    let mut response = [0; 32];
    matches!(stream.read(&mut response), Ok(size) if response[..size].starts_with(b"HTTP/1.1 200"))
}

fn fed(home: &Path, args: &[&str]) -> String {
    let output = willdeep(home).arg("fed").args(args).output().unwrap();
    assert_success(&output, "run federation command");
    if matches!(args.first(), Some(&"group-say" | &"group-stop")) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stderr.contains("cannot forward"),
            "host failed to deliver a group message: {stderr}"
        );
    }
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn stored(home: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(home.join("federation.json")).unwrap()).unwrap()
}

fn group_session_text(home: &Path, hosted: bool) -> String {
    let state = stored(home);
    let section = if hosted { "groups" } else { "joined_groups" };
    let Some(id) = state[section][0]["session_id"].as_str() else {
        return String::new();
    };
    let id = uuid::Uuid::parse_str(id).unwrap();
    willdeep_core::SessionStore::new(home)
        .load(id)
        .map(|session| {
            session
                .messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[test]
fn federation_three_runtimes_route_manual_replies_and_mentions() {
    let _serial = process_test_guard();
    let root = temporary_root();
    std::fs::create_dir_all(&root).unwrap();
    let mut host = node_with_mode(
        &root,
        "host",
        MockMode::FederationReplySequence("host replies", "host follows gamma"),
    );
    let mut beta = node_with_mode(
        &root,
        "beta",
        MockMode::FederationReplyAfterFirstWait(
            "@gamma beta replies",
            "@gamma late beta reply after stop",
        ),
    );
    let mut gamma = node(&root, "gamma", "@host gamma replies");
    for member in [&beta, &gamma] {
        let invite = fed(&host.home, &["invite"]);
        fed(&member.home, &["join", &invite]);
    }
    fed(&host.home, &["group", "team", "beta", "gamma"]);
    wait_until(Duration::from_secs(30), || {
        [&beta, &gamma]
            .into_iter()
            .all(|node| intake_responds(&node.listen))
    });
    fed(&host.home, &["group-stop", "team"]);
    wait_until(Duration::from_secs(30), || {
        [&beta, &gamma]
            .into_iter()
            .all(|member| group_session_text(&member.home, false).contains("自动回复已暂停"))
    });
    for member in [&beta, &gamma] {
        assert!(
            runtime_state(&member.home, "turns.json").is_empty(),
            "an initial pause notice creates a session without a model turn"
        );
    }
    fed(&host.home, &["group-say", "team", "@beta 请开始讨论"]);
    wait_until(Duration::from_secs(300), || {
        thread::sleep(Duration::from_millis(500));
        let at_host = group_session_text(&host.home, true);
        let at_beta = group_session_text(&beta.home, false);
        let at_gamma = group_session_text(&gamma.home, false);
        at_host.contains("@gamma beta replies")
            && at_host.contains("@host gamma replies")
            && at_host.contains("host follows gamma")
            && at_beta.contains("@gamma beta replies")
            && at_gamma.contains("@beta 请开始讨论")
            && at_gamma.contains("host replies")
            && at_gamma.contains("@gamma beta replies")
            && at_gamma.contains("host follows gamma")
            && runtime_state(&host.home, "turns.json").len() >= 2
    });
    let host_text = group_session_text(&host.home, true);
    for member in [&beta, &gamma] {
        assert!(
            group_session_text(&member.home, false).contains("自动回复已暂停"),
            "the pause notice must survive later model checkpoints and final saves"
        );
    }
    let beta_at = host_text.find("@gamma beta replies").unwrap();
    let gamma_at = host_text.find("@host gamma replies").unwrap();
    assert!(
        beta_at < gamma_at,
        "gamma can reply only after beta's @ mention"
    );
    assert_eq!(
        host._provider.requests(),
        2,
        "host answers both group turns"
    );
    assert_eq!(
        beta._provider.requests(),
        1,
        "beta replies to the human once"
    );
    assert_eq!(
        gamma._provider.requests(),
        1,
        "gamma replies to beta's @ once"
    );
    let beta_turns = runtime_state(&beta.home, "turns.json").len();
    fed(&host.home, &["group-say", "team", "@beta 再说一条"]);
    wait_until(Duration::from_secs(120), || {
        runtime_state(&beta.home, "turns.json").len() > beta_turns && beta._provider.requests() >= 2
    });
    assert!(
        !group_session_text(&host.home, true).contains("late beta reply after stop"),
        "the second beta model response is held before pause"
    );
    fed(&host.home, &["group-stop", "team"]);
    wait_until(Duration::from_secs(10), || {
        stored(&beta.home)["joined_groups"][0]["paused"] == true
            && stored(&gamma.home)["joined_groups"][0]["paused"] == true
    });
    let second_turn_id = runtime_state(&beta.home, "turns.json")[beta_turns]["metadata"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_until(Duration::from_secs(15), || {
        runtime_state(&beta.home, "turns.json").iter().any(|turn| {
            turn["metadata"]["id"] == second_turn_id
                && matches!(
                    turn["metadata"]["status"].as_str(),
                    Some("cancelled" | "interrupted")
                )
        })
    });
    let counts = [&host, &beta, &gamma].map(|node| runtime_state(&node.home, "turns.json").len());
    beta._provider.release_stream.store(true, Ordering::SeqCst);
    thread::sleep(Duration::from_secs(4));
    assert!(
        !group_session_text(&host.home, true).contains("late beta reply after stop"),
        "a reply completed after pause must not reach the host"
    );
    for (node, expected) in [&host, &beta, &gamma].into_iter().zip(counts) {
        assert_eq!(
            runtime_state(&node.home, "turns.json").len(),
            expected,
            "no additional model turn follows group-stop"
        );
    }
    for node in [&mut host, &mut beta, &mut gamma] {
        node.guard.stop_daemon();
    }
    drop((host, beta, gamma));
    std::fs::remove_dir_all(root).unwrap();
}
