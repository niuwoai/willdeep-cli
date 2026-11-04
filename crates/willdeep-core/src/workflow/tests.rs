use super::*;
use serde_json::{Value, json};
use uuid::Uuid;
use willdeep_runtime_protocol::{AgentRun, AgentRunReason, AgentStatus, AgentUsageSummary};
mod lifecycle;
mod validation;

struct Fixture {
    home: std::path::PathBuf,
    store: WorkflowStore,
}
impl Fixture {
    fn new() -> Self {
        let home = std::env::temp_dir().join(format!("willdeep-workflow-test-{}", Uuid::new_v4()));
        let store = WorkflowStore::new(&home, Uuid::new_v4());
        Self { home, store }
    }
    fn start(&self, spec: &WorkflowSpecV1, input: Value) -> WorkflowRun {
        self.store.save_definition(spec).unwrap();
        self.store
            .start(&spec.id, input, WorkflowLimits::default())
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.home.exists() {
            std::fs::remove_dir_all(&self.home).expect("remove isolated fixture");
        }
    }
}
fn spec(nodes: Value, output: Value) -> WorkflowSpecV1 {
    serde_json::from_value(
        json!({"version":1,"id":"test-v1","name":"Test","nodes":nodes,"output":output}),
    )
    .unwrap()
}
fn leaf(id: &str, dependencies: &[&str], input: Value) -> Value {
    json!({"id":id,"kind":"agent","profile":"generalist","prompt":"Return the input as structured JSON.","depends_on":dependencies,"input":input})
}
fn agent(task: &WorkflowReadyTask, status: AgentStatus) -> AgentRun {
    AgentRun {
        id: Uuid::new_v4(),
        request_id: task.request_id,
        agent_id: Uuid::new_v4(),
        parent_session_id: task.parent_session_id,
        workflow_owner: Some(willdeep_runtime_protocol::agent_team::WorkflowRunOwner {
            workflow_run_id: task.run_id,
            node_instance_id: task.node_instance_id.to_string(),
        }),
        sequence: 1,
        reason: AgentRunReason::Spawn,
        status,
        label: task.node_path.clone(),
        model: Some("fixture-model".into()),
        queued_at_ms: 1,
        started_at_ms: Some(2),
        finished_at_ms: if matches!(status, AgentStatus::Queued | AgentStatus::Running) {
            None
        } else {
            Some(3)
        },
        max_turns: task.max_turns,
        token_budget: task.token_budget,
        timeout_seconds: task.timeout_seconds,
        current_turn: 1,
        current_tool: None,
        usage: AgentUsageSummary {
            model_calls: 1,
            input_tokens: Some(3),
            output_tokens: Some(4),
            total_tokens: Some(7),
            accounted_tokens: 7,
            ..Default::default()
        },
        report: Some("Human report, deliberately separate from JSON.".into()),
        delivered: false,
    }
}
fn complete(store: &WorkflowStore, task: &WorkflowReadyTask, output: Value) -> AgentRun {
    let run = agent(task, AgentStatus::Completed);
    store
        .complete_task(
            task.run_id,
            task.request_id,
            WorkflowTaskResult {
                agent_run: run.clone(),
                output,
            },
        )
        .unwrap();
    run
}
fn status(run: &WorkflowRun, path: &str) -> WorkflowNodeStatus {
    run.nodes.iter().find(|n| n.path == path).unwrap().status
}

#[test]
fn durable_admission_is_immutable_idempotent_and_parent_isolated() {
    let f = Fixture::new();
    let original = example_spec();
    assert_eq!(f.store.save_definition(&original).unwrap(), original);
    assert_eq!(f.store.save_definition(&original).unwrap(), original);
    let mut changed = original.clone();
    changed.name = "Changed".into();
    assert!(
        f.store
            .save_definition(&changed)
            .unwrap_err()
            .to_string()
            .contains("new id")
    );
    assert_eq!(
        f.store.get_definition(&original.id).unwrap(),
        Some(original.clone())
    );
    assert_eq!(f.store.list_definitions().unwrap(), vec![original.clone()]);
    let request = Uuid::new_v4();
    let run = f
        .store
        .start_with_request(
            &original.id,
            json!({"ok":true}),
            WorkflowLimits::default(),
            request,
        )
        .unwrap();
    assert_eq!(run.id, request);
    assert_eq!(run.spec, original);
    assert_eq!(
        f.store
            .start_with_request(&original.id, run.input.clone(), run.limits.clone(), request)
            .unwrap(),
        run
    );
    assert!(
        f.store
            .start_with_request(&original.id, json!(null), run.limits.clone(), request)
            .is_err()
    );
    let mut limits = run.limits.clone();
    limits.max_concurrent = 1;
    assert!(
        f.store
            .start_with_request(&original.id, run.input.clone(), limits, request)
            .is_err()
    );
    assert_eq!(f.store.list_runs().unwrap(), vec![run]);
    let other = WorkflowStore::new(&f.home, Uuid::new_v4());
    assert_eq!(other.get(request).unwrap(), None);
    assert!(other.list_runs().unwrap().is_empty());
    assert!(other.list_definitions().unwrap().is_empty());
}

