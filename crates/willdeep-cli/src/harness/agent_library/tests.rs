use super::*;
use crate::agent_registry::service::CustomerSession;
use willdeep_core::agent_definition::AgentDefinition;
use willdeep_core::agent_definition::store::{AgentLibrary, Origin};
use willdeep_core::provider::{ApiDialect, Provider, ProviderError, ProviderKind};
use willdeep_core::types::{Completion, Message, ToolDefinition};

struct NeverRun;

#[async_trait]
impl Provider for NeverRun {
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        panic!("library host tests must not call a model")
    }
}

struct Fixture {
    home: PathBuf,
    catalog: Arc<SubagentCatalog>,
    host: LibraryHost,
}

impl Fixture {
    fn new() -> Self {
        Self::with_session(None)
    }

    fn with_session(session: Option<&CustomerSession>) -> Self {
        Self::with_provider(session, Arc::new(NeverRun))
    }

    fn with_provider(session: Option<&CustomerSession>, provider: Arc<dyn Provider>) -> Self {
        let home = std::env::temp_dir().join(format!("library-host-{}", uuid::Uuid::new_v4()));
        let workspace = home.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        if let Some(session) = session {
            session.save(&home).unwrap();
        }
        let base_profiles = willdeep_core::subagent::builtin_profiles(provider);
        let parent_session = uuid::Uuid::new_v4();
        let background = willdeep_core::BackgroundTaskRegistry::default().with_agent_run_store(
            willdeep_core::subagent::AgentRunStore::new(&home, parent_session),
        );
        let catalog = Arc::new(
            SubagentCatalog::new(&workspace, base_profiles.clone(), Arc::new(background))
                .with_parent_session(parent_session)
                .with_state_home(home.join("workers")),
        );
        // Parse ConfigFile directly; no daily config, environment credential
        // resolution, Runtime or external Provider is needed by these tests.
        let file = toml::from_str::<crate::config::ConfigFile>("version=1\n").unwrap();
        let mut parent = ProviderConfig::new(
            ProviderKind::OpenAiCompatible,
            ApiDialect::ChatCompletions,
            "http://127.0.0.1:1/v1",
            "",
            "parent-model",
        );
        parent.allow_unauthenticated = true;
        let host = LibraryHost::new(&home, catalog.clone(), base_profiles, file, parent).unwrap();
        Self {
            home,
            catalog,
            host,
        }
    }

    async fn create(&self) -> Value {
        self.host
            .execute("create", request(json!({"definition": definition()}), None))
            .await
            .unwrap()
    }

    async fn get(&self) -> Value {
        self.host
            .execute("get", json!({"id": definition().definition_id}))
            .await
            .unwrap()
    }

