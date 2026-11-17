use super::*;
use willdeep_core::provider::{Provider, ProviderError};

struct Fixture {
    workspace: PathBuf,
    listener: tokio::net::TcpListener,
    parent: ProviderConfig,
}
impl Fixture {
    async fn new() -> Self {
        let workspace =
            std::env::temp_dir().join(format!("willdeep-harness-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(workspace.join(".willdeep")).unwrap();
        std::fs::write(
            workspace.join(".willdeep/model-policy.json"),
            b"{\"version\":1,\"allow\":[]}",
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let mut parent = ProviderConfig::new(
            ProviderKind::OpenAiCompatible,
            ApiDialect::ChatCompletions,
            base,
            "fixture",
            "parent",
        );
        parent.bind_workspace(&workspace).unwrap();
        Self {
            workspace,
            listener,
            parent,
        }
    }
    fn loaded(&self, settings: &str) -> LoadedConfig {
        LoadedConfig { file: toml::from_str(&format!(
            "version = 1\n{settings}\n[providers.alternate]\nprovider = 'openai-compatible'\napi_base = '{}'\napi_key = 'fixture'\nmodel = 'alternate'\n",
            self.parent.base_url,
        )).unwrap() }
    }
    async fn denied(&self, provider: &Arc<dyn Provider>) {
        assert!(provider.policy_context().is_some());
        assert!(matches!(
            provider.complete(&[], &[]).await,
            Err(ProviderError::PolicyDenied(_))
        ));
    }
    async fn zero_requests(&self) {
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), self.listener.accept())
                .await
                .is_err()
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.workspace);
    }
}

#[tokio::test]
async fn compression_profile_candidates_and_main_fallback_keep_project_policy() {
    let fixture = Fixture::new().await;
    let main = willdeep_core::provider::MainModelHandle::new(
        build_provider(fixture.parent.clone()).unwrap(),
    );
    for settings in [
        "[agent]\ncompressor_profile = 'alternate'",
        "[agent]\ncompressor_model = 'summary'",
        "",
    ] {
        let loaded = fixture.loaded(settings);
        let compression =
            build_compression_settings(&loaded, &fixture.parent, main.follower()).unwrap();
        for (provider, _) in compression.candidates {
            fixture.denied(&provider).await;
        }
    }
    main.set_model("switched-main").unwrap();
    fixture.denied(&main.follower()).await;
    fixture.zero_requests().await;
}

#[tokio::test]
async fn worker_profiles_tiers_and_explicit_connection_replacements_keep_policy() {
    let fixture = Fixture::new().await;
    let loaded = fixture.loaded("[subagents.tester]\nprovider_profile = 'alternate'\n[subagents.implementer]\nmodel = 'explicit-model'\n[worker_tiers.advanced]\nprovider_profile = 'alternate'\nmodel = 'tier'");
    let main = willdeep_core::provider::MainModelHandle::new(
        build_provider(fixture.parent.clone()).unwrap(),
    );
    let profiles = worker_profiles::configure_worker_profiles(
        &loaded.file,
        &fixture.parent,
        &main.follower(),
        fixture.parent.kind,
    )
    .unwrap();
    assert_eq!(
        profiles
            .iter()
            .find(|profile| profile.id == "tester")
            .unwrap()
            .provider
            .ledger_identity()
            .unwrap()
            .model,
        "alternate"
    );
    assert_eq!(
        profiles
            .iter()
            .find(|profile| profile.id == "implementer")
            .unwrap()
            .provider
            .ledger_identity()
            .unwrap()
            .model,
        "explicit-model"
    );
    for profile in &profiles {
        fixture.denied(&profile.provider).await;
    }
    for (_, binding) in resolve_tier_bindings(
        &loaded.file,
        &fixture.parent,
        &main.follower(),
        fixture.parent.kind,
        128_000,
    )
    .unwrap()
    {
        fixture.denied(&binding.provider).await;
    }
    // An otherwise unbound profile is rebound by the shared main-model handle.
    let mut replacement = fixture.parent.clone();
    replacement.policy_context = None;
    main.set_provider(build_provider(replacement).unwrap())
        .unwrap();
    for profile in profiles {
        fixture.denied(&profile.provider).await;
    }
    fixture.zero_requests().await;
}

#[tokio::test]
async fn library_connections_are_rebound_without_freezing_inherited_followers() {
    let fixture = Fixture::new().await;
    let main = willdeep_core::provider::MainModelHandle::new(
        build_provider(fixture.parent.clone()).unwrap(),
    );
    let mut inherited = willdeep_core::builtin_profiles(main.follower());
    worker_profiles::bind_library_policy(&mut inherited, &fixture.parent).unwrap();
    main.set_model("after-library-refresh").unwrap();
    for profile in inherited {
        assert_eq!(
            profile.provider.ledger_identity().unwrap().model,
            "after-library-refresh"
        );
        fixture.denied(&profile.provider).await;
    }
    let mut unbound = fixture.parent.clone();
    unbound.policy_context = None;
    let mut explicit = willdeep_core::builtin_profiles(build_provider(unbound).unwrap());
    for profile in &mut explicit {
        profile.definition_model_bound = true;
    }
    worker_profiles::bind_library_policy(&mut explicit, &fixture.parent).unwrap();
    for profile in explicit {
        fixture.denied(&profile.provider).await;
    }
    fixture.zero_requests().await;
}

#[tokio::test]
async fn title_local_auxiliary_and_independent_web_prediction_keep_policy() {
    let fixture = Fixture::new().await;
    let loaded = fixture.loaded(&format!(
        "default_provider = 'alternate'\n[local_model]\nenabled = true\nbase_url = '{}'\nsummary_model = 'local-title'\nprefer_for_titles = true\n[agent]\ntitle_model = 'title'",
        fixture.parent.base_url,
    ));
    for provider in auxiliary_providers(&loaded, &fixture.parent, None).unwrap() {
        fixture.denied(&provider).await;
    }
    for provider in input_suggestion_providers(
        &loaded,
        Some("alternate"),
        Some("prediction".into()),
        fixture.parent.policy_context.clone().unwrap(),
    )
    .unwrap()
    {
        fixture.denied(&provider).await;
    }
    fixture.zero_requests().await;
}