#[test]
fn stable_intent_is_saved_before_ready_and_pending_never_ticks() {
    let f = Fixture::new();
    let definition = spec(
        json!([leaf("a", &[], json!({"$ref":"input#/a~1b/~0"}))]),
        json!({"$ref":"node:a#"}),
    );
    let run = f.start(&definition, json!({"a/b":{"~":42}}));
    assert!(f.store.pending(run.id).unwrap().is_empty());
    assert!(f.store.get(run.id).unwrap().unwrap().nodes.is_empty());
    let poll = f.store.poll(run.id).unwrap();
    let task = &poll.ready[0];
    assert_eq!(task.input, json!(42));
    let reopened = WorkflowStore::new(&f.home, run.parent_session_id);
    assert_eq!(reopened.pending(run.id).unwrap(), poll.ready);
    assert_eq!(reopened.poll(run.id).unwrap().ready, poll.ready);
    let mut accepted = agent(task, AgentStatus::Running);
    accepted.report = None;
    reopened
        .record_dispatch(run.id, task.request_id, accepted.clone())
        .unwrap();
    assert!(reopened.pending(run.id).unwrap().is_empty());
    assert!(reopened.poll(run.id).unwrap().ready.is_empty());
    let mut wrong = accepted.clone();
    wrong.id = Uuid::new_v4();
    assert!(
        reopened
            .record_dispatch(run.id, task.request_id, wrong)
            .is_err()
    );
    accepted.status = AgentStatus::Interrupted;
    accepted.usage.uncertain_execution = true;
    reopened
        .fail_task(run.id, task.request_id, accepted)
        .unwrap();
    assert_eq!(
        reopened.poll(run.id).unwrap().run.status,
        WorkflowRunStatus::Interrupted
    );
    assert!(reopened.poll(run.id).unwrap().ready.is_empty());
}

#[test]
fn dag_has_no_phase_barrier_and_completions_unlock_only_dependencies() {
    let f = Fixture::new();
    let mut a = leaf("a", &[], json!(1));
    a["phase"] = json!("first");
    let mut b = leaf("b", &[], json!(2));
    b["phase"] = json!("second");
    let c = leaf("c", &["a"], json!({"$ref":"node:a#"}));
    let mut definition = spec(
        json!([c, b, a]),
        json!({"a":{"$ref":"node:a#"},"c":{"$ref":"node:c#"}}),
    );
    definition.phases = vec![
        WorkflowPhase {
            id: "first".into(),
            name: "First".into(),
        },
        WorkflowPhase {
            id: "second".into(),
            name: "Second".into(),
        },
    ];
    let run = f.start(&definition, Value::Null);
    let initial = f.store.poll(run.id).unwrap();
    assert_eq!(initial.ready.len(), 2);
    let a = initial.ready.iter().find(|t| t.node_path == "a").unwrap();
    let b = initial.ready.iter().find(|t| t.node_path == "b").unwrap();
    complete(&f.store, a, json!(10));
    let next = f.store.poll(run.id).unwrap();
    assert_eq!(next.ready.len(), 2); // b's durable intent plus c
    let c = next.ready.iter().find(|t| t.node_path == "c").unwrap();
    assert_eq!(c.input, json!(10));
    complete(&f.store, c, json!(30));
    complete(&f.store, b, json!(20));
    let done = f.store.poll(run.id).unwrap().run;
    assert_eq!(done.status, WorkflowRunStatus::Completed);
    assert_eq!(done.output, Some(json!({"a":10,"c":30})));
}

#[test]
fn foreach_advances_each_pipeline_independently_and_preserves_input_order() {
    let f = Fixture::new();
    let definition = spec(
        json!([{
            "id":"each","kind":"foreach","items":{"$ref":"input#"},
            "body":{"nodes":[leaf("inspect", &[], json!({"$ref":"item#"})),leaf("verify", &["inspect"], json!({"$ref":"node:inspect#"}))],"output":{"$ref":"node:verify#"}}
        }]),
        json!({"$ref":"node:each#"}),
    );
    let run = f.start(&definition, json!(["first", "second"]));
    let initial = f.store.poll(run.id).unwrap();
    assert_eq!(initial.ready.len(), 2);
    let first = initial
        .ready
        .iter()
        .find(|t| t.node_path == "each/0/inspect")
        .unwrap();
    let second = initial
        .ready
        .iter()
        .find(|t| t.node_path == "each/1/inspect")
        .unwrap();
    complete(&f.store, second, json!("second inspected"));
    let next = f.store.poll(run.id).unwrap();
    let verify_second = next
        .ready
        .iter()
        .find(|t| t.node_path == "each/1/verify")
        .unwrap();
    assert!(!next.ready.iter().any(|t| t.node_path == "each/0/verify"));
    complete(&f.store, verify_second, json!("second verified"));
    complete(&f.store, first, json!("first inspected"));
    let next = f.store.poll(run.id).unwrap();
    complete(&f.store, &next.ready[0], json!("first verified"));
    let done = f.store.poll(run.id).unwrap().run;
    assert_eq!(done.status, WorkflowRunStatus::Completed);
    assert_eq!(
        done.output,
        Some(json!(["first verified", "second verified"]))
    );
}

