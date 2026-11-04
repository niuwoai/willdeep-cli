use super::*;
use crate::agent_definition::store::ModelBinding;
use crate::agent_definition::{AgentDefinition, SCHEMA};
use crate::provider::{ProviderError, ProviderIdentity};
use crate::types::{Completion, Message, ToolDefinition};
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "workflow_snapshot_tests.rs"]
mod workflow_tests;

#[derive(Clone)]
struct RecordingProvider {
    model: String,
    histories: Arc<Mutex<Vec<(String, String)>>>,
    gate: Option<Arc<tokio::sync::Semaphore>>,
    started: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Provider for RecordingProvider {
    fn ledger_identity(&self) -> Option<ProviderIdentity> {
        Some(ProviderIdentity {
            provider: "fixture".into(),
            model: self.model.clone(),
            local: true,
        })
    }
    fn with_model(&self, model: &str) -> Result<Arc<dyn Provider>, ProviderError> {
        Ok(Arc::new(Self {
            model: model.into(),
            ..self.clone()
        }))
    }
    async fn complete(
        &self,
        messages: &[Message],
        _: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        self.histories.lock().unwrap().push((
            self.model.clone(),
            messages
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ));
        self.started.notify_one();
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(Completion {
            content: if messages
                .iter()
                .any(|message| message.content.contains("Workflow result contract:"))
            {
                serde_json::json!({"inspection":"complete"}).to_string()
            } else {
                "inspection complete".into()
            },
            reasoning: None,
            tool_calls: Vec::new(),
            finish_reason: Some("stop".into()),
            usage: None,
        })
    }
}

fn fixture(
    gated: bool,
    capacity: usize,
) -> (
    SubagentCatalog,
    Arc<BackgroundTaskRegistry>,
    RecordingProvider,
    PathBuf,
) {
    let root = std::env::temp_dir().join(format!("catalog-snapshot-{}", uuid::Uuid::new_v4()));
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let home = root.join("workers");
    let parent = uuid::Uuid::new_v4();
    let provider = RecordingProvider {
        model: "original-model".into(),
        histories: Default::default(),
        gate: gated.then(|| Arc::new(tokio::sync::Semaphore::new(0))),
        started: Default::default(),
    };
    let background = Arc::new(
        BackgroundTaskRegistry::default()
            .with_agent_team_settings(super::super::AgentTeamSettings {
                max_concurrent: capacity,
                max_queued: 1,
                token_budget: None,
            })
            .unwrap()
            .with_agent_run_store(super::super::AgentRunStore::new(&home, parent)),
    );
    let catalog = SubagentCatalog::new(
        &workspace,
        super::super::builtin_profiles(Arc::new(provider.clone())),
        background.clone(),
    )
    .with_state_home(home)
    .with_parent_session(parent);
    (catalog, background, provider, root)
}

fn specialist(
    catalog: &SubagentCatalog,
    id: &str,
    instructions: &str,
    model: &str,
) -> SubagentProfile {
    let definition = AgentDefinition {
        schema: SCHEMA.into(),
        definition_id: id.into(),
        version: "1.0.0".into(),
        name: BTreeMap::from([("en".into(), "Fixture".into())]),
        purpose: "Inspect fixture".into(),
        base_role: "generalist".into(),
        instructions: instructions.into(),
        input_modalities: vec!["text".into()],
        tools: vec!["workspace.read".into()],
        skills: Vec::new(),
        forked_from: None,
    };
    let mut profile = definition
        .project(&catalog.profile(Some("generalist")).unwrap())
        .unwrap();
    profile.model = Some(model.into());
    profile.provider = profile.provider.with_model(model).unwrap();
    profile.definition_model_bound = true;
    profile.definition_binding = Some(ModelBinding {
        model: Some(model.into()),
        context_window: Some(65536),
        max_turns: Some(7),
        ..Default::default()
    });
    profile.context_window = 65536;
    profile.max_turns = 7;
    profile
}

fn args(profile: &str) -> SpawnAgentArgs {
    SpawnAgentArgs {
        profile: Some(profile.into()),
        prompt: "inspect fixture".into(),
        ..Default::default()
    }
}

async fn settled(
    background: &BackgroundTaskRegistry,
    request: uuid::Uuid,
) -> willdeep_runtime_protocol::AgentRun {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let run = background
                .agent_run_store()
                .unwrap()
                .get(request)
                .unwrap()
                .unwrap();
            if !run.is_pending() {
                return run;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn refresh_is_shared_new_dispatch_sees_new_id_and_in_flight_keeps_snapshot() {
    let (catalog, background, provider, root) = fixture(true, 2);
    let clone = catalog.clone();
    catalog
        .replace_specialist_profiles(vec![specialist(
            &catalog,
            "user/old",
            "ORIGINAL_INSTRUCTIONS",
            "old-model",
        )])
        .unwrap();
    let first_id = uuid::Uuid::new_v4();
    catalog
        .enqueue_owned(first_id, uuid::Uuid::new_v4(), args("user/old"), None, None)
        .await
        .unwrap();
    provider.started.notified().await;
    let captured = catalog.profile(Some("user/old")).unwrap();
    clone
        .replace_specialist_profiles(vec![specialist(
            &catalog,
            "user/new",
            "NEW_INSTRUCTIONS",
            "new-model",
        )])
        .unwrap();
    assert!(catalog.profile(Some("user/old")).is_none());
    assert!(catalog.public_profile_ids().contains(&"user/new".into()));
    assert!(
        catalog
            .profile(Some("generalist"))
            .unwrap()
            .definition
            .is_none()
    );
    assert!(captured.capability_prompt.contains("ORIGINAL_INSTRUCTIONS"));
    let second_id = uuid::Uuid::new_v4();
    catalog
        .enqueue_owned(
            second_id,
            uuid::Uuid::new_v4(),
            args("user/new"),
            None,
            None,
        )
        .await
        .unwrap();
    provider.started.notified().await;
    provider.gate.as_ref().unwrap().add_permits(2);
    assert_eq!(
        settled(&background, first_id).await.model.as_deref(),
        Some("old-model")
    );
    assert_eq!(
        settled(&background, second_id).await.model.as_deref(),
        Some("new-model")
    );
    let histories = provider.histories.lock().unwrap();
    assert!(histories.iter().any(|(model, text)| model == "old-model"
        && text.contains("ORIGINAL_INSTRUCTIONS")
        && !text.contains("NEW_INSTRUCTIONS")));
    assert!(
        histories
            .iter()
            .any(|(model, text)| model == "new-model" && text.contains("NEW_INSTRUCTIONS"))
    );
    drop(histories);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn removed_specialist_continuation_keeps_definition_model_binding_and_limits() {
    let (catalog, background, provider, root) = fixture(false, 2);
    let mut original = specialist(&catalog, "user/saved", "FROZEN_DEFINITION", "saved-model");
    original
        .definition_binding
        .as_mut()
        .unwrap()
        .provider_profile = Some("original-provider-profile".into());
    catalog.replace_specialist_profiles(vec![original]).unwrap();
    let original_request = uuid::Uuid::new_v4();
    let agent = uuid::Uuid::new_v4();
    catalog
        .enqueue_owned(original_request, agent, args("user/saved"), None, None)
        .await
        .unwrap();
    settled(&background, original_request).await;
    // A newer configured model must never leak into the old dispatch.
    catalog
        .replace_specialist_profiles(vec![specialist(
            &catalog,
            "user/saved",
            "NEW_DEFINITION",
            "newest-model",
        )])
        .unwrap();
    let saved = background
        .agent_run_store()
        .unwrap()
        .load(original_request)
        .unwrap()
        .unwrap();
    let dispatch = serde_json::from_value(saved.dispatch).unwrap();
    let while_newest_is_installed = catalog
        .clone()
        .with_definition_bindings(|binding, fallback| {
            assert_eq!(
                binding.provider_profile.as_deref(),
                Some("original-provider-profile")
            );
            assert_eq!(binding.model.as_deref(), Some("saved-model"));
            Ok(fallback.with_model(binding.model.as_deref().unwrap())?)
        })
        .restored_profile(&dispatch)
        .unwrap();
    assert_eq!(
        while_newest_is_installed.model.as_deref(),
        Some("saved-model")
    );
    assert!(
        while_newest_is_installed
            .capability_prompt
            .contains("FROZEN_DEFINITION")
    );
    assert!(
        !while_newest_is_installed
            .capability_prompt
            .contains("NEW_DEFINITION")
    );
    catalog.replace_specialist_profiles(Vec::new()).unwrap();
    let duplicate = catalog
        .enqueue_owned(original_request, agent, args("user/saved"), None, None)
        .await
        .unwrap();
    assert_eq!(duplicate.id, original_request);
    assert_eq!(provider.histories.lock().unwrap().len(), 1);
    let missing = catalog
        .continue_background_agent(agent, "continue inspection", uuid::Uuid::new_v4())
        .await
        .unwrap_err();
    assert!(
        missing
            .to_string()
            .contains("host definition binding resolver")
    );
    let resolved = Arc::new(AtomicUsize::new(0));
    let observed = resolved.clone();
    let restored = catalog.with_definition_bindings(move |binding, fallback| {
        assert_eq!(
            binding.provider_profile.as_deref(),
            Some("original-provider-profile")
        );
        assert_eq!(binding.model.as_deref(), Some("saved-model"));
        assert_eq!(binding.context_window, Some(65536));
        assert_eq!(binding.max_turns, Some(7));
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(fallback.with_model(binding.model.as_deref().unwrap())?)
    });
    let continuation = uuid::Uuid::new_v4();
    restored
        .continue_background_agent(agent, "continue inspection", continuation)
        .await
        .unwrap();
    let run = settled(&background, continuation).await;
    assert_eq!(run.model.as_deref(), Some("saved-model"));
    assert_eq!(run.max_turns, Some(7));
    assert_eq!(resolved.load(Ordering::SeqCst), 1);
    let histories = provider.histories.lock().unwrap();
    assert_eq!(histories.len(), 2);
    assert!(histories.iter().all(|(model, text)| model == "saved-model"
        && text.contains("FROZEN_DEFINITION")
        && !text.contains("NEW_DEFINITION")));
    drop(histories);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stable_intent_is_idempotent_and_workflow_uses_parent_scheduler() {
    use super::super::types::WorkflowDispatchIdentity;
    let (catalog, background, provider, root) = fixture(true, 1);
    let request = uuid::Uuid::new_v4();
    let agent = uuid::Uuid::new_v4();
    let plain = catalog
        .enqueue_owned(request, agent, args("scout"), None, None)
        .await
        .unwrap();
    assert!(plain.workflow_owner.is_none());
    provider.started.notified().await;
    let workflow_request = uuid::Uuid::new_v4();
    let identity = WorkflowDispatchIdentity {
        workflow_run_id: uuid::Uuid::new_v4(),
        node_instance_id: "node:iteration:0".into(),
        request_id: workflow_request,
        agent_id: uuid::Uuid::new_v4(),
    };
    let queued = catalog
        .run_authorized_with_identity(args("scout"), None, None, identity.clone())
        .await
        .unwrap();
    assert_eq!(
        queued.status,
        willdeep_runtime_protocol::AgentStatus::Queued
    );
    assert_eq!(queued.parent_session_id, plain.parent_session_id);
    assert_eq!(
        queued.workflow_owner.as_ref().unwrap().workflow_run_id,
        identity.workflow_run_id
    );
    assert_eq!(catalog.available_dispatch_capacity().unwrap(), 0);
    let duplicate = catalog
        .run_authorized_with_identity(args("scout"), None, None, identity.clone())
        .await
        .unwrap();
    assert_eq!(duplicate.id, queued.id);
    let mut changed = args("scout");
    changed.prompt = "different intent".into();
    assert!(
        catalog
            .run_authorized_with_identity(changed, None, None, identity.clone())
            .await
            .is_err()
    );
    assert!(
        catalog
            .enqueue_owned(
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                args("scout"),
                None,
                None
            )
            .await
            .is_err()
    );
    provider.gate.as_ref().unwrap().add_permits(2);
    settled(&background, request).await;
    settled(&background, workflow_request).await;
    let terminal = catalog
        .run_authorized_with_identity(args("scout"), None, None, identity)
        .await
        .unwrap();
    assert_eq!(terminal.id, workflow_request);
    assert_eq!(provider.histories.lock().unwrap().len(), 2);
    assert_eq!(
        background
            .agent_run_store()
            .unwrap()
            .list(None, None, 64)
            .unwrap()
            .runs
            .len(),
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn queued_workflow_survives_host_restart_without_installed_specialist() {
    use super::super::types::WorkflowDispatchIdentity;
    let (catalog, background, provider, root) = fixture(false, 1);
    catalog
        .replace_specialist_profiles(vec![specialist(
            &catalog,
            "user/recovered",
            "ORIGINAL_RESTART_INSTRUCTIONS",
            "restart-model",
        )])
        .unwrap();
    let identity = WorkflowDispatchIdentity {
        workflow_run_id: uuid::Uuid::new_v4(),
        node_instance_id: "loop:4".into(),
        request_id: uuid::Uuid::new_v4(),
        agent_id: uuid::Uuid::new_v4(),
    };
    let initial = catalog
        .run_authorized_with_identity(args("user/recovered"), None, None, identity.clone())
        .await
        .unwrap();
    settled(&background, initial.id).await;
    let store = background.agent_run_store().unwrap().clone();
    let continuation_id = uuid::Uuid::new_v4();
    store
        .prepare_continuation(identity.agent_id, "queued continuation", continuation_id)
        .unwrap();
    let workspace = catalog.workspace.clone();
    let home = catalog.state_home.clone().unwrap();
    drop(catalog);
    drop(background);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(owner) = store.claim() {
                drop(owner);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let restarted_background =
        Arc::new(BackgroundTaskRegistry::default().with_agent_run_store(store.clone()));
    let mut bases = super::super::builtin_profiles(provider.with_model("new-host-model").unwrap());
    for base in &mut bases {
        base.max_turns = 1;
        base.context_window = 10000;
    }
    let restarted = SubagentCatalog::new(&workspace, bases, restarted_background.clone())
        .with_state_home(&home)
        .with_parent_session(initial.parent_session_id);
    assert!(restarted.profile(Some("user/recovered")).is_none());
    assert_eq!(
        restarted
            .restore_queued_runs_checked(|leaf| {
                Ok(Some(leaf.workflow_owner == initial.workflow_owner))
            })
            .await
            .unwrap(),
        1
    );
    let resumed = settled(&restarted_background, continuation_id).await;
    assert_eq!(
        resumed.status,
        willdeep_runtime_protocol::AgentStatus::Completed
    );
    assert_eq!(resumed.workflow_owner, initial.workflow_owner);
    assert_eq!(resumed.model.as_deref(), Some("restart-model"));
    assert_eq!(resumed.max_turns, Some(7));
    let histories = provider.histories.lock().unwrap();
    assert_eq!(histories.len(), 2);
    assert!(
        histories
            .iter()
            .all(|(model, text)| model == "restart-model"
                && text.contains("ORIGINAL_RESTART_INSTRUCTIONS"))
    );
    drop(histories);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn owner_cannot_be_forged_by_tool_json_and_legacy_dispatch_is_readable() {
    let owner =
        serde_json::json!({"workflow_run_id":uuid::Uuid::new_v4(), "node_instance_id":"forged"});
    let args: SpawnAgentArgs =
        serde_json::from_value(serde_json::json!({"prompt":"inspect", "workflow_owner":owner}))
            .unwrap();
    assert!(args.workflow_owner.is_none());
    let record: super::super::dispatch_store::DispatchRecord = serde_json::from_value(serde_json::json!({
        "version":1,"id":uuid::Uuid::new_v4(),"parent_session":null,"args":{"prompt":"inspect"},"approved_targets":null,"approved_command":null,"model":null,
        "prepared":{"workspace":"/fixture","root_workspace":"/fixture","branch":null,"dedicated":false}
    })).unwrap();
    assert!(record.execution.is_none());
    assert!(record.workflow_owner.is_none());
}
