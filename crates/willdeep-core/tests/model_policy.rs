use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use willdeep_core::Message;
use willdeep_core::provider::{
    ApiDialect, MainModelHandle, ModelPolicyContext, NoopProviderEvents, ProviderConfig,
    ProviderError, ProviderEvent, ProviderEventSink, ProviderKind, ReasoningProfile,
    build_provider, capabilities::fetch_model_capabilities, list_models,
    model_metadata::fetch_model_metadata, policy::send_bound_request,
};

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("willdeep-model-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(path.join(".willdeep")).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn file(&self) -> PathBuf {
        self.0.join(".willdeep/model-policy.json")
    }
    fn policy(&self, allow: Value) {
        std::fs::write(
            self.file(),
            serde_json::to_vec(&json!({"version":1,"allow":allow})).unwrap(),
        )
        .unwrap();
    }
    fn allow(&self, base: &str) {
        self.policy(json!([{"provider":"openai-compatible","model":"allowed","base_url":base}]));
    }
    fn config(&self, base: &str) -> ProviderConfig {
        let mut config = ProviderConfig::new(
            ProviderKind::OpenAiCompatible,
            ApiDialect::ChatCompletions,
            base,
            "fixture",
            "allowed",
        );
        config.request_timeout_secs = 3;
        config.bind_workspace(self.path()).unwrap();
        config
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Server {
    base: String,
    hits: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn new(status: u16, extra: String) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = hits.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut bytes = [0_u8; 32 * 1024];
                if stream.read(&mut bytes).await.unwrap_or_default() == 0 {
                    continue;
                }
                seen.fetch_add(1, Ordering::SeqCst);
                let body = r#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"data":[]}"#;
                let response = format!(
                    "HTTP/1.1 {status} X\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Self { base, hits, task }
    }
    async fn assert_hits(&self, expected: usize) {
        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(self.hits.load(Ordering::SeqCst), expected);
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn denied<T>(result: Result<T, ProviderError>) {
    assert!(matches!(result, Err(ProviderError::PolicyDenied(_))));
}

#[tokio::test]
async fn session_policy_owner_round_trips_and_survives_fork_without_parent() {
    let project = Project::new();
    let worktree = Project::new();
    let server = Server::new(200, String::new()).await;
    project.policy(json!([]));
    let store = willdeep_core::SessionStore::new(project.path());
    let mut child = willdeep_core::Session::new(worktree.0.clone(), None, "child");
    child.parent_session_id = Some(uuid::Uuid::new_v4());
    child.model_policy_workspace = Some(project.0.clone());
    store.save(&mut child).unwrap();
    let restored = store.load(child.id).unwrap();
    let fork = restored.fork_snapshot();
    assert_eq!(fork.model_policy_workspace.as_deref(), Some(project.path()));
    for session in [&restored, &fork] {
        let mut config = worktree.config(&server.base);
        config.policy_context = Some(store.model_policy_context(session).unwrap());
        assert_eq!(
            config.policy_context.as_ref().unwrap().workspace(),
            Some(project.path())
        );
        denied(build_provider(config).unwrap().complete(&[], &[]).await);
    }
    server.assert_hits(0).await;
}

#[tokio::test]
async fn legacy_session_without_policy_field_resolves_root_and_child_ancestry() {
    let project = Project::new();
    let worktree = Project::new();
    let server = Server::new(200, String::new()).await;
    project.policy(json!([]));
    let store = willdeep_core::SessionStore::new(project.path());
    let mut value =
        serde_json::to_value(willdeep_core::Session::new(project.0.clone(), None, "root")).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("model_policy_workspace");
    let mut parent: willdeep_core::Session = serde_json::from_value(value).unwrap();
    assert!(parent.model_policy_workspace.is_none());
    store.save(&mut parent).unwrap();
    let mut value = serde_json::to_value(willdeep_core::Session::new(
        worktree.0.clone(),
        None,
        "child",
    ))
    .unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("model_policy_workspace");
    let mut child: willdeep_core::Session = serde_json::from_value(value).unwrap();
    child.parent_session_id = Some(parent.id);
    for session in [&parent, &child] {
        let context = store.model_policy_context(session).unwrap();
        assert_eq!(context.workspace(), Some(project.path()));
        let mut config = worktree.config(&server.base);
        config.policy_context = Some(context);
        denied(build_provider(config).unwrap().complete(&[], &[]).await);
    }
    server.assert_hits(0).await;
}

#[test]
fn legacy_child_missing_or_cyclic_policy_ancestry_fails_closed() {
    let project = Project::new();
    let store = willdeep_core::SessionStore::new(project.path());
    let mut child = willdeep_core::Session::new(project.0.clone(), None, "legacy");
    child.model_policy_workspace = None;
    child.parent_session_id = Some(uuid::Uuid::new_v4());
    denied(store.model_policy_context(&child));
    child.parent_session_id = Some(child.id);
    denied(store.model_policy_context(&child));
}

#[tokio::test]
async fn absent_policy_preserves_legacy_and_explicit_allow_sends() {
    let project = Project::new();
    let server = Server::new(200, String::new()).await;
    let config = project.config(&server.base);
    let provider = build_provider(config).unwrap();
    assert_eq!(
        provider
            .complete(&[Message::user("fixture")], &[])
            .await
            .unwrap()
            .content,
        "ok"
    );
    project.allow(&server.base);
    assert_eq!(provider.complete(&[], &[]).await.unwrap().content, "ok");
    server.assert_hits(2).await;
}

#[tokio::test]
async fn all_protocols_buffer_stream_and_with_model_deny_before_network() {
    let server = Server::new(200, String::new()).await;
    for (dialect, kind, provider_name) in [
        (
            ApiDialect::ChatCompletions,
            ProviderKind::OpenAiCompatible,
            "openai-compatible",
        ),
        (
            ApiDialect::Responses,
            ProviderKind::OpenAiCompatible,
            "openai-compatible",
        ),
        (
            ApiDialect::AnthropicMessages,
            ProviderKind::Anthropic,
            "anthropic",
        ),
    ] {
        let project = Project::new();
        project
            .policy(json!([{"provider":provider_name,"model":"allowed","base_url":server.base}]));
        let mut config = project.config(&server.base);
        config.dialect = dialect;
        config.kind = kind;
        // Local/credential-free classification never exempts a request.
        config.allow_unauthenticated = true;
        let provider = build_provider(config)
            .unwrap()
            .with_model("forbidden")
            .unwrap();
        denied(
            provider
                .complete(&[Message::user("private fixture")], &[])
                .await,
        );
        denied(
            provider
                .complete_with_events(&[], &[], &NoopProviderEvents)
                .await,
        );
        assert!(provider.policy_context().is_some());
    }
    server.assert_hits(0).await;
}

#[tokio::test]
async fn main_handle_profile_replacement_and_follower_cannot_drop_project() {
    let project = Project::new();
    let foreign = Project::new();
    let server = Server::new(200, String::new()).await;
    project.policy(json!([]));
    foreign.allow(&server.base);
    let handle = MainModelHandle::new(build_provider(project.config(&server.base)).unwrap());
    let follower = handle.follower();
    let mut replacement = foreign.config(&server.base);
    // Even a completely unbound profile replacement inherits the handle's project.
    replacement.policy_context = None;
    handle
        .set_provider(build_provider(replacement).unwrap())
        .unwrap();
    denied(follower.complete(&[], &[]).await);
    handle
        .set_provider(build_provider(foreign.config(&server.base)).unwrap())
        .unwrap();
    denied(
        follower
            .complete_with_events(&[], &[], &NoopProviderEvents)
            .await,
    );
    handle.set_model("forbidden").unwrap();
    denied(handle.current().unwrap().complete(&[], &[]).await);
    server.assert_hits(0).await;
}

#[tokio::test]
async fn provider_model_origin_and_exact_base_must_all_match() {
    let project = Project::new();
    let server = Server::new(200, String::new()).await;
    project.allow(&server.base);
    for (kind, model, base) in [
        (ProviderKind::SomeIm, "allowed", server.base.clone()),
        (
            ProviderKind::OpenAiCompatible,
            "forbidden",
            server.base.clone(),
        ),
        (
            ProviderKind::OpenAiCompatible,
            "allowed",
            format!("{}0", server.base),
        ),
        (
            ProviderKind::OpenAiCompatible,
            "allowed",
            format!("{}/chat/completions", server.base),
        ),
    ] {
        let mut config = project.config(&base);
        config.kind = kind;
        config.model = model.into();
        denied(build_provider(config).unwrap().complete(&[], &[]).await);
    }
    let other = Server::new(200, String::new()).await;
    denied(
        build_provider(project.config(&other.base))
            .unwrap()
            .complete(&[], &[])
            .await,
    );
    server.assert_hits(0).await;
    other.assert_hits(0).await;
    let origin = server.base.trim_end_matches("/v1");
    let origin_project = Project::new();
    origin_project
        .policy(json!([{"provider":"openai-compatible","model":"allowed","origin":origin}]));
    build_provider(origin_project.config(&format!("{}/other", server.base)))
        .unwrap()
        .complete(&[], &[])
        .await
        .unwrap();
    server.assert_hits(1).await;
}

#[tokio::test]
async fn invalid_empty_oversized_and_removed_policy_fail_closed() {
    let project = Project::new();
    let server = Server::new(200, String::new()).await;
    let provider = build_provider(project.config(&server.base)).unwrap();
    for bytes in [
        br#"{"version":1,"allow":[]}"#.as_slice(),
        br#"{"version":1,"allow":[],"typo":true}"#.as_slice(),
        br#"{"version":1,"version":1,"allow":[]}"#.as_slice(),
        br#"{"version":2,"allow":[]}"#.as_slice(),
        br#"{"version":1,"allow":[{"provider":"auto","model":"allowed","origin":"https://example.com"}]}"#.as_slice(),
        br#"{"version":1,"allow":[{"provider":"openai-compatible","model":"allowed","origin":"https://example.com/path"}]}"#.as_slice(),
        br#"{"version":1,"allow":[{"provider":"openai-compatible","model":"allowed","origin":"https://example.com","base_url":"https://example.com/v1"}]}"#.as_slice(),
        br#"{"version":1,"allow":[{"provider":"openai-compatible","model":"allowed","base_url":"https://example.com/v1?key=fixture"}]}"#.as_slice(),
        b"not JSON".as_slice(),
    ] {
        std::fs::write(project.file(), bytes).unwrap();
        denied(provider.complete(&[], &[]).await);
    }
    std::fs::write(project.file(), vec![b' '; 64 * 1024 + 1]).unwrap();
    denied(provider.complete(&[], &[]).await);
    std::fs::remove_file(project.file()).unwrap();
    denied(provider.complete(&[], &[]).await);
    server.assert_hits(0).await;
}

#[tokio::test]
async fn unreadable_nonregular_and_symlink_policy_never_disable_enforcement() {
    let project = Project::new();
    let server = Server::new(200, String::new()).await;
    let provider = build_provider(project.config(&server.base)).unwrap();
    std::fs::create_dir(project.file()).unwrap();
    denied(provider.complete(&[], &[]).await);
    std::fs::remove_dir(project.file()).unwrap();
    #[cfg(unix)]
    {
        let target = project.path().join("outside-policy.json");
        std::fs::write(&target, b"{\"version\":1,\"allow\":[]}").unwrap();
        std::os::unix::fs::symlink(&target, project.file()).unwrap();
        denied(provider.complete(&[], &[]).await);
        std::fs::remove_file(project.file()).unwrap();
        project.allow(&server.base);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(project.file(), std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root can legitimately read mode-000 files; test the error when the OS denies it.
        if std::fs::File::open(project.file()).is_err() {
            denied(provider.complete(&[], &[]).await);
        }
        std::fs::set_permissions(project.file(), std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    server.assert_hits(0).await;
}

struct DisableOnRetry(PathBuf);
#[async_trait::async_trait]
impl ProviderEventSink for DisableOnRetry {
    async fn emit(&self, event: ProviderEvent) {
        if matches!(event, ProviderEvent::RetryStarted { .. }) {
            std::fs::write(&self.0, b"{\"version\":1,\"allow\":[]}").unwrap();
        }
    }
}

#[tokio::test]
async fn retry_rechecks_policy_and_policy_denial_is_not_replayed() {
    let project = Project::new();
    let server = Server::new(503, "Retry-After: 0\r\n".into()).await;
    project.allow(&server.base);
    let provider = build_provider(project.config(&server.base)).unwrap();
    denied(
        provider
            .complete_with_events(&[], &[], &DisableOnRetry(project.file()))
            .await,
    );
    server.assert_hits(1).await;
    denied(provider.complete(&[], &[]).await);
    server.assert_hits(1).await;
}

#[tokio::test]
async fn model_catalog_capability_and_metadata_probes_obey_policy() {
    let project = Project::new();
    let server = Server::new(200, String::new()).await;
    project.policy(json!([]));
    let config = project.config(&server.base);
    denied(list_models(&config).await);
    denied(fetch_model_capabilities(&config).await);
    denied(fetch_model_metadata(&config).await);
    server.assert_hits(0).await;
}

#[tokio::test]
async fn final_json_wire_model_and_reasoning_cannot_override_identity() {
    let project = Project::new();
    let server = Server::new(200, String::new()).await;
    project.allow(&server.base);
    let config = project.config(&server.base);
    denied(
        send_bound_request(
            &config,
            reqwest::Client::new()
                .post(format!("{}/chat/completions", server.base))
                .json(&json!({"model":"forbidden","messages":[]})),
            "allowed",
        )
        .await,
    );
    for parameter in [
        "model",
        "model.name",
        "messages",
        "input",
        "system",
        "tools",
        "stream",
    ] {
        let mut config = config.clone();
        config.reasoning = Some(ReasoningProfile {
            default_level: Some("balanced".into()),
            parameter: Some(parameter.into()),
            levels: [("balanced".into(), json!("forbidden"))].into(),
        });
        assert!(config.validate().is_err(), "{parameter}");
        let mut body = json!({"model":"allowed"});
        assert!(
            config
                .apply_reasoning(&mut body, "reasoning_effort")
                .is_err()
        );
        assert_eq!(body["model"], "allowed");
    }
    server.assert_hits(0).await;
}

#[tokio::test]
async fn policy_transport_ignores_supplied_proxy_and_refuses_redirects() {
    let project = Project::new();
    let target = Server::new(200, String::new()).await;
    let source = Server::new(
        302,
        format!("Location: {}/chat/completions\r\n", target.base),
    )
    .await;
    project.policy(json!([
        {"provider":"openai-compatible","model":"allowed","origin":source.base.trim_end_matches("/v1")},
        {"provider":"openai-compatible","model":"allowed","origin":target.base.trim_end_matches("/v1")},
    ]));
    let config = project.config(&source.base);
    let proxy = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(target.base.trim_end_matches("/v1")).unwrap())
        .build()
        .unwrap();
    let response = send_bound_request(
        &config,
        proxy
            .post(format!("{}/chat/completions", source.base))
            .json(&json!({"model":"allowed"})),
        "allowed",
    )
    .await
    .unwrap();
    assert_eq!(response.status(), 302);
    source.assert_hits(1).await;
    target.assert_hits(0).await;
}

#[tokio::test]
async fn ambiguous_ownership_rechecks_projects_when_policy_appears() {
    let first = Project::new();
    let second = Project::new();
    let server = Server::new(200, String::new()).await;
    let roots = [first.0.clone(), second.0.clone()];
    let context = ModelPolicyContext::unambiguous_workspace(None, &roots).unwrap();
    let mut config = first.config(&server.base);
    config.policy_context = context;
    let provider = build_provider(config).unwrap();
    provider.complete(&[], &[]).await.unwrap();
    second.allow(&server.base);
    denied(provider.complete(&[], &[]).await);
    server.assert_hits(1).await;
    denied(ModelPolicyContext::unambiguous_workspace(None, &roots));
    denied(ModelPolicyContext::unambiguous_workspace(None, &[]));
}

#[tokio::test]
async fn edited_policy_cannot_expand_startup_baseline_or_reset_by_rebinding() {
    let project = Project::new();
    let allowed = Server::new(200, String::new()).await;
    let forbidden = Server::new(200, String::new()).await;
    project.allow(&allowed.base);
    let config = project.config(&allowed.base);
    let handle = MainModelHandle::new(build_provider(config.clone()).unwrap());
    handle.follower().complete(&[], &[]).await.unwrap();
    // An Agent may edit this file, including replacing an exact base by an
    // origin and adding models/endpoints. Those edits never grant authority.
    project.policy(json!([
        {"provider":"openai-compatible","model":"allowed","origin":allowed.base.trim_end_matches("/v1")},
        {"provider":"openai-compatible","model":"expanded","base_url":allowed.base},
        {"provider":"openai-compatible","model":"allowed","base_url":forbidden.base},
    ]));
    denied(
        handle
            .follower()
            .with_model("expanded")
            .unwrap()
            .complete(&[], &[])
            .await,
    );
    // A fresh context for the same canonical project still uses the baseline.
    let replacement = build_provider(project.config(&forbidden.base)).unwrap();
    handle.set_provider(replacement).unwrap();
    denied(handle.follower().complete(&[], &[]).await);
    let mut other_path = project.config(&format!("{}/sibling", allowed.base));
    other_path.allow_unauthenticated = true;
    denied(build_provider(other_path).unwrap().complete(&[], &[]).await);
    forbidden.assert_hits(0).await;
    allowed.assert_hits(1).await;

    // Runtime narrowing is sticky too, even if the original file is restored.
    project.policy(json!([]));
    denied(
        build_provider(config.clone())
            .unwrap()
            .complete(&[], &[])
            .await,
    );
    project.allow(&allowed.base);
    denied(
        build_provider(project.config(&allowed.base))
            .unwrap()
            .complete(&[], &[])
            .await,
    );
    std::fs::remove_file(project.file()).unwrap();
    denied(build_provider(config).unwrap().complete(&[], &[]).await);
    forbidden.assert_hits(0).await;
    allowed.assert_hits(1).await;
}

#[tokio::test]
async fn external_upload_download_and_image_routes_need_separate_base_permission() {
    let project = Project::new();
    let server = Server::new(200, String::new()).await;
    project.allow(&server.base);
    let config = project.config(&server.base);
    // Image generation shares its configured API base.
    send_bound_request(
        &config,
        reqwest::Client::new()
            .post(format!("{}/images/generations", server.base))
            .json(&json!({"model":"allowed"})),
        "allowed",
    )
    .await
    .unwrap();
    for path in ["/api/v1/customer/tmp-files", "/result.png"] {
        let mut service = config.clone();
        service.base_url = format!("{}{path}", server.base.trim_end_matches("/v1"));
        denied(
            send_bound_request(
                &service,
                reqwest::Client::new().get(&service.base_url),
                "allowed",
            )
            .await,
        );
    }
    server.assert_hits(1).await;
}