    fn profile(&self) -> SubagentProfile {
        self.catalog
            .profile(Some(&definition().definition_id))
            .expect("saved specialist must be immediately available")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // This nonce directory belongs exclusively to this fixture.
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn definition() -> AgentDefinition {
    AgentDefinition::parse(include_bytes!(
        "../../../../../docs/examples/agent-definition.v1.json"
    ))
    .unwrap()
}

fn changed_definition() -> AgentDefinition {
    let mut definition = definition();
    definition.version = "1.0.0-rc2".into();
    definition.instructions = "UPDATED_HOST_INSTRUCTIONS: inspect the current fixture.".into();
    definition
}

fn request(mut params: Value, expected: Option<u64>) -> Value {
    params["request_id"] = uuid::Uuid::new_v4().to_string().into();
    if let Some(expected) = expected {
        params["expected_revision"] = expected.into();
    }
    params
}

fn revision(receipt: &Value) -> u64 {
    receipt["installation_revision"].as_u64().unwrap()
}

fn assert_applied(receipt: &Value, replayed: bool) {
    assert_eq!(receipt["saved"], true);
    assert_eq!(receipt["applied"], true);
    assert_eq!(receipt["catalog_refreshed"], true);
    assert_eq!(receipt["replayed"], replayed);
    assert!(receipt.get("refresh_error").is_none());
    assert_ne!(receipt["status"], "saved_not_applied");
}

fn session(user_id: &str) -> CustomerSession {
    CustomerSession {
        origin: "http://127.0.0.1:1".into(),
        token: "synthetic-library-host-session".into(),
        user_id: user_id.into(),
        tenant_id: "fixture-tenant".into(),
    }
}

#[tokio::test]
async fn create_refreshes_the_same_catalog_and_host_reads_the_saved_definition() {
    let fixture = Fixture::new();
    let catalog_clone = fixture.catalog.as_ref().clone();
    let mut params = request(json!({"definition": definition()}), None);
    params["expected_scope"] = json!({"library_id":"model-supplied-scope"});
    let created = fixture.host.execute("create", params).await.unwrap();
    assert_applied(&created, false);
    assert_eq!(created["scope"], fixture.host.scope);
    assert_eq!(created["definition_id"], definition().definition_id);
    let profile = fixture.profile();
    assert_eq!(
        profile.definition.as_ref().unwrap().revision().unwrap(),
        created["definition_revision"].as_str().unwrap()
    );
    assert!(
        profile
            .capability_prompt
            .contains(&definition().instructions)
    );
    assert!(catalog_clone.profile(Some(&profile.id)).is_some());
    assert!(fixture.catalog.public_profile_ids().contains(&profile.id));
    assert!(
        fixture
            .catalog
            .profile(Some("reviewer"))
            .unwrap()
            .definition
            .is_none()
    );

    let loaded = fixture.get().await;
    assert_eq!(
        loaded["installation_revision"],
        created["installation_revision"]
    );
    assert_eq!(loaded["package"]["definition"], created["definition"]);
    let list = fixture.host.execute("list", json!({})).await.unwrap();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        fixture
            .host
            .execute("history", json!({"id":profile.id}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn update_bind_and_enable_refresh_without_mutating_old_owned_profiles() {
    let fixture = Fixture::new();
    let created = fixture.create().await;
    let original = fixture.profile();
    let changed = changed_definition();
    let updated = fixture
        .host
        .execute(
            "update",
            request(json!({"definition":changed}), Some(revision(&created))),
        )
        .await
        .unwrap();
    assert_applied(&updated, false);
    assert!(revision(&updated) > revision(&created));
    assert_ne!(
        updated["definition_revision"],
        created["definition_revision"]
    );
    let updated_profile = fixture.profile();
    assert_eq!(
        updated_profile.definition.as_ref().unwrap().instructions,
        changed.instructions
    );
    assert!(
        updated_profile
            .capability_prompt
            .contains(&changed.instructions)
    );
    assert!(
        !updated_profile
            .capability_prompt
            .contains(&definition().instructions)
    );

    let binding = json!({"model":"bound-model","context_window":32768,"max_turns":7});
    let bound = fixture
        .host
        .execute(
            "bind",
            request(
                json!({"id":original.id,"binding":binding}),
                Some(revision(&updated)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&bound, false);
    assert!(revision(&bound) > revision(&updated));
    assert_eq!(bound["definition_revision"], updated["definition_revision"]);
    let bound_profile = fixture.profile();
    assert_eq!(bound_profile.model.as_deref(), Some("bound-model"));
    assert_eq!(
        bound_profile.provider.ledger_identity().unwrap().model,
        "bound-model"
    );
    assert!(bound_profile.definition_model_bound);
    assert_eq!(bound_profile.context_window, 32768);
    assert_eq!(bound_profile.max_turns, 7);
    assert_eq!(
        bound_profile
            .definition_binding
            .as_ref()
            .unwrap()
            .model
            .as_deref(),
        Some("bound-model")
    );
    assert_eq!(bound_profile.tool_names, original.tool_names);
    assert_eq!(bound_profile.write_scope, original.write_scope);

    let disabled = fixture
        .host
        .execute(
            "enable",
            request(
                json!({"id":original.id,"value":false}),
                Some(revision(&bound)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&disabled, false);
    assert!(fixture.catalog.profile(Some(&original.id)).is_none());
    assert!(!fixture.catalog.public_profile_ids().contains(&original.id));
    assert_eq!(fixture.get().await["installation"]["enabled"], false);
    let enabled = fixture
        .host
        .execute(
            "enable",
            request(
                json!({"id":original.id,"value":true}),
                Some(revision(&disabled)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&enabled, false);
    assert_eq!(enabled["definition_revision"], bound["definition_revision"]);
    assert_eq!(fixture.profile().model.as_deref(), Some("bound-model"));

    assert_eq!(
        original.definition.as_ref().unwrap().instructions,
        definition().instructions
    );
    assert_eq!(original.model, updated_profile.model);
    assert_eq!(original.context_window, updated_profile.context_window);
    assert_eq!(original.max_turns, updated_profile.max_turns);
    assert!(!original.definition_model_bound);
    assert!(Arc::ptr_eq(&original.provider, &updated_profile.provider));
    assert!(!Arc::ptr_eq(&original.provider, &bound_profile.provider));
    assert_eq!(bound_profile.model.as_deref(), Some("bound-model"));
    assert_eq!(
        bound_profile.definition.as_ref().unwrap().instructions,
        changed.instructions
    );
}

#[tokio::test]
async fn replay_refreshes_current_installation_instead_of_the_old_receipt_definition() {
    let fixture = Fixture::new();
    let create_request = request(json!({"definition":definition()}), None);
    let created = fixture
        .host
        .execute("create", create_request.clone())
        .await
        .unwrap();
    let updated = fixture
        .host
        .execute(
            "update",
            request(
                json!({"definition":changed_definition()}),
                Some(revision(&created)),
            ),
        )
        .await
        .unwrap();
    fixture
        .catalog
        .replace_specialist_profiles(Vec::new())
        .unwrap();
    assert!(
        fixture
            .catalog
            .profile(Some(&definition().definition_id))
            .is_none()
    );

    let replayed = fixture
        .host
        .execute("create", create_request)
        .await
        .unwrap();
    assert_applied(&replayed, true);
    assert_eq!(
        replayed["installation_revision"],
        created["installation_revision"]
    );
    assert_eq!(
        replayed["definition_revision"],
        created["definition_revision"]
    );
    assert_eq!(
        fixture.get().await["installation_revision"],
        updated["installation_revision"]
    );
    assert_eq!(
        fixture
            .profile()
            .definition
            .as_ref()
            .unwrap()
            .revision()
            .unwrap(),
        updated["definition_revision"].as_str().unwrap()
    );
}

#[tokio::test]
async fn stale_installation_revision_rejects_writes_even_when_definition_hash_is_unchanged() {
    let fixture = Fixture::new();
    let created = fixture.create().await;
    let id = definition().definition_id;
    let bound = fixture
        .host
        .execute(
            "bind",
            request(
                json!({"id":id,"binding":{"max_turns":7}}),
                Some(revision(&created)),
            ),
        )
        .await
        .unwrap();
    assert_eq!(bound["definition_revision"], created["definition_revision"]);
    let before = fixture.get().await;
    for (operation, params) in [
        ("update", json!({"definition":changed_definition()})),
        ("bind", json!({"id":id,"binding":{"max_turns":8}})),
        ("enable", json!({"id":id,"value":false})),
        ("fork", json!({"id":id,"new_id":"user/stale-fork"})),
    ] {
        let error = fixture
            .host
            .execute(operation, request(params, Some(revision(&created))))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("agent_revision_conflict"),
            "{operation}: {error}"
        );
        assert_eq!(fixture.get().await, before);
    }
    assert_eq!(fixture.profile().max_turns, 7);
    assert!(fixture.catalog.profile(Some("user/stale-fork")).is_none());
}

#[tokio::test]
async fn account_scope_change_rejects_writes_and_replays_despite_forged_current_scope() {
    let original_session = session("original-user");
    let fixture = Fixture::with_session(Some(&original_session));
    let create_request = request(json!({"definition":definition()}), None);
    let created = fixture
        .host
        .execute("create", create_request.clone())
        .await
        .unwrap();
    let before = AgentLibrary::new(&fixture.home).installations().unwrap();
    let replacement = session("replacement-user");
    replacement.save(&fixture.home).unwrap();
    let current_scope = crate::agent_registry::capture_scope(&fixture.home).unwrap();
    assert_ne!(current_scope, fixture.host.scope);
    for (operation, mut params) in [
        ("create", create_request),
        (
            "enable",
            request(
                json!({"id":definition().definition_id,"value":false}),
                Some(revision(&created)),
            ),
        ),
    ] {
        params["expected_scope"] = current_scope.clone();
        let error = fixture.host.execute(operation, params).await.unwrap_err();
        assert!(
            error.to_string().contains("agent_scope_conflict"),
            "{operation}: {error}"
        );
    }
    assert_eq!(
        AgentLibrary::new(&fixture.home).installations().unwrap(),
        before
    );
    assert!(
        replacement
            .library(&fixture.home)
            .installations()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn same_revision_private_overlay_rejects_unscoped_writes_after_list_or_get() {
    for read in ["list", "get"] {
        for (operation, params) in [
            ("update", json!({"definition":changed_definition()})),
            (
                "bind",
                json!({"id":definition().definition_id,"binding":{"max_turns":7}}),
            ),
            (
                "enable",
                json!({"id":definition().definition_id,"value":false}),
            ),
            (
                "fork",
                json!({"id":definition().definition_id,"new_id":"user/scope-fork"}),
            ),
        ] {
            let session = session("same-user");
            let fixture = Fixture::with_session(Some(&session));
            let package =
                willdeep_core::agent_definition::AgentPackage::local(definition()).unwrap();
            let local = AgentLibrary::new(&fixture.home);
            local
                .install(&package, Origin::Local, &BTreeMap::new())
                .unwrap();
            let loaded = fixture
                .host
                .execute(read, json!({"id":definition().definition_id}))
                .await
                .unwrap();
            let expected = if read == "list" {
                assert_eq!(loaded["items"][0]["scope"], "local");
                revision(&loaded["items"][0])
            } else {
                assert_eq!(loaded["target_scope"], "local");
                revision(&loaded)
            };
            let private = session.library(&fixture.home);
            private
                .install(&package, Origin::Private, &BTreeMap::new())
                .unwrap();
            let local_before = local.installations().unwrap();
            let private_before = private.installations().unwrap();
            assert_eq!(
                private_before[&definition().definition_id].installation_revision,
                expected
            );
            assert_eq!(
                crate::agent_registry::capture_scope(&fixture.home).unwrap(),
                fixture.host.scope
            );
            // No target_scope: the host must carry the identity read before the overlay.
            let params = request(params, Some(expected));
            assert!(params.get("target_scope").is_none());
            let error = fixture.host.execute(operation, params).await.unwrap_err();
            assert!(
                error.to_string().contains("agent_scope_conflict"),
                "{read} -> {operation}: {error}"
            );
            assert_eq!(private.installations().unwrap(), private_before);
            assert_eq!(local.installations().unwrap(), local_before);
            assert!(fixture.catalog.profile(Some("user/scope-fork")).is_none());
        }
    }
}

#[tokio::test]
async fn model_cannot_override_observed_target_but_rereading_can_select_private() {
    let session = session("same-user");
    let fixture = Fixture::with_session(Some(&session));
    let created = fixture.create().await;
    let package = willdeep_core::agent_definition::AgentPackage::local(definition()).unwrap();
    let private = session.library(&fixture.home);
    private
        .install(&package, Origin::Private, &BTreeMap::new())
        .unwrap();
    let params =
        json!({"id":definition().definition_id,"target_scope":"private","binding":{"max_turns":7}});
    let error = fixture
        .host
        .execute("bind", request(params, Some(revision(&created))))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("agent_scope_conflict"));
    let current = fixture.get().await;
    assert_eq!(current["target_scope"], "private");
    assert_eq!(revision(&current), revision(&created));
    let bound = fixture
        .host
        .execute(
            "bind",
            request(
                json!({"id":definition().definition_id,"binding":{"max_turns":7}}),
                Some(revision(&current)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&bound, false);
    assert_eq!(bound["target_scope"], "private");
    let disabled = fixture
        .host
        .execute(
            "enable",
            request(
                json!({"id":definition().definition_id,"value":false}),
                Some(revision(&bound)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&disabled, false);
    assert_eq!(disabled["target_scope"], "private");
    assert!(!private.installations().unwrap()[&definition().definition_id].enabled);
    assert_eq!(
        AgentLibrary::new(&fixture.home).installations().unwrap()[&definition().definition_id]
            .installation_revision,
        revision(&created)
    );
}

#[tokio::test]
async fn managing_an_unobserved_installation_requires_a_read_even_with_explicit_scope() {
    let fixture = Fixture::new();
    let package = willdeep_core::agent_definition::AgentPackage::local(definition()).unwrap();
    let local = AgentLibrary::new(&fixture.home);
    local
        .install(&package, Origin::Local, &BTreeMap::new())
        .unwrap();
    let before = local.installations().unwrap();
    for (operation, params) in [
        (
            "update",
            json!({"definition":changed_definition(),"target_scope":"local"}),
        ),
        (
            "bind",
            json!({"id":definition().definition_id,"target_scope":"local","binding":{}}),
        ),
        (
            "enable",
            json!({"id":definition().definition_id,"value":false}),
        ),
        (
            "fork",
            json!({"id":definition().definition_id,"new_id":"user/unread-fork"}),
        ),
    ] {
        let error = fixture
            .host
            .execute(operation, request(params, Some(1)))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("agent_target_required"),
            "{operation}: {error}"
        );
    }
    assert_eq!(local.installations().unwrap(), before);
}

#[tokio::test]
async fn saved_not_applied_clears_all_specialists_keeps_builtins_and_replay_recovers() {
    let mut fixture = Fixture::new();
    let builtin_ids = fixture.catalog.public_profile_ids();
    let builtin = fixture.catalog.profile(Some("reviewer")).unwrap();
    let created = fixture.create().await;
    let mut second = definition();
    second.definition_id = "user/second-host-specialist".into();
    let second_receipt = fixture
        .host
        .execute("create", request(json!({"definition":second}), None))
        .await
        .unwrap();
    assert_applied(&second_receipt, false);
    let original = fixture.profile();
    let catalog_clone = fixture.catalog.as_ref().clone();
    let update = request(
        json!({"definition":changed_definition()}),
        Some(revision(&created)),
    );
    // Force the real projection path to fail only after the transaction commits.
    let bases = std::mem::take(&mut fixture.host.base_profiles);
    let saved = fixture
        .host
        .execute("update", update.clone())
        .await
        .unwrap();
    assert_eq!(saved["saved"], true);
    assert_eq!(saved["applied"], false);
    assert_eq!(saved["catalog_refreshed"], false);
    assert_eq!(saved["status"], "saved_not_applied");
    assert_eq!(saved["replayed"], false);
    assert!(
        saved["refresh_error"]
            .as_str()
            .unwrap()
            .contains("agent base role unavailable")
    );
    assert!(revision(&saved) > revision(&created));
    assert_eq!(
        fixture.get().await["definition_revision"],
        saved["definition_revision"]
    );
    for id in [&original.id, &second.definition_id] {
        assert!(fixture.catalog.profile(Some(id)).is_none());
        assert!(catalog_clone.profile(Some(id)).is_none());
    }
    assert_eq!(fixture.catalog.public_profile_ids(), builtin_ids);
    let remaining = fixture.catalog.profile(Some("reviewer")).unwrap();
    assert!(remaining.definition.is_none());
    assert_eq!(remaining.capability_prompt, builtin.capability_prompt);
    assert!(Arc::ptr_eq(&remaining.provider, &builtin.provider));
    assert_eq!(
        original.definition.as_ref().unwrap().instructions,
        definition().instructions
    );

    let failed_replay = fixture
        .host
        .execute("update", update.clone())
        .await
        .unwrap();
    assert_eq!(failed_replay["replayed"], true);
    assert_eq!(failed_replay["status"], "saved_not_applied");
    assert_eq!(
        failed_replay["installation_revision"],
        saved["installation_revision"]
    );
    assert!(fixture.catalog.profile(Some(&original.id)).is_none());
    fixture.host.base_profiles = bases;
    let recovered = fixture.host.execute("update", update).await.unwrap();
    assert_applied(&recovered, true);
    assert_eq!(
        recovered["installation_revision"],
        saved["installation_revision"]
    );
    assert_eq!(
        fixture.get().await["installation_revision"],
        saved["installation_revision"]
    );
    assert_eq!(
        fixture.profile().definition.as_ref().unwrap().instructions,
        changed_definition().instructions
    );
    assert!(
        fixture
            .catalog
            .profile(Some(&second.definition_id))
            .is_some()
    );
}

#[tokio::test]
async fn create_spawn_update_bind_uses_receipts_without_an_intervening_read() {
    #[derive(Default)]
    struct DispatchProbe(std::sync::Mutex<Vec<String>>);
    #[async_trait]
    impl Provider for DispatchProbe {
        async fn complete(
            &self,
            messages: &[Message],
            _: &[ToolDefinition],
        ) -> Result<Completion, ProviderError> {
            self.0.lock().unwrap().push(messages[0].content.clone());
            Ok(Completion {
                reasoning: None,
                content: "fixture inspection complete".into(),
                tool_calls: Vec::new(),
                finish_reason: Some("stop".into()),
                usage: None,
            })
        }
    }
    let provider = Arc::new(DispatchProbe::default());
    let fixture = Fixture::with_provider(None, provider.clone());
    let created = fixture.create().await;
    let original = fixture.profile();
    // The real spawn admission path captures an owned definition before updates.
    let spawned = fixture
        .catalog
        .enqueue_owned(
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            willdeep_core::subagent::SpawnAgentArgs {
                prompt: "Inspect this isolated fixture".into(),
                profile: Some(definition().definition_id),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    let updated = fixture
        .host
        .execute(
            "update",
            request(
                json!({"definition":changed_definition()}),
                Some(revision(&created)),
            ),
        )
        .await
        .unwrap();
    let bound = fixture
        .host
        .execute(
            "bind",
            request(
                json!({"id":definition().definition_id,"binding":{"max_turns":7}}),
                Some(revision(&updated)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&bound, false);
    assert_eq!(fixture.profile().max_turns, 7);
    let store =
        willdeep_core::subagent::AgentRunStore::new(&fixture.home, spawned.parent_session_id);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while store.get(spawned.id).unwrap().unwrap().is_pending() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture worker completes without an external model");
    let run = store.get(spawned.id).unwrap().unwrap();
    assert_eq!(run.max_turns, Some(original.max_turns as u64));
    let prompts = provider.0.lock().unwrap();
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains(&definition().instructions));
    assert!(!prompts[0].contains(&changed_definition().instructions));
}

#[tokio::test]
async fn replay_preserves_the_original_preconditions_without_rewinding_observations() {
    let fixture = Fixture::new();
    let created = fixture.create().await;
    let bind = request(
        json!({"id":definition().definition_id,"binding":{"max_turns":7}}),
        Some(revision(&created)),
    );
    let bound = fixture.host.execute("bind", bind.clone()).await.unwrap();
    let enabled = fixture
        .host
        .execute(
            "enable",
            request(
                json!({"id":definition().definition_id,"value":true}),
                Some(revision(&bound)),
            ),
        )
        .await
        .unwrap();
    fixture
        .catalog
        .replace_specialist_profiles(Vec::new())
        .unwrap();
    let replayed = fixture.host.execute("bind", bind.clone()).await.unwrap();
    assert_applied(&replayed, true);
    assert_eq!(revision(&replayed), revision(&bound));
    assert_eq!(fixture.profile().max_turns, 7);
    // No reread: the later enable receipt must still be the observed revision.
    let updated = fixture
        .host
        .execute(
            "update",
            request(
                json!({"definition_text":serde_json::to_string(&changed_definition()).unwrap()}),
                Some(revision(&enabled)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&updated, false);
    let mut changed = bind;
    changed["binding"]["max_turns"] = 8.into();
    let error = fixture.host.execute("bind", changed).await.unwrap_err();
    assert!(error.to_string().contains("agent_request_conflict"));
}

#[tokio::test]
async fn replay_keeps_its_local_target_after_a_private_installation_is_read() {
    let session = session("same-user");
    let fixture = Fixture::with_session(Some(&session));
    let created = fixture.create().await;
    let bind = request(
        json!({"id":definition().definition_id,"binding":{"max_turns":7}}),
        Some(revision(&created)),
    );
    fixture.host.execute("bind", bind.clone()).await.unwrap();
    let private = session.library(&fixture.home);
    let package = willdeep_core::agent_definition::AgentPackage::local(definition()).unwrap();
    private
        .install(&package, Origin::Private, &BTreeMap::new())
        .unwrap();
    let current = fixture.get().await;
    assert_eq!(current["target_scope"], "private");
    assert_eq!(revision(&current), revision(&created));
    let local = AgentLibrary::new(&fixture.home);
    let local_before = local.installations().unwrap();
    let private_before = private.installations().unwrap();
    // Replaying must use the original local target, even though the newly read
    // private installation matches the original request's numeric revision.
    let error = fixture.host.execute("bind", bind).await.unwrap_err();
    assert!(error.to_string().contains("agent_scope_conflict"));
    assert_eq!(local.installations().unwrap(), local_before);
    assert_eq!(private.installations().unwrap(), private_before);
}

#[tokio::test]
async fn fork_observes_the_destination_without_replacing_the_source_precondition() {
    let fixture = Fixture::new();
    let created = fixture.create().await;
    let fork = request(
        json!({"id":definition().definition_id,"new_id":"user/host-fork"}),
        Some(revision(&created)),
    );
    let forked = fixture.host.execute("fork", fork.clone()).await.unwrap();
    assert_applied(&forked, false);
    assert_eq!(forked["target_scope"], "local");
    assert_eq!(forked["source_definition_id"], definition().definition_id);
    assert_eq!(
        forked["definition"]["forked_from"],
        created["definition_revision"]
    );
    let bound = fixture
        .host
        .execute(
            "bind",
            request(
                json!({"id":"user/host-fork","binding":{"max_turns":7}}),
                Some(revision(&forked)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&bound, false);
    let source = fixture
        .host
        .execute(
            "enable",
            request(
                json!({"id":definition().definition_id,"value":false}),
                Some(revision(&created)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&source, false);
    assert_applied(&fixture.host.execute("fork", fork).await.unwrap(), true);
    let enabled = fixture
        .host
        .execute(
            "enable",
            request(
                json!({"id":"user/host-fork","value":true}),
                Some(revision(&bound)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&enabled, false);
}

#[tokio::test]
async fn saved_not_applied_receipts_advance_observations_before_any_reread() {
    let mut fixture = Fixture::new();
    let created = fixture.create().await;
    let bases = std::mem::take(&mut fixture.host.base_profiles);
    let update = request(
        json!({"definition":changed_definition()}),
        Some(revision(&created)),
    );
    let saved = fixture
        .host
        .execute("update", update.clone())
        .await
        .unwrap();
    assert_eq!(saved["status"], "saved_not_applied");
    let bound = fixture
        .host
        .execute(
            "bind",
            request(
                json!({"id":definition().definition_id,"binding":{"max_turns":7}}),
                Some(revision(&saved)),
            ),
        )
        .await
        .unwrap();
    assert_eq!(bound["saved"], true);
    assert_eq!(bound["status"], "saved_not_applied");
    assert!(revision(&bound) > revision(&saved));
    fixture.host.base_profiles = bases;
    let replayed = fixture.host.execute("update", update).await.unwrap();
    assert_applied(&replayed, true);
    assert_eq!(revision(&replayed), revision(&saved));
    assert_eq!(fixture.profile().max_turns, 7);
    let enabled = fixture
        .host
        .execute(
            "enable",
            request(
                json!({"id":definition().definition_id,"value":true}),
                Some(revision(&bound)),
            ),
        )
        .await
        .unwrap();
    assert_applied(&enabled, false);
}

#[tokio::test]
async fn unsupported_operations_are_rejected_before_registry_or_network_access() {
    let fixture = Fixture::new();
    for operation in ["catalog", "install", "sync", "delete", "pin", "rollback"] {
        let error = fixture
            .host
            .execute(operation, json!({}))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported local Agent library operation")
        );
    }
    assert!(
        AgentLibrary::new(&fixture.home)
            .installations()
            .unwrap()
            .is_empty()
    );
    let error = fixture.host.execute("create", json!([])).await.unwrap_err();
    assert!(error.to_string().contains("parameters must be an object"));
}
