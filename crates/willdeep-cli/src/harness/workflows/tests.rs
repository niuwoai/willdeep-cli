use super::*;
use std::sync::atomic::AtomicUsize;
use willdeep_core::provider::{Provider, ProviderError};
use willdeep_core::tools::{ApprovalMode, ToolRegistry};
use willdeep_core::types::{Completion, Message, ToolDefinition, Usage};
use willdeep_core::{AgentConfig, AgentEvent};

#[derive(Default)]
struct Events(Mutex<Vec<AgentEvent>>);
#[async_trait]
impl EventSink for Events {
    async fn emit(&self, event: AgentEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[derive(Default)]
struct Worker {
    calls: AtomicUsize,
    bad_first: AtomicBool,
    blocked: AtomicBool,
    release: tokio::sync::Notify,
    histories: Mutex<Vec<Vec<Message>>>,
}
#[async_trait]
impl Provider for Worker {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<Completion, ProviderError> {
        assert!(!tools.iter().any(|tool| matches!(
            tool.name.as_str(),
            "spawn_agent" | "workflow_control" | "agent_library_manage"
        )));
        assert!(
            messages
                .iter()
                .any(|m| m.content.contains("Workflow result contract"))
        );
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.histories.lock().unwrap().push(messages.to_vec());
        if self.blocked.load(Ordering::SeqCst) {
            self.release.notified().await;
        }
        Ok(Completion {
            reasoning: None,
            content: if self.bad_first.swap(false, Ordering::SeqCst) {
                "CONCLUSION: wrong result format".into()
            } else {
                json!({"ok":true,"call":call}).to_string()
            },
            tool_calls: vec![],
            finish_reason: Some("stop".into()),
            usage: Some(Usage {
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                cache_read_tokens: None,
                ..Usage::default()
            }),
        })
    }
}

struct Fixture {
    root: std::path::PathBuf,
    worker: Arc<Worker>,
    service: Arc<WorkflowService>,
    background: Arc<BackgroundTaskRegistry>,
    _agent: Arc<Agent>,
    guard: Option<WorkflowDriverGuard>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("workflow-driver-{}", Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let parent = Uuid::new_v4();
        let ledger = willdeep_core::subagent::AgentRunStore::new(root.join("workers"), parent);
        let background = Arc::new(BackgroundTaskRegistry::default().with_agent_run_store(ledger));
        let worker = Arc::new(Worker::default());
        let catalog = Arc::new(
            SubagentCatalog::new(
                &workspace,
                willdeep_core::subagent::builtin_profiles(worker.clone()),
                background.clone(),
            )
            .with_parent_session(parent)
            .with_state_home(root.join("workers"))
            .with_global_rules(Vec::new()),
        );
        let service = WorkflowService::new(
            &root,
            parent,
            catalog.clone(),
            background.clone(),
            Arc::new(Events::default()),
        );
        let tools = ToolRegistry::new(&workspace, ApprovalMode::FullAccess)
            .unwrap()
            .with_workflow_host(service.clone());
        let agent = Arc::new(
            Agent::new(
                worker.clone(),
                tools,
                AgentConfig {
                    max_turns: 4,
                    system_prompt: "parent".into(),
                    context_window: 32_000,
                    token_budget: None,
                },
            )
            .with_subagents(catalog)
            .with_background_tasks(background.clone()),
        );
        service.bind_agent(&agent);
        let guard = Some(WorkflowDriverGuard::new(service.clone()));
        Self {
            root,
            worker,
            service,
            background,
            _agent: agent,
            guard,
        }
    }
    fn spec(&self) -> WorkflowSpecV1 {
        serde_json::from_value(json!({
            "version":1,"id":"pipeline","name":"pipeline",
            "nodes":[
                {"id":"inspect","kind":"agent","profile":"generalist","prompt":"Inspect", "max_turns":1,"token_budget":100,"timeout_seconds":5},
                {"id":"verify","depends_on":["inspect"],"kind":"agent","profile":"generalist","prompt":"Verify", "input":{"$ref":"node:inspect#"}}
            ],"output":{"$ref":"node:verify#"}
        })).unwrap()
    }
    async fn start(&self, request: Uuid) -> WorkflowRun {
        self.service
            .operate(
                "run",
                json!({"definition_id":"pipeline","input":{},"request_id":request}),
            )
            .await
            .unwrap();
        self.wait(request).await
    }
    async fn wait(&self, id: Uuid) -> WorkflowRun {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                self.service.pending_reports().unwrap();
                let run = self.service.store.get_run(id).unwrap().unwrap();
                if run.status.is_terminal() {
                    return run;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("workflow driver must settle without a parent model turn")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.guard.take();
        // Only this test's nonce directory is owned here.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn driver_advances_after_parent_returns_and_collects_each_leaf_once() {
    let fixture = Fixture::new();
    fixture
        .service
        .store
        .save_definition(&fixture.spec())
        .unwrap();
    let id = Uuid::new_v4();
    let run = fixture.start(id).await;
    assert_eq!(
        run.status,
        willdeep_runtime_protocol::workflow::WorkflowRunStatus::Completed,
        "workflow states: {:#?}",
        fixture.service.store.list_runs().unwrap()
    );
    assert_eq!(fixture.worker.calls.load(Ordering::SeqCst), 2);
    let leaf = run.nodes[0].agent_run.as_ref().unwrap();
    assert!(
        fixture
            .service
            .catalog
            .retry_background_agent(leaf.agent_id, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("workflow nodes")
    );
    assert!(
        fixture
            .service
            .catalog
            .continue_background_agent(leaf.agent_id, "extra task", Uuid::new_v4())
            .await
            .unwrap_err()
            .to_string()
            .contains("workflow nodes")
    );
    assert_eq!(run.output.as_ref().unwrap()["call"], 2);
    assert!(fixture.background.drain_pending().is_empty());
    let ledger = fixture.background.agent_run_store().unwrap();
    for node in &run.nodes {
        let agent = node.agent_run.as_ref().unwrap();
        assert!(agent.workflow_owner.is_some());
        let output = ledger.structured_output(agent.id).unwrap().unwrap();
        assert_eq!(output["ok"], true);
        assert!(ledger.get(agent.id).unwrap().unwrap().delivered);
        assert!(agent.report.as_ref().unwrap().contains("worker-facts"));
    }
    let first = run
        .nodes
        .iter()
        .find(|node| node.node_id == "inspect")
        .unwrap()
        .agent_run
        .as_ref()
        .unwrap();
    assert_eq!(first.max_turns, Some(1));
    assert_eq!(first.token_budget, Some(100));
    assert_eq!(first.timeout_seconds, Some(5));
    assert_eq!(fixture.service.pending_reports().unwrap().len(), 1);
    fixture.service.acknowledge_report(id).unwrap();
    assert!(fixture.service.pending_reports().unwrap().is_empty());
    fixture
        .service
        .operate(
            "run",
            json!({"definition_id":"pipeline","input":{},"request_id":id}),
        )
        .await
        .unwrap();
    assert_eq!(fixture.worker.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn invalid_worker_json_blocks_downstream_and_retry_preserves_history() {
    let fixture = Fixture::new();
    fixture.worker.bad_first.store(true, Ordering::SeqCst);
    fixture
        .service
        .store
        .save_definition(&fixture.spec())
        .unwrap();
    let old = fixture.start(Uuid::new_v4()).await;
    assert_eq!(
        old.status,
        willdeep_runtime_protocol::workflow::WorkflowRunStatus::Failed
    );
    assert_eq!(fixture.worker.calls.load(Ordering::SeqCst), 1);
    let request = Uuid::new_v4();
    fixture
        .service
        .operate("retry", json!({"run_id":old.id,"request_id":request}))
        .await
        .unwrap();
    let retried = fixture.wait(request).await;
    assert_eq!(
        retried.status,
        willdeep_runtime_protocol::workflow::WorkflowRunStatus::Completed,
        "workflow states: {:#?}",
        fixture.service.store.list_runs().unwrap()
    );
    assert_eq!(retried.source_run_id, Some(old.id));
    assert_eq!(fixture.service.store.get_run(old.id).unwrap().unwrap(), old);
    assert_eq!(fixture.worker.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn stopping_graph_cancels_its_actual_worker_and_never_starts_downstream() {
    let fixture = Fixture::new();
    fixture.worker.blocked.store(true, Ordering::SeqCst);
    fixture
        .service
        .store
        .save_definition(&fixture.spec())
        .unwrap();
    let id = Uuid::new_v4();
    fixture
        .service
        .operate(
            "run",
            json!({"definition_id":"pipeline","input":{},"request_id":id}),
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while fixture.worker.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(DRIVER_POLL).await;
        }
    })
    .await
    .unwrap();
    let leaf = fixture
        .background
        .agent_run_store()
        .unwrap()
        .list(None, None, 64)
        .unwrap()
        .runs
        .remove(0);
    assert!(fixture.service.allows_queued_recovery(&leaf).unwrap());
    fixture
        .service
        .operate("stop", json!({"run_id":id}))
        .await
        .unwrap();
    assert!(!fixture.service.allows_queued_recovery(&leaf).unwrap());
    let stopped = fixture.wait(id).await;
    assert_eq!(
        stopped.status,
        willdeep_runtime_protocol::workflow::WorkflowRunStatus::Cancelled
    );
    assert_eq!(fixture.worker.calls.load(Ordering::SeqCst), 1);
    assert!(!fixture.background.has_pending_tasks());
}

#[tokio::test]
async fn later_phases_and_retry_keep_the_run_catalog_after_a_chat_update() {
    let fixture = Fixture::new();
    let mut definition = willdeep_core::agent_definition::AgentDefinition::parse(include_bytes!(
        "../../../../../docs/examples/agent-definition.v1.json"
    ))
    .unwrap();
    definition.instructions = "ORIGINAL_WORKFLOW_DEFINITION".into();
    let base = fixture.service.catalog.profile(Some("reviewer")).unwrap();
    fixture
        .service
        .catalog
        .replace_specialist_profiles(vec![definition.project(&base).unwrap()])
        .unwrap();
    let mut spec = fixture.spec();
    for node in &mut spec.nodes {
        if let willdeep_runtime_protocol::workflow::WorkflowNodeKind::Agent(agent) = &mut node.kind
        {
            agent.profile = definition.definition_id.clone();
        }
    }
    fixture.service.store.save_definition(&spec).unwrap();
    fixture.worker.blocked.store(true, Ordering::SeqCst);
    let id = Uuid::new_v4();
    fixture
        .service
        .operate("run", json!({"definition_id":"pipeline","request_id":id}))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while fixture.worker.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(DRIVER_POLL).await;
        }
    })
    .await
    .unwrap();
    let saved = fixture.service.store.host_snapshot(id).unwrap().unwrap();
    definition.version = "1.0.0-rc2".into();
    definition.instructions = "UPDATED_CHAT_DEFINITION".into();
    fixture
        .service
        .catalog
        .replace_specialist_profiles(vec![definition.project(&base).unwrap()])
        .unwrap();
    fixture.worker.blocked.store(false, Ordering::SeqCst);
    fixture.worker.release.notify_one();
    let completed = fixture.wait(id).await;
    assert_eq!(
        completed.status,
        willdeep_runtime_protocol::workflow::WorkflowRunStatus::Completed,
        "workflow states: {:#?}",
        fixture.service.store.list_runs().unwrap()
    );
    {
        let prompts = fixture.worker.histories.lock().unwrap();
        assert_eq!(prompts.len(), 2);
        for messages in prompts.iter() {
            let prompt = messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(prompt.contains("ORIGINAL_WORKFLOW_DEFINITION"));
            assert!(!prompt.contains("UPDATED_CHAT_DEFINITION"));
        }
    }
    let retry = Uuid::new_v4();
    fixture
        .service
        .operate("retry", json!({"run_id":id,"request_id":retry}))
        .await
        .unwrap();
    assert_eq!(
        fixture.wait(retry).await.status,
        willdeep_runtime_protocol::workflow::WorkflowRunStatus::Completed,
        "workflow states: {:#?}",
        fixture.service.store.list_runs().unwrap()
    );
    assert_eq!(
        fixture.service.store.host_snapshot(retry).unwrap().unwrap(),
        saved
    );
    assert_eq!(fixture.worker.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn dropping_an_inactive_host_preserves_another_hosts_durable_graph() {
    let mut fixture = Fixture::new();
    let spec = fixture.spec();
    fixture.service.store.save_definition(&spec).unwrap();
    let run = fixture
        .service
        .store
        .start_with_request(
            &spec.id,
            json!({}),
            WorkflowLimits::default(),
            Uuid::new_v4(),
        )
        .unwrap();
    fixture.guard.take();
    assert_eq!(fixture.service.store.get_run(run.id).unwrap().unwrap(), run);
}

#[tokio::test]
async fn dropping_host_preserves_new_admission_not_yet_owned_by_its_driver() {
    let mut fixture = Fixture::new();
    fixture.worker.blocked.store(true, Ordering::SeqCst);
    let spec = fixture.spec();
    fixture.service.store.save_definition(&spec).unwrap();
    let id = Uuid::new_v4();
    fixture
        .service
        .operate("run", json!({"definition_id":spec.id,"request_id":id}))
        .await
        .unwrap();
    let external = fixture
        .service
        .store
        .start_with_request(
            &spec.id,
            json!({}),
            WorkflowLimits::default(),
            Uuid::new_v4(),
        )
        .unwrap();
    fixture.guard.take();
    assert_eq!(
        fixture.service.store.get_run(external.id).unwrap().unwrap(),
        external
    );
    assert_ne!(
        fixture.service.store.get_run(id).unwrap().unwrap().status,
        willdeep_runtime_protocol::workflow::WorkflowRunStatus::Running
    );
}

#[tokio::test]
async fn dropping_host_cancels_workers_outside_the_display_snapshot_window() {
    let mut fixture = Fixture::new();
    fixture.worker.blocked.store(true, Ordering::SeqCst);
    for index in 0..4 {
        let mut spec = fixture.spec();
        spec.id = format!("parallel-{index}");
        spec.nodes = (0..16)
            .map(|node| {
                serde_json::from_value(json!({
                    "id":format!("leaf-{node}"),"kind":"agent",
                    "profile":"generalist","prompt":"Inspect independently"
                }))
                .unwrap()
            })
            .collect();
        spec.output = Value::Null;
        fixture.service.store.save_definition(&spec).unwrap();
        fixture
            .service
            .operate(
                "run",
                json!({
                    "definition_id":spec.id,"request_id":Uuid::new_v4()
                }),
            )
            .await
            .unwrap();
    }
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while fixture.background.running_subagents() < 64 {
            fixture.service.pending_reports().unwrap();
            tokio::time::sleep(DRIVER_POLL).await;
        }
    })
    .await
    .expect("all 64 workers must be accepted before cancellation");
    assert_eq!(fixture.background.snapshots().len(), 50);
    fixture.guard.take();
    assert!(
        fixture.service.store.list_runs().unwrap().iter().all(
            |run| run.status != willdeep_runtime_protocol::workflow::WorkflowRunStatus::Running
        )
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while fixture.background.running_subagents() != 0 {
            tokio::time::sleep(DRIVER_POLL).await;
        }
    })
    .await
    .expect("host cancellation must reach all workers, including the oldest eight");
}

#[tokio::test]
async fn terminal_graph_repairs_a_crash_before_the_leaf_acknowledgement() {
    let fixture = Fixture::new();
    let mut spec = fixture.spec();
    spec.nodes.truncate(1);
    spec.output = json!({"$ref":"node:inspect#"});
    fixture.service.store.save_definition(&spec).unwrap();
    let run = fixture
        .service
        .store
        .start_with_request(
            &spec.id,
            json!({}),
            WorkflowLimits::default(),
            Uuid::new_v4(),
        )
        .unwrap();
    let ready = fixture.service.store.poll(run.id).unwrap().ready.remove(0);
    let frozen = fixture.service.frozen_catalog(&run).unwrap();
    let accepted = fixture
        ._agent
        .dispatch_workflow_node_with_catalog(
            worker_args(&ready).unwrap(),
            WorkflowDispatchIdentity {
                workflow_run_id: run.id,
                node_instance_id: ready.node_instance_id.to_string(),
                request_id: ready.request_id,
                agent_id: Uuid::new_v5(&run.id, ready.request_id.as_bytes()),
            },
            &frozen,
        )
        .await
        .unwrap();
    fixture
        .service
        .store
        .record_dispatch(run.id, ready.request_id, accepted)
        .unwrap();
    let ledger = fixture.background.agent_run_store().unwrap();
    let observed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let observed = ledger.get(ready.request_id).unwrap().unwrap();
            if !observed.is_pending() {
                break observed;
            }
            tokio::time::sleep(DRIVER_POLL).await;
        }
    })
    .await
    .unwrap();
    fixture
        .service
        .store
        .complete_task(
            run.id,
            ready.request_id,
            WorkflowTaskResult {
                output: ledger.structured_output(observed.id).unwrap().unwrap(),
                agent_run: observed.clone(),
            },
        )
        .unwrap();
    assert!(
        fixture
            .service
            .store
            .poll(run.id)
            .unwrap()
            .run
            .status
            .is_terminal()
    );
    assert!(!ledger.get(observed.id).unwrap().unwrap().delivered);
    assert!(ledger.needs_recovery().unwrap());
    fixture.service.drive_once(&fixture._agent).await.unwrap();
    assert!(ledger.get(observed.id).unwrap().unwrap().delivered);
    assert!(!ledger.needs_recovery().unwrap());
    assert_eq!(fixture.worker.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn completed_json_still_requires_its_output_schema() {
    let fixture = Fixture::new();
    let mut spec = fixture.spec();
    if let willdeep_runtime_protocol::workflow::WorkflowNodeKind::Agent(agent) =
        &mut spec.nodes[0].kind
    {
        agent.output_schema = Some(
            json!({"type":"object","properties":{"required_evidence":{"type":"string"}},"required":["required_evidence"]}),
        );
    }
    fixture.service.store.save_definition(&spec).unwrap();
    let run = fixture.start(Uuid::new_v4()).await;
    assert_eq!(
        run.status,
        willdeep_runtime_protocol::workflow::WorkflowRunStatus::Failed
    );
    assert_eq!(fixture.worker.calls.load(Ordering::SeqCst), 1);
    assert!(
        run.nodes[0]
            .error
            .as_ref()
            .unwrap()
            .contains("required_evidence")
    );
}