#[test]
fn switch_freezes_selection_and_does_not_instantiate_unselected_scope() {
    let f = Fixture::new();
    let definition = spec(
        json!([{
            "id":"choose","kind":"switch","condition":{"op":"eq","left":{"$ref":"input#"},"right":true},
            "then_scope":{"nodes":[leaf("yes", &[], json!(1))],"output":{"$ref":"node:yes#"}},
            "else_scope":{"nodes":[leaf("no", &[], json!(0))],"output":{"$ref":"node:no#"}}
        }]),
        json!({"$ref":"node:choose#"}),
    );
    let run = f.start(&definition, json!(true));
    let initial = f.store.poll(run.id).unwrap();
    assert_eq!(initial.ready.len(), 1);
    assert_eq!(initial.ready[0].node_path, "choose/then/yes");
    complete(&f.store, &initial.ready[0], json!({"selected":true}));
    let done = f.store.poll(run.id).unwrap().run;
    assert_eq!(done.output, Some(json!({"selected":true})));
    assert!(!done.nodes.iter().any(|n| n.node_id == "no"));
}

fn repeat_spec(max: u32) -> WorkflowSpecV1 {
    spec(
        json!([{
            "id":"loop","kind":"repeat","initial":0,"max_iterations":max,
            "body":{"nodes":[leaf("step", &[], json!({"$ref":"previous#"}))],"output":{"$ref":"node:step#"}},
            "until":{"op":"ge","left":{"$ref":"node:step#"},"right":2},"carry":{"$ref":"node:step#"}
        }]),
        json!({"$ref":"node:loop#"}),
    )
}

#[test]
fn repeat_persists_previous_carry_and_stops_when_until_succeeds() {
    let f = Fixture::new();
    let run = f.start(&repeat_spec(4), Value::Null);
    let first = f.store.poll(run.id).unwrap();
    assert_eq!(first.ready[0].input, json!(0));
    complete(&f.store, &first.ready[0], json!(1));
    let reopened = WorkflowStore::new(&f.home, run.parent_session_id);
    let second = reopened.poll(run.id).unwrap();
    assert_eq!(second.ready[0].node_path, "loop/1/step");
    assert_eq!(second.ready[0].input, json!(1));
    complete(&reopened, &second.ready[0], json!(2));
    let done = reopened.poll(run.id).unwrap().run;
    assert_eq!(done.status, WorkflowRunStatus::Completed);
    assert_eq!(done.output, Some(json!(2)));
    assert!(!done.nodes.iter().any(|n| n.path == "loop/2/step"));
}

#[test]
fn exhausted_repeat_is_a_durable_failure_without_an_extra_iteration() {
    let f = Fixture::new();
    let run = f.start(&repeat_spec(2), Value::Null);
    for _ in 0..2 {
        let poll = f.store.poll(run.id).unwrap();
        assert_eq!(poll.ready.len(), 1);
        complete(&f.store, &poll.ready[0], json!(0));
    }
    let done = f.store.poll(run.id).unwrap();
    assert_eq!(done.run.status, WorkflowRunStatus::Failed);
    assert!(done.run.error.unwrap().contains("max_iterations"));
    assert!(done.ready.is_empty());
    assert_eq!(done.run.nodes.len(), 3);
}

#[test]
fn invalid_output_blocks_descendants_but_independent_work_can_finish() {
    let f = Fixture::new();
    let mut a = leaf("a", &[], Value::Null);
    a["output_schema"] = json!({"type":"integer"});
    let definition = spec(
        json!([
            a,
            leaf("b", &["a"], Value::Null),
            leaf("independent", &[], Value::Null)
        ]),
        Value::Null,
    );
    let run = f.start(&definition, Value::Null);
    let initial = f.store.poll(run.id).unwrap();
    let a = initial.ready.iter().find(|t| t.node_path == "a").unwrap();
    let independent = initial
        .ready
        .iter()
        .find(|t| t.node_path == "independent")
        .unwrap();
    complete(&f.store, a, json!("bad"));
    let waiting = f.store.poll(run.id).unwrap().run;
    assert_eq!(waiting.status, WorkflowRunStatus::Running);
    assert_eq!(status(&waiting, "a"), WorkflowNodeStatus::Failed);
    assert_eq!(status(&waiting, "b"), WorkflowNodeStatus::Blocked);
    assert!(
        waiting
            .nodes
            .iter()
            .find(|n| n.path == "a")
            .unwrap()
            .output
            .is_none()
    );
    complete(&f.store, independent, json!("ok"));
    assert_eq!(
        f.store.poll(run.id).unwrap().run.status,
        WorkflowRunStatus::Failed
    );
}
