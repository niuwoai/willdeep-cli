use super::*;
use crate::subagent::WorkflowDispatchIdentity;
use crate::{Agent, AgentConfig, ApprovalMode, ToolRegistry};
use serde_json::json;

fn identity() -> WorkflowDispatchIdentity {
    WorkflowDispatchIdentity {
        workflow_run_id: uuid::Uuid::new_v4(),
        node_instance_id: "stage/0".into(),
        request_id: uuid::Uuid::new_v4(),
        agent_id: uuid::Uuid::new_v4(),
    }
}

fn host(catalog: &SubagentCatalog, provider: &RecordingProvider) -> Agent {
    Agent::new(
        Arc::new(provider.clone()),
        ToolRegistry::new(&catalog.workspace, ApprovalMode::ReadOnly).unwrap(),
        AgentConfig {
            max_turns: 1,
            system_prompt: String::new(),
            context_window: 32000,
            token_budget: None,
        },
    )
    .with_subagents(Arc::new(catalog.clone()))
}

#[tokio::test]
async fn workflow_freezes_all_phases_and_keeps_parent_runtime_handles() {
    let (catalog, background, provider, root) = fixture(false, 2);
    catalog
        .replace_specialist_profiles(vec![specialist(
            &catalog,
            "user/phases",
            "RUN_ORIGINAL",
            "run-model",
        )])
        .unwrap();
    let approval = crate::tools::SharedApprovalMode::new(ApprovalMode::ReadOnly);
    let catalog = catalog.with_parent_approval_mode(approval.clone());
    let saved = catalog.export_workflow_snapshot().unwrap();
    let encoded = serde_json::to_string(&saved).unwrap();
    for forbidden in ["api_key", "provider_config", "base_url", "authorization"] {
        assert!(!encoded.contains(&format!("\"{forbidden}\":")));
    }
    let frozen = catalog.from_workflow_snapshot(&saved).unwrap();
    assert_eq!(frozen.export_workflow_snapshot().unwrap(), saved);
    assert!(!Arc::ptr_eq(&catalog.profiles, &frozen.profiles));
    assert!(Arc::ptr_eq(&catalog.background, &frozen.background));
    assert!(Arc::ptr_eq(
        &catalog.dispatch_admission,
        &frozen.dispatch_admission
    ));
    assert!(Arc::ptr_eq(&catalog.claimed_files, &frozen.claimed_files));
    assert!(Arc::ptr_eq(
        &catalog.owned_background_agents,
        &frozen.owned_background_agents
    ));
    approval.set(ApprovalMode::FullAccess);
    assert_eq!(
        frozen.parent_approval_mode.as_ref().unwrap().get(),
        ApprovalMode::FullAccess
    );
    let agent = host(&catalog, &provider);
    let first = identity();
    agent
        .dispatch_workflow_node_with_catalog(args("user/phases"), first.clone(), &frozen)
        .await
        .unwrap();
    assert_eq!(
        settled(&background, first.request_id).await.status,
        willdeep_runtime_protocol::AgentStatus::Completed
    );
    catalog
        .replace_specialist_profiles(vec![specialist(
            &catalog,
            "user/replacement",
            "CHAT_REPLACEMENT",
            "chat-model",
        )])
        .unwrap();
    assert!(catalog.profile(Some("user/phases")).is_none());
    let mut second = identity();
    second.workflow_run_id = first.workflow_run_id;
    second.node_instance_id = "stage/1".into();
    agent
        .dispatch_workflow_node_with_catalog(args("user/phases"), second.clone(), &frozen)
        .await
        .unwrap();
    assert_eq!(
        settled(&background, second.request_id)
            .await
            .model
            .as_deref(),
        Some("run-model")
    );
    let histories = provider.histories.lock().unwrap();
    assert_eq!(histories.len(), 2);
    assert!(histories.iter().all(|(model, prompt)| model == "run-model"
        && prompt.contains("RUN_ORIGINAL")
        && !prompt.contains("CHAT_REPLACEMENT")));
    drop(histories);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn workflow_frozen_catalog_shares_queue_with_ordinary_dispatch() {
    let (catalog, background, provider, root) = fixture(true, 1);
    let frozen = catalog
        .from_workflow_snapshot(&catalog.export_workflow_snapshot().unwrap())
        .unwrap();
    let ordinary = uuid::Uuid::new_v4();
    catalog
        .enqueue_owned(ordinary, uuid::Uuid::new_v4(), args("scout"), None, None)
        .await
        .unwrap();
    provider.started.notified().await;
    let node = identity();
    let queued = host(&catalog, &provider)
        .dispatch_workflow_node_with_catalog(args("scout"), node.clone(), &frozen)
        .await
        .unwrap();
    assert_eq!(
        queued.status,
        willdeep_runtime_protocol::AgentStatus::Queued
    );
    assert_eq!(catalog.available_dispatch_capacity().unwrap(), 0);
    assert_eq!(frozen.available_dispatch_capacity().unwrap(), 0);
    provider.gate.as_ref().unwrap().add_permits(2);
    let ordinary = settled(&background, ordinary).await;
    assert!(ordinary.report.unwrap().contains("inspection complete"));
    assert_eq!(
        settled(&background, node.request_id).await.status,
        willdeep_runtime_protocol::AgentStatus::Completed
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_restart_restores_removed_definition_through_original_binding() {
    let (catalog, background, provider, root) = fixture(false, 1);
    let mut original = specialist(&catalog, "user/bound", "ORIGINAL_BOUND", "bound-model");
    original
        .definition_binding
        .as_mut()
        .unwrap()
        .provider_profile = Some("bound-provider".into());
    catalog.replace_specialist_profiles(vec![original]).unwrap();
    let saved = catalog.export_workflow_snapshot().unwrap();
    let restarted = SubagentCatalog::new(
        &catalog.workspace,
        super::super::super::builtin_profiles(provider.with_model("changed-host-model").unwrap()),
        background,
    );
    assert!(
        restarted
            .from_workflow_snapshot(&saved)
            .err()
            .expect("restore must fail")
            .to_string()
            .contains("binding resolver")
    );
    let resolved = restarted
        .clone()
        .with_definition_bindings(|binding, fallback| {
            assert_eq!(binding.provider_profile.as_deref(), Some("bound-provider"));
            assert_eq!(binding.model.as_deref(), Some("bound-model"));
            assert_eq!(binding.max_turns, Some(7));
            fallback.with_model("bound-model").map_err(Into::into)
        })
        .from_workflow_snapshot(&saved)
        .unwrap();
    let profile = resolved.profile(Some("user/bound")).unwrap();
    assert_eq!(profile.model.as_deref(), Some("bound-model"));
    assert_eq!(profile.max_turns, 7);
    assert!(profile.capability_prompt.contains("ORIGINAL_BOUND"));
    let unavailable = restarted.with_definition_bindings(|_, _| {
        Err(AgentError::Subagent("original provider disabled".into()))
    });
    assert!(
        unavailable
            .from_workflow_snapshot(&saved)
            .err()
            .expect("restore must fail")
            .to_string()
            .contains("original provider disabled")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn workflow_freezes_followers_and_tiers_and_does_not_add_new_tier_bindings() {
    let (catalog, _, provider, root) = fixture(false, 1);
    let main = crate::provider::MainModelHandle::new(Arc::new(provider.clone()));
    let tier = crate::provider::MainModelHandle::new(provider.with_model("original-tier").unwrap());
    let catalog = SubagentCatalog::new(
        &catalog.workspace,
        super::super::super::builtin_profiles(main.follower()),
        catalog.background.clone(),
    )
    .with_tier_binding(
        crate::WorkerTier::Advanced,
        TierBinding {
            provider: tier.follower(),
            model: None,
            window: 64000,
            hosted_job_prompt: false,
        },
    );
    let saved = catalog.export_workflow_snapshot().unwrap();
    let live_frozen = catalog.from_workflow_snapshot(&saved).unwrap();
    main.set_model("changed-main").unwrap();
    tier.set_model("changed-tier").unwrap();
    let frozen = catalog.from_workflow_snapshot(&saved).unwrap();
    for frozen in [&frozen, &live_frozen] {
        assert_eq!(
            crate::provider::provider_model(
                frozen
                    .profile(Some("generalist"))
                    .unwrap()
                    .provider
                    .as_ref()
            )
            .as_deref(),
            Some("original-model")
        );
        assert_eq!(
            frozen
                .dispatch_model("generalist", Some("advanced"))
                .model
                .as_deref(),
            Some("original-tier")
        );
        assert_eq!(
            crate::provider::provider_model(
                frozen
                    .tier_binding(crate::WorkerTier::Advanced)
                    .unwrap()
                    .provider
                    .as_ref()
            )
            .as_deref(),
            Some("original-tier")
        );
    }
    let mut changed = catalog.clone();
    changed.tier_bindings.clear();
    assert!(
        changed
            .from_workflow_snapshot(&saved)
            .err()
            .expect("restore must fail")
            .to_string()
            .contains("tier provider is unavailable")
    );
    let without_tiers = changed.export_workflow_snapshot().unwrap();
    let restored = catalog.from_workflow_snapshot(&without_tiers).unwrap();
    assert!(restored.tier_binding(crate::WorkerTier::Advanced).is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[derive(Clone)]
struct WrongBinding {
    route: &'static str,
    hides_identity: bool,
}
#[async_trait::async_trait]
impl Provider for WrongBinding {
    fn ledger_identity(&self) -> Option<ProviderIdentity> {
        if self.route.is_empty() {
            return None;
        }
        Some(ProviderIdentity {
            provider: self.route.into(),
            model: "wrong-model".into(),
            local: true,
        })
    }
    fn with_model(&self, _: &str) -> Result<Arc<dyn Provider>, ProviderError> {
        Ok(Arc::new(Self {
            route: if self.hides_identity { "" } else { self.route },
            ..self.clone()
        }))
    }
    async fn complete(
        &self,
        _: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        panic!("a mismatched binding must never execute")
    }
}

#[test]
fn workflow_blocks_provider_route_or_model_drift_and_rejects_invalid_snapshots() {
    let (catalog, _, _, root) = fixture(false, 1);
    let mut original = specialist(&catalog, "user/bound", "ORIGINAL", "saved-model");
    original
        .definition_binding
        .as_mut()
        .unwrap()
        .provider_profile = Some("original-route".into());
    catalog.replace_specialist_profiles(vec![original]).unwrap();
    let saved = catalog.export_workflow_snapshot().unwrap();
    for (route, hides_identity) in [
        ("different-provider", false),
        ("fixture", false),
        ("fixture", true),
    ] {
        let changed = catalog.clone().with_definition_bindings(move |_, _| {
            Ok(Arc::new(WrongBinding {
                route,
                hides_identity,
            }))
        });
        assert!(changed.from_workflow_snapshot(&saved).is_err());
    }
    let mut invalid = saved.clone();
    invalid["version"] = json!(99);
    assert!(catalog.from_workflow_snapshot(&invalid).is_err());
    invalid = saved.clone();
    invalid["provider_config"] = json!({});
    assert!(catalog.from_workflow_snapshot(&invalid).is_err());
    invalid = saved;
    invalid["profiles"]["user/bound"]["execution"]["id"] = json!("forged");
    assert!(catalog.from_workflow_snapshot(&invalid).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn workflow_authorization_uses_frozen_write_scope_and_preserves_safety_gates() {
    let (catalog, background, provider, root) = fixture(false, 1);
    let mut writer = specialist(&catalog, "user/policy", "WRITER", "saved-model");
    writer.write_scope = SubagentWriteScope::FileSet;
    catalog.replace_specialist_profiles(vec![writer]).unwrap();
    let frozen_writer = catalog
        .from_workflow_snapshot(&catalog.export_workflow_snapshot().unwrap())
        .unwrap();
    catalog
        .replace_specialist_profiles(vec![specialist(
            &catalog,
            "user/policy",
            "READER",
            "new-model",
        )])
        .unwrap();
    let agent = host(&catalog, &provider);
    let error = agent
        .dispatch_workflow_node_with_catalog(args("user/policy"), identity(), &frozen_writer)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("declared up front"));
    let mut writes = args("user/policy");
    writes.target_file = Some("approved.rs".into());
    assert!(
        agent
            .dispatch_workflow_node_with_catalog(writes, identity(), &frozen_writer)
            .await
            .is_err()
    );
    let frozen_reader = catalog
        .from_workflow_snapshot(&catalog.export_workflow_snapshot().unwrap())
        .unwrap();
    let mut newest = specialist(&catalog, "user/policy", "NEW_WRITER", "newer-model");
    newest.write_scope = SubagentWriteScope::FileSet;
    catalog.replace_specialist_profiles(vec![newest]).unwrap();
    let readonly = identity();
    agent
        .dispatch_workflow_node_with_catalog(args("user/policy"), readonly.clone(), &frozen_reader)
        .await
        .unwrap();
    assert_eq!(
        settled(&background, readonly.request_id).await.status,
        willdeep_runtime_protocol::AgentStatus::Completed
    );
    let mut expert = args("user/policy");
    expert.worker_tier = Some("expert".into());
    let error = agent
        .dispatch_workflow_node_with_catalog(expert, identity(), &frozen_reader)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("deep requires escalation"));
    let mut command = args("user/policy");
    command.target_command = Some("pwd".into());
    let error = agent
        .dispatch_workflow_node_with_catalog(command, identity(), &frozen_reader)
        .await
        .unwrap_err();
    assert!(matches!(error, ToolError::ApprovalDenied(_)));
    assert_eq!(provider.histories.lock().unwrap().len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}
